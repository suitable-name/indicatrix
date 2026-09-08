//! Space-aware gamut mapping.
//!
//! Compresses out-of-gamut CIE XYZ colours into a target [`ColorSpace`] by walking the
//! chromaticity radially toward that space's white point at constant luminance, rather
//! than clipping each RGB channel independently (which shifts hue as well as
//! saturation).
//!
//! Generalises `optics::raytracer::xyz_to_srgb_gamma`'s gamut-mapping step to any
//! [`ColorSpace`]; see `tests/color_tests.rs` for the cross-check.

use glam::Vec3;

use super::space::ColorSpace;

/// Radially compresses an out-of-gamut colour toward `space`'s white point.
///
/// Walks the CIE xyY chromaticity of `xyz` toward `space`'s white point at constant
/// luminance until every RGB channel from `space`'s XYZ->RGB matrix
/// ([`ColorSpace::xyz_to_linear`]) is non-negative; in-gamut colours pass through
/// unchanged. Result is linear RGB -- apply [`ColorSpace::transfer_function`] or use
/// [`ColorSpace::encode`] for the full pipeline. Non-finite or near-zero `xyz`
/// (component sum `<= 1e-6`) maps to `Vec3::ZERO` rather than dividing by near-zero.
///
/// Thin wrapper around [`project_to_gamut_bounded`] with `max = f32::INFINITY`.
#[must_use]
pub fn project_to_gamut(xyz: Vec3, space: ColorSpace) -> Vec3 {
    project_to_gamut_bounded(xyz, space, f32::INFINITY)
}

/// As [`project_to_gamut`], but also caps every output channel at `max`.
///
/// `ColorSpace::encode`'s ACES tone mapping scales RGB by one luminance-derived
/// factor so a saturated colour's hue survives (see
/// [`crate::color::ToneMap::AcesFilmic`]); hard-clamping a channel above `1.0`
/// per channel during quantization would reintroduce that hue shift. This
/// bounded walk desaturates an over-bright colour toward white instead.
///
/// Reuses [`project_to_gamut`]'s walk with both a floor and ceiling per channel.
/// The white-point fallback step is not itself clamped to `max`: every channel
/// there is equal by construction, so a small overshoot (e.g. ACES's ~1.033
/// highlight asymptote) is hue-neutral and left for `encode`'s final clamp.
#[must_use]
pub fn project_to_gamut_bounded(xyz: Vec3, space: ColorSpace, max: f32) -> Vec3 {
    const STEPS: u32 = 32;

    let sum = xyz.x + xyz.y + xyz.z;
    if !sum.is_finite() || sum <= 1e-6 {
        return Vec3::ZERO;
    }

    let x = xyz.x / sum;
    let y = xyz.y / sum;
    let luminance = xyz.y.max(0.0);

    let linear = space.xyz_to_linear(xyz);
    let in_range =
        |v: Vec3| v.x >= 0.0 && v.y >= 0.0 && v.z >= 0.0 && v.x <= max && v.y <= max && v.z <= max;
    if in_range(linear) {
        return linear;
    }

    // Walk toward the white point, taking the smallest step in range.
    let (white_x, white_y) = space.white_point_xy();

    let mapped_xyz_for_t = |t: f32| -> Vec3 {
        let xp = (white_x - x).mul_add(t, x);
        let yp = (white_y - y).mul_add(t, y);
        if yp > 1e-6 {
            Vec3::new(
                (xp / yp) * luminance,
                luminance,
                ((1.0 - xp - yp) / yp) * luminance,
            )
        } else {
            Vec3::new(
                (white_x / white_y) * luminance,
                luminance,
                ((1.0 - white_x - white_y) / white_y) * luminance,
            )
        }
    };

    let mut resolved = space.xyz_to_linear(mapped_xyz_for_t(1.0)).max(Vec3::ZERO);
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let candidate = space.xyz_to_linear(mapped_xyz_for_t(t));
        if in_range(candidate) {
            resolved = candidate;
            break;
        }
    }
    resolved
}

/// sRGB-specialised convenience wrapper around [`project_to_gamut`].
///
/// Named entry point for callers that only need sRGB, rather than plumbing
/// `ColorSpace::Srgb` through everywhere.
#[must_use]
pub fn project_to_srgb(xyz: Vec3) -> Vec3 {
    project_to_gamut(xyz, ColorSpace::Srgb)
}
