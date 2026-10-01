//! The orbit camera's pose math, shared by the desktop's
//! `gui::render::camera_lighting` and the web app's Solid view.
//!
//! [`orbit_distance_bounds`], [`fit_distance_for_radius`] and [`wrap_pitch`] moved
//! here verbatim from the desktop (which re-exports them and keeps their tests as
//! the pins); [`orbit_step`], [`zoom_step`], [`standard_view`] and [`RESET_POSE`]
//! are the desktop callbacks' inline arithmetic as named functions, with their own
//! tests below.

use super::types::CameraPose;
use indicatrix::optics::raytracer::{DEFAULT_FOV_DEG, DEFAULT_POSE};
use std::f32::consts::FRAC_PI_2;

/// The desktop's Reset pose (`camera_lighting`'s `on_reset_camera`): the shared
/// [`DEFAULT_POSE`].
pub const RESET_POSE: CameraPose = CameraPose {
    yaw: DEFAULT_POSE.yaw,
    pitch: DEFAULT_POSE.pitch,
    distance: DEFAULT_POSE.distance,
};

/// The orbit camera's zoom clamp for a solid with the given bounding radius.
///
/// A fixed `[1.2, 8.0]` range clips a large preform's facets out of the frame at
/// minimum distance and leaves a small one lost in mostly empty space at maximum,
/// so the clamp scales with the mesh instead.
///
/// The two factors are chosen so a mesh at exactly
/// [`super::DEFAULT_MESH_BOUNDING_RADIUS`] (`1.5`, a standard round brilliant's
/// own half-width) reproduces that same `[1.2, 8.0]` range exactly.
#[must_use]
pub fn orbit_distance_bounds(bounding_radius: f64) -> (f32, f32) {
    const MIN_FACTOR: f32 = 0.8;
    const MAX_FACTOR: f32 = 16.0 / 3.0;
    let radius = (bounding_radius as f32).max(0.01);
    (radius * MIN_FACTOR, radius * MAX_FACTOR)
}

/// The orbit distance that frames a sphere of `bounding_radius` exactly at the
/// vertical edges of the camera's own field of view, times a small margin so the
/// "Fit" pose leaves a little breathing room.
///
/// Mirrors `Camera::generate_ray`'s own
/// `v = ... * fov_tan` convention.
#[must_use]
pub fn fit_distance_for_radius(bounding_radius: f64) -> f32 {
    const MARGIN: f32 = 1.15;
    let half_fov_tan = (DEFAULT_FOV_DEG.to_radians() * 0.5).tan();
    (bounding_radius as f32).max(0.01) / half_fov_tan * MARGIN
}

/// Wraps an orbit pitch into `[-pi, pi)`: the camera may orbit freely over the
/// poles (see `Camera::new`'s pole handling), the wrap only keeps the persisted
/// value bounded.
#[must_use]
pub fn wrap_pitch(pitch: f32) -> f32 {
    use std::f32::consts::PI;
    (pitch + PI).rem_euclid(2.0 * PI) - PI
}

/// One orbit drag step of `(dx, dy)` logical pixels: the desktop's
/// `ViewportModel.camera_orbit` rule.
///
/// Horizontal drag is inverted (the hand is on
/// the gem, not on the camera), vertical keeps its sign, 0.008 rad per pixel, and
/// the pitch wraps ([`wrap_pitch`]). Distance is unchanged.
#[must_use]
pub fn orbit_step(pose: CameraPose, dx: f32, dy: f32) -> CameraPose {
    CameraPose {
        yaw: dx.mul_add(-0.008, pose.yaw),
        pitch: wrap_pitch(dy.mul_add(0.008, pose.pitch)),
        distance: pose.distance,
    }
}

/// One wheel step of `delta` logical pixels.
///
/// The desktop's `ViewportModel.camera_zoom` rule (0.002 per pixel, scrolling up moves closer),
/// clamped to [`orbit_distance_bounds`] of the shown solid's `bounding_radius`.
#[must_use]
pub fn zoom_step(distance: f32, delta: f32, bounding_radius: f64) -> f32 {
    let (min, max) = orbit_distance_bounds(bounding_radius);
    delta.mul_add(-0.002, distance).clamp(min, max)
}

/// The desktop's pose pills (`ViewportModel.set_view`).
///
/// `0` Front (girdle
/// edge-on, index 0 towards the viewer), `1` Top, `2` Bottom, `3` Left, `4` Right
/// -- each a canonical yaw/pitch with the distance kept -- and `5` Fit, which
/// keeps the angle and recomputes the distance from the shown solid's
/// `bounding_radius` ([`fit_distance_for_radius`] clamped to
/// [`orbit_distance_bounds`]).
///
/// Any other kind is Front, as on the desktop.
#[must_use]
pub fn standard_view(kind: i32, pose: CameraPose, bounding_radius: f64) -> CameraPose {
    if kind == 5 {
        let (min, max) = orbit_distance_bounds(bounding_radius);
        return CameraPose {
            distance: fit_distance_for_radius(bounding_radius).clamp(min, max),
            ..pose
        };
    }
    let (yaw, pitch) = match kind {
        1 => (0.0, FRAC_PI_2),
        2 => (0.0, -FRAC_PI_2),
        3 => (-FRAC_PI_2, 0.0),
        4 => (FRAC_PI_2, 0.0),
        _ => (0.0, 0.0),
    };
    CameraPose { yaw, pitch, ..pose }
}
