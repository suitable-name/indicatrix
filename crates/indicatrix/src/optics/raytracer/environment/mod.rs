//! Environment/lighting sources a ray can sample when it misses the gemstone.
//!
//! The analytic gemological studio rig ([`LightingPreset`], [`sample_studio_environment`])
//! and the loaded-HDR-panorama alternative ([`EnvironmentSource::HdrMap`]).
//!
//! # Lighting models
//!
//! Every [`LightingPreset`] samples one [`LightingModel`]:
//! - `Studio` (`Daylight`, `Incandescent`, `RingLights`, `DarkSpotlight`): the analytic
//!   studio rig -- a charcoal backdrop, one key softbox, one fill and sixteen ring
//!   pinpoints. Its arithmetic is pinned by golden images and never changes.
//! - `IsoHemisphere`: a uniformly radiant upper hemisphere at radiance 1, nothing below
//!   the girdle plane -- the ISO-standard viewing geometry.
//! - `LightTent`: a jewellery light tent -- dim tent walls, one broad overhead softbox,
//!   three black cards on the ring positions away from the key (the contrast
//!   photographers add so a diamond reads as a facet pattern rather than a white blur),
//!   one small hard spark light for scintillation, black velvet below the girdle.
//! - `DaylightDome`: a clear sky, brighter at the horizon than at the zenith with an
//!   aureole around the sun, a 2 degree sun disc, dark ground.
//!
//! The three lit models also darken every exit direction inside the observer's
//! head-shadow cone (`HEAD_SHADOW_*`), the term that gives a face-up stone its dark
//! table reflections; `Studio` ignores the observer. Radiances are chosen so that at
//! exposure 1 the ambient terms land near middle grey after the ACES curve and only a
//! direct reflection of a light source clips to white.
//!
//! Split from a single `environment.rs` into this module tree by seam: [`spectral`]
//! holds the illuminant spectral-power curves (D65 table, Planckian fit), [`rig`] holds
//! the actual per-direction radiance evaluation (studio rig falloffs, backdrop fill, NEE
//! sampling), and this file keeps the preset/source types, the module docs, and the
//! public entry points. Every path reachable as `environment::X` before the split stays
//! reachable at exactly that path via the re-exports below.

use super::color::illuminant_white_balance;
use crate::renderer::env_map::EnvironmentMap;
use glam::Vec3;

mod rig;
mod spectral;
#[cfg(test)]
mod tests;

// rig.rs
pub use rig::sample_studio_environment_with_rig;
pub(super) use rig::{
    environment_nee_pdf, fill_backdrop, sample_environment_channel, sample_environment_channels,
    sample_environment_for_nee,
};

// spectral.rs
use spectral::{BlackbodyNorm, IlluminantSpectrum};
pub use spectral::{blackbody_spectrum, d65_relative_spectral_power};

/// color temperature and rig-intensity parameters for one named studio lighting preset.
///
/// Returned by [`LightingPreset::params`] -- the single lookup both
/// `sample_studio_environment` (which lights the traced image) and
/// `illuminant_temperature_k` (which derives the von-Kries white balance for that same
/// image) share, so the two cannot independently drift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingRigParams {
    /// Blackbody color temperature in Kelvin, fed to [`blackbody_spectrum`].
    pub temp_k: f32,
    /// Multiplier on the key-softbox and ring-emitter intensity terms (does not affect
    /// the fill light or the ambient backdrop).
    pub spot_mult: f32,
}

/// The gemological studio lighting rig presets, as a closed, exhaustively-matched set
/// of variants.
///
/// An unrecognised preset is not representable, unlike a `&str`-keyed lookup where a
/// caller could pass a string that silently falls through to a default.
///
/// `Daylight` is index `0` / the [`Default`], and is what any legacy or unrecognised
/// persisted label -- including the old, mislabelled `"D65 Daylight (5500K)"` string --
/// migrates to via [`Self::from_label`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LightingPreset {
    #[default]
    Daylight,
    Incandescent,
    RingLights,
    DarkSpotlight,
    IsoHemisphere,
    LightTent,
    DaylightDome,
    /// A 365 nm UV lamp: the `Studio` rig geometry lit by a narrow Gaussian line, no
    /// ambient backdrop and no white balance (see [`LightingPreset::uses_white_balance`]).
    /// Appended last, so every earlier discriminant and postcard index is unchanged.
    UvLamp365,
    /// A 395 nm UV LED lamp (same rig as [`Self::UvLamp365`]); its tail reaches into the
    /// violet, so a non-fluorescent stone looks faintly violet under it.
    UvLamp395,
}

