//! Committing a pending retarget proposal, and closing the dialog without
//! committing one.

use super::{
    RETARGET_ASYNC,
    material::{target_display_name, target_material_selection},
};
use crate::{
    MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            retarget::{self, RetargetProposal},
            stale::{self, ResultKind},
            stall_guard::stall_guard,
            state::EditorState,
            view,
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::{Edit, EditError, MaterialSelection};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// Why [`apply_pending_retarget`] applied nothing.
pub(super) enum RetargetApplyError {
    /// The design changed (another edit, Undo/Redo) since this proposal was built --
    /// the same stale-generation guard `solve_actions::setup_optimize_apply_callback`
    /// documents for the identical race.
    Stale,
    Edit(EditError),
}

/// Applies `pending`'s [`RetargetProposal`] through `state` iff its generation still
/// matches, via exactly one [`EditorState::apply`] call -- the ONLY way this group
/// mutates `design`. When `material_change` is `Some`, the angle retarget and the
/// material change are combined into one `Edit::Batch` (see that variant's own doc
/// comment: it validates both sub-edits before writing anything back, so a stale
/// tier index can never leave the design half-retargeted), so the whole "Retarget for
/// material" action is ONE undo step; `None` (the target already matches
/// `state.design.material`, e.g. a Shift-mode preview the user never actually
/// retargeted) pushes the plain `Edit::RetargetAngles` alone, exactly as before.
/// Returns the number of tiers the retarget named on success.
pub(super) fn apply_pending_retarget(
    state: &mut EditorState,
    pending: (RetargetProposal, u64),
    material_change: Option<MaterialSelection>,
) -> Result<usize, RetargetApplyError> {
    let (proposal, started_generation) = pending;
    if state.generation.load(AtomicOrdering::Relaxed) != started_generation {
        return Err(RetargetApplyError::Stale);
    }
    let angle_edit = retarget::apply(&state.design, &proposal);
    let applied = proposal.rows.len();
    let edit = match material_change {
        Some(material) => Edit::Batch(vec![angle_edit, Edit::SetMaterial { material }]),
        None => angle_edit,
    };
    state
        .apply(edit)
        .map(|()| applied)
        .map_err(RetargetApplyError::Edit)
}

/// "Apply": commits the target material (see below) together with the held
/// proposal, then re-solves and refreshes the shared viewport via
/// [`view::refresh_all_now`] -- the same Solve path `tier_actions::setup_solve_callback`
/// uses, NOT `refresh_editor_panel_stale`: every pavilion/crown angle just moved, so
/// the design needs a real re-solve shown immediately, not left marked stale.
///
/// # Also commits the target material, as ONE undo step
///
/// Moving every tier's angle without also committing `design.material` would leave
/// critical-angle-shifted tiers reading as "Windows" against the design's OLD
/// material -- e.g. Diamond-shifted angles judged against Quartz's own, much
/// shallower critical angle -- until the cutter separately pressed "Apply" on the
/// design settings panel's own material combo, so both commit together as one
/// step instead. The target material is resolved from `RetargetModel`'s own picker fields
/// BEFORE calling [`apply_pending_retarget`] (its own stale-generation check still
/// runs first inside that call, against the generation the proposal was actually
/// built against), then handed to it as `material_change` -- `Some` only when it
/// actually differs from `state.design.material` (e.g. never for a Shift-mode
/// preview the cutter didn't retarget). `apply_pending_retarget` combines the angle
/// retarget and the material change into one `Edit::Batch` (see that variant's own
/// doc comment), so pressing Undo once fully reverts a retarget that also changed
/// material, not twice.
pub(in crate::gui::editor) fn setup_retarget_apply_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_apply(move || {
        stall_guard("retarget_on_apply", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A Shift-mode proposal lives in `EditorState::pending_retarget`; an
            // Optimize-mode one (built off-thread) lives in `RETARGET_ASYNC::pending`
            // instead -- see this group's own `mod.rs` doc comment.
            let pending = st
                .pending_retarget
                .take()
                .or_else(|| RETARGET_ASYNC.with(|cell| cell.borrow_mut().pending.take()));
            let Some(pending) = pending else {
                return;
            };
            // The pending result is being consumed right now (applied, or attempted
            // and refused) either way -- nothing is left to badge as stale.
            stale::clear(ResultKind::Retarget);

            // Resolved BEFORE `apply_pending_retarget` -- see this function's own doc
            // comment ("Also commits the target material, as ONE undo step"). Reading
            // `st.design.material` here (rather than after the retarget applies) is safe:
            // a retarget's own `Edit::RetargetAngles`/`Edit::Batch` never touches
            // `design.material` itself, only tier angles.
            let custom = render_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .custom_materials
                .as_ref()
                .clone();
            let combo_index = ui.global::<RetargetModel>().get_target_material_index();
            let ri_text = ui.global::<RetargetModel>().get_target_ri_override_text();
            let material_selection =
                target_material_selection(&st.design, &custom, combo_index, &ri_text);
            // Named from the `MaterialSelection` the cutter actually picked, not
            // `pending.0.target.gem.name` (which falls back to Diamond for "(none)"/a
            // typed custom RI -- see `target_display_name`'s own doc comment), same
            // as the dialog's own live readout.
            let target_name = target_display_name(&material_selection);
            let material_change =
                (material_selection != st.design.material).then_some(material_selection);

            match apply_pending_retarget(&mut st, pending, material_change) {
                Ok(applied) => {
                    // `view::
                    // refresh_all` now takes `Rc<RefCell<EditorState>>` under the
                    // name `view::refresh_all_now` (see that function's own doc
                    // comment) -- this call site's own logic is otherwise untouched.
                    drop(st);
                    view::refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
                    ui.global::<RetargetModel>().set_is_open(false);
                    show_toast(
                        &ui,
                        &format!("Retargeted {applied} tier(s) for {target_name}."),
                        "success",
                    );
                }
                Err(RetargetApplyError::Stale) => {
                    show_toast(
                        &ui,
                        "The design changed since this retarget proposal was built -- \
                     re-open Retarget for material.",
                        "error",
                    );
                }
                Err(RetargetApplyError::Edit(e)) => {
                    show_toast(&ui, &e.to_string(), "error");
                }
            }
        });
    });
}

/// "Cancel"/the backdrop click: discards whatever proposal was pending and closes
/// the dialog -- no edit was ever applied, so there is nothing to undo.
///
/// `render_ctx`/`preview_state`/`solid_last_solved` (added beyond this function's
/// original signature): closing the dialog must never leave a candidate's ghost
/// geometry stuck in the shared viewport, so this always resubmits the real, live
/// design on the way out, regardless of how `RetargetModel.preview_enabled` was
/// left.
pub(in crate::gui::editor) fn setup_retarget_close_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_close(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // Also abandons any in-flight `RetargetMode::Optimize` worker -- closing the
        // dialog must not leave a search running (and later overwriting
        // `RETARGET_ASYNC::pending`) behind an already-dismissed proposal.
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
        state.borrow_mut().pending_retarget = None;
        stale::clear(ResultKind::Retarget);
        ui.global::<RetargetModel>().set_is_open(false);
        let st = state.borrow();
        view::submit_preview_replan(
            &ui,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
    });
}
