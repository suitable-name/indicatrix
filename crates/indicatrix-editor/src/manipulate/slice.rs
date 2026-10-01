//! Mouse-driven slicing: the plane through a line dragged across the stone and the eye,
//! snapped to the gear, and the provisional tier it becomes.

use super::{
    drag::snap_to_step,
    frame::facet_normal,
    projection::{ScreenPoint, ScreenSize, unproject},
};
use crate::loading::next_free_block_name;
use glam::Vec3;
use indicatrix::{geometry::meet_solver::MeetConstraint, optics::raytracer::Camera};
use indicatrix_cut_core::{ConstraintTier, ScheduleMeta, expected_orbit};

/// A drag shorter than this many pixels does not define a line.
const MIN_SLICE_DRAG_PX: f32 = 4.0;

/// How far (pixels) beside the line's midpoint the side probe is taken.
const SIDE_PROBE_PX: f32 = 10.0;

/// A cross product shorter than this is a degenerate (parallel-ray) plane.
const MIN_NORMAL_LENGTH: f32 = 1e-6;

/// Which side of the dragged `a -> b` screen line is cut away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SliceSide {
    /// The part to the right of the drag direction (screen y grows down, so dragging
    /// left to right removes what is below the line). The default.
    #[default]
    Right,
    /// The part to the left of the drag direction.
    Left,
}

impl SliceSide {
    /// The other side (what the Flip action switches to).
    #[must_use]
    pub const fn flipped(self) -> Self {
        match self {
            Self::Right => Self::Left,
            Self::Left => Self::Right,
        }
    }
}

/// The outward normal of the cutting plane through the eye and the screen line
/// `a -> b`, oriented so the removed half-space `{x : n . x > m}` lies on `side`.
///
/// `normal = normalize(cross(dir_a, dir_b))` with `dir_*` the camera rays through the
/// two pixels. The side is decided by a probe 10 px to the right of the line's midpoint
/// (the drag direction rotated by +90 degrees in screen space, y down): its ray must
/// point along `normal` for [`SliceSide::Right`]; [`SliceSide::Left`] flips the result.
///
/// `None` when the drag is shorter than 4 px or the two rays are (nearly) parallel.
#[must_use]
pub fn slice_normal(
    camera: &Camera,
    a: ScreenPoint,
    b: ScreenPoint,
    size: ScreenSize,
    side: SliceSide,
) -> Option<Vec3> {
    let (dx, dy) = a.delta_to(b);
    let length = dx.hypot(dy);
    if !length.is_finite() || length < MIN_SLICE_DRAG_PX {
        return None;
    }
    let dir_a = unproject(camera, a, size).dir;
    let dir_b = unproject(camera, b, size).dir;
    let cross = dir_a.cross(dir_b);
    if !cross.is_finite() || cross.length() < MIN_NORMAL_LENGTH {
        return None;
    }
    let normal = cross.normalize();
    // Rotate the drag direction by +90 degrees in screen space (y down): (dx, dy) -> (-dy, dx).
    let (rx, ry) = (-dy / length, dx / length);
    let mid = a.midpoint(b);
    let probe = ScreenPoint::new(
        SIDE_PROBE_PX.mul_add(rx, mid.x),
        SIDE_PROBE_PX.mul_add(ry, mid.y),
    );
    let probe_dir = unproject(camera, probe, size).dir;
    let right_oriented = if probe_dir.dot(normal) > 0.0 {
        normal
    } else {
        -normal
    };
    Some(match side {
        SliceSide::Right => right_oriented,
        SliceSide::Left => -right_oriented,
    })
}

/// A cutting-plane normal snapped to the gear: what a new facet's tier will hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnappedFacet {
    /// The tier's signed angle in degrees, rounded to the angle step (`+0.0` for a
    /// crown-side facet at 0, `-0.0` for a pavilion-side one).
    pub angle_deg: f64,
    /// The nearest whole index-wheel tooth, in `0..gear`.
    pub index: f64,
    /// The unit normal rebuilt from the snapped angle and index.
    pub normal: Vec3,
}

/// Snaps `normal` to the nearest facet the gear can cut.
///
/// `theta = acos(|n.y|)` in degrees, rounded to `angle_step_deg` (no rounding for a
/// non-positive step). Sign: `n.y >= 0` is a crown facet (positive angle, `+0.0` at 0);
/// `n.y < 0` a pavilion facet (negative, `-0.0` at 0). `index = round(atan2(n.z, n.x) *
/// gear / 2 pi)` wrapped into `0..gear` (a gear of 0 teeth counts as 1). The returned
/// normal is rebuilt from the snapped pair with `FacetFrame`'s own construction.
#[must_use]
pub fn snap_to_gear(normal: Vec3, gear_teeth: u32, angle_step_deg: f64) -> SnappedFacet {
    let gear = gear_teeth.max(1);
    let gear_f = f64::from(gear);
    let theta = f64::from(normal.y.abs().clamp(0.0, 1.0))
        .acos()
        .to_degrees();
    let theta = snap_to_step(theta, angle_step_deg);
    let angle_deg = if normal.y >= 0.0 { theta } else { -theta };
    let azimuth = f64::from(normal.z).atan2(f64::from(normal.x));
    let wrapped = (azimuth * gear_f / std::f64::consts::TAU)
        .round()
        .rem_euclid(gear_f);
    // `rem_euclid` can hand back `-0.0`; an index is never negative zero.
    let index = if wrapped == 0.0 { 0.0 } else { wrapped };
    SnappedFacet {
        angle_deg,
        index,
        normal: facet_normal(angle_deg, index, gear),
    }
}

/// The mast at which the plane with unit `normal` just touches the stone from outside:
/// `max(n . p)` over the stone's corner points, `1.0` when there are none (or the
/// maximum is not finite).
#[must_use]
pub fn tangency_mast(normal: Vec3, corner_points: &[Vec3]) -> f64 {
    let max = corner_points
        .iter()
        .map(|&p| normal.dot(p))
        .max_by(f32::total_cmp);
    max.filter(|m| m.is_finite()).map_or(1.0, f64::from)
}

/// The provisional tier a slice becomes.
///
/// `indices` is the whole symmetric orbit of `snapped.index` under `meta`'s symmetry
/// order, mirror and gear when `symmetric`, else just that one index. The constraint is
/// `ScaleReference(mast)`, the name the next free `G<n>`/`C<n>`/`P<n>` block name among
/// `existing_names`; no imported meet or notes, nothing detached.
#[must_use]
pub fn slice_tier(
    snapped: &SnappedFacet,
    mast: f64,
    meta: &ScheduleMeta,
    symmetric: bool,
    existing_names: &[String],
) -> ConstraintTier {
    let indices = if symmetric {
        expected_orbit(
            snapped.index,
            meta.symmetry_order,
            meta.mirror,
            meta.gear_teeth_abs(),
        )
    } else {
        vec![snapped.index]
    };
    ConstraintTier {
        angle_deg: snapped.angle_deg,
        name: next_free_block_name(snapped.angle_deg, existing_names),
        indices,
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}