/// Which environment the preset samples -- see this module's "Lighting models" doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LightingModel {
    Studio,
    IsoHemisphere,
    LightTent,
    DaylightDome,
}

impl LightingModel {
    /// The `u32` discriminant bound in `GpuTransportParams::studio_model`.
    #[must_use]
    pub const fn gpu_id(self) -> u32 {
        match self {
            Self::Studio => 0,
            Self::IsoHemisphere => 1,
            Self::LightTent => 2,
            Self::DaylightDome => 3,
        }
    }
}

impl LightingPreset {
    /// All nine presets, in the same order as their UI index / the `lighting_options`
    /// combo box list (`apps/indicatrix-cut/ui/models/viewport.slint`).
    pub const ALL: [Self; 9] = [
        Self::Daylight,
        Self::Incandescent,
        Self::RingLights,
        Self::DarkSpotlight,
        Self::IsoHemisphere,
        Self::LightTent,
        Self::DaylightDome,
        Self::UvLamp365,
        Self::UvLamp395,
    ];

    /// This preset's color temperature and rig-intensity multiplier -- the single
    /// source of truth both `sample_studio_environment` and `illuminant_temperature_k`
    /// read from. See the type's doc comment for why that matters.
    #[must_use]
    pub const fn params(self) -> LightingRigParams {
        match self {
            Self::Incandescent => LightingRigParams {
                temp_k: 3200.0,
                spot_mult: 1.2,
            },
            Self::RingLights => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.6,
            },
            Self::DarkSpotlight => LightingRigParams {
                temp_k: 6000.0,
                spot_mult: 2.4,
            },
            Self::LightTent => LightingRigParams {
                temp_k: 5000.0,
                spot_mult: 1.0,
            },
            // The UV lamps are not Planckian: `temp_k` is unused by their spectrum, white
            // balance and CPU path (they never reach the GPU); 6500 K is a placeholder.
            Self::Daylight
            | Self::IsoHemisphere
            | Self::DaylightDome
            | Self::UvLamp365
            | Self::UvLamp395 => LightingRigParams {
                temp_k: 6500.0,
                spot_mult: 1.0,
            },
        }
    }

    /// The user-facing display label.
    ///
    /// D65 daylight is 6500K, so this must read `"6500K"`, not `"5500K"`, to stay
    /// consistent with the actually-rendered color.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Daylight => "D65 Daylight (6500K)",
            Self::Incandescent => "Incandescent (3200K)",
            Self::RingLights => "Gem Studio Ring Lights",
            Self::DarkSpotlight => "Dramatic Dark Spotlight",
            Self::IsoHemisphere => "ISO hemisphere",
            Self::LightTent => "Light tent + black cards",
            Self::DaylightDome => "Daylight sky + sun",
            Self::UvLamp365 => "UV lamp 365 nm",
            Self::UvLamp395 => "UV lamp 395 nm",
        }
    }

    /// Parses a persisted or UI-supplied label back into a preset. Falls back to
    /// [`Self::Daylight`] for anything unrecognised -- including the legacy
    /// `"D65 Daylight (5500K)"` label an older settings file may still contain, which
    /// already resolved to D65 6500K, so migration is silent. The lit models' first
    /// labels (`"ISO hemisphere (GemRay-style)"`, `"Soft dome + ring lights"`,
    /// `"Daylight dome + sun"`) resolve to their current presets the same way.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "Incandescent (3200K)" => Self::Incandescent,
            "Gem Studio Ring Lights" => Self::RingLights,
            "Dramatic Dark Spotlight" => Self::DarkSpotlight,
            "ISO hemisphere" | "ISO hemisphere (GemRay-style)" => Self::IsoHemisphere,
            "Light tent + black cards" | "Soft dome + ring lights" => Self::LightTent,
            "Daylight sky + sun" | "Daylight dome + sun" => Self::DaylightDome,
            "UV lamp 365 nm" => Self::UvLamp365,
            "UV lamp 395 nm" => Self::UvLamp395,
            _ => Self::Daylight,
        }
    }

    /// The index into [`Self::ALL`] / the UI combo box's `lighting_options` list.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Daylight => 0,
            Self::Incandescent => 1,
            Self::RingLights => 2,
            Self::DarkSpotlight => 3,
            Self::IsoHemisphere => 4,
            Self::LightTent => 5,
            Self::DaylightDome => 6,
            Self::UvLamp365 => 7,
            Self::UvLamp395 => 8,
        }
    }

    /// Inverse of [`Self::index`]; out-of-range indices fall back to [`Self::Daylight`]
    /// (index 0), matching [`Self::from_label`]'s fallback.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Incandescent,
            2 => Self::RingLights,
            3 => Self::DarkSpotlight,
            4 => Self::IsoHemisphere,
            5 => Self::LightTent,
            6 => Self::DaylightDome,
            7 => Self::UvLamp365,
            8 => Self::UvLamp395,
            _ => Self::Daylight,
        }
    }

    /// Which environment model this preset samples.
    #[must_use]
    pub const fn model(self) -> LightingModel {
        match self {
            Self::Daylight
            | Self::Incandescent
            | Self::RingLights
            | Self::DarkSpotlight
            | Self::UvLamp365
            | Self::UvLamp395 => LightingModel::Studio,
            Self::IsoHemisphere => LightingModel::IsoHemisphere,
            Self::LightTent => LightingModel::LightTent,
            Self::DaylightDome => LightingModel::DaylightDome,
        }
    }

    /// Whether the illuminant is the tabulated CIE D65 curve rather than a Planckian fit.
    #[must_use]
    pub const fn uses_d65(self) -> bool {
        matches!(
            self,
            Self::Daylight | Self::IsoHemisphere | Self::DaylightDome
        )
    }

    /// Whether this preset is a UV lamp ([`Self::UvLamp365`], [`Self::UvLamp395`]): a
    /// narrow Gaussian line source, CPU-only (`scene_routes_to_gpu` is `false`).
    #[must_use]
    pub const fn is_uv_lamp(self) -> bool {
        matches!(self, Self::UvLamp365 | Self::UvLamp395)
    }

    /// Centre wavelength and FWHM (nm) of a UV lamp's Gaussian line, `None` for every
    /// other preset: 365 nm / 10 nm and 395 nm / 12 nm.
    #[must_use]
    pub const fn uv_line(self) -> Option<(f32, f32)> {
        match self {
            Self::UvLamp365 => Some((365.0, 10.0)),
            Self::UvLamp395 => Some((395.0, 12.0)),
            _ => None,
        }
    }

    /// Whether the von-Kries white balance toward D65 applies: only the Planckian
    /// presets (`Incandescent`, `RingLights`, `DarkSpotlight`, `LightTent`). The D65
    /// presets are already D65-white (identity), and the UV lamps have no meaningful
    /// white point at all (identity: the stone shows the lamp's own color).
    #[must_use]
    pub const fn uses_white_balance(self) -> bool {
        !self.uses_d65() && !self.is_uv_lamp()
    }

    /// Whether the `Studio` rig adds its dim ambient backdrop term. `false` for the UV
    /// lamps: a dark room with only the lamp lit.
    #[must_use]
    pub const fn has_ambient_fill(self) -> bool {
        !self.is_uv_lamp()
    }

    /// Relative spectral power of this preset's illuminant at `lambda_nm`: the
    /// tabulated CIE D65 curve where [`Self::uses_d65`], a unit-peak Gaussian line for
    /// the UV lamps, else a Planckian fit at the preset's color temperature.
    #[must_use]
    pub fn spectral_power(self, lambda_nm: f32) -> f32 {
        self.illuminant_spectrum().power(lambda_nm)
    }

    /// This preset's illuminant curve with the temperature-only Planck constants
    /// resolved once, for callers that evaluate several wavelengths. Calling `.power(l)` on
    /// the result is bit-identical to `Self::spectral_power` at `l` for every `l`.
    fn illuminant_spectrum(self) -> IlluminantSpectrum {
        if self.uses_d65() {
            IlluminantSpectrum::D65
        } else if let Some((centre_nm, fwhm_nm)) = self.uv_line() {
            IlluminantSpectrum::Gaussian {
                centre_nm,
                sigma_nm: fwhm_nm / (2.0 * (2.0 * std::f32::consts::LN_2).sqrt()),
            }
        } else {
            IlluminantSpectrum::Blackbody(BlackbodyNorm::new(self.params().temp_k))
        }
    }

    /// Convenience constructor for the common case of tracing against the analytic
    /// studio rig: `LightingPreset::RingLights.studio(1.0, 0.85, 0.95)` reads at the
    /// call site much like the old positional `&str` argument list did.
    #[must_use]
    pub const fn studio(
        self,
        exposure: f32,
        light_yaw: f32,
        light_pitch: f32,
    ) -> EnvironmentSource<'static> {
        EnvironmentSource::Studio {
            preset: self,
            exposure,
            light_yaw,
            light_pitch,
            backdrop: 0.0,
            surface_glare: 1.0,
        }
    }
}

