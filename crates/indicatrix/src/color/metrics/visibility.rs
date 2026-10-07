//! Whether an exited ray is visibly returned to the observer -- the shared
//! head-shadow/illumination test behind brilliance, extinction, and
//! scintillation classification.

use glam::Vec3;

use super::lighting::ExitLighting;
use crate::optics::raytracer::DEFAULT_HEAD_SHADOW_DEG;

/// Cosine of the half-angle of the observer's head-shadow cone at the default size (16
/// degrees): an exit direction closer than this to the eye direction is lost to the
/// observer's own shadow. A literal, not `cos(16 deg)`, so the default stays bit-identical
/// to the metrics before the cone followed the scene.
pub(super) const HEAD_SHADOW_COS: f32 = 0.96;

/// A cosine no dot product of unit vectors exceeds: the cone is switched off.
const NO_HEAD_SHADOW_COS: f32 = 2.0;

/// The cosine of the head-shadow cone the metrics apply.
///
/// `Studio` rigs ignore the scene's head-shadow slider, so they (and an HDR map, which has
/// no head shadow) keep the fixed [`HEAD_SHADOW_COS`]. The lit models follow the scene's
/// `head_shadow_deg`: the default 16 degrees returns exactly [`HEAD_SHADOW_COS`], `0` (or
/// less) switches the cone off, anything else is `cos(deg + (acos(0.96) - 16 deg))`
/// (hard edge, at the render's centre of the smoothstep fall-off; the shift keeps the cone
/// continuous through the default). NaN is the default.
///
/// The optimizer builds its environment from a preset with the default head shadow, so
/// it always scores under 0.96.
#[must_use]
pub(super) fn head_shadow_cone_cos(studio_rig: bool, head_shadow_deg: f32) -> f32 {
    if studio_rig
        || head_shadow_deg.is_nan()
        || head_shadow_deg.to_bits() == DEFAULT_HEAD_SHADOW_DEG.to_bits()
    {
        return HEAD_SHADOW_COS;
    }
    if head_shadow_deg <= 0.0 {
        return NO_HEAD_SHADOW_COS;
    }
    // `0.96` is not exactly `cos(16 deg)` (it is `cos(16.2602 deg)`): shift every other size
    // by the same angle so the cone is continuous through the default instead of stepping
    // 0.26 degrees there.
    let offset = HEAD_SHADOW_COS.acos() - 16.0f32.to_radians();
    (head_shadow_deg.clamp(1.0, 89.0).to_radians() + offset).cos()
}

/// Whether a ray that exited the gem in direction `exit_dir` is visibly returned to an
/// observer at `cam_forward` -- not lost to the observer's own head-shadow, and looking at
/// a light source of `lighting` (the radiance the tracer lights the image with, see
/// [`ExitLighting`]). The same test used for `brilliance_pct`/`extinction_pct`
/// classification in the main loop, factored out so the Scintillation temporal sub-poses
/// (see `cell_returned_at_yaw_offset`) apply an identical definition of "returned".
#[must_use]
pub(super) fn ray_is_visibly_returned(
    exit_dir: Vec3,
    cam_forward: Vec3,
    lighting: &ExitLighting<'_>,
) -> bool {
    let is_head_shadow = (-exit_dir).dot(cam_forward) > lighting.head_shadow_cos();
    !is_head_shadow && lighting.is_illuminated(exit_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_is_exactly_the_literal_cosine() {
        assert_eq!(
            head_shadow_cone_cos(false, DEFAULT_HEAD_SHADOW_DEG).to_bits(),
            0.96f32.to_bits()
        );
        assert_eq!(
            head_shadow_cone_cos(false, f32::NAN).to_bits(),
            0.96f32.to_bits()
        );
    }

    #[test]
    fn zero_size_removes_the_cone_for_a_lit_model() {
        let cos = head_shadow_cone_cos(false, 0.0);
        assert!(cos > 1.0, "no unit dot product may exceed {cos}");
    }

    #[test]
    fn other_sizes_follow_the_scene_for_a_lit_model() {
        let offset = HEAD_SHADOW_COS.acos() - 16.0f32.to_radians();
        let cos = head_shadow_cone_cos(false, 30.0);
        let expected = (30.0f32.to_radians() + offset).cos();
        assert!((cos - expected).abs() < 1e-6);
    }

    #[test]
    fn the_cone_is_continuous_through_the_default() {
        let below = head_shadow_cone_cos(false, 15.999);
        let above = head_shadow_cone_cos(false, 16.001);
        assert!((below - 0.96).abs() < 1e-4, "{below}");
        assert!((above - 0.96).abs() < 1e-4, "{above}");
        // Monotone: a larger size is a larger cone, a smaller cosine.
        assert!(above < below);
    }

    #[test]
    fn studio_rigs_ignore_the_size() {
        for deg in [0.0, 5.0, 16.0, 40.0] {
            assert_eq!(head_shadow_cone_cos(true, deg).to_bits(), 0.96f32.to_bits());
        }
    }
}
