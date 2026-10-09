//! Turning pointer travel along a handle into a snapped angle, mast or index value, and
//! the coalescing key one drag gesture shares.

use super::{
    handles::{HandleKind, HandleLayout},
    projection::ScreenPoint,
};
use crate::session::clamp_nudge_to_side;
use indicatrix::geometry::plane::tier_is_crown_side;

/// A facet is never steeper than a vertical wall.
const MAX_ANGLE_DEG: f64 = 90.0;

/// Below this many pixels per unit a handle is edge-on to the view: dragging it would
/// divide by (almost) zero, so the value stays where it was.
const MIN_PIXELS_PER_UNIT: f32 = 1e-6;

/// Tags a drag key so it never equals another key family's value in practice.
const DRAG_KEY_TAG: u64 = 0xD4A6_0000_0000_0000;

/// How finely a drag snaps. Shift selects [`Self::Fine`]; the toolbar's Snap pill
/// selects [`Self::Off`]. The index handle rounds to whole teeth in every mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapMode {
    /// Angle to 0.1 deg, mast to 0.01.
    #[default]
    Coarse,
    /// Angle to 0.01 deg, mast to 0.001.
    Fine,
    /// No snapping of angle or mast.
    Off,
}

impl SnapMode {
    /// The angle step in degrees, `None` when unsnapped.
    #[must_use]
    pub const fn angle_step_deg(self) -> Option<f64> {
        match self {
            Self::Coarse => Some(0.1),
            Self::Fine => Some(0.01),
            Self::Off => None,
        }
    }

    /// The mast step in mast units, `None` when unsnapped.
    #[must_use]
    pub const fn mast_step(self) -> Option<f64> {
        match self {
            Self::Coarse => Some(0.01),
            Self::Fine => Some(0.001),
            Self::Off => None,
        }
    }
}

/// What a drag gesture captured at pointer-down: the handle, the quantities it started
/// from, where the pointer was, and the handle layout it is measured against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragStart {
    /// The handle being dragged.
    pub kind: HandleKind,
    /// The tier's signed angle at pointer-down.
    pub start_angle_deg: f64,
    /// The tier's solved mast at pointer-down.
    pub start_mast: f64,
    /// The pointer's pick-frame pixel at pointer-down.
    pub pointer: ScreenPoint,
    /// The handle layout at pointer-down (kept fixed for the whole gesture, so the
    /// mapping from pixels to units does not shift under the pointer).
    pub layout: HandleLayout,
}

/// The value a drag currently asks for, by handle kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragValue {
    /// The tier's new signed angle in degrees.
    AngleDeg(f64),
    /// The tier's new mast (same sign as the starting mast).
    Mast(f64),
    /// Whole teeth to turn the tier's indices by, counted from the drag start.
    IndexTeeth(i64),
}

/// `value` rounded to the nearest multiple of `step`, tidied so `413 * 0.1` reads
/// `41.3` rather than `41.300000000000004`.
///
/// A non-positive or NaN `step` returns
/// `value` unchanged; a negative zero stays negative.
#[must_use]
pub fn snap_to_step(value: f64, step: f64) -> f64 {
    if step.is_nan() || step <= 0.0 {
        return value;
    }
    let snapped = (value / step).round() * step;
    if step >= 1e-6 {
        (snapped * 1e9).round() / 1e9
    } else {
        snapped
    }
}

/// Pointer travel from the drag start, projected on the unit screen direction `dir`,
/// expressed in units at `pixels_per_unit`. Zero when the handle is edge-on.
fn travel_units(start: &DragStart, pointer: ScreenPoint, dir: (f32, f32), per_unit: f32) -> f64 {
    if !per_unit.is_finite() || per_unit <= MIN_PIXELS_PER_UNIT {
        return 0.0;
    }
    let (dx, dy) = start.pointer.delta_to(pointer);
    let pixels = dx.mul_add(dir.0, dy * dir.1);
    f64::from(pixels / per_unit)
}

fn angle_value(start: &DragStart, pointer: ScreenPoint, snap: SnapMode) -> DragValue {
    let layout = &start.layout;
    // `angle_dir` points where the normal moves as `theta = |angle|` grows: toward a
    // larger angle on the crown, toward a MORE negative angle on the pavilion.
    let side: f64 = if tier_is_crown_side(start.start_angle_deg) {
        1.0
    } else {
        -1.0
    };
    let degrees = travel_units(start, pointer, layout.angle_dir, layout.pixels_per_degree);
    let raw = side.mul_add(degrees, start.start_angle_deg);
    let snapped = snap
        .angle_step_deg()
        .map_or(raw, |step| snap_to_step(raw, step));
    let capped = if snapped.abs() > MAX_ANGLE_DEG {
        MAX_ANGLE_DEG.copysign(snapped)
    } else {
        snapped
    };
    DragValue::AngleDeg(clamp_nudge_to_side(start.start_angle_deg, capped))
}

fn mast_value(start: &DragStart, pointer: ScreenPoint, snap: SnapMode) -> DragValue {
    let layout = &start.layout;
    let units = travel_units(
        start,
        pointer,
        layout.depth_dir,
        layout.pixels_per_mast_unit,
    );
    let raw = (start.start_mast.abs() + units).max(0.0);
    let magnitude = snap
        .mast_step()
        .map_or(raw, |step| snap_to_step(raw, step))
        .max(0.0);
    DragValue::Mast(magnitude.copysign(start.start_mast))
}

fn index_value(start: &DragStart, pointer: ScreenPoint) -> DragValue {
    let layout = &start.layout;
    let teeth = travel_units(start, pointer, layout.index_dir, layout.pixels_per_tooth);
    DragValue::IndexTeeth(teeth.round() as i64)
}

/// The value a drag asks for with the pointer at `pointer`, `snap` applied.
///
/// - **Angle**: `start_angle` plus the pointer travel along `angle_dir` divided by
///   `pixels_per_degree`, snapped, then held to the tier's own side of zero by
///   [`clamp_nudge_to_side`] (so a crown tier stops at `+0.0`, a pavilion tier at
///   `-0.0`) and to at most 90 degrees. The travel counts toward a larger `|angle|`,
///   which is a larger angle on the crown and a MORE negative one on the pavilion --
///   the way the handle's tip points.
/// - **Mast**: keeps `start_mast`'s sign; the magnitude is `|start|` plus the travel
///   along `depth_dir` divided by `pixels_per_mast_unit`, never below 0, then snapped.
///   Dragging along `+depth_dir` (the outward normal) grows the magnitude.
/// - **Index**: the travel along `index_dir` in whole teeth (rounded), whatever `snap`
///   says.
///
/// A handle that is edge-on to the view (pixels-per-unit about 0) keeps its start value.
#[must_use]
pub fn drag_value(start: &DragStart, pointer: ScreenPoint, snap: SnapMode) -> DragValue {
    match start.kind {
        HandleKind::Angle => angle_value(start, pointer, snap),
        HandleKind::Depth => mast_value(start, pointer, snap),
        HandleKind::Index => index_value(start, pointer),
    }
}

/// The coalescing key of one drag of `kind` on `tier`.
///
/// Every `EditorSession` coalescing call of one gesture passes the same key, so the
/// gesture is one undo step, and a drag of a different tier or handle never merges
/// into it.
#[must_use]
pub const fn drag_coalesce_key(tier: usize, kind: HandleKind) -> u64 {
    let kind_id: u64 = match kind {
        HandleKind::Angle => 0,
        HandleKind::Depth => 1,
        HandleKind::Index => 2,
    };
    DRAG_KEY_TAG | ((tier as u64) << 2) | kind_id
}
