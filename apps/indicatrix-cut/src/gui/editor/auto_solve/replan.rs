//! The solid-preview replan handshake: converting an already-solved mast list into
//! viewport planes, stashing the design snapshot a matching preview frame will
//! consume, and resubmitting a follow-up replan once a partial frame goes stale.

use super::runtime::RUNTIME;
use crate::{
    MainWindow, SolidPreviewModel,
    bridge::render_thread::RenderContext,
    gui::editor::view::{ReplanSource, submit_preview_replan_for},
};
use indicatrix::geometry::{ToolPrimitive, meet_solver::SolvedTier};
use indicatrix_cut_core::{Design, design::ToolPlacements};
use indicatrix_editor::solve_policy::IDLE_REPLAN_DEBOUNCE;
use slint::ComponentHandle;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

// The solved-masts-to-viewport-planes conversion lives in
// `indicatrix_editor::solve_policy` (shared with the web app); re-exported here at
// its old path. `IDLE_REPLAN_DEBOUNCE` is how long a "one edit behind" frame waits
// for a follow-up edit before [`schedule_idle_replan_if_stale`] fires.
pub(in crate::gui::editor) use indicatrix_editor::solve_policy::design_to_gpu_planes_from_solved;

/// The concave tools (and each one's `(concave tier, placement)`) that go with
/// [`design_to_gpu_planes_from_solved`]'s planes, from the SAME solve.
///
/// `solved` is the design's own mast list (`None` when it does not solve, which has no
/// planes either, so nothing to cut). A design without concave tiers returns empty
/// vectors without touching the geometry, so planar callers pay nothing. When the tiers
/// do not resolve (an invalid tier, too many placements) the tools are empty too: the
/// flat stone is truthful, a half-resolved set of tools is not. The tier table's
/// manufacturability warnings are what tell the cutter why the tools are missing.
#[must_use]
pub(in crate::gui::editor) fn design_to_gpu_tools_from_solved(
    design: &Design,
    solved: Option<&[SolvedTier]>,
) -> (Vec<ToolPrimitive>, ToolPlacements) {
    if design.concave_tiers.is_empty() {
        return (Vec::new(), Vec::new());
    }
    solved
        .and_then(|solved| design.concave_tools_from_solved(solved).ok())
        .unwrap_or_default()
}

/// Records `design`/`multi_selected` alongside the `generation` they were
/// snapshotted at. Called by `view::submit_preview_replan` on every replan, with
/// an `Arc::clone` of the SAME `Arc<Design>` snapshot that drained edit-intent
/// frame built for `ReplanRequest::design` too (one `Design` clone per drained
/// frame, shared via `Arc` rather than cloned again for each consumer) -- see
/// [`take_matching_design`] for the reader half of this mechanism.
pub(in crate::gui::editor) fn stash_current_design(
    generation: u64,
    design: Arc<Design>,
    multi_selected: BTreeSet<usize>,
) {
    RUNTIME.with(|cell| {
        cell.borrow_mut().current_design = Some((generation, design, multi_selected));
    });
}

/// Called once a solid-preview frame lands naming `generation`
/// (`solid_preview::preview_state::PreviewFrame::generation`) -- see
/// `gui::SlintSolidSink::apply`'s own doc comment for why THIS module (`Runtime`
/// is confined to the UI/event-loop thread, exactly like that frame-apply
/// closure once it hops back via `slint::Weak::upgrade_in_event_loop`) is the
/// only safe place to make the comparison.
///
/// Returns the `(Design, multi_selected)` [`stash_current_design`] last recorded
/// for EXACTLY this generation -- `None` when an edit has moved the live design
/// past it (an older frame still finishing after a newer edit landed, which
/// itself already called [`stash_current_design`] again and overwrote this
/// field), in which case the caller must change nothing, matching every other
/// staleness check in this module.
///
/// # Consumes the match -- a `take`, not a `peek`
///
/// A successful match also REMOVES the stash (`Option::take`), not merely reads
/// it: `solid_preview::preview_state::PreviewFrame::solved`/`generation` are
/// carried forward unchanged by every subsequent `Reproject` frame too (an
/// ordinary camera orbit re-issues the SAME masts/generation the last `Replan`
/// produced -- see `WorkerMemory::generation`'s own doc comment), so without
/// this, every single orbit/zoom frame after an edit would re-trigger a full
/// tier-table rebuild for no reason, and possibly stutter a drag. The FIRST
/// frame to ever carry a given generation is always that generation's own
/// `Replan` result (`memory.generation` is set inside `resolve_replan_state`,
/// and the very frame built right after is what reaches the sink first), so
/// consuming it here costs nothing: no genuine result is ever missed, only the
/// redundant repeats are skipped.
///
/// On a match, ALSO drops whatever debounced auto-solve `scheduling::on_edit`
/// scheduled: that timer would only recompute the exact masts the caller is about
/// to push from the frame's own `solved` anyway, so letting it fire would still pay
/// the redundant `Design::solve()` this whole mechanism exists to avoid. A debounce
/// that survives to see a MATCHING generation here can only be the one this SAME
/// edit's own `scheduling::on_edit` call scheduled: no newer edit landed (that
/// would have bumped the generation and re-stashed, failing the filter above), so
/// no newer debounce could have replaced it either. This never touches an
/// in-flight `super::dispatch::dispatch_background_solve` that already fired
/// before this frame landed -- `super::dispatch::apply_background_solve_result`'s
/// own `generation`/sequence checks still guard that arrival exactly as before.
pub(in crate::gui::editor) fn take_matching_design(
    generation: u64,
) -> Option<(Arc<Design>, BTreeSet<usize>)> {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let is_match = rt
            .current_design
            .as_ref()
            .is_some_and(|(stashed_generation, ..)| *stashed_generation == generation);
        if !is_match {
            return None;
        }
        let (_, design, multi_selected) = rt.current_design.take()?;
        rt.debounce = None;
        Some((design, multi_selected))
    })
}

