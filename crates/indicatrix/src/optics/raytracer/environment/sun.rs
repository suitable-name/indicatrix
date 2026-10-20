//! The direct sun of [`LightingModel::DaylightSun`](super::LightingModel::DaylightSun):
//! disc constants, radiance factor and the cone sampler / pdf for next-event estimation.
//!
//! Moved verbatim out of `rig.rs` (wave 0 of the backdrop campaign); no arithmetic changed.

use super::rig::horizon_blend;
use glam::Vec3;

// ---------------------------------------------------------------------------------
// The direct sun of `LightingModel::DaylightSun` (GPU id 5).
//
// Geometry. A disc of angular radius 0.27 degrees about the key direction (the real sun
// is 0.266 +- 0.008). `SUN_DISC_COS` is cos(0.27 deg) rounded to f32; the f32 value is
// `1 - 186 * 2^-24`, so the solid angle is derived from that very literal:
//     SUN_ONE_MINUS_COS = 186 * 2^-24                  = 1.1086464e-5
//     SUN_SOLID_ANGLE   = 2 pi * SUN_ONE_MINUS_COS     = 6.9658306e-5 sr
// (the exact small-angle value pi r^2 for 0.27 degrees is 6.9766e-5 sr; the 0.16 % difference
// is the f32 rounding of the cosine, and is irrelevant because every consumer reads the
// literals; the disc radius it encodes is 0.2698 degrees).
//
// Radiance. The sky it sits in is `DaylightDome`'s: `sky(y) = 0.18 - 0.08 y` (y = d.y)
// plus a `0.30 cos^8` aureole around the key. Horizontal irradiance of that sky on an
// upward-facing plane (unit radiance units of the model):
//     E_sky_dome = 2 pi * integral_0^1 (0.18 y - 0.08 y^2) dy = 2 pi (0.09 - 0.08 / 3)
//                = 0.39794
//     E_aureole  = 0.30 * sin(e) * 2 pi / 10          (lobe fully above the horizon for
//                                                      e >~ 30 deg; sin(e) = key.y)
//                = 0.17927 at the default key pitch of 72 degrees (sin = 0.95106)
//     E_sky      = 0.57721 at e = 72 deg.
// The direct sun delivers E_sun = L_sun * SUN_SOLID_ANGLE * sin(e). Wanting E_sun about
// 82 % of E_sky + E_sun (a clear day: 80-85 % direct, the rest skylight):
//     E_sun = 0.82 / 0.18 * 0.57721 = 2.6295  ->  L_sun = 2.6295 / (6.9658e-5 * 0.95106)
//           = 39 691  ->  SUN_RADIANCE = 40 000   (E_sun = 2.650, direct share 82.1 %).
// At other key elevations the share follows sin(e): 73.9 % at 30 degrees (E_sun 1.393,
// E_sky 0.492), 87.5 % at 90 degrees. The radiance ratio to the sky (40 000 / 0.14 ~ 3e5)
// is the right order for a real clear sky (sun ~1.6e9 cd/m2, sky ~5e3 cd/m2, ratio ~3e5).
//
// The sun is NOT dimmed by the observer's head shadow: the shadow models the observer's
// body blocking the large sky dome reflected in a face-up table, and the NEE draw has no
// access to the observer. It IS faded by the horizon blend evaluated once at the key
// direction (`sun_radiance_factor`), so a key below the horizon switches the sun off
// everywhere on the disc.
//
// Spectrum: D65 for now. Direct sun is ~5500-5800 K (warmer than the 6500 K whole-sky mix);
// a separate sun SPD would need its own `spec_power` input in the twins.
// ---------------------------------------------------------------------------------

/// Cosine of the sun disc's angular radius (0.27 degrees), as an f32 literal.
pub(super) const SUN_DISC_COS: f32 = 0.999_988_9;
/// `1 - SUN_DISC_COS`, exactly (`186 * 2^-24`), kept as its own literal so the cone sampler
/// never subtracts two nearly equal numbers.
pub(super) const SUN_ONE_MINUS_COS: f32 = 1.108_646_4e-5;
/// Solid angle of the sun disc in steradians: `2 pi * SUN_ONE_MINUS_COS`.
pub(super) const SUN_SOLID_ANGLE: f32 = 6.965_831e-5;
/// Radiance of the sun disc in the dome's radiance units (see the derivation above).
pub(super) const SUN_RADIANCE: f32 = 40_000.0;

/// The sun's wavelength-independent radiance factor: [`SUN_RADIANCE`] faded by the horizon
/// blend at the key direction. The disc is uniform, so this one number is the sun's radiance
/// for every direction inside it, for the BSDF-sampled lookup and the NEE draw alike.
pub(super) fn sun_radiance_factor(key_dir: Vec3) -> f32 {
    SUN_RADIANCE * horizon_blend(key_dir)
}

/// A direction drawn uniformly over the sun disc about `key_dir` from two uniform `[0, 1)`
/// randoms (equal-area cone sampling: `1 - cos(theta) = u0 * (1 - cos(theta_max))`).
/// `sin(theta)` comes from `(1 - cos)(1 + cos)` with `1 - cos` kept exact, so the radial
/// resolution is not quantised by the f32 spacing of the cosine near 1. The WGSL twin
/// `daylight_sun_cone_direction` is operation for operation the same.
pub(super) fn sun_cone_direction(key_dir: Vec3, u0: f32, u1: f32) -> Vec3 {
    let one_minus_cos = u0 * SUN_ONE_MINUS_COS;
    let cos_t = 1.0 - one_minus_cos;
    let sin_t = (one_minus_cos * (2.0 - one_minus_cos)).max(0.0).sqrt();
    let phi = 2.0 * std::f32::consts::PI * u1;
    let (sin_p, cos_p) = phi.sin_cos();
    let a = if key_dir.x.abs() > 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t = (a - key_dir * key_dir.dot(a)).normalize_or_zero();
    let b = key_dir.cross(t);
    (t * (sin_t * cos_p) + b * (sin_t * sin_p) + key_dir * cos_t).normalize_or_zero()
}

/// The solid-angle pdf of [`sun_cone_direction`] at `dir`: `1 / SUN_SOLID_ANGLE` inside the
/// disc, `0.0` outside. `dir` must be a unit vector.
pub(super) fn sun_nee_pdf(key_dir: Vec3, dir: Vec3) -> f32 {
    if dir.dot(key_dir) >= SUN_DISC_COS {
        1.0 / SUN_SOLID_ANGLE
    } else {
        0.0
    }
}
