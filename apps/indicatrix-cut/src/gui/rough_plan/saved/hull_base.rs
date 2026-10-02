//! The imported-mesh base in the plan file: its convex outline's corners.

use super::dto::RoughDto;
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{RoughBase, import_hull};

/// The hull base the corners of `dto.hull` describe.
///
/// # Errors
///
/// Returns the message for a corner list that is not a usable outline.
pub(super) fn base_from_hull(dto: &RoughDto) -> Result<RoughBase, String> {
    let corners: Vec<DVec3> = dto.hull.iter().map(|&c| DVec3::from_array(c)).collect();
    import_hull(&corners).map_err(|e| format!("rough.hull: {e}"))
}

/// Writes the corners of `base` into `dto` when it is a hull; false for any other base.
pub(super) fn write_hull(dto: &mut RoughDto, base: &RoughBase) -> bool {
    let Some(corners) = base.hull_corners() else {
        return false;
    };
    "hull".clone_into(&mut dto.base);
    dto.hull = corners;
    true
}
