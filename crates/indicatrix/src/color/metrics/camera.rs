//! Observer point-of-view camera basis shared by every metrics ray-fan.

use glam::Vec3;

/// Builds the observer-PoV view basis (forward, right, up) from camera yaw/pitch.
///
/// Uses the exact same convention as `Camera::new` in `optics::raytracer` (same
/// `world_up` fallback threshold and axis), so that gemological metrics are evaluated
/// against the same frame that is actually rendered.
#[must_use]
pub fn camera_view_basis(cam_yaw: f32, cam_pitch: f32) -> (Vec3, Vec3, Vec3) {
    let cos_cp = cam_pitch.cos();
    let sin_cp = cam_pitch.sin();
    let cos_cy = cam_yaw.cos();
    let sin_cy = cam_yaw.sin();
    let cam_forward = Vec3::new(-cos_cp * sin_cy, -sin_cp, -cos_cp * cos_cy).normalize();
    let world_up = if cos_cp.abs() < 1e-4 {
        Vec3::new(0.0, 0.0, -1.0)
    } else {
        Vec3::Y
    };
    let cam_right = cam_forward.cross(world_up).normalize();
    let cam_up = cam_right.cross(cam_forward).normalize();
    (cam_forward, cam_right, cam_up)
}
