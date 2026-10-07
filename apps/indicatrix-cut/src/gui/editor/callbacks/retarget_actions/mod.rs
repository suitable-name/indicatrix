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
//! # Optimize is a search the cutter starts, and it runs off the UI thread
//!
//! Shift mode is pure angle arithmetic and stays synchronous. Optimize starts from the Shift
//! result and searches every crown and pavilion angle inside a range the cutter picks
//! (`indicatrix_editor::retarget::search`), which takes seconds to minutes, so it never runs
//! until the Search button is pressed and then runs on a worker thread of [`optimize_run`]'s
//! own. Switching to Optimize only lists the Shift angles the search would start from.
//! When the search ends, the report (up to three valid options, best first) waits in
//! [`optimize_run`]'s results slot; picking one copies it into [`RETARGET_ASYNC`]'s `pending`
//! proposal and into [`check_run`]'s slot (its verdict, masts and optical table), so Apply,
//! the embedded comparison and the compare window all read an Optimize option exactly like a
//! Shift result. [`RETARGET_ASYNC`] is this dialog's own module-local run tracker:
//! deliberately NOT a new `EditorState` field (that group is owned elsewhere) and NOT
//! threaded through any `setup_retarget_*` call site (`gui::editor::mod`'s wiring is likewise
//! owned elsewhere) -- a `thread_local!` is sound here because Slint's event loop is
//! single-threaded, exactly like `EditorState`'s own `RefCell`. The worker's completion
//! handler is `Send`, so it cannot capture `Rc<RefCell<EditorState>>`; it hands the report
//! back through `upgrade_in_event_loop`, and [`apply::setup_retarget_apply_callback`] reads
//! the pending proposal back out on the main thread.
//!
//! # Shift mode's validity check runs off the UI thread too
//!
//! Building the Shift plan is angle arithmetic and stays synchronous, but judging it (does
//! the retargeted stone still close, keep its girdle, keep its table on the crown side)
//! needs two solves, two meshes and three small optical traces. [`check_run`] runs that on
//! a worker thread behind a serial guard and keeps the result -- verdict, the re-anchored
//! masts and the optical table -- in its own `thread_local!`, because
//! `EditorState::pending_retarget` keeps its older proposal-only type for the web app.
//! Apply is disabled while the verdict is `Checking...` or `Not valid`; the Rust-side
//! [`apply::setup_retarget_apply_callback`] enforces the same gate for the Return key and
//! the compare window's "Keep after". Compare stays available for an invalid change.
//!
//! # Slint-free view-model split, for testability
//!
//! [`proposal_view::plan_view`]/[`proposal_view::candidate_row_views`]/
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
//! own synchronous rebuild), [`check_run`] (Shift mode's off-thread validity check and
//! optical comparison), [`optimize_run`] (the Optimize search, its progress, its options
//! and the pick of one), [`apply`] (committing a pending proposal, and closing the dialog), and
//! [`snapshot`] (the separate "Snapshot Design"/"Compare to Snapshot" feature that
//! also lives in this dialog). [`RETARGET_ASYNC`] and [`apply_ghost_preview_or_revert`]
//! are shared by more than one of those siblings, so they stay here rather than in
//! any one of them. [`compare_hooks`] is the narrow read-only surface the visual
//! before/after compare window (`gui::editor::compare`) reaches this dialog's pending
//! proposal and snapshot through -- it never commits anything itself; its "Keep
//! after" invokes `RetargetModel.apply()`, i.e. [`apply::setup_retarget_apply_callback`].

mod advanced_in_use;
mod apply;
mod check_run;
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
pub(in crate::gui::editor) use snapshot::{
    hold_reference_snapshot, original_for_compare, setup_snapshot_callbacks, snapshot_for_compare,
};

use super::super::{
    auto_solve,
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
    /// This dialog's own async Optimize run tracker -- see this module's doc comment
    /// ("Optimize is a search the cutter starts") for why this is a module-local
    /// `thread_local!` rather than a new `EditorState` field.
    static RETARGET_ASYNC: RefCell<RetargetAsyncRun> = RefCell::new(RetargetAsyncRun::default());
}

/// [`RETARGET_ASYNC`]'s payload.
#[derive(Default)]
struct RetargetAsyncRun {
    /// Cancels the in-flight worker and reads its progress -- `None` whenever nothing is
    /// running.
    handle: Option<optimize_run::SearchHandle>,
    /// Bumped every time a run starts, is cancelled, or is superseded by a newer
    /// one, so a worker that finishes after being superseded can tell (by comparing
    /// the `run_id` it was launched with) and discard its own result instead of
    /// overwriting a newer run's.
    run_id: u64,
    /// The Optimize option the cutter picked, as a proposal, paired with the design
    /// generation it ran against -- exactly [`super::super::state::EditorState::pending_retarget`]'s own
    /// shape, read back by [`apply::setup_retarget_apply_callback`] the same way.
    pending: Option<(RetargetProposal, u64)>,
    /// The [`crate::ActivityModel`]
    /// id [`optimize_run::start_search`] registered for the currently running search, if
    /// any -- finished by [`Self::cancel_and_supersede`] on every path that stops
    /// tracking `handle` (cancel, supersede, dialog close), so an abandoned run
    /// never leaves a chip behind in the status strip.
    activity_id: Option<u64>,
}

impl RetargetAsyncRun {
    /// Cancels whatever is running (if anything), forgets the options a finished search
    /// offered and bumps `run_id`, so any result still in flight is discarded on arrival --
    /// shared by every place that starts a new run, cancels one outright, changes an input
    /// the options were computed from, or closes the dialog.
    fn cancel_and_supersede(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.run_id = self.run_id.wrapping_add(1);
        self.pending = None;
        optimize_run::forget_results();
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
    let anchors = check_run::latest_anchors();
    let Some(candidate) = build_retarget_candidate(design, proposal, &anchors) else {
        return false;
    };
    view::submit_design_ghost_preview(ui, render_ctx, preview_state, &candidate)
}

/// `design` with `proposal`'s angle retarget and the re-anchored `anchors` applied, on a
/// clone -- the candidate geometry both the viewport ghost
/// ([`apply_ghost_preview_or_revert`]) and the visual compare window's "after" side
/// (`gui::editor::compare`, via [`compare_hooks::retarget_compare_inputs`]) show. `None`
/// when the retarget edit no longer applies to `design` (a stale tier index).
///
/// The exact edit Apply commits ([`retarget::apply_with_anchors`]), so what the cutter
/// compares is what they would keep. [`Design::apply_edit`] on a clone, never
/// [`super::super::state::EditorState::apply`]: see [`apply_ghost_preview_or_revert`]'s
/// own doc comment for why a preview must never touch `History`. The target MATERIAL is
/// the caller's to attach when it matters (the compare window does; the solid ghost has
/// no material to show).
///
/// Tiers that follow a relation are brought into line afterwards, as the editor session does
/// when Apply commits the edit; a relation that cannot be satisfied leaves them where the
/// edit put them (the verdict says so).
#[must_use]
pub(in crate::gui::editor) fn build_retarget_candidate(
    design: &Design,
    proposal: &RetargetProposal,
    anchors: &[retarget::AnchorChange],
) -> Option<Design> {
    let mut candidate = design.clone();
    let edit = retarget::apply_with_anchors(design, proposal, anchors);
    candidate.apply_edit(edit).ok()?;
    let _ = retarget::anchors::fold_relations(&mut candidate);
    Some(candidate)
}
