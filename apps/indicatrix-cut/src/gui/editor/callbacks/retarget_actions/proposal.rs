//! Opening the dialog, and rebuilding the proposal when the mode/crown/target
//! controls change -- Shift mode's own synchronous rebuild, and the hand-off into
//! [`super::optimize_run`] for `RetargetMode::Optimize`.

use super::{
    RETARGET_ASYNC, apply_ghost_preview_or_revert,
    material::{initial_target_index, resolve_target_selection, resolved_material_from_selection},
    optimize_run::start_optimize_run,
    proposal_view::{
        RetargetView, push_retarget_view, push_target_error, push_target_readout, retarget_view,
    },
    sync_embedded_comparison,
};
use crate::{
    MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            edit_intent::{EditIntent, EditIntentQueue},
            retarget::{CrownShift, RetargetMode, RetargetProposal},
            stale::{self, ResultKind},
            stall_guard::stall_guard,
            state::{EditorState, design_material_options},
            view,
        },
        render::render_visibility::recompute_tab_visible,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::Design;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// Reads `render_ctx`'s current custom materials, resolves the target against them
/// plus `RetargetModel`'s own target picker fields, then rebuilds and pushes a full
/// [`RetargetView`] -- the common body [`setup_retarget_open_callback`] and Shift
/// mode's branch of [`setup_retarget_proposal_changed_callback`] both need. Only
/// ever called with [`RetargetMode::Shift`] since `RetargetMode::Optimize` moved
/// off-thread -- see this group's own `mod.rs` doc comment.
pub(super) fn rebuild_and_push(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
    crown: CrownShift,
    mode: RetargetMode,
) -> Option<RetargetProposal> {
    let combo_index = ui.global::<RetargetModel>().get_target_material_index();
    let ri_text = ui.global::<RetargetModel>().get_target_ri_override_text();
    // Cloned out of the lock alongside the resolution below, rather than re-locking
    // for the proposal: `retarget_view` needs the same catalogue the target was
    // resolved against, and re-locking could observe a different
    // one if a custom material were saved in between.
    let (selection, target, custom_materials) = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked with the Result-returning resolver FIRST, so an
        // unparseable override is reported instead of silently resolved against
        // `design.material` -- see `push_target_error`'s own doc comment.
        match resolve_target_selection(design, &ctx.custom_materials, combo_index, &ri_text) {
            Ok(selection) => {
                let target = resolved_material_from_selection(&selection, &ctx.custom_materials);
                let custom_materials = ctx.custom_materials.as_ref().clone();
                (selection, target, custom_materials)
            }
            Err(message) => {
                drop(ctx);
                push_target_error(ui, &message);
                return None;
            }
        }
    };
    push_target_readout(ui, &selection, &target);
    let (view, proposal) = retarget_view(design, &target, crown, mode, &custom_materials);
    push_retarget_view(ui, view);
    proposal
}

/// "Retarget for material...": opens the dialog and builds the first proposal
/// (Shift mode, default crown settings -- reset here even if a previous session left
/// the dialog's `in-out` properties on Optimize/a nonzero crown fraction). Also
/// seeds the target picker (see this group's own `mod.rs` doc comment, "Where the
/// target material comes from") from `design`'s own current material, and drops
/// whatever a previous session's async Optimize run left in [`super::RETARGET_ASYNC`].
pub(in crate::gui::editor) fn setup_retarget_open_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_open(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
        stale::clear(ResultKind::Retarget);

        let mut st = state.borrow_mut();
        ui.global::<RetargetModel>().set_mode_index(0);
        ui.global::<RetargetModel>().set_crown_fraction(0.0);
        ui.global::<RetargetModel>().set_scale_crown_by_ratio(false);
        ui.global::<RetargetModel>().set_is_busy(false);
        ui.global::<RetargetModel>().set_optimize_evaluations(0);
        ui.global::<RetargetModel>().set_optimize_max_evaluations(0);
        // The viewport ghost preview is never used while the dialog is open (the
        // viewport is paused behind the modal and the embedded comparison pane shows
        // the proposal), so it is forced off: a previous session's value must not
        // put a stale ghost into the shared viewport.
        ui.global::<RetargetModel>().set_preview_enabled(false);

        let options = {
            let ctx = render_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            design_material_options(&ctx.custom_materials)
        };
        ui.global::<RetargetModel>()
            .set_target_material_index(initial_target_index(&st.design.material, &options));
        ui.global::<RetargetModel>().set_target_ri_override_text(
            st.design
                .material
                .refractive_index_override
                .map(|v| format!("{v}"))
                .unwrap_or_default()
                .into(),
        );
        ui.global::<RetargetModel>()
            .set_material_options(ModelRc::new(VecModel::from(
                options
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));

        let generation = st.generation.load(AtomicOrdering::Relaxed);
        let proposal = rebuild_and_push(
            &ui,
            &render_ctx,
            &st.design,
            CrownShift::default(),
            RetargetMode::Shift,
        );
        let solved = proposal.is_some();
        if let Some(proposal) = proposal {
            stale::stamp(ResultKind::Retarget, generation);
            st.pending_retarget = Some((proposal, generation));
        } else {
            st.pending_retarget = None;
        }
        // Released before the embedded comparison's own handler runs: it reads the
        // editor state to build the before/after pair.
        drop(st);
        ui.global::<RetargetModel>().set_is_open(true);
        // The modal hides the main viewport, so pause it for as long as the dialog
        // is open; the embedded comparison pane is the live view instead.
        recompute_tab_visible(&ui, &render_ctx);
        sync_embedded_comparison(&ui, solved);
    });
}

