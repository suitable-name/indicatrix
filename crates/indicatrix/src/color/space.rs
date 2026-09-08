//! Wide-gamut colour-space definitions.
//!
//! XYZ->RGB matrices, reference white points, and transfer functions for sRGB, Display
//! P3, Rec.2020, and `ACEScg`, plus [`ColorSpace::encode`] -- a single entry point
//! carrying a CIE XYZ radiance sample to encoded 8-bit output.
//!
//! # Intended wiring (not performed by this module)
//!
//! `optics::raytracer::xyz_to_srgb_gamma` performs this pipeline inline (XYZ -> linear
//! sRGB, gamut compression, ACES tone mapping, flat 1/2.2 gamma). This module is a
//! generalised, self-contained replacement, not yet wired into the renderer:
//!
//! ```ignore
//! // old:
//! let pixel = xyz_to_srgb_gamma(xyz);
//! // new:
//! let pixel = ColorSpace::Srgb.encode(xyz, ToneMap::AcesFilmic { exposure: 1.0 });
//! ```
//!
//! Matches gamut- and tone-mapping exactly; the one difference is the true piecewise
//! sRGB curve instead of flat 1/2.2 gamma (see `tests/color_tests.rs` for the deviation).

use glam::Vec3;

use super::gamut;

/// A transfer function (opto-electronic encoding curve) mapping a scene-linear value in
/// `[0, 1]` to its encoded, display-ready counterpart, and back.
///
/// Each variant is a genuinely distinct curve -- see below for constants and rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFunction {
    /// Piecewise sRGB curve (IEC 61966-2-1): `12.92 * x` for `x <= 0.0031308`,
    /// `1.055 * x^(1/2.4) - 0.055` above. Shared by sRGB and Display P3, which reuses
    /// the sRGB transfer function verbatim.
    Srgb,
    /// Piecewise Rec.2020 curve (ITU-R BT.2020-2, Table 4): `4.5 * x` below `beta`,
    /// `alpha * x^0.45 - (alpha - 1)` above, `alpha ~= 1.0992968`, `beta ~= 0.01805397`.
    /// Distinct exponent (0.45, not 1/2.4) from sRGB -- not the same curve.
    Rec2020,
    /// No encoding curve: `ACEScg` is a scene-linear working space (for compositing,
    /// not direct display), so `encode`/`decode` are the identity function.
    Linear,
}

impl TransferFunction {
    /// Encodes a scene-linear value in `[0, 1]`. Caller must clamp first (as
    /// [`ColorSpace::encode`] does) -- a negative base to a fractional power is invalid.
    #[must_use]
    pub fn encode(self, linear: f32) -> f32 {
        match self {
            Self::Srgb => {
                if linear <= 0.003_130_8 {
                    linear * 12.92
                } else {
                    1.055f32.mul_add(linear.powf(1.0 / 2.4), -0.055)
                }
            }
            Self::Rec2020 => {
                const ALPHA: f32 = 1.099_296_8;
                const BETA: f32 = 0.018_053_97;
                if linear < BETA {
                    linear * 4.5
                } else {
                    ALPHA.mul_add(linear.powf(0.45), -(ALPHA - 1.0))
                }
            }
            Self::Linear => linear,
        }
    }

    /// Decodes a transfer-encoded value back to scene-linear; exact inverse of
    /// [`Self::encode`] up to rounding, including at the piecewise breakpoint.
    #[must_use]
    pub fn decode(self, encoded: f32) -> f32 {
        match self {
            Self::Srgb => {
                if encoded <= 0.040_45 {
                    encoded / 12.92
                } else {
                    ((encoded + 0.055) / 1.055).powf(2.4)
                }
            }
            Self::Rec2020 => {
                const ALPHA: f32 = 1.099_296_8;
                const BETA: f32 = 0.018_053_97;
                let breakpoint = 4.5 * BETA;
                if encoded < breakpoint {
                    encoded / 4.5
                } else {
                    ((encoded + (ALPHA - 1.0)) / ALPHA).powf(1.0 / 0.45)
                }
            }
            Self::Linear => encoded,
        }
    }
}

