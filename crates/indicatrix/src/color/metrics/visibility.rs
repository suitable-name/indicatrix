//! Whether an exited ray is visibly returned to the observer -- the shared
//! head-shadow/illumination test behind brilliance, extinction, and
//! scintillation classification.

use glam::Vec3;

use super::{image_types::ExtinctionReason, lighting::ExitLighting};
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

/// Why a ray that exited the gem in direction `exit_dir` is not visibly returned to an
/// observer whose eye lies in unit direction `observer` from the stone (the reverse of the
/// viewing ray, as in the beauty render), or `Ok` when it is. Not returned means lost to
/// the observer's own head-shadow cone around `observer`
/// ([`ExtinctionReason::ObserverShadow`]) or not looking at a light source of `lighting`
/// ([`ExtinctionReason::InsufficientIllumination`]). The single definition behind both the
/// bucketing and the per-pixel reason, so the two cannot drift apart.
pub(super) fn returned_or_reason(
    exit_dir: Vec3,
    observer: Vec3,
    lighting: &ExitLighting<'_>,
) -> Result<(), ExtinctionReason> {
    // `exit_dir . observer` is bit for bit `(-exit_dir) . (-observer)`: negating every
    // component flips the sign of each product and of their sum, exactly.
    if exit_dir.dot(observer) > lighting.head_shadow_cos() {
        Err(ExtinctionReason::ObserverShadow)
    } else if lighting.is_illuminated(exit_dir) {
        Ok(())
    } else {
        Err(ExtinctionReason::InsufficientIllumination)
    }
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
    returned_or_reason(exit_dir, -cam_forward, lighting).is_ok()
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

    /// A lit model whose cone is 30 degrees: an exit pointing straight back at the eye is
    /// shadowed, the same exit judged against an eye 60 degrees away is not.
    #[test]
    fn the_cone_is_centred_on_the_observer_passed_in() {
        use crate::optics::raytracer::LightingPreset;
        let lighting = ExitLighting::new(
            LightingPreset::IsoHemisphere
                .studio(1.0, 0.85, 0.95)
                .with_head_shadow(30.0),
        );
        let exit_dir = Vec3::new(0.1, 0.9, 0.2).normalize();
        // The eye lies straight along the exit direction; the viewing ray is its reverse.
        let towards_eye = exit_dir;
        assert_eq!(
            returned_or_reason(exit_dir, towards_eye, &lighting),
            Err(ExtinctionReason::ObserverShadow)
        );
        assert!(!ray_is_visibly_returned(exit_dir, -towards_eye, &lighting));
        // Rotate the eye 60 degrees away from the exit direction (about an axis
        // perpendicular to it): clearly outside the cone, whose edge sits just above 30.
        let axis = exit_dir.cross(Vec3::X).normalize();
        let far_observer = glam::Quat::from_axis_angle(axis, 60.0f32.to_radians()) * towards_eye;
        assert_ne!(
            returned_or_reason(exit_dir, far_observer, &lighting),
            Err(ExtinctionReason::ObserverShadow)
        );
    }

    /// The helper decides exactly as the pre-refactor expression did, for every direction
    /// of a grid and every lighting family: the legacy callers' scores stay bit-identical.
    #[test]
    fn the_shared_helper_matches_the_original_predicate_bitwise() {
        use crate::optics::raytracer::LightingPreset;
        let observer = Vec3::new(0.2, -0.9, 0.35).normalize();
        for preset in [
            LightingPreset::RingLights,
            LightingPreset::LightTent,
            LightingPreset::IsoHemisphere,
        ] {
            for deg in [0.0, 16.0, 30.0] {
                let lighting =
                    ExitLighting::new(preset.studio(1.0, 0.85, 0.95).with_head_shadow(deg));
                for i in 0..24 {
                    for j in 0..12 {
                        let azimuth = i as f32 * std::f32::consts::TAU / 24.0;
                        let y = (j as f32 + 0.5) / 12.0;
                        let r = y.mul_add(-y, 1.0).sqrt();
                        let exit_dir = Vec3::new(r * azimuth.cos(), y, r * azimuth.sin());
                        assert_eq!(
                            exit_dir.dot(-observer).to_bits(),
                            (-exit_dir).dot(observer).to_bits(),
                            "the eye-side dot is bit for bit the legacy one"
                        );
                        // `!(a > b)`, written so that a NaN still counts as "not shadowed".
                        let original = (-exit_dir)
                            .dot(observer)
                            .partial_cmp(&lighting.head_shadow_cos())
                            != Some(std::cmp::Ordering::Greater)
                            && lighting.is_illuminated(exit_dir);
                        assert_eq!(
                            ray_is_visibly_returned(exit_dir, observer, &lighting),
                            original
                        );
                        assert_eq!(
                            returned_or_reason(exit_dir, -observer, &lighting).is_ok(),
                            original
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn studio_rigs_ignore_the_size() {
        for deg in [0.0, 5.0, 16.0, 40.0] {
            assert_eq!(head_shadow_cone_cos(true, deg).to_bits(), 0.96f32.to_bits());
        }
    }
}
