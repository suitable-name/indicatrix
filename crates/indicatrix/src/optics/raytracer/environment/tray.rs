//! The backdrop as a lit tray: a matte grey surface of albedo `rho` below the girdle plane,
//! extending to the horizon, lit by the lighting rig.
//!
//! Its radiance is `rho * E_rig / pi`, where `E_rig` is the rig's irradiance on an upward
//! plane. The value is computed here once per (preset, light pose), on the host, and carried
//! as one `f32` (`EnvironmentSource::Studio::backdrop`) at exposure 1; the lighting arms in
//! `rig.rs` apply the exposure and the illuminant's spectral power, and show it below the
//! horizon to every escape (the camera ray and every exit alike). `0.0` means As Lit, the
//! preset's own ground.

use super::rig::unit_radiance;
use super::sun::{SUN_SOLID_ANGLE, sun_radiance_factor};
use super::{LightingModel, LightingPreset};
use crate::optics::studio_rig::StudioRig;
use glam::{DVec3, Vec3};
use std::f64::consts::PI;
use std::sync::{Mutex, PoisonError};

/// Bumped when the tray's physics changes; named by the preview fingerprint.
pub const TRAY_MODEL_VERSION: u32 = 1;
/// White paper.
pub const TRAY_ALBEDO_WHITE: f32 = 0.90;
/// The photographic 18 % grey card.
pub const TRAY_ALBEDO_GREY: f32 = 0.18;
/// Black velvet.
pub const TRAY_ALBEDO_BLACK: f32 = 0.02;

/// Directions of the irradiance quadrature: a Fibonacci spiral over the upper hemisphere.
/// 32768 points put at least 60 samples in the smallest smooth lobe (the tent spark,
/// 0.012 sr), so the tray does not wobble while the light is dragged.
const IRRADIANCE_SAMPLES: u32 = 32_768;

/// The last `(preset, yaw bits, pitch bits, value)` computed; one entry is enough because the
/// light pose changes rarely relative to frames.
static CACHE: Mutex<Option<(LightingPreset, u32, u32, f32)>> = Mutex::new(None);

/// Direction `index` of the upper-hemisphere spiral, `y = (index + 0.5) / N`, `phi = index *
/// golden angle` (equal-area: every direction carries `2 pi / N` sr).
fn hemisphere_direction(index: u32) -> (Vec3, f64) {
    let y = (f64::from(index) + 0.5) / f64::from(IRRADIANCE_SAMPLES);
    let radius = y.mul_add(-y, 1.0).max(0.0).sqrt();
    let phi = f64::from(index) * (PI * (3.0 - 5.0_f64.sqrt()));
    let (sin_phi, cos_phi) = phi.sin_cos();
    (
        DVec3::new(radius * cos_phi, y, radius * sin_phi).as_vec3(),
        y,
    )
}

/// `E_geom / pi` without the cache; see [`rig_irradiance_over_pi`].
#[allow(
    clippy::cast_possible_truncation,
    reason = "the f64 accumulation is rounded to the f32 the scene carries, once"
)]
fn irradiance_over_pi_uncached(preset: LightingPreset, light_yaw: f32, light_pitch: f32) -> f32 {
    // ASET is scored as the ISO dome with a neutral D65 tray: `E / pi := 1`.
    if preset.model() == LightingModel::Aset {
        return 1.0;
    }
    let rig = StudioRig::new(light_yaw, light_pitch);
    // The sun disc (7e-5 sr) cannot be sampled: the quadrature runs over the sky alone (the
    // sun preset's non-disc arithmetic is the dome's, bit for bit) and the disc is added
    // analytically below.
    let geometry = if preset.model() == LightingModel::DaylightSun {
        LightingPreset::DaylightDome
    } else {
        preset
    };
    let mut sum = 0.0_f64;
    for index in 0..IRRADIANCE_SAMPLES {
        let (dir, cos_theta) = hemisphere_direction(index);
        sum = f64::from(unit_radiance(dir, geometry, &rig)).mul_add(cos_theta, sum);
    }
    // E / pi = (2 pi / N) * sum / pi
    let mut over_pi = (2.0 / f64::from(IRRADIANCE_SAMPLES)) * sum;
    if preset.model() == LightingModel::DaylightSun {
        let key = rig.key_dir;
        over_pi += f64::from(sun_radiance_factor(key))
            * f64::from(SUN_SOLID_ANGLE)
            * f64::from(key.y.max(0.0))
            / PI;
    }
    over_pi as f32
}

/// `E_geom / pi` of `preset` at the light pose (yaw and pitch in radians).
///
/// This is the rig's irradiance on an upward plane at exposure 1, observer off, divided by pi. Cached (one
/// entry, keyed by the preset and the pose's bits); a pure function of its inputs.
#[must_use]
pub fn rig_irradiance_over_pi(preset: LightingPreset, light_yaw: f32, light_pitch: f32) -> f32 {
    let (yaw_bits, pitch_bits) = (light_yaw.to_bits(), light_pitch.to_bits());
    let hit = (*CACHE.lock().unwrap_or_else(PoisonError::into_inner))
        .filter(|entry| entry.0 == preset && entry.1 == yaw_bits && entry.2 == pitch_bits);
    if let Some((_, _, _, value)) = hit {
        return value;
    }
    let value = irradiance_over_pi_uncached(preset, light_yaw, light_pitch);
    *CACHE.lock().unwrap_or_else(PoisonError::into_inner) =
        Some((preset, yaw_bits, pitch_bits, value));
    value
}

/// `albedo * rig_irradiance_over_pi(..)`: the value `EnvironmentSource::Studio::backdrop`
/// carries. An albedo of `0.0` (or less) is As Lit and returns `0.0` without any integration.
#[must_use]
pub fn tray_radiance(albedo: f32, preset: LightingPreset, light_yaw: f32, light_pitch: f32) -> f32 {
    if albedo > 0.0 {
        albedo * rig_irradiance_over_pi(preset, light_yaw, light_pitch)
    } else {
        0.0
    }
}