/// Policy for compressing scene-linear radiance into the encodable `[0, 1]` range
/// before a space's transfer function is applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToneMap {
    /// No tone mapping: values are clamped to `[0, 1]` by [`ColorSpace::encode`]. Useful
    /// when values are already display-range or a caller applies its own tone mapping.
    None,
    /// ACES filmic tonemap (Narkowicz 2015 fit, matching `optics::raytracer::aces_tonemap`),
    /// applied to luminance only then rescaled back into RGB so hue/saturation survive --
    /// per-channel tone mapping shifts hue, which is wrong for saturated dispersion
    /// "fire" colours (see `aces_tonemap`'s docs).
    ///
    /// `exposure` is a linear multiplier applied before the curve; `1.0` reproduces
    /// `xyz_to_srgb_gamma`'s tone-mapping step exactly.
    AcesFilmic {
        /// Linear exposure multiplier applied to radiance before tone mapping.
        exposure: f32,
    },
}

/// A target RGB colour space: its XYZ->RGB matrix, reference white point, and transfer
/// function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    /// IEC 61966-2-1 sRGB, D65 white.
    Srgb,
    /// Display P3 (Apple / SMPTE EG 432-1 primaries), D65 white. Same transfer function
    /// as sRGB but wider primaries, especially red/green -- where dispersion "fire" lands.
    DisplayP3,
    /// ITU-R BT.2020-2 Rec.2020, D65 white. The widest gamut of the four, with its own
    /// distinct piecewise transfer function.
    Rec2020,
    /// Academy `ACEScg` (AP1 primaries), D60 white. Scene-linear -- no transfer function
    /// -- the right target for compositing rather than direct display.
    AcesCg,
}

impl ColorSpace {
    /// The XYZ->RGB matrix, row-major (`row[0]` produces R, `row[1]` produces G,
    /// `row[2]` produces B), for CIE 1931 XYZ with `Y` normalized to 1.0 at the space's
    /// own reference white.
    ///
    /// # Sources
    ///
    /// - **sRGB**: bit-identical to `optics::raytracer::xyz_to_linear_srgb`'s constants
    ///   (Bruce-Lindbloom-rounded sRGB D65 matrix, IEC 61966-2-1 primaries/white).
    /// - **Display P3**: primaries R(0.680, 0.320) G(0.265, 0.690) B(0.150, 0.060), D65
    ///   white, per SMPTE EG 432-1; cross-checked against `colour-science`.
    /// - **Rec.2020**: primaries R(0.708, 0.292) G(0.170, 0.797) B(0.131, 0.046), D65
    ///   white, per ITU-R BT.2020-2 Table 3; cross-checked against `colour-science`.
    /// - **`ACEScg`**: AP1 primaries R(0.713, 0.293) G(0.165, 0.830) B(0.128, 0.044),
    ///   D60 white (x=0.32168, y=0.33767), per Academy S-2014-004 Table 1; cross-checked
    ///   against the Academy's published `XYZ_to_AP1` matrix.
    #[must_use]
    pub const fn xyz_to_rgb_matrix(self) -> [[f32; 3]; 3] {
        match self {
            Self::Srgb => [
                [3.240_454_2, -1.537_138_5, -0.498_531_4],
                [-0.969_266, 1.876_010_8, 0.041_556_0],
                [0.055_643_4, -0.204_025_9, 1.057_225_2],
            ],
            Self::DisplayP3 => [
                [2.493_497, -0.931_383_6, -0.402_710_78],
                [-0.829_489, 1.762_664, 0.023_624_686],
                [0.035_845_83, -0.076_172_39, 0.956_884_5],
            ],
            Self::Rec2020 => [
                [1.716_651_2, -0.355_670_78, -0.253_366_3],
                [-0.666_684_3, 1.616_481_2, 0.015_768_546],
                [0.017_639_857, -0.042_770_613, 0.942_103_1],
            ],
            Self::AcesCg => [
                [1.641_023_4, -0.324_803_3, -0.236_424_7],
                [-0.663_662_86, 1.615_331_6, 0.016_756_348],
                [0.011_721_894, -0.008_284_442, 0.988_394_86],
            ],
        }
    }

