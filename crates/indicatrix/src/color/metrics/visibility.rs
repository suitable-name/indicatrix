//! Whether an exited ray is visibly returned to the observer -- the shared
//! head-shadow/illumination test behind brilliance, extinction, and
//! scintillation classification.

use glam::Vec3;

/// Whether a ray that exited the gem in direction `exit_dir` is visibly returned to an
/// observer at `cam_forward` under the given key/fill/overhead-ring illumination -- not
/// lost to the observer's own head-shadow, and actually collected by at least one light
/// source. The same test used for `brilliance_pct`/`extinction_pct` classification in
/// the main loop below, factored out so the Scintillation temporal sub-poses (see
/// `cell_returned_at_yaw_offset`) apply an identical definition of "returned".
#[must_use]
pub(super) fn ray_is_visibly_returned(
    exit_dir: Vec3,
    cam_forward: Vec3,
    key_dir: Vec3,
    fill_dir: Vec3,
    sin_lp: f32,
) -> bool {
    // 1. Head-shadow cone (angle < 16 deg from viewing vector)
    let is_head_shadow = (-exit_dir).dot(cam_forward) > 0.96;

    // 2. Light collection from Key, Fill, or Overhead ring illumination
    let key_dot = exit_dir.dot(key_dir).max(0.0);
    let fill_dot = exit_dir.dot(fill_dir).max(0.0);
    let ring_dot = sin_lp.mul_add(-0.8, exit_dir.y).abs() < 0.35;
    let is_illuminated = (key_dot > 0.70) || (fill_dot > 0.75) || (ring_dot && exit_dir.y > 0.2);

    !is_head_shadow && is_illuminated
}