/// The mode/crown/target controls changed: rebuilds the proposal from their current
/// values. Shift mode stays synchronous (pure angle arithmetic); `RetargetMode::
/// Optimize` hands off to [`start_optimize_run`] instead -- see this group's own
/// `mod.rs` doc comment ("`RetargetMode::Optimize` runs off the UI thread").
///
/// Also registers [`RetargetModel::cancel_optimize`]'s handler: both callbacks are
/// wired from this one `setup_*` function (rather than a separate one) so this
/// group's async run tracking stays entirely inside this function's own closures,
/// with no new `setup_retarget_*` call site needed in `gui::editor::mod` (owned
/// elsewhere) to wire it up.
///
/// `preview_state`/`solid_last_solved` (added beyond this function's original
/// signature): every rebuild here also shows or reverts the live ghost preview via
/// [`apply_ghost_preview_or_revert`], matching `RetargetModel.preview_enabled`'s
/// current value.
///
/// `retarget_dialog.slint`'s crown
/// slider fires its `changed(value)` interaction callback on every pixel of drag
/// (`ui/components/retarget_dialog.slint`'s own `crown_slider`), so calling
/// `RetargetModel.proposal_changed()` straight into a synchronous `rebuild_and_push`
/// plus ghost-preview/replan resubmit on every tick would rebuild the WHOLE
/// proposal once per pixel dragged. The SHIFT-mode branch (the one a crown-slider
/// drag actually takes) instead posts an [`EditIntent::RetargetCrown`] into a
/// queue this function builds once, draining at most once per 16ms tick -- see
/// [`EditIntentQueue`]'s own doc comment. The OPTIMIZE-mode branch is
/// unaffected: it already hands off to an off-thread, cancellable search
/// ([`start_optimize_run`]), so there is nothing to coalesce there.
pub(in crate::gui::editor) fn setup_retarget_proposal_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    {
        let intent_queue = {
            let state = Rc::clone(state);
            let render_ctx = Arc::clone(render_ctx);
            let preview_state = Arc::clone(preview_state);
            let solid_last_solved = Arc::clone(solid_last_solved);
            let ui_weak = ui.as_weak();
            EditIntentQueue::new(move |_intent| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                apply_retarget_shift_intent(
                    &ui,
                    &state,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                );
            })
        };
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_proposal_changed(move || {
            stall_guard("retarget_on_proposal_changed", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                // A new request always supersedes whatever Optimize run was in flight.
                RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());

                if ui.global::<RetargetModel>().get_mode_index() == 1 {
                    let mut st = state.borrow_mut();
                    let crown = CrownShift {
                        fraction: f64::from(ui.global::<RetargetModel>().get_crown_fraction()),
                        scale_by_ratio: ui.global::<RetargetModel>().get_scale_crown_by_ratio(),
                    };
                    // An Optimize run "intentionally receives no
                    // further update" to `st.pending_retarget` on success
                    // (`start_optimize_run`'s own doc comment) -- the result instead
                    // reaches `RETARGET_ASYNC::pending`, which `setup_apply_callback`
                    // prefers. But a STALE Shift proposal left in `pending_retarget`
                    // from before the user switched modes was never cleared by that
                    // path, so switching Shift -> Optimize and clicking Apply before
                    // the search finished silently applied the old Shift angles
                    // instead of refusing (there is nothing yet to apply) or waiting.
                    // Cleared here, unconditionally, so Apply can only ever take an
                    // Optimize result from `RETARGET_ASYNC::pending` once this branch
                    // has run -- the Shift branch below is untouched, it still writes
                    // its own proposal into this same field.
                    st.pending_retarget = None;
                    start_optimize_run(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &mut st,
                        crown,
                    );
                } else {
                    intent_queue.post(EditIntent::RetargetCrown);
                }
            });
        });
    }

    {
        let state = Rc::clone(state);
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_cancel_optimize(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // A stray Shift-mode `pending_retarget` cannot actually be present here
            // (this button only matters `while is_busy`, which only Optimize mode
            // sets, and that branch already clears `pending_retarget` before
            // dispatching -- see `on_proposal_changed`'s own comment on why),
            // but cleared defensively anyway since it costs nothing.
            state.borrow_mut().pending_retarget = None;
            cancel_optimize_run(&ui);
        });
    }
}