/// Selects what `trace_spectral_ray` samples when a ray misses the gemstone.
///
/// Either the analytic studio rig (`Studio`, the default -- see
/// `sample_studio_environment`) or a loaded HDR equirectangular panorama (`HdrMap`, via
/// [`crate::renderer::env_map::EnvironmentMap`]). The analytic rig stays useful for
/// controlled comparisons where a real photograph would introduce variables (its own
/// exposure, white balance, capture noise) a study wants held constant.
#[derive(Clone, Copy)]
pub enum EnvironmentSource<'a> {
    Studio {
        preset: LightingPreset,
        exposure: f32,
        light_yaw: f32,
        light_pitch: f32,
        /// Radiance of the backdrop card a camera ray sees where it misses the stone,
        /// in the preset's own spectral-power units (so it renders neutral after white
        /// balance) and independent of `exposure`. `0.0` shows the environment itself.
        /// The stone's optics never see the card -- only the primary ray does -- so
        /// leakage and windows stay as dark as the real ground, behind a neutral grey
        /// backdrop card. See [`BACKDROP_GREY`].
        backdrop: f32,
        /// Scale of the stone's first-surface specular (Fresnel) reflection, `1.0`
        /// leaving it as is and `0.0` removing the mirror image of the light so only
        /// light that entered the stone remains (the effect of cross-polarised
        /// viewing). Everything that went into the stone is untouched. See
        /// [`EnvironmentSource::with_surface_glare`].
        surface_glare: f32,
    },
    HdrMap(&'a EnvironmentMap),
}

