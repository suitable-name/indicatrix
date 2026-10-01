//! Wires `super::super::retarget` (the "Retarget for material" proposal
//! builder/applier) and `ui/components/retarget_dialog.slint` into the Edit tab --
//! `setup_*` functions, one per `RetargetDialog`/`EditorView` callback, mirroring
//! `solve_actions`'s Optimize callbacks in shape.
//!
//! # Where the target material comes from
//!
//! [`material::resolve_target_selection`] resolves the proposal's TARGET from
//! `RetargetModel`'s OWN `target_material_index`/`target_ri_override_text`
//! properties, NOT from `state.design.material` or the design settings panel's
//! combo: this dialog owns its target end to end, seeded from the design's current
//! material on `open()` (see [`proposal::setup_retarget_open_callback`]) and changed
//! only by this dialog's own picker. `retarget::apply` still only ever produces
//! `Edit::RetargetAngles` -- see [`apply::setup_retarget_apply_callback`] for how a
//! target material change actually reaches `design.material`.
//!
//! # `RetargetMode::Optimize` runs off the UI thread
//!
//! Shift mode is pure angle arithmetic and stays synchronous. `RetargetMode::Optimize`
//! runs a real `optimize_design` search, so
//! [`proposal::setup_retarget_proposal_changed_callback`] hands it to
//! [`super::super::optimize_solve::spawn_optimize_solve`] -- the exact same
//! worker/cancel/progress machinery the standalone Optimize panel uses
//! (`solve_actions::setup_optimize_callback`) -- rather than blocking the UI thread for
//! however long the search takes. [`RETARGET_ASYNC`] is this dialog's own
//! module-local run tracker: deliberately NOT a new `EditorState` field (that group is
//! owned elsewhere) and NOT threaded through any `setup_retarget_*` call site (`gui::
//! editor::mod`'s wiring is likewise owned elsewhere) -- a `thread_local!` is sound
//! here because Slint's event loop is single-threaded, exactly like `EditorState`'s own
//! `RefCell`. The worker's completion handler is `Send` (a hard requirement of
//! [`super::super::optimize_solve::spawn_optimize_solve`]'s own signature) so it cannot
//! capture `Rc<RefCell<EditorState>>` directly; it stashes the finished proposal into
//! [`RETARGET_ASYNC`] instead, and [`apply::setup_retarget_apply_callback`] reads it
//! back out on the main thread exactly like it already reads
//! `EditorState::pending_retarget` for a Shift-mode proposal.
//!
//! # Slint-free view-model split, for testability
//!
//! [`proposal_view::retarget_view`]/[`proposal_view::row_view`]/
//! [`material::resolve_target_selection`]/[`apply::apply_pending_retarget`] take and
//! return only plain Rust types, so this group's `tests` module can exercise the
//! actual decision logic directly -- `proposal_view::push_retarget_view`/
//! `proposal_view::push_target_readout` are the only two functions that touch a
//! Slint type, thin enough not to need tests.
//!
//! # Module layout
//!
//! Split by concern: [`material`] (target material resolution/display),
//! [`proposal_view`] (the Slint-free proposal view model and its `push_*`
//! renderers), [`proposal`] (the open/proposal-changed callbacks and Shift mode's
//! own synchronous rebuild), [`optimize_run`] (`RetargetMode::Optimize`'s off-thread
//! search), [`apply`] (committing a pending proposal, and closing the dialog), and
//! [`snapshot`] (the separate "Snapshot Design"/"Compare to Snapshot" feature that
//! also lives in this dialog). [`RETARGET_ASYNC`] and [`apply_ghost_preview_or_revert`]
//! are shared by more than one of those siblings, so they stay here rather than in
//! any one of them. [`compare_hooks`] is the narrow read-only surface the visual
//! before/after compare window (`gui::editor::compare`) reaches this dialog's pending
//! proposal and snapshot through -- it never commits anything itself; its "Keep
//! after" invokes `RetargetModel.apply()`, i.e. [`apply::setup_retarget_apply_callback`].

mod apply;
mod compare_hooks;
mod material;
mod optimize_run;
mod proposal;
mod proposal_view;
mod snapshot;

#[cfg(test)]
mod tests;

pub(in crate::gui::editor) use apply::{
    setup_retarget_apply_callback, setup_retarget_close_callback,
};
pub(in crate::gui::editor) use compare_hooks::{
    RetargetCompareInputs, RetargetKeepGuard, current_retarget_guard, discard_retarget_preview,
    retarget_compare_inputs,
};
pub(in crate::gui::editor) use proposal::{
    setup_retarget_open_callback, setup_retarget_proposal_changed_callback,
};
pub(in crate::gui::editor) use snapshot::{setup_snapshot_callbacks, snapshot_for_compare};

use super::super::{
    auto_solve,
    optimize_solve::OptimizeSolveHandle,
    retarget::{self, RetargetProposal},
    view,
};
use crate::{
    CompareModel, MainWindow, RetargetModel, bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::SolidPreviewState,
};
use indicatrix_cut_core::Design;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
};