/// The shared body of "cancel the in-flight `RetargetMode::Optimize` search":
/// [`setup_retarget_proposal_changed_callback`]'s `on_cancel_optimize` handler (the
/// dialog's own Cancel button while busy) and
/// [`super::optimize_run::start_optimize_run`]'s own `ActivityRegistry` cancel
/// closure (the status strip's `ActivityChip`, which has no `Rc<RefCell<EditorState>>`
/// to reach `state.pending_retarget` with -- see
/// [`super::RetargetAsyncRun::cancel_and_supersede`]'s own doc comment for why that
/// alone is sufficient: `RETARGET_ASYNC::pending`, not `state.pending_retarget`, is
/// what actually holds an Optimize-mode proposal). Both reach the exact same effect
/// through this one function.
pub(super) fn cancel_optimize_run(ui: &MainWindow) {
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
    stale::clear(ResultKind::Retarget);
    ui.global::<RetargetModel>().set_is_busy(false);
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: vec!["Optimize cancelled.".to_string()],
            anchored_errors: Vec::new(),
            solve_error: String::new(),
        },
    );
    // No proposal is held any more, so the comparison pane must not keep showing
    // the one it had.
    sync_embedded_comparison(ui, false);
}

/// [`setup_retarget_proposal_changed_callback`]'s Shift-mode body, run once per
/// drained [`EditIntent::RetargetCrown`] instead of once per crown-slider tick --
/// see that function's own doc comment. Reads `RetargetModel.crown_fraction`/
/// `scale_crown_by_ratio` fresh (exactly as the original per-tick call did), so a
/// coalesced burst always rebuilds against the LATEST slider position, not
/// whatever it was when the first tick of the burst posted.
///
/// Never solves: the ghost overlay's candidate is queued on the ghost preview's
/// background worker ([`view::submit_design_ghost_preview`]), so a slider drag stays
/// responsive however slow the candidate is to solve.
fn apply_retarget_shift_intent(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let mut st = state.borrow_mut();
    let crown = CrownShift {
        fraction: f64::from(ui.global::<RetargetModel>().get_crown_fraction()),
        scale_by_ratio: ui.global::<RetargetModel>().get_scale_crown_by_ratio(),
    };
    ui.global::<RetargetModel>().set_is_busy(false);
    let generation = st.generation.load(AtomicOrdering::Relaxed);
    let proposal = rebuild_and_push(ui, render_ctx, &st.design, crown, RetargetMode::Shift);
    // `true`: the ghost is queued and draws when its solve lands (a later tick
    // supersedes it), so the real design is not replanned over it. `false`: no ghost
    // is wanted, and the replan puts the real design back, which also drops a ghost
    // still being solved.
    if !apply_ghost_preview_or_revert(ui, render_ctx, preview_state, &st.design, proposal.as_ref())
    {
        view::submit_preview_replan(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
    }
    // Same stamping as the Optimize
    // completion path (`optimize_run::finish_optimize_run`) -- see
    // `stale::ResultKind::Retarget`'s own doc comment.
    let solved = proposal.is_some();
    if let Some(proposal) = proposal {
        stale::stamp(ResultKind::Retarget, generation);
        st.pending_retarget = Some((proposal, generation));
    } else {
        stale::clear(ResultKind::Retarget);
        st.pending_retarget = None;
    }
    // Released first: the embedded comparison's handler reads the editor state.
    drop(st);
    sync_embedded_comparison(ui, solved);
}
