//! Deciding whether a solve should run synchronously, be auto-solved after a debounce,
//! or be skipped, plus recording/reading this design's last measured solve time.

use super::{dispatch::dispatch_background_solve, runtime::RUNTIME};
use crate::{
    EditorModel, MainWindow, bridge::render_thread::RenderContext, gui::editor::state::EditorState,
};
use slint::ComponentHandle;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Solve cost is driven by plane count, not tier count -- a
/// wide-orbit round-brilliant tier emits many planes at once (the corpus's largest
/// measured design is 103 tiers but 210 planes, `mod.rs`'s own baseline), so a small
/// tier count can still be an expensive, UI-thread-blocking solve.
///
/// A rough estimate, not a re-measurement of this crate's own corpus: roughly double
/// a 16-tier reference point at the corpus's own ~2 planes-per-tier ratio, kept
/// comfortably under the 210-plane/5.9s worst case.
const SYNC_SOLVE_PLANE_LIMIT: usize = 32;

/// Once a design has a real measured solve time, that measurement
/// is a far better signal than any plane-count estimate -- a design that solved in
/// under this long most recently is still fine to solve again synchronously,
/// regardless of how many planes it has.
const SYNC_SOLVE_TIME_LIMIT: Duration = Duration::from_millis(500);

/// Whether a solve for a design with `plane_count` planes should still run
/// synchronously on the UI thread (the "New"/Load/explicit-Solve fast path,
/// `super::super::view::refresh_all`'s sync branch) rather than through
/// `super::dispatch::dispatch_background_solve`, which handles a plane-count cutoff
/// instead of a tier-count one. Prefers this design's own last REAL measured solve
/// time (see [`last_solve`]) when one exists, since a real measurement beats
/// any estimate; falls back to `plane_count` only for a design that has never
/// solved yet (a fresh "New" design, most commonly).
///
/// Pure and unit tested directly, mirroring [`should_schedule_auto_solve`]'s own
/// "measurement first, otherwise estimate" shape -- `last_solve` is a parameter
/// here (rather than read from `Runtime` internally) for exactly the same
/// testability reason that function documents on itself.
#[must_use]
pub(in crate::gui::editor) fn should_solve_synchronously(
    plane_count: usize,
    last_solve: Option<Duration>,
) -> bool {
    last_solve.map_or(plane_count <= SYNC_SOLVE_PLANE_LIMIT, |last| {
        last <= SYNC_SOLVE_TIME_LIMIT
    })
}

/// Debounce delay between an edit landing and an eligible auto-solve actually
/// dispatching -- short enough that a burst of keystrokes (typing an angle, say) only
/// ever triggers the LAST one, long enough that it reads as "just happened," not
/// "laggy."
const AUTO_SOLVE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Whether an edit that just landed should schedule a debounced auto-solve --
/// `budget` `0` disables auto-solve outright (today's stale-marker-only behaviour);
/// otherwise, a design with no measurement YET (`last_solve: None` -- nothing has
/// solved since the last New/Load) is scheduled optimistically: a fresh design's
/// schedule starts empty and solves near-instantly, so refusing to try would only
/// delay that design's very first auto-solve for no reason. Once a real measurement
/// exists, it alone decides.
///
/// Pure and unit tested directly -- the one genuinely decision-shaped piece of logic
/// in this module.
pub(super) const fn should_schedule_auto_solve(
    last_solve: Option<Duration>,
    budget: Duration,
) -> bool {
    if budget.is_zero() {
        return false;
    }
    match last_solve {
        Some(last) => last.as_millis() < budget.as_millis(),
        None => true,
    }
}

/// The banner text shown while auto-solve is disabled FOR THIS DESIGN specifically
/// (as opposed to the user having set the budget to `0` outright) -- this design's own
/// last measured solve already exceeds the configured budget.
pub(super) fn auto_solve_off_note(last_solve: Duration) -> String {
    format!(
        "Auto-solve off for this design: last solve took {:.1}s.",
        last_solve.as_secs_f32()
    )
}

/// Records a just-completed solve's wall time (background or synchronous) as this
/// design's new estimate for [`should_schedule_auto_solve`]'s next decision.
pub(in crate::gui::editor) fn record_solve_duration(elapsed: Duration) {
    RUNTIME.with(|cell| cell.borrow_mut().last_solve = Some(elapsed));
}

/// The currently loaded design's last REAL measured solve time, if any -- the
/// production counterpart to the `#[cfg(test)]`-only
/// [`last_measured_solve_duration`] below, exposed so `super::super::view::refresh_all`
/// can pass a real measurement to [`should_solve_synchronously`] for every caller that
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
/// `EditorState` itself is replaced wholesale (New/Load Selected/Open Native, all via
/// `super::super::view::refresh_all`), since a different design's solve cost, debounce
/// timer, and any dispatch still running against the OLD design have nothing to do
/// with the one that just replaced it.
///
/// # Why bumping `current_seq` here still matters
///
/// A background solve captures its own `generation: Arc<AtomicU64>` snapshot at
/// dispatch time (see `super::dispatch::dispatch_background_solve`). New/Load
/// Selected/Open Native all go through `EditorState::replace_wholesale` (see e.g.
/// `callbacks::tier_actions::apply_loaded_design`) -- which, despite the name,
/// REUSES the very SAME `Arc<AtomicU64>` across the replacement and bumps it once
/// (see `EditorState::replace_wholesale`'s own doc comment). So
/// `super::dispatch::apply_background_solve_result`'s `generation` check alone WOULD
/// already catch a dispatch from the design being replaced, once that dispatch's
/// (potentially multi-second) completion finally arrives -- but bumping
/// `Runtime::current_seq` here retires it immediately instead: every one of this
/// function's callers reaches it (via `refresh_all`, called unconditionally at the
/// top of every New/Load Selected/Open Native path) BEFORE that stale completion's
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
pub(in crate::gui::editor) fn reset_for_new_design() {
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
        return;
    }

    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = state.design.clone();
    let generation = Arc::clone(&state.generation);
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
                );
            }
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(timer));
}