    /// The space's reference white point chromaticity, `(x, y)` -- the target
    /// out-of-gamut chromaticities compress toward; see [`gamut::project_to_gamut`].
    #[must_use]
    pub const fn white_point_xy(self) -> (f32, f32) {
        match self {
            // D60 (Academy S-2014-004) -- ACEScg's own white, distinct from D65.
            Self::AcesCg => (0.321_68, 0.337_67),
            // D65, matching `optics::raytracer::xyz_to_srgb_gamma`'s WHITE_X/WHITE_Y.
            Self::Srgb | Self::DisplayP3 | Self::Rec2020 => (0.3127, 0.3290),
        }
    }

    /// The transfer function associated with this space.
    #[must_use]
    pub const fn transfer_function(self) -> TransferFunction {
        match self {
            Self::Srgb | Self::DisplayP3 => TransferFunction::Srgb,
            Self::Rec2020 => TransferFunction::Rec2020,
            Self::AcesCg => TransferFunction::Linear,
        }
    }

    /// Converts a CIE XYZ sample to this space's linear (not transfer-encoded) RGB via
    /// [`Self::xyz_to_rgb_matrix`]. Does **not** gamut-map -- out-of-gamut input can
    /// produce negative components; see [`gamut::project_to_gamut`] for that.
    #[must_use]
    pub fn xyz_to_linear(self, xyz: Vec3) -> Vec3 {
        let m = self.xyz_to_rgb_matrix();
        Vec3::new(
            m[0][0].mul_add(xyz.x, m[0][1].mul_add(xyz.y, m[0][2] * xyz.z)),
            m[1][0].mul_add(xyz.x, m[1][1].mul_add(xyz.y, m[1][2] * xyz.z)),
            m[2][0].mul_add(xyz.x, m[2][1].mul_add(xyz.y, m[2][2] * xyz.z)),
        )
    }

    /// Converts a CIE XYZ radiance sample to encoded 8-bit RGBA: gamut-maps into this
    /// space (preserving luminance and hue, see [`gamut::project_to_gamut`]), applies
    /// `tonemap`, applies the transfer function, and quantizes each channel to
    /// `0..=255`. Alpha is always `255`.
    ///
    /// Every channel is finite and in `0..=255` for any input, including non-positive,
    /// extreme, or far-out-of-gamut `xyz`; non-finite `xyz` maps to opaque black.
    ///
    /// # Highlights desaturate instead of clipping a single channel
    ///
    /// [`ToneMap::AcesFilmic`] rescales RGB by one luminance-derived factor so hue
    /// survives; a channel pushed above `1.0` is routed through
    /// [`gamut::project_to_gamut_bounded`] (`max = 1.0`) instead of a per-channel
    /// clamp, so it desaturates toward white rather than hue-shifting.
    #[must_use]
    pub fn encode(self, xyz: Vec3, tonemap: ToneMap) -> [u8; 4] {
        let sum = xyz.x + xyz.y + xyz.z;
        if !sum.is_finite() || sum <= 1e-6 {
            return [0, 0, 0, 255];
        }

        let toned_rgb = match tonemap {
            ToneMap::None => gamut::project_to_gamut(xyz, self),
            ToneMap::AcesFilmic { exposure } => {
                let luminance = xyz.y.max(0.0);
                let exposed_luminance = (luminance * exposure).max(0.0);
                let y_tm = crate::optics::raytracer::aces_tonemap(exposed_luminance);
                // Scaling `xyz` before gamut-projecting (rather than scaling the
                // projected RGB) is equivalent for in-range colours since gamut
                // projection is linear in luminance -- letting one bounded projection
                // call handle both compression and desaturation together.
                let luminance_scale = (y_tm / exposed_luminance.max(1e-5)) * exposure;
                let toned_xyz = xyz * luminance_scale;
                gamut::project_to_gamut_bounded(toned_xyz, self, 1.0)
            }
        };

        let tf = self.transfer_function();
        let encode_channel = |c: f32| -> u8 {
            let linear = if c.is_finite() {
                c.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let encoded = tf.encode(linear).clamp(0.0, 1.0);
            (encoded * 255.0) as u8
        };

        [
            encode_channel(toned_rgb.x),
            encode_channel(toned_rgb.y),
            encode_channel(toned_rgb.z),
            255,
        ]
    }
}
