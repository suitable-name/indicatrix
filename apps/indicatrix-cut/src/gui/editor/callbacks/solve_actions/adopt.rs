//! The tier list's "Adopt" action.

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            stall_guard::stall_guard,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::Edit;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// The tier list's "Adopt" action: switches a pinned tier over to the meet
/// instruction the source file actually stated for it
/// (`ConstraintTier::imported_meet`), through [`Edit::SetConstraint`] like any other
/// edit. A silent no-op if the tier has nothing to adopt or the index is stale:
/// `EditorView` only shows the button when `imported_meet_text` is non-empty, so this
/// only guards a race with a concurrent edit.
///
/// Calls `refresh_editor_panel_stale` (with `dirty = {index}`), not `refresh_all`'s
/// real re-solve -- deliberately the one exception to "Solve on explicit action,
/// not on every edit" that it would otherwise be: adopting a suggested meet isn't
/// really a new edit whose result is unknown, since the value being adopted is
/// exactly what the tier already showed after the design's last real solve, so
/// paying the whole-schedule solve cost to re-confirm one already-known value
/// would be a bigger hammer than it needs. `refresh_editor_panel_stale` pushes the
/// immediate UI update from the CACHED last solve (which reads every OTHER row
/// from that cache -- see `push_stale_content`'s own doc comment) and lets the
/// background dirty-set replan confirm it via `resolve_dirty` instead, the same
/// approach `callbacks::tier_actions::setup_pin_to_mast_callback` uses for its own
/// per-row "Pin" action. "Solve, Adopt, and it says not solved again" cannot
/// happen: `refresh_editor_panel_stale` reads the actual cached solve rather than
/// a fixed "Not solved" banner for the untouched rows.
pub(in crate::gui::editor) fn setup_adopt_meet_callback(
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
    ui.global::<EditorModel>().on_adopt_meet(move |index: i32| {
        stall_guard("on_adopt_meet", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if index < 0 {
                return;
            }
            let index = index as usize;
            let mut st = state.borrow_mut();
            let Some(constraint) = st
                .design
                .tiers
                .get(index)
                .and_then(|t| t.imported_meet.clone())
            else {
                return;
            };
            match st.apply(Edit::SetConstraint { index, constraint }) {
                Ok(()) => {
                    let dirty: BTreeSet<usize> = std::iter::once(index).collect();
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        dirty,
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
    });
}
