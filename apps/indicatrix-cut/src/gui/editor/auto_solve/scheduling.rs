//! Deciding whether a solve should run synchronously, be auto-solved after a debounce,
//! or be skipped, plus recording/reading this design's last measured solve time.

use super::{dispatch::dispatch_background_solve, runtime::RUNTIME};
use crate::{
    EditorModel, MainWindow, bridge::render_thread::RenderContext, gui::editor::state::EditorState,
};
use indicatrix_editor::solve_policy::AUTO_SOLVE_DEBOUNCE;
use slint::ComponentHandle;
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

// The pure policy -- the synchronous-solve cutoffs, auto-solve eligibility and its
// "off for this design" note -- lives in `indicatrix_editor::solve_policy`, shared
// with the web app; re-exported here at its old paths.
pub(super) use indicatrix_editor::solve_policy::{auto_solve_off_note, should_schedule_auto_solve};

/// Records a just-completed solve's wall time (background or synchronous) as this
/// design's new estimate for [`should_schedule_auto_solve`]'s next decision.
pub(in crate::gui::editor) fn record_solve_duration(elapsed: Duration) {
    RUNTIME.with(|cell| cell.borrow_mut().last_solve = Some(elapsed));
}

/// The currently loaded design's last REAL measured solve time, if any -- the
/// production counterpart to the `#[cfg(test)]`-only
/// [`last_measured_solve_duration`] below, exposed so `super::super::view::refresh_all`
/// can pass a real measurement to `should_solve_synchronously_for` for every caller that
/// is NOT replacing `EditorState` wholesale (an explicit Solve/Adopt/Optimize Apply/etc.
/// on the design already loaded) instead of unconditionally wiping it via
/// [`reset_for_new_design`] first, which made that rule unreachable on exactly
/// the actions it exists for.
#[must_use]
pub(in crate::gui::editor) fn last_solve() -> Option<Duration> {
    RUNTIME.with(|cell| cell.borrow().last_solve)
}

/// This design's last measured solve time -- a test-only window onto `Runtime`'s
/// thread-local state, so `record_solve_duration`/`reset_for_new_design` can be
/// asserted on directly. Identical in body to the production [`last_solve`] getter
/// just above; kept as its own `#[cfg(test)]` function (rather than tests calling
/// [`last_solve`] directly) purely so a rename of either one does not silently
/// change what the other means to read.
///
/// `should_schedule_auto_solve` reads `Runtime::last_solve` itself (via
/// `super::dispatch`'s `on_edit` caller). `view::refresh_all`'s sync-vs-background
/// decision reads [`last_solve`] for every caller that is NOT replacing
/// `EditorState` wholesale, and passes `None` only for the `wholesale: true` case,
/// right after [`reset_for_new_design`] has cleared the measurement -- reaching back
/// for the value it just cleared would judge a freshly loaded design by the previous
/// one's solve time, precisely the case where the two have nothing to do with each
/// other.
#[cfg(test)]
#[must_use]
pub(super) fn last_measured_solve_duration() -> Option<Duration> {
    RUNTIME.with(|cell| cell.borrow().last_solve)
}