thread_local! {
    /// This dialog's own async `RetargetMode::Optimize` run tracker -- see this
    /// module's doc comment ("`RetargetMode::Optimize` runs off the UI thread") for
    /// why this is a module-local `thread_local!` rather than a new `EditorState`
    /// field.
    static RETARGET_ASYNC: RefCell<RetargetAsyncRun> = RefCell::new(RetargetAsyncRun::default());
}

/// [`RETARGET_ASYNC`]'s payload.
#[derive(Default)]
struct RetargetAsyncRun {
    /// Cancels the in-flight worker -- `None` whenever nothing is running.
    handle: Option<OptimizeSolveHandle>,
    /// Bumped every time a run starts, is cancelled, or is superseded by a newer
    /// one, so a worker that finishes after being superseded can tell (by comparing
    /// the `run_id` it was launched with) and discard its own result instead of
    /// overwriting a newer run's.
    run_id: u64,
    /// The finished proposal an async Optimize run produced, paired with the design
    /// generation it ran against -- exactly [`super::super::state::EditorState::pending_retarget`]'s own
    /// shape, read back by [`apply::setup_retarget_apply_callback`] the same way.
    pending: Option<(RetargetProposal, u64)>,
    /// The [`crate::ActivityModel`]
    /// id [`optimize_run::start_optimize_run`] registered for the currently running search, if
    /// any -- finished by [`Self::cancel_and_supersede`] on every path that stops
    /// tracking `handle` (cancel, supersede, dialog close), so an abandoned run
    /// never leaves a chip behind in the status strip.
    activity_id: Option<u64>,
}

impl RetargetAsyncRun {
    /// Cancels whatever is running (if anything) and bumps `run_id`, so any result
    /// still in flight is discarded on arrival -- shared by every place that starts
    /// a new run, cancels one outright, or closes the dialog.
    fn cancel_and_supersede(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.run_id = self.run_id.wrapping_add(1);
        self.pending = None;
        if let (Some(activity), Some(id)) = (auto_solve::activity(), self.activity_id.take()) {
            activity.finish(id);
        }
    }
}

/// Points the dialog's embedded comparison pane (`MainWindow`'s own `CompareModel`
/// instance, handled in `gui::editor::compare`) at the proposal just built:
/// `solved` re-opens the embedded session on it, otherwise the session is dropped so
/// the pane never keeps showing a proposal that no longer exists. The handlers read
/// the editor state, so callers must not hold a borrow of it across this call.
fn sync_embedded_comparison(ui: &MainWindow, solved: bool) {
    let model = ui.global::<CompareModel>();
    if solved {
        model.invoke_open_retarget_embedded();
    } else {
        model.invoke_close_embedded();
    }
}

/// Shows `design` retargeted by `proposal` as a ghost overlay
/// in the shared solid viewport, iff `RetargetModel.preview_enabled` is on AND
/// `proposal` is `Some` (a `None` proposal -- an anchored-tier refusal, a solve
/// error -- has nothing to preview). Returns whether the candidate was queued on the
/// background solve worker: the viewport applies the ghost when that solve lands and
/// drops it if a newer request superseded it. The caller resubmits the real, live
/// design through the ordinary [`view::submit_preview_replan`] path when it wasn't
/// queued (see both call sites).
///
/// Builds the candidate via [`Design::apply_edit`] on a clone -- explicitly
/// documented as safe for exactly this ("usable standalone by a caller that wants
/// edit/undo without a `History` stack") -- rather than
/// [`super::super::state::EditorState::apply`], so this never touches
/// `History`/`is_dirty`/the design generation: a preview must never look like a
/// real edit to anything else in this crate.
fn apply_ghost_preview_or_revert(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    design: &Design,
    proposal: Option<&RetargetProposal>,
) -> bool {
    if !ui.global::<RetargetModel>().get_preview_enabled() {
        return false;
    }
    let Some(proposal) = proposal else {
        return false;
    };
    let Some(candidate) = build_retarget_candidate(design, proposal) else {
        return false;
    };
    view::submit_design_ghost_preview(ui, render_ctx, preview_state, &candidate)
}

/// `design` with `proposal`'s angle retarget applied, on a clone -- the candidate
/// geometry both the viewport ghost ([`apply_ghost_preview_or_revert`]) and the
/// visual compare window's "after" side (`gui::editor::compare`, via
/// [`compare_hooks::retarget_compare_inputs`]) show. `None` when the retarget edit
/// no longer applies to `design` (a stale tier index).
///
/// [`Design::apply_edit`] on a clone, never [`super::super::state::EditorState::apply`]:
/// see [`apply_ghost_preview_or_revert`]'s own doc comment for why a preview must
/// never touch `History`. Only the angles move here -- the target MATERIAL is the
/// caller's to attach when it matters (the compare window does; the solid ghost
/// has no material to show).
#[must_use]
pub(in crate::gui::editor) fn build_retarget_candidate(
    design: &Design,
    proposal: &RetargetProposal,
) -> Option<Design> {
    let mut candidate = design.clone();
    let edit = retarget::apply(design, proposal);
    candidate.apply_edit(edit).ok()?;
    Some(candidate)
}
