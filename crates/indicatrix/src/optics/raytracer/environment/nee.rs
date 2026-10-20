//! Next-event-estimation sampling and pdf lookups against the analytic sun and an `HdrMap`.
//!
//! Moved verbatim out of `rig.rs` (backdrop campaign, lane B1) to keep that file under its
//! size limit; no arithmetic changed.

use super::sun::{SUN_SOLID_ANGLE, sun_cone_direction, sun_nee_pdf, sun_radiance_factor};
use super::{EnvironmentSource, LightingModel, LightingPreset};
use glam::Vec3;

/// One next-event-estimation draw toward the environment: the sampled
/// direction, its solid-angle-measure pdf under [`EnvironmentMap::sample`]'s own
/// importance distribution, and the raw RGB radiance in that direction (from which a
/// caller lifts each channel's own spectral radiance via
/// [`crate::renderer::env_map::rgb_to_spectral_radiance`] -- the same lift
/// [`EnvironmentMap::radiance_at`] itself uses).
///
/// [`EnvironmentMap::sample`]: crate::renderer::env_map::EnvironmentMap::sample
/// [`EnvironmentMap::radiance_at`]: crate::renderer::env_map::EnvironmentMap::radiance_at
///
/// For the analytic sun ([`LightingModel::DaylightSun`]) `rgb` is unused and `sun` carries
/// the wavelength-independent radiance factor instead, because the sun's radiance is the
/// preset's own spectral power times a scalar, not an RGB texel: use [`Self::radiance`],
/// which picks the right source.
pub(in super::super) struct EnvNeeSample {
    pub(in super::super) dir: Vec3,
    pub(in super::super) pdf: f32,
    pub(in super::super) rgb: [f32; 3],
    /// `Some` for an analytic-sun draw, `None` for an HDR-map draw.
    pub(in super::super) sun: Option<SunNeeRadiance>,
}

/// The radiance of an analytic-sun NEE draw: `geometry * (spectral_power(lambda) *
/// exposure)`, the very expression [`DirectionLighting::Scaled`] evaluates for a direction
/// inside the disc, so the light-sampled and the BSDF-sampled technique see the same
/// radiance.
#[derive(Clone, Copy)]
pub(in super::super) struct SunNeeRadiance {
    geometry: f32,
    exposure: f32,
    preset: LightingPreset,
}

impl EnvNeeSample {
    /// This draw's spectral radiance at `lambda_nm`: the HDR texel lifted to a spectrum, or
    /// the sun's `geometry * (spectral_power * exposure)`.
    #[must_use]
    pub(in super::super) fn radiance(&self, lambda_nm: f32) -> f32 {
        self.sun.map_or_else(
            || crate::renderer::env_map::rgb_to_spectral_radiance(self.rgb, lambda_nm),
            |sun| sun.geometry * (sun.preset.spectral_power(lambda_nm) * sun.exposure),
        )
    }
}

/// Whether `environment` offers a light-sampling (NEE) technique at all: an HDR map
/// (importance-sampled texels) or the analytic sun of [`LightingPreset::DaylightSun`]
/// (uniform cone sampling). Every other analytic rig is smooth enough for BSDF sampling
/// alone. Replaces the old `matches!(environment, HdrMap(_))` switch at the entry points.
#[must_use]
pub(in super::super) fn environment_supports_nee(environment: EnvironmentSource<'_>) -> bool {
    match environment {
        EnvironmentSource::Studio { preset, .. } => preset.model() == LightingModel::DaylightSun,
        EnvironmentSource::HdrMap(_) => true,
    }
}

/// Draws one NEE direction from `environment`'s own importance distribution, from two
/// independent uniform `[0, 1)` randoms.
///
/// - [`EnvironmentSource::HdrMap`]: the map's importance distribution; `None` for a
///   degenerate draw (`pdf <= 0.0`, e.g. a direction whose row/column solid angle collapsed
///   to nothing at a pole).
/// - [`EnvironmentSource::Studio`] with [`LightingPreset::DaylightSun`]: a direction
///   uniform over the sun disc ([`sun_cone_direction`]), `pdf = 1 / SUN_SOLID_ANGLE`.
/// - every other analytic rig: `None` (no light-sampling technique, see
///   `scattering::NeeContext`'s doc comment).
///
/// Either way a `None` means the caller should simply skip this NEE contribution for the
/// current event, not fabricate one.
#[must_use]
pub(in super::super) fn sample_environment_for_nee(
    environment: EnvironmentSource<'_>,
    u0: f32,
    u1: f32,
) -> Option<EnvNeeSample> {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            light_yaw,
            light_pitch,
            ..
        } if preset.model() == LightingModel::DaylightSun => {
            let key_dir = key_dir_only(light_yaw, light_pitch);
            Some(EnvNeeSample {
                dir: sun_cone_direction(key_dir, u0, u1),
                pdf: 1.0 / SUN_SOLID_ANGLE,
                rgb: [0.0; 3],
                sun: Some(SunNeeRadiance {
                    geometry: sun_radiance_factor(key_dir),
                    exposure,
                    preset,
                }),
            })
        }
        EnvironmentSource::Studio { .. } => None,
        EnvironmentSource::HdrMap(map) => {
            let (dir, rgb, pdf) = map.sample(u0, u1);
            (pdf > 0.0).then_some(EnvNeeSample {
                dir,
                pdf,
                rgb,
                sun: None,
            })
        }
    }
}

/// The key light's direction for a light pose, with exactly the arithmetic of
/// `StudioRig::new`'s `key_dir` (bit-identical, pinned by a test) but without building the
/// whole rig (~40 sin/cos and 16 normalizes), which the per-event NEE entry points do not need.
fn key_dir_only(light_yaw: f32, light_pitch: f32) -> Vec3 {
    let cos_pitch = light_pitch.cos();
    let sin_pitch = light_pitch.sin();
    let cos_yaw = light_yaw.cos();
    let sin_yaw = light_yaw.sin();
    Vec3::new(cos_pitch * sin_yaw, sin_pitch, cos_pitch * cos_yaw).normalize()
}

/// Test hook: [`key_dir_only`] for the bit-identity test against `StudioRig`.
#[cfg(test)]
pub(super) fn key_dir_only_for_test(light_yaw: f32, light_pitch: f32) -> Vec3 {
    key_dir_only(light_yaw, light_pitch)
}

/// The solid-angle-measure pdf [`sample_environment_for_nee`] would assign to `dir`,
/// computed independently of any particular sample -- the OTHER half of a balance-heuristic
/// MIS weight: evaluating the light-sampling technique's own density at a direction the
/// COMPETING (BSDF/phase) technique produced. `0.0` for every [`EnvironmentSource::Studio`]
/// preset except [`LightingPreset::DaylightSun`] (matching [`sample_environment_for_nee`]'s
/// `None`: no light-sampling technique exists to compete against there); for the sun it is
/// `1 / SUN_SOLID_ANGLE` inside the disc and `0.0` outside ([`sun_nee_pdf`]).
#[must_use]
pub(in super::super) fn environment_nee_pdf(environment: EnvironmentSource<'_>, dir: Vec3) -> f32 {
    match environment {
        EnvironmentSource::Studio {
            preset,
            light_yaw,
            light_pitch,
            ..
        } if preset.model() == LightingModel::DaylightSun => {
            sun_nee_pdf(key_dir_only(light_yaw, light_pitch), dir)
        }
        EnvironmentSource::Studio { .. } => 0.0,
        EnvironmentSource::HdrMap(map) => map.pdf(dir),
    }
}
