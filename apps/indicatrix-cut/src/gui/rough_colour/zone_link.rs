//! The wizard's side of what the Rough Planner's 3D view does with it (`zoning` feature only): the
//! zone handles the view draws and drags, and the polished windows it paints on the mesh.
//!
//! The view (`gui::rough_plan::zoning_hooks`) lives in the planner's private module and never
//! touches the wizard's state; it calls the functions here, which read and change it through the
//! wizard's host. None of them holds a borrow of the wizard's state across a call back into the
//! planner. The pure parts (handles, the drag mapping, the brush, the undo step per drag) are in
//! [`super::handles`], [`super::mesh_paint`] and `wizard::state`, with their tests.

use slint::ComponentHandle as _;

use super::{
    handles::Handle,
    mesh_paint::{MeshRay, brush_radius_mm, triangles_under_brush},
    wizard::{
        actions::radius_px,
        host::with_host,
        state::ZONE_COLOURS,
        view_model::{push_canvas, push_surfaces, push_zones},
    },
};
use indicatrix_cut_core::rough_plan::colour_fit::zones::ZoneEdit;

/// The handles of the selected zone and the colour to draw them in.
#[derive(Debug, Clone, PartialEq)]
pub struct HandleSet {
    /// The handles, in the rough frame.
    pub handles: Vec<Handle>,
    /// The zone's outline colour (the one of its outlines on the photos).
    pub colour: [u8; 3],
}

/// The handles to show now: the wizard is open on the Fit step and a zone is selected.
///
/// `None`
/// otherwise (the window is hidden, another step shows, no zone is selected, or the selected
/// zone has no handle left because every parameter is locked).
#[must_use]
pub fn current_handles() -> Option<HandleSet> {
    with_host(|host| {
        if !host.window.window().is_visible() {
            return None;
        }
        let state = host.state.borrow();
        let handles = state.zone_handles();
        (!handles.is_empty()).then(|| HandleSet {
            handles,
            colour: ZONE_COLOURS[state.selected_zone.min(ZONE_COLOURS.len() - 1)],
        })
    })
    .flatten()
}

/// A handle drag starts: the zones as they are become the origin of the drag's edits.
pub fn drag_begin() {
    let _ = with_host(|host| host.state.borrow_mut().begin_zone_drag());
}

/// The pointer moved: `edit` (relative to the zones at the drag's start) is applied. Returns the
/// sentence to show when it is refused (a locked parameter, an invalid result).
///
/// # Errors
///
/// The edit is refused; the zones keep their last good value.
pub fn drag_to(edit: &ZoneEdit) -> Result<(), String> {
    let outcome = with_host(|host| {
        let result = host.state.borrow_mut().drag_zone(edit);
        if result.is_ok() {
            push_zones(host);
        }
        result
    });
    match outcome {
        Some(Ok(())) | None => Ok(()),
        Some(Err(error)) => Err(error.to_string()),
    }
}

/// The drag ended: the whole drag is one undo step. Returns whether the zones changed.
#[must_use]
pub fn drag_end() -> bool {
    with_host(|host| {
        let changed = host.state.borrow_mut().end_zone_drag();
        push_canvas(host);
        push_zones(host);
        changed
    })
    .unwrap_or(false)
}

/// A click in the 3D view along `ray` (rough frame, mm).
///
/// When the wizard is on the Surfaces step with Paint window or Erase on, the triangles under
/// the brush are painted or erased in the wizard's one set of polished windows. Returns whether
/// the click was used (the view then does nothing else with it, even when the ray missed the
/// mesh).
#[must_use]
pub fn mesh_click(ray: &MeshRay) -> bool {
    with_host(|host| {
        let paint_mode = host.window.get_paint_mode();
        let (mesh, extent) = {
            let state = host.state.borrow();
            if !host.window.window().is_visible() || !state.paints_on_mesh(paint_mode) {
                return false;
            }
            let Some(context) = state.context.as_ref() else {
                return false;
            };
            (std::sync::Arc::clone(&context.mesh), state.extent_mm())
        };
        let radius = brush_radius_mm(radius_px(host), extent);
        let triangles = triangles_under_brush(&mesh, ray, radius);
        if !triangles.is_empty() {
            host.state
                .borrow_mut()
                .paint_triangles(triangles, paint_mode == 1);
            push_canvas(host);
            push_surfaces(host);
        }
        true
    })
    .unwrap_or(false)
}
