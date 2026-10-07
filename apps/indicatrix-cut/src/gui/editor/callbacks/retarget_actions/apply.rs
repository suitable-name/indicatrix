//! Committing a pending retarget proposal, and closing the dialog without
//! committing one.

use super::{
    RETARGET_ASYNC,
    check_run::{ApplyGate, apply_gate, cached_original_solved, reset_check},
    material::{target_display_name, target_material_selection},
    optimize_run::forget_results,
};
use crate::{
    CompareModel, MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            relation_ui::edit_error_text,
            retarget::{self, AnchorChange, RetargetProposal},
            stale::{self, ResultKind},
            stall_guard::stall_guard,
            state::EditorState,
            view,
        },
        render::render_visibility::recompute_tab_visible,
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix::geometry::meet_solver::SolvedTier;
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
/// retargeted) pushes the angle edit alone, exactly as before.
///
/// `anchors` are the `ScaleReference` masts the validity check re-anchored
/// ([`retarget::apply_with_anchors`]); they travel in the SAME batch as the angles, so the
/// angles, the masts and the material are one undo step. Empty keeps the plain
/// `Edit::RetargetAngles`. Returns the number of tiers the retarget named on success.
pub(super) fn apply_pending_retarget(
    state: &mut EditorState,
    pending: (RetargetProposal, u64),
    material_change: Option<MaterialSelection>,
    anchors: &[AnchorChange],
) -> Result<usize, RetargetApplyError> {
    let (proposal, started_generation) = pending;
    if state.generation.load(AtomicOrdering::Relaxed) != started_generation {
        return Err(RetargetApplyError::Stale);
    }
    let angle_edit = retarget::apply_with_anchors(&state.design, &proposal, anchors);
    let applied = proposal.rows.len();
    let edit = match material_change {
        Some(material) => {
            // Flattened into one batch (angles, masts, material), so the history label
            // reads "Retarget for <material>" and Undo reverts all of it at once.
            let mut edits = match angle_edit {
                Edit::Batch(edits) => edits,
                single => vec![single],
            };
            edits.push(Edit::SetMaterial { material });
            Edit::Batch(edits)
        }
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
            let optimize_mode = ui.global::<RetargetModel>().get_mode_index() == 1;
            let Some(pending) = take_pending(&ui, &mut st, optimize_mode) else {
                return;
            };
            // Shift mode's validity gate: the Apply button is already disabled while the
            // check runs or fails, but the Return key and the compare window's "Keep
            // after" reach this handler too. A refused proposal stays pending.
            let anchors = match apply_gate(pending.1) {
                ApplyGate::Open(anchors) => anchors,
                ApplyGate::Checking => {
                    keep_pending(&mut st, optimize_mode, pending);
                    show_toast(
                        &ui,
                        "Still checking this change -- try again in a moment.",
                        "info",
                    );
                    return;
                }
                ApplyGate::Blocked(headline) => {
                    keep_pending(&mut st, optimize_mode, pending);
                    show_toast(
                        &ui,
                        &format!(
                            "{headline} Change the settings, or use Compare to see what goes wrong."
                        ),
                        "error",
                    );
                    return;
                }
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

            // The design as it is BEFORE the apply, kept as the reference snapshot on
            // success so the Optimize comparison can offer "Original".
            let original_design = st.design.clone();
            // Never the viewport slot: while the dialog is open it can hold the candidate's
            // ghost solve.
            let original_solved =
                original_solve_for_snapshot(cached_original_solved(pending.1), &original_design);

            match apply_pending_retarget(&mut st, pending, material_change, &anchors) {
                Ok(applied) => {
                    // Overwrites any earlier held snapshot; no database write.
                    super::snapshot::hold_original_before_retarget(
                        &ui,
                        original_design,
                        original_solved,
                        &super::snapshot::original_snapshot_label(&target_name),
                    );
                    // (`from_retarget_apply` is set inside `hold_original_before_retarget`.)
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
                    // A late check result must not repaint a dialog that is gone, and the
                    // options of a finished search are of no use any more.
                    reset_check(&ui);
                    forget_results();
                    release_modal_viewport(&ui, &render_ctx);
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
                    // A tier relation that refused the edit keeps the real reason in the
                    // session; take it, or fall back to the error's own text.
                    let text = edit_error_text(&mut st, &e);
                    show_toast(&ui, &text, "error");
                }
            }
        });
    });
}

/// The original design's solve for the "Original" snapshot: the check run's cached solve of
/// that very design when it matches the tier count, otherwise a fresh solve of `design`,
/// otherwise `None` (the snapshot then simply has no old masts).
pub(super) fn original_solve_for_snapshot(
    cached: Option<Vec<SolvedTier>>,
    design: &indicatrix_cut_core::Design,
) -> Option<Vec<SolvedTier>> {
    cached
        .filter(|solved| solved.len() == design.tiers.len())
        .or_else(|| design.solve().ok())
}

/// Takes the proposal Apply commits. A Shift-mode proposal lives in
/// `EditorState::pending_retarget`; the Optimize option the cutter picked lives in
/// `RETARGET_ASYNC::pending` instead -- see this group's own `mod.rs` doc comment. In Optimize
/// mode only the latter counts: the Shift angles listed before a search are a starting point,
/// never something to apply, so with nothing picked the cutter is told to search first.
fn take_pending(
    ui: &MainWindow,
    st: &mut EditorState,
    optimize_mode: bool,
) -> Option<(RetargetProposal, u64)> {
    let picked = RETARGET_ASYNC.with(|cell| cell.borrow_mut().pending.take());
    let pending = if optimize_mode {
        picked
    } else {
        st.pending_retarget.take().or(picked)
    };
    if pending.is_none() && optimize_mode {
        show_toast(
            ui,
            "Search for options first, then pick one to apply.",
            "info",
        );
    }
    pending
}

/// Puts a refused proposal back where Apply found it, so the cutter can fix the settings and
/// try again: the Shift proposal on the editor state, an Optimize option in the async slot.
fn keep_pending(st: &mut EditorState, optimize_mode: bool, pending: (RetargetProposal, u64)) {
    if optimize_mode {
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().pending = Some(pending));
    } else {
        st.pending_retarget = Some(pending);
    }
}

/// Runs after `RetargetModel.is_open` went `false` (Apply, Cancel, the header X,
/// Escape or the backdrop): drops the embedded comparison session, then recomputes
/// the viewport's visibility so local tracing and remote dispatch resume.
fn release_modal_viewport(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    ui.global::<CompareModel>().invoke_close_embedded();
    recompute_tab_visible(ui, render_ctx);
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
        reset_check(&ui);
        state.borrow_mut().pending_retarget = None;
        stale::clear(ResultKind::Retarget);
        ui.global::<RetargetModel>().set_is_open(false);
        release_modal_viewport(&ui, &render_ctx);
        let st = state.borrow();
        revert_ghost_to_live_design(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
    });
}

/// Puts the REAL, live design back into the shared solid viewport, undoing any
/// candidate ghost [`super::apply_ghost_preview_or_revert`] left there -- the close
/// handler's revert above, shared with the visual compare window's "Discard"
/// ([`super::compare_hooks::discard_retarget_preview`]) so the two can never revert
/// differently. An ordinary [`view::submit_preview_replan`] with an empty dirty set:
/// nothing about the design changed, only what the viewport shows.
pub(super) fn revert_ghost_to_live_design(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
    st: &EditorState,
) {
    view::submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        st,
        BTreeSet::new(),
        false,
    );
}
