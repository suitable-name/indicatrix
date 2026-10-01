//! Cross-checks of a loaded layout against the rough and the settings it was planned with.
//!
//! [`super::convert`] checks every number on its own. A file can still be self-contradictory
//! (a hand edit, a bug in a writer): stones whose carat does not follow from their volume,
//! totals that are not the sums, a yield against another rough, pieces that are not the
//! saw plan's pieces. The renderers and the metrics believe these numbers, so the loader
//! recomputes each one the way the planner derives it and refuses a file that disagrees
//! beyond [`RELATIVE_TOLERANCE`], naming the field.

use indicatrix_cut_core::{
    carat_weight,
    rough_plan::{Axis, PlacedStone, RoughLayout},
};

/// How far a recomputed figure may differ from the stored one, relative to the larger.
pub const RELATIVE_TOLERANCE: f64 = 1e-6;

/// The smallest difference that always passes, so a figure near zero is not held to a
/// relative tolerance it cannot meet.
const ABSOLUTE_FLOOR: f64 = 1e-12;

/// What a layout is checked against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoughFrame {
    /// The specific gravity the plan was made with.
    pub specific_gravity: f64,
    /// The modelled rough's volume in mm³ (`RoughModel::measure`).
    pub volume_mm3: f64,
    /// The extents of the rough's base bounding box, which starts at the origin, in mm.
    pub bbox_mm: [f64; 3],
}

/// Whether `a` and `b` agree within [`RELATIVE_TOLERANCE`] of the larger.
///
/// The bound is a loader cross-check with a tolerance of a millionth, so the fused
/// multiply-add (one rounding less than the two-step form) cannot change which files pass
/// in any way that matters; nothing pinned bit-for-bit depends on it.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= RELATIVE_TOLERANCE.mul_add(a.abs().max(b.abs()), ABSOLUTE_FLOOR)
}

/// The slack allowed on a length of about `scale` mm: a millionth of it, at least a
/// millionth of a millimetre.
fn slack(scale: f64) -> f64 {
    RELATIVE_TOLERANCE * scale.abs().max(1.0)
}

/// The rough axis a table normal points along, with the planner's tie rule (x before y
/// before z when components are equal).
#[must_use]
pub fn table_axis_of(normal: [f64; 3]) -> Axis {
    let [dot_x, dot_y, dot_z] = normal.map(f64::abs);
    if dot_x >= dot_y && dot_x >= dot_z {
        Axis::X
    } else if dot_y >= dot_x && dot_y >= dot_z {
        Axis::Y
    } else {
        Axis::Z
    }
}

/// Checks one stone: carat, table axis, stone inside its piece, piece inside the rough's
/// bounding box. `path` is the stone's own path (`layouts[0].stones[2]`).
fn check_stone(stone: &PlacedStone, frame: &RoughFrame, path: &str) -> Result<(), String> {
    let expected = carat_weight(stone.volume_mm3, frame.specific_gravity);
    if !close(stone.carat, expected) {
        return Err(format!(
            "{path}.carat {} does not follow from its volume {} mm\u{b3} at specific gravity {} (that gives {expected})",
            stone.carat, stone.volume_mm3, frame.specific_gravity
        ));
    }
    let axis = table_axis_of(stone.pose.axes[1]);
    if stone.table_axis != axis {
        return Err(format!(
            "{path}.table_axis '{}' disagrees with the pose: axes[1] points along {axis}",
            stone.table_axis
        ));
    }
    for k in 0..3 {
        let (piece, own) = (stone.piece_size_mm[k], stone.stone_size_mm[k]);
        if own > piece + slack(piece) {
            return Err(format!(
                "{path}.stone_size_mm[{k}] {own} is larger than the piece it comes from ({piece})"
            ));
        }
        let (origin, reach) = (stone.piece_origin_mm[k], frame.bbox_mm[k]);
        if origin < -slack(reach) {
            return Err(format!(
                "{path}.piece_origin_mm[{k}] {origin} lies outside the rough (it starts at 0)"
            ));
        }
        if origin + piece > reach + slack(reach) {
            return Err(format!(
                "{path}.piece_size_mm[{k}] {piece} reaches past the end of the rough ({reach} mm) from {origin}"
            ));
        }
    }
    Ok(())
}

/// Checks that every stone's piece is the saw plan's piece at its position in traversal
/// order: the slab thickness, bar width and piece length on the layout's three cut axes.
/// A layout without pieces in its saw plan (an exact fit may have none) has nothing to
/// compare; `path` is the layout's path.
fn check_saw_plan(layout: &RoughLayout, path: &str) -> Result<(), String> {
    let axes = layout.cut_order.axes();
    let mut stones = layout.stones.iter().enumerate();
    for slab in &layout.cut_plan.slabs {
        for bar in &slab.bars {
            for &length in &bar.pieces_mm {
                let Some((j, stone)) = stones.next() else {
                    return Ok(());
                };
                for (stage, size) in [slab.thickness_mm, bar.width_mm, length]
                    .into_iter()
                    .enumerate()
                {
                    let own = stone.piece_size_mm[axes[stage]];
                    if !close(own, size) {
                        return Err(format!(
                            "{path}.stones[{j}].piece_size_mm[{}] is {own} but the saw plan cuts {size} there",
                            axes[stage]
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Checks the layout's totals: carat and volume are the sums over its stones, the yield is
/// the volume over the rough's.
fn check_totals(layout: &RoughLayout, frame: &RoughFrame, path: &str) -> Result<(), String> {
    let carat: f64 = layout.stones.iter().map(|stone| stone.carat).sum();
    if !close(layout.total_carat, carat) {
        return Err(format!(
            "{path}.total_carat {} is not the sum of its stones' carats ({carat})",
            layout.total_carat
        ));
    }
    let volume: f64 = layout.stones.iter().map(|stone| stone.volume_mm3).sum();
    if !close(layout.total_volume_mm3, volume) {
        return Err(format!(
            "{path}.total_volume_mm3 {} is not the sum of its stones' volumes ({volume})",
            layout.total_volume_mm3
        ));
    }
    let yield_fraction = layout.total_volume_mm3 / frame.volume_mm3;
    if !close(layout.yield_fraction, yield_fraction) {
        return Err(format!(
            "{path}.yield_fraction {} is not the total volume over the rough's volume ({yield_fraction})",
            layout.yield_fraction
        ));
    }
    Ok(())
}

/// Checks `layout` against `frame`. `path` is the layout's own path (`layouts[2]`).
///
/// # Errors
///
/// Returns a message naming the first field that disagrees with the figures it follows
/// from.
pub fn check_layout(layout: &RoughLayout, frame: &RoughFrame, path: &str) -> Result<(), String> {
    for (j, stone) in layout.stones.iter().enumerate() {
        check_stone(stone, frame, &format!("{path}.stones[{j}]"))?;
    }
    check_saw_plan(layout, path)?;
    check_totals(layout, frame, path)
}
