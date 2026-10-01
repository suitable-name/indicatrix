//! Whether an exited ray is visibly returned to the observer -- the shared
//! head-shadow/illumination test behind brilliance, extinction, and
//! scintillation classification.

use glam::Vec3;

use super::lighting::ExitLighting;

/// Cosine of the half-angle of the observer's head-shadow cone (16 degrees): an exit
/// direction closer than this to the eye direction is lost to the observer's own shadow.
const HEAD_SHADOW_COS: f32 = 0.96;

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
    let is_head_shadow = (-exit_dir).dot(cam_forward) > HEAD_SHADOW_COS;
    !is_head_shadow && lighting.is_illuminated(exit_dir)
}
