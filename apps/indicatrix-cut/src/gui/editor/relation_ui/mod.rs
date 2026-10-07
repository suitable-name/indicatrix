//! Tier relations in the desktop editor: a tier's angle that FOLLOWS other tiers
//! (`P2 = P1 - 2`), typed into the Tier form's Angle field with a leading `=`.
//!
//! The engine and the session API live in `indicatrix_editor::session` (relations are
//! kept true through every edit, undo and redo, and a direct edit of a driven angle is
//! refused). This group is the desktop's side of it:
//!
//! - [`logic`] -- Slint-free decisions: which edit a Tier form save becomes, which rows an
//!   edit moves (a tier and everything that follows it), the wording and field of a
//!   refusal. Unit-tested without a window.
//! - this file -- the few callbacks that have no other home (`RelationModel`: "Remove
//!   relation", the hint of an angle cell that cannot be edited) and the refresh that
//!   includes a tier's followers.
//!
//! Callers elsewhere in the editor: the Tier form's Save (`tier_form.rs`), the inline angle
//! cell, keyboard and wheel nudges (`nudge.rs`), delete (`tier_crud.rs`), the linked step
//! series (`tier_generation.rs`) and the angle drag handle (`manipulate/`).

mod logic;

pub(in crate::gui::editor) use logic::{
    DRIVEN_ANGLE_HINT, is_relation_text, plan_tier_save, relation_cleared_sentence,
    relation_placeholder_angle_deg, save_error_text_and_field, split_driven_targets,
    with_followers,
};

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_cut_core::EditError;
use slint::ComponentHandle;

use super::{
    state::EditorState,
    view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
};
use crate::{
    MainWindow, RelationModel,
    bridge::render_thread::RenderContext,
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};

/// [`refresh_editor_panel_stale`] and the preview replan for an edit of the tiers in
/// `dirty`, widened to every tier that follows one of them: the rows a relation moved
/// along with the edited tier are as stale as the edited tier itself.
pub(in crate::gui::editor) fn refresh_with_followers(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    st: &EditorState,
    dirty: impl IntoIterator<Item = usize>,
) {
    let dirty = with_followers(&st.design, dirty);
    refresh_editor_panel_stale(ui, render_ctx, st, &dirty);
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        st,
        dirty,
        false,
    );
}

/// The text to show for `error`, which one of the session's bare-`EditError` actions
/// (`apply`, nudge, drag, ...) just returned. When a tier relation caused the refusal -- a
/// loop, an angle out of range, a direct edit of a tier that follows one -- the session
/// kept the real reason (taken here); otherwise it is the error's own text.
pub(in crate::gui::editor) fn edit_error_text(st: &mut EditorState, error: &EditError) -> String {
    st.take_refusal()
        .map_or_else(|| error.to_string(), |refusal| refusal.to_string())
}

/// The sentence for the relations the last edits freed because a tier they read was
/// removed (taking the session's notice), if any -- for the end of a delete toast.
pub(in crate::gui::editor) fn take_cleared_relations_sentence(
    st: &mut EditorState,
) -> Option<String> {
    st.take_relation_notice()
        .as_ref()
        .and_then(relation_cleared_sentence)
}

/// Registers `RelationModel`'s callbacks: "Remove relation" in the Tier form and the hint
/// for an angle cell that cannot be edited. (The linked step series registers beside the
/// plain one, in `tier_generation.rs`.)
pub(in crate::gui::editor) fn setup_relation_callbacks(
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
    ui.global::<RelationModel>()
        .on_remove_relation(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let label = st.design.relation_label(index);
            match st.clear_tier_relation(index) {
                Ok(None) => {}
                Ok(Some(_)) => {
                    // The angle did not move, but the row's link badge and the form's
                    // Angle field change.
                    refresh_with_followers(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        [index],
                    );
                    drop(st);
                    show_toast(
                        &ui,
                        &format!("{label} no longer follows a relation. It keeps its angle."),
                        "info",
                    );
                }
                Err(error) => {
                    drop(st);
                    show_toast(&ui, &error.to_string(), "error");
                }
            }
        });
    let ui_weak = ui.as_weak();
    ui.global::<RelationModel>()
        .on_explain_driven_angle(move || {
            if let Some(ui) = ui_weak.upgrade() {
                show_toast(&ui, DRIVEN_ANGLE_HINT, "info");
            }
        });
}
