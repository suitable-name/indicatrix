//! Observer point-of-view camera basis shared by every metrics ray-fan.

use crate::optics::raytracer::{Camera, DEFAULT_FOV_DEG};
use glam::Vec3;

/// Distance handed to [`Camera::new`] for the basis only: the basis is the normalised
/// direction towards the origin, so any positive distance yields the same frame.
const BASIS_CAMERA_DISTANCE: f32 = 1.0;

/// Builds the observer-PoV view basis (forward, right, up) from camera yaw/pitch.
///
/// Reads the frame straight off [`Camera::new`], the render camera itself, so the
/// gemological metrics are evaluated against the frame that is actually rendered,
/// including its `world_up` choice at and past the poles.
#[must_use]
pub fn camera_view_basis(cam_yaw: f32, cam_pitch: f32) -> (Vec3, Vec3, Vec3) {
    // The field of view scales the ray spread, never the forward/right/up frame, so the
    // render camera's own default serves.
    let camera = Camera::new(cam_yaw, cam_pitch, BASIS_CAMERA_DISTANCE, DEFAULT_FOV_DEG);
    (camera.forward, camera.right, camera.up)
}