/// Once a "one edit behind" preview frame lands (a partial,
/// subgraph-resolved replan -- `SolidPreviewModel.stale`, pushed by
/// `gui::SlintSolidSink::apply` from `PreviewFrame::stale`, a file this module
/// does not own), the cutter would otherwise be stuck reading the stale banner until another
/// edit happened to trigger a fresh replan. Debounces (`IDLE_REPLAN_DEBOUNCE`) a
/// follow-up FULL replan of the SAME design/generation instead, so the preview
/// catches up on its own once the edit stream actually goes idle -- cheap,
/// since the partial resolve's masts are already chained forward; this only
/// asks the worker to verify them properly rather than compute anything new.
///
/// A no-op when `super::runtime::init` has not run yet. The staleness check itself
/// deliberately happens only at FIRE time, inside the scheduled closure below,
/// never here at schedule time: `editor::apply_matching_preview_frame` (this
/// function's one caller) runs BEFORE `gui::SlintSolidSink::apply` (a file this
/// module does not own) pushes THIS SAME frame's own `SolidPreviewModel.stale` value --
/// reading it here would see the PREVIOUS frame's staleness instead. Scheduling
/// unconditionally on every matched frame and checking once the debounce
/// elapses costs nothing but a Timer that a closer-together edit would replace
/// anyway (same trade-off `scheduling::on_edit`'s own debounce already makes).
///
/// Called from `editor::apply_matching_preview_frame` right after
/// `view::push_solved_preview`, with the SAME `design`/`multi_selected` that
/// call's own [`take_matching_design`] just handed back -- this module has no
/// other way to reach a `Design` snapshot for `generation` (see the parent module
/// doc comment, "Why a `thread_local!`, not a new `EditorState` field").
///
/// The "anything newer since" check at fire time deliberately does not compare
/// against a live generation counter (this module holds none for the
/// solid-preview replan path -- see above): instead it re-checks
/// `super::runtime::Runtime::current_design`, which every
/// `super::super::view::submit_preview_replan` call (the only other writer)
/// repopulates on its own. If a further edit landed after this frame, that edit's
/// own replan already stashed a NEWER entry there, and this fires nothing (a
/// fresher replan is already in flight or has already landed); if it is still
/// empty, no further edit happened, and resubmitting for `generation` -- once
/// `SolidPreviewModel.stale` confirms there is still something to catch up on --
/// is exactly this frame's own unfinished business.
///
/// # `current_design.is_none()` alone is ambiguous
///
/// `current_design` also reads `None` for a reason that has NOTHING to do with
/// "no further edit happened": `scheduling::reset_for_new_design`/
/// `super::dispatch::cancel_in_flight_solve` both clear it deliberately, on every
/// New/Load/Cancel. Scenario the bare check missed: a partial frame for design A
/// lands and arms this timer; within [`IDLE_REPLAN_DEBOUNCE`], the cutter loads
/// design B (the background branch, `SolidPreviewModel.stale` stays `true` from A's
/// own partial frame) -- `
/// current_design` is `None` either way, so the old check could not tell "A's edit
/// stream went idle" apart from "A was replaced by B entirely," and would
/// resubmit A's stashed masts/generation on top of B, whose rows/status/warnings/
/// yield A's stale completion would then overwrite until B's own solve lands.
///
/// The fix: snapshot `super::runtime::Runtime::current_seq` at SCHEDULE time
/// (`epoch` below) and require it still match at fire time -- the same epoch
/// `scheduling::reset_for_new_design`/`super::dispatch::cancel_in_flight_solve`
/// already bump (and now also use to drop this very timer, belt-and-braces). An
/// ordinary further edit does NOT bump `current_seq` (only a dispatch/reset/cancel
/// does), so this adds no false negative for the case the bare `current_design`
/// check already handles correctly.
pub(in crate::gui::editor) fn schedule_idle_replan_if_stale(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    design: &Design,
    multi_selected: &BTreeSet<usize>,
) {
    let Some((preview_state, solid_last_solved)) = RUNTIME.with(|cell| {
        let rt = cell.borrow();
        rt.preview_state.clone().zip(rt.solid_last_solved.clone())
    }) else {
        return;
    };
    let epoch = RUNTIME.with(|cell| cell.borrow().current_seq);
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = design.clone();
    let multi_selected = multi_selected.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        IDLE_REPLAN_DEBOUNCE,
        move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let nothing_newer_since = RUNTIME.with(|cell| {
                let rt = cell.borrow();
                rt.current_design.is_none() && rt.current_seq == epoch
            });
            if !nothing_newer_since || !ui.global::<SolidPreviewModel>().get_stale() {
                return;
            }
            submit_preview_replan_for(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                ReplanSource {
                    design: &design,
                    generation,
                    multi_selected: &multi_selected,
                },
                BTreeSet::new(),
                true,
            );
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().idle_replan = Some(timer));
}
