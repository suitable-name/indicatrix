//! [`EnvironmentSource`] (the analytic studio rig or an HDR panorama), the two legacy backdrop
//! levels and the white-balance lookup.
//!
//! Moved verbatim out of `mod.rs` (wave 0 of the backdrop campaign); no arithmetic changed
//! (wave 1 changed only the `backdrop` documentation).
//! Re-exported from the parent so every existing path still resolves.

use super::super::color::illuminant_white_balance;
use super::{DEFAULT_HEAD_SHADOW_DEG, LightingPreset, clamp_head_shadow_deg};
use crate::renderer::env_map::EnvironmentMap;
use glam::Vec3;

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
        /// Radiance at exposure 1 of the tray below the girdle plane, seen by every escape
        /// and by the camera: a matte surface lit by the rig, `rho * E_rig / pi`, in the
        /// preset's own spectral-power units (so it renders neutral after white balance).
        /// The lighting arms apply the exposure. `0.0` = the preset's own ground (As Lit).
        /// Resolved by `render_setup::Backdrop::resolve` (see `tray.rs`); the stone's
        /// windows show it exactly as the surround does.
        backdrop: f32,
        /// Scale of the stone's first-surface specular (Fresnel) reflection, `1.0`
        /// leaving it as is and `0.0` removing the mirror image of the light so only
        /// light that entered the stone remains (the effect of cross-polarised
        /// viewing). Everything that went into the stone is untouched. See
        /// [`EnvironmentSource::with_surface_glare`].
        surface_glare: f32,
        /// Width of the observer head shadow the three lit models darken around the eye
        /// direction, in degrees (`0.0`: off; default [`DEFAULT_HEAD_SHADOW_DEG`], which
        /// renders bit-identically to before the field existed). `Studio` ignores it. See
        /// [`head_shadow_cosines`](super::head_shadow_cosines) and [`EnvironmentSource::with_head_shadow`].
        head_shadow_deg: f32,
    },
    HdrMap(&'a EnvironmentMap),
}

/// A literal tray radiance of 0.23 (about sRGB 160 at exposure 1).
///
/// Historic card level, kept as a convenient fixed radiance for tests and PGO scenes; user backdrops resolve through
/// `render_setup::Backdrop::resolve`.
pub const BACKDROP_GREY: f32 = 0.23;

impl EnvironmentSource<'_> {
    /// Puts a lit tray of radiance `backdrop` (at exposure 1) under the stone (see the
    /// `Studio` variant's field); an HDR map is its own backdrop and is returned unchanged.
    #[must_use]
    pub const fn with_backdrop(self, backdrop: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                surface_glare,
                head_shadow_deg,
                ..
            } => Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
            },
            hdr @ Self::HdrMap(_) => hdr,
        }
    }

    /// Sets the head-shadow size in degrees (see the `Studio` variant's field): `0.0`
    /// turns it off, NaN means the default, other values are clamped to `3..=88`. An HDR
    /// map is returned unchanged.
    #[must_use]
    pub const fn with_head_shadow(self, head_shadow_deg: f32) -> Self {
        match self {
            Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                ..
            } => Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg: clamp_head_shadow_deg(head_shadow_deg),
            },
            hdr @ Self::HdrMap(_) => hdr,
        }
    }

    /// The head-shadow size in force, degrees: the `Studio` field, the default for an
    /// HDR map (which has no head shadow).
    #[must_use]
    pub const fn head_shadow_deg(&self) -> f32 {
        match *self {
            Self::Studio {
                head_shadow_deg, ..
            } => head_shadow_deg,
            Self::HdrMap(_) => DEFAULT_HEAD_SHADOW_DEG,
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
                head_shadow_deg,
                ..
            } => Self::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare: clamp_surface_glare(surface_glare),
                head_shadow_deg,
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
#[expect(
    clippy::redundant_pub_crate,
    reason = "re-exported as `pub(crate)` from `environment` for color::metrics, the transport code and the GPU estimator checks"
)]
pub(crate) fn environment_white_balance(environment: EnvironmentSource<'_>) -> Vec3 {
    match environment {
        EnvironmentSource::Studio { preset, .. } => illuminant_white_balance(preset),
        EnvironmentSource::HdrMap(_) => Vec3::ONE,
    }
}