/// Backdrop radiance that tone-maps to a neutral grey backdrop card (about sRGB 160).
pub const BACKDROP_GREY: f32 = 0.23;
/// Backdrop radiance that tone-maps to white: a light box behind the stone.
pub const BACKDROP_WHITE: f32 = 8.0;

impl EnvironmentSource<'_> {
    /// Puts a backdrop card of radiance `backdrop` behind the stone (see the `Studio`
    /// variant's field); an HDR map is its own backdrop and is returned unchanged.
    #[must_use]
    pub const fn with_backdrop(self, backdrop: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                surface_glare,
                ..
            } => Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
            },
            hdr @ Self::HdrMap(_) => hdr,
        }
    }

    /// Sets the surface-glare scale (see the `Studio` variant's field), clamped to
    /// `0.0..=1.0`; a NaN is treated as `1.0`. `1.0` is the default and leaves the
    /// render bit-identical to one without the field. An HDR map is returned unchanged
    /// (glare is only applied for the analytic lighting presets).
    #[must_use]
    pub const fn with_surface_glare(self, surface_glare: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                ..
            } => Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare: clamp_surface_glare(surface_glare),
            },
            hdr @ Self::HdrMap(_) => hdr,
        }
    }

    /// The analytic lighting preset, `None` for an HDR panorama.
    #[must_use]
    pub const fn lighting_preset(&self) -> Option<LightingPreset> {
        match *self {
            Self::Studio { preset, .. } => Some(preset),
            Self::HdrMap(_) => None,
        }
    }

    /// The surface-glare scale in force: the `Studio` field, `1.0` for an HDR map.
    #[must_use]
    pub const fn surface_glare(&self) -> f32 {
        match *self {
            Self::Studio { surface_glare, .. } => surface_glare,
            Self::HdrMap(_) => 1.0,
        }
    }
}

