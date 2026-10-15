//! What the planner knows about the design of a planned stone: where it sits in the planner's
//! caliper frame and how wide it is, read from the library.
//!
//! The numbers a stone's piece of rough (`indicatrix_cut_core::rough_plan::stone_preform_planes`)
//! and its colour (`rough_colour::preview::adopt`) need to move into the design's own frame and to
//! be shown at the stone's real size. The planner's actions ("Open in Editor", "Use Color") run
//! on the UI thread and reach the design meshes of the view through the registration here.

use super::design_mesh::{MeshLibrary, MeshSource};
use indicatrix_cut_core::rough_plan::DesignPlacement;
use std::{cell::RefCell, sync::Arc};

thread_local! {
    /// The planner's design meshes (for the actions, which run on the UI thread).
    static MESHES: RefCell<Option<Arc<MeshLibrary>>> = const { RefCell::new(None) };
}

/// A design's placement in the planner's caliper frame and its widths in model units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::gui) struct DesignInfo {
    /// The caliper turn and centring.
    pub(in crate::gui) placement: DesignPlacement,
    /// The design's width along the caliper, model units.
    pub(in crate::gui) width_units: f64,
    /// The design's axis-aligned width in its own model frame, model units
    /// (`measure_solid(facet planes).width_axis`): what the editor's girdle diameter is a
    /// measurement of, so the stone's real width in the editor is this times the pose's
    /// millimetres per unit.
    pub(in crate::gui) width_axis_units: f64,
}

/// Remembers the design meshes of the view (called when the view is set up).
pub(super) fn register_meshes(meshes: &Arc<MeshLibrary>) {
    MESHES.with(|cell| *cell.borrow_mut() = Some(Arc::clone(meshes)));
}

/// Forgets the registered design meshes. They read through the library database handle, so a
/// registration that outlived the planner window kept the library file open for the process.
pub(in crate::gui::rough_plan) fn release_meshes() {
    MESHES.with(|cell| cell.borrow_mut().take());
}

/// Names the design meshes registration in `out` while it is still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub(in crate::gui) fn note_holders(out: &mut Vec<&'static str>) {
    if MESHES.with(|cell| cell.borrow().is_some()) {
        out.push("rough_plan::view::design_info MESHES (vault through the mesh library)");
    }
}

/// Whether `width` can be a design's width: finite and positive.
const fn is_width(width: f64) -> bool {
    width.is_finite() && width > 0.0
}

/// The placement and the widths of design `entry_id`, read from the library. `None` for a design
/// that is gone, unreadable or has no solid, and for one whose caliper width or axis-aligned
/// width is not finite and positive.
pub(in crate::gui::rough_plan) fn design_info(entry_id: i64) -> Option<DesignInfo> {
    let meshes = MESHES.with(|cell| cell.borrow().clone())?;
    let placement = meshes.placement(entry_id)?;
    let mesh = meshes.mesh(entry_id).ok()?;
    let (width_units, width_axis_units) = (mesh.caliper_width(), mesh.width_axis_units);
    (is_width(width_units) && is_width(width_axis_units)).then_some(DesignInfo {
        placement,
        width_units,
        width_axis_units,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_finite_positive_number_is_a_width() {
        assert!(is_width(2.5));
        assert!(!is_width(0.0));
        assert!(!is_width(-1.0));
        assert!(!is_width(f64::NAN));
        assert!(!is_width(f64::INFINITY));
    }

    #[test]
    fn a_design_is_unknown_before_the_view_registers_its_meshes() {
        assert_eq!(design_info(1), None);
    }
}
