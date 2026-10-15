//! The orbit camera's pose math, shared by the desktop's
//! `gui::render::camera_lighting` and the web app's Solid view.
//!
//! [`orbit_distance_bounds`], [`fit_distance_for_radius`] and [`wrap_pitch`] moved
//! here verbatim from the desktop (which re-exports them and keeps their tests as
//! the pins); [`orbit_step`], [`zoom_step`], [`standard_view`] and [`RESET_POSE`]
//! are the desktop callbacks' inline arithmetic as named functions, with their own
//! tests below. [`fit_distance_for_view`] (every app's Fit pose) and
//! [`planar_bounding_radius`] (the desktop's) are tested against projected stone meshes
//! in `camera/tests.rs`.

use super::types::{CameraPose, StoneGeometryBuf};
use indicatrix::{
    geometry::stone_metrics::{SolidStatus, build_solid_mesh},
    optics::raytracer::{DEFAULT_FOV_DEG, DEFAULT_POSE},
};
use std::f32::consts::FRAC_PI_2;

/// How much of the image the fitted stone leaves free: its outline spans `1 / FIT_MARGIN`
/// of the half-extent of the image's shorter side.
const FIT_MARGIN: f32 = 1.15;

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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the camera precision; a mesh bounding radius is in the normal float range"
    )]
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
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the camera precision; a mesh bounding radius is in the normal float range"
)]
pub fn fit_distance_for_radius(bounding_radius: f64) -> f32 {
    let half_fov_tan = (DEFAULT_FOV_DEG.to_radians() * 0.5).tan();
    (bounding_radius as f32).max(0.01) / half_fov_tan * FIT_MARGIN
}

/// The orbit distance at which a sphere of `bounding_radius` is seen whole in an image
/// of `aspect` (width over height), whatever the orbit angle.
///
/// The sphere is centred on the orbit centre, and its outline sits at `1 / FIT_MARGIN`
/// of the shorter side's half-extent.
///
/// Unlike [`fit_distance_for_radius`] it fits the sphere's true outline (a sphere
/// at distance `d` spans `r / sqrt(d^2 - r^2)` in tangent units, not `r / d`), and it
/// fits the narrower of the two fields of view: the camera's field of view is the
/// vertical one (`Camera::generate_ray`), so an image taller than wide sees only
/// `aspect` times as far sideways.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the camera precision; a mesh bounding radius is in the normal float range"
)]
pub fn fit_distance_for_view(bounding_radius: f64, aspect: f32) -> f32 {
    let narrow = if aspect.is_finite() && aspect > 0.0 {
        aspect.min(1.0)
    } else {
        1.0
    };
    let outline_tan = (DEFAULT_FOV_DEG.to_radians() * 0.5).tan() * narrow / FIT_MARGIN;
    // `r / sqrt(d^2 - r^2) = outline_tan` solved for `d`.
    (bounding_radius as f32).max(0.01) * outline_tan.recip().hypot(1.0)
}

/// The farthest corner of `stone`'s plane arrangement from the orbit centre, in the
/// model units the renderers draw in; `None` when the planes do not close a solid.
///
/// Measured from the planes themselves, so it describes the stone the caller holds
/// now rather than the last frame the solid worker happened to draw. Concave tools are
/// ignored: they only remove material, so the planar solid encloses the carved one.
#[must_use]
pub fn planar_bounding_radius(stone: &StoneGeometryBuf) -> Option<f64> {
    let planes: Vec<_> = stone
        .planes
        .iter()
        .map(|plane| plane.to_halfspace_f64())
        .collect();
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => Some(
            mesh.positions
                .iter()
                .map(|position| position.length())
                .fold(0.0_f64, f64::max),
        ),
        _ => None,
    }
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
/// `bounding_radius` ([`fit_distance_for_view`] clamped to
/// [`orbit_distance_bounds`]).
///
/// Any other kind is Front, as on the desktop.
///
/// Fit frames the stone the way the desktop's does, for a picture at least as wide as
/// tall: the web and mobile callers draw the Solid picture at least 1.6:1 wide
/// ([`super::view::with_min_aspect`]), so the shorter side is always the height. The
/// desktop draws at its viewports' own aspect and passes that aspect itself.
#[must_use]
pub fn standard_view(kind: i32, pose: CameraPose, bounding_radius: f64) -> CameraPose {
    if kind == 5 {
        let (min, max) = orbit_distance_bounds(bounding_radius);
        return CameraPose {
            distance: fit_distance_for_view(bounding_radius, 1.0).clamp(min, max),
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

#[cfg(test)]
mod tests;