/// Clamps a surface-glare scale to `0.0..=1.0`, mapping NaN to `1.0` (off).
const fn clamp_surface_glare(value: f32) -> f32 {
    if value >= 1.0 || value.is_nan() {
        1.0
    } else if value > 0.0 {
        value
    } else {
        0.0
    }
}

/// The von-Kries white-balance scale (Bradford LMS-space, per-cone -- see
/// [`compute_illuminant_white_balance`]) [`trace_spectral_ray`] applies, via
/// [`apply_von_kries_white_balance`], to its final XYZ integration for a `Studio`
/// `environment`. Only the analytic studio rig has a single well-defined illuminant
/// color temperature to neutralize against -- a loaded HDR panorama has no one
/// blackbody temperature standing in for it, so this returns `Vec3::ONE` for `HdrMap`
/// (a mathematical no-op scale), but `trace_spectral_ray`'s own `HdrMap` arm does not
/// even call [`apply_von_kries_white_balance`] with it: the full
/// XYZ->LMS->XYZ round trip is not quite the identity at `Vec3::ONE` in f32 (the two
/// published Bradford matrices are not exact inverses), so skipping the call entirely
/// for `HdrMap` is exact, matching `transport_bounce.wgsl`'s own `params.env_mode == 1u`
/// (`Studio`-only) gate on the identical transform, instead of merely close to it.
#[inline]
pub(crate) fn environment_white_balance(environment: EnvironmentSource<'_>) -> Vec3 {
    match environment {
        EnvironmentSource::Studio { preset, .. } => illuminant_white_balance(preset),
        EnvironmentSource::HdrMap(_) => Vec3::ONE,
    }
}

/// Evaluates high-dynamic-range gemological studio lighting at a specific continuous
/// wavelength `lambda_nm`, with no observer in the scene.
///
/// The lit models' head shadow is off here; see
/// [`sample_studio_environment_observed`].
///
/// Builds a fresh [`StudioRig`](crate::optics::studio_rig::StudioRig) every call, right
/// for a single ad-hoc lookup. Callers that look up many directions under one light pose
/// (`color::metrics`' illumination test) build the rig once and call
/// [`sample_studio_environment_with_rig`]; `trace_spectral_ray`'s per-bounce
/// environment lookups likewise borrow the rig that `trace_spectral_ray_inner` builds
/// once per trace, so no per-sample
/// `StudioRig::new` runs; `accumulate_miss_radiance` then evaluates all `NUM_CHANNELS`
/// channels in one pass over the wavelength-independent geometry.
#[must_use]
pub fn sample_studio_environment(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> f32 {
    sample_studio_environment_observed(
        dir,
        lambda_nm,
        lighting_preset,
        exposure,
        light_yaw,
        light_pitch,
        Vec3::ZERO,
    )
}

/// [`sample_studio_environment`] with an observer in the scene.
///
/// `observer` is the unit direction from the stone towards the eye (the reverse of the
/// pixel's primary ray), and the lit models darken every exit direction inside the
/// head-shadow cone around it -- the dark table reflections a real face-up stone
/// shows. `Studio` presets ignore it, and `Vec3::ZERO` disables the shadow for every
/// model (every dot product is then `0.0`, outside the cone).
#[must_use]
pub fn sample_studio_environment_observed(
    dir: Vec3,
    lambda_nm: f32,
    lighting_preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    observer: Vec3,
) -> f32 {
    // Key/fill/ring directions come from the shared `StudioRig` (see its module doc
    // for why this is not recomputed inline here) -- the same construction the GPU
    // environment check and every per-ray trace use, so they cannot silently drift apart.
    let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
    sample_studio_environment_with_rig(dir, lambda_nm, lighting_preset, exposure, &rig, observer)
}