/// Clears the running estimate, drops any pending debounced auto-solve, and
/// invalidates every still-in-flight background solve -- called whenever
/// `EditorState` itself is replaced wholesale (New/Load Selected/Open, all via
/// `super::super::view::refresh_all`), since a different design's solve cost, debounce
/// timer, and any dispatch still running against the OLD design have nothing to do
/// with the one that just replaced it.
///
/// # Why bumping `current_seq` here still matters
///
/// A background solve captures its own `generation: Arc<AtomicU64>` snapshot at
/// dispatch time (see `super::dispatch::dispatch_background_solve`). New/Load
/// Selected/Open all go through `EditorState::replace_wholesale` (see e.g.
/// `callbacks::tier_actions::apply_loaded_design`) -- which, despite the name,
/// REUSES the very SAME `Arc<AtomicU64>` across the replacement and bumps it once
/// (see `EditorState::replace_wholesale`'s own doc comment). So
/// `super::dispatch::apply_background_solve_result`'s `generation` check alone WOULD
/// already catch a dispatch from the design being replaced, once that dispatch's
/// (potentially multi-second) completion finally arrives -- but bumping
/// `Runtime::current_seq` here retires it immediately instead: every one of this
/// function's callers reaches it (via `refresh_all`, called unconditionally at the
/// top of every New/Load Selected/Open path) BEFORE that stale completion's
/// `upgrade_in_event_loop` closure can run (both run on the UI/event-loop thread, so
/// ordering is never racy), so `super::dispatch::is_current` now correctly fails
/// right away -- both the "Solving..." banner/ticker
/// (`super::dispatch::spawn_solving_ticker`'s own `is_current(seq)` check) and the
/// eventual completion stop describing the design that was just replaced, rather
/// than the banner sitting on a stale "Solving... (103 tiers)" message until that
/// old dispatch happens to finish and the generation check silently drops it.
///
/// Dropping `debounce` cancels whatever single-shot auto-solve [`on_edit`] had
/// pending against the design being replaced -- Slint stops a `Timer`'s callback
/// once the `Timer` itself is dropped (see `Runtime::debounce`'s own doc comment).
/// Dropping `idle_replan` is the identical fix for
/// `super::replan::schedule_idle_replan_if_stale`'s own timer: without it, a
/// partial-frame idle replan armed for the design being replaced would otherwise fire
/// up to `super::replan::IDLE_REPLAN_DEBOUNCE` later and resubmit that OLD design's
/// masts on top of whatever this reset is about to load -- see that function's own
/// doc comment for the epoch check this pairs with.
///
/// Safe to call at any time: bumping `current_seq`/dropping `debounce`/`idle_replan`
/// with no solve in flight is a no-op beyond the wasted counter tick.
///
/// `generation` is the just-replaced `EditorState`'s OWN generation counter
/// value (already bumped by `EditorState::replace_wholesale` before this is
/// called -- see `view::viewport::refresh_all`'s own call site) --
/// bumps the shared `SolidPreviewState`'s generation floor to it (when the
/// solid-preview handle has been stashed via `runtime::init`), so a `PlanJob`
/// still queued or in flight for the OLD design cannot have the PLAN worker
/// solve and hand back a frame for a design this reset has already moved
/// past -- see `SolidPreviewState::bump_generation_floor`'s own doc comment.
pub(in crate::gui::editor) fn reset_for_new_design(generation: u64) {
    if let Some(preview_state) = super::runtime::preview_state() {
        preview_state.bump_generation_floor(generation);
    }
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.last_solve = None;
        rt.current_seq += 1;
        rt.debounce = None;
        rt.idle_replan = None;
        // Not load-bearing for correctness -- `EditorState::replace_wholesale`
        // reuses and bumps the SAME `Arc<AtomicU64>` (see its own doc comment),
        // so a stash from before this replacement already carries an older
        // generation number that can never again equal the live one. Cleared
        // anyway so a large superseded design isn't held onto for no reason.
        rt.current_design = None;
        // A pending dispatch queued behind an in-flight solve
        // named the OLD `Design`/generation -- it must not be replayed once this
        // reset has moved on to a different one entirely.
        rt.pending_dispatch = None;
        // Deliberately NOT reset to `false` here: whatever worker
        // `dispatch_background_solve` already spawned against the OLD design (if
        // any) is still physically running and will still call
        // `apply_background_solve_result`, which clears this flag itself the
        // moment that completion is observed (staleness or not) -- see that
        // function's own doc comment. Clearing it early here would let a NEW
        // dispatch for the just-loaded design start a second worker thread while
        // the stale one is still finishing, which is exactly the "unbounded
        // concurrent solve threads" failure mode this mechanism exists to
        // prevent.
    });
}

/// Called at the end of `super::super::view::refresh_editor_panel_stale` -- every edit
/// callback's stale-refresh path calls that function with a fixed, already-established
/// signature, so this hook reads everything it needs (the budget property, `state`'s
/// design/generation) rather than asking for new parameters. See
/// [`should_schedule_auto_solve`] for the eligibility rule.
pub(in crate::gui::editor) fn on_edit(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    let budget_ms =
        u64::try_from(ui.global::<EditorModel>().get_auto_solve_budget_ms()).unwrap_or(0);
    let budget = Duration::from_millis(budget_ms);
    let last_solve = RUNTIME.with(|cell| cell.borrow().last_solve);

    if !should_schedule_auto_solve(last_solve, budget) {
        // Only worth a dedicated note when THIS design is the reason (its own last
        // solve exceeded a real, nonzero budget) -- a `0` budget is a deliberate,
        // global "auto-solve off" choice that the ordinary stale banner already
        // covers without needing an extra explanation.
        if !budget.is_zero()
            && let Some(last) = last_solve
        {
            ui.global::<EditorModel>()
                .set_status_text(auto_solve_off_note(last).into());
            ui.global::<EditorModel>().set_status_is_problem(true);
        }
        // a `PendingDispatch` queued behind an in-flight solve named an
        // OLDER design as of the click that queued it -- once auto-solve is no
        // longer eligible for THIS edit (any reason: a zero budget, or this
        // design's own measured cost now exceeding it), that queued replay must
        // not fire either once the in-flight solve completes. The next edit
        // that IS eligible queues a fresh one from the then-current design.
        RUNTIME.with(|cell| cell.borrow_mut().pending_dispatch = None);
        return;
    }

    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = state.design.clone();
    let generation = Arc::clone(&state.generation);
    // Captured HERE, the same moment `design` itself is snapshotted -- see
    // `dispatch_background_solve`'s own doc comment for why the eventual
    // worker/replay must use this captured value rather than reloading
    // `generation` live.
    let started_generation = generation.load(Ordering::Relaxed);
    let multi_selected = state.multi_selected.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        AUTO_SOLVE_DEBOUNCE,
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                dispatch_background_solve(
                    &ui,
                    &render_ctx,
                    design.clone(),
                    &generation,
                    multi_selected.clone(),
                    started_generation,
                );
            }
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(timer));
}
