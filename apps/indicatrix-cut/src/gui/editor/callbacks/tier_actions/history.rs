//! Undo/Redo: the two `History` navigation callbacks.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::ComponentHandle;

use super::misc::clamp_selection_to_tier_count;
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// "Undo": a no-op (no toast, no viewport refresh) when there is nothing to undo --
/// `EditorView`'s own button is already disabled via `can_undo` in that case, so this
/// only guards against a stale click racing a state change, not the common path.
pub(in crate::gui::editor) fn setup_undo_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_undo(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        match st.undo() {
            Ok(true) => {
                // Undo can move any tier (or a whole structural AddTier/RemoveTier
                // change), so an edit whose blast radius isn't tracked precisely forces
                // a full solve rather than guessing a `dirty` set -- and, for the same
                // reason, no cached mast is trusted for.
                refresh_editor_panel_stale(
                    &ui,
                    &render_ctx,
                    &st,
                    &(0..st.design.tiers.len()).collect(),
                );
                clamp_selection_to_tier_count(&ui, st.design.tiers.len());
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    true,
                );
            }
            Ok(false) => {}
            // The recorded inverse edit failed to replay -- the design/history stack
            // are left as `EditorState::undo` found them (the failed edit stays on the
            // undo stack, see `History::undo`'s doc comment), so this is safe to just
            // report rather than panic.
            Err(e) => show_toast(&ui, &format!("Undo failed: {e}"), "error"),
        }
    });
}

/// "Redo": same no-op-when-nothing-to-do treatment as [`setup_undo_callback`].
pub(in crate::gui::editor) fn setup_redo_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_redo(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        match st.redo() {
            Ok(true) => {
                // Same "blast radius unknown, trust nothing cached" reasoning as
                // `setup_undo_callback`'s matching arm.
                refresh_editor_panel_stale(
                    &ui,
                    &render_ctx,
                    &st,
                    &(0..st.design.tiers.len()).collect(),
                );
                clamp_selection_to_tier_count(&ui, st.design.tiers.len());
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    true,
                );
            }
            Ok(false) => {}
            // See `setup_undo_callback`'s matching arm -- same recovery guarantee,
            // symmetrically for the redo stack.
            Err(e) => show_toast(&ui, &format!("Redo failed: {e}"), "error"),
        }
    });
}
