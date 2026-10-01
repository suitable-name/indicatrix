//! Pick-frame screen types and the camera projection, mirroring the solid
//! rasterizer's own `project` and the path tracer's `Camera::generate_ray`.

use glam::Vec3;
use indicatrix::optics::raytracer::{Camera, Ray};

/// The rasterizer's near-plane guard (`indicatrix_solid::raster::NEAR_EPS`): a point
/// whose depth along the camera's forward axis is at or below this does not project.
const NEAR_EPS: f32 = 1e-4;

/// A point in the pick frame: physical pixels of the solid raster, origin top-left.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenPoint {
    /// Pixels from the left edge.
    pub x: f32,
    /// Pixels from the top edge (grows downward).
    pub y: f32,
}

impl ScreenPoint {
    /// The point `(x, y)`.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// The `(dx, dy)` pixel offset from `self` to `other`.
    #[must_use]
    pub const fn delta_to(self, other: Self) -> (f32, f32) {
        (other.x - self.x, other.y - self.y)
    }

    /// The straight-line distance in pixels between `self` and `other`.
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        let (dx, dy) = self.delta_to(other);
        dx.hypot(dy)
    }

    /// The point halfway between `self` and `other`.
    #[must_use]
    pub fn midpoint(self, other: Self) -> Self {
        Self::new(
            0.5_f32.mul_add(other.x - self.x, self.x),
            0.5_f32.mul_add(other.y - self.y, self.y),
        )
    }
}

/// The pick frame's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenSize {
    /// Width in pixels.
    pub width: f32,
    /// Height in pixels.
    pub height: f32,
}

impl ScreenSize {
    /// A frame of `width` by `height` pixels.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Whether both sides are strictly positive (a NaN side is not).
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }
}

/// The exact algebraic inverse of [`Camera::generate_ray`] with zero jitter, op for op
/// the rasterizer's `project`: the pick-frame pixel `p` lands on.
///
/// `None` when `p` is behind (or within a hair of) the camera plane, or when `size` is
/// not a positive frame.
#[must_use]
pub fn project(camera: &Camera, p: Vec3, size: ScreenSize) -> Option<ScreenPoint> {
    if !size.is_valid() {
        return None;
    }
    let delta = p - camera.origin;
    let depth = delta.dot(camera.forward);
    if depth <= NEAR_EPS {
        return None;
    }
    let right_comp = delta.dot(camera.right);
    let up_comp = delta.dot(camera.up);
    let u_ratio = right_comp / depth;
    let v_ratio = up_comp / depth;
    let aspect = size.width / size.height;
    Some(ScreenPoint::new(
        size.width * (0.5 + u_ratio / (2.0 * aspect * camera.fov_tan)),
        size.height * (0.5 - v_ratio / (2.0 * camera.fov_tan)),
    ))
}

/// The camera ray through the pick-frame pixel `p` -- [`Camera::generate_ray`] with
/// zero jitter, so a click lands on the same ray the raster and the path tracer cast.
#[must_use]
pub fn unproject(camera: &Camera, p: ScreenPoint, size: ScreenSize) -> Ray {
    camera.generate_ray(p.x, p.y, size.width, size.height, 0.0, 0.0)
}
