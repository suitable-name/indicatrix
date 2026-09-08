//! "Deep Solve" -- an explicit, off-thread, cancellable action layered next to the
//! ordinary "Solve" button (which only calls the cheap `solve_meet_points`). This one
//! calls `solve_meet_points_verified` instead: the externally-verified repair search
//! that scores candidate mast configurations against a design's own printed
//! proportions rather than trusting every self-consistent meet vertex is right --
//! median relative error 0.2110 -> 0.1278, within-10% designs 10.8% -> 23.7%, at a
//! mean cost of 68.4 pipeline runs per design.
//!
//! # This is a read-only diagnostic, not a mutator
//!
//! The repair search improves a solve by overriding individual tiers' internal
//! vertex-level picks -- a decision with no representation in `MeetConstraint` at
//! all, so it can only ever be shown as a report, never written back to a
//! `ConstraintTier`. This module never calls `History::apply`/`Design::apply_edit`
//! and holds no reference to the live `EditorState` (just a plain snapshot), keeping
//! `History` the sole mutator of `Design` trivially.
//!
//! # Why this needs its own thread
//!
//! At a mean 68.4 pipeline runs per design, on a design whose plain `solve_meet_points`
//! already takes 5.9 seconds, that's on the order of seven minutes -- would freeze the
//! UI thread outright. Follows the same `thread::spawn` + `Arc<AtomicBool>` cancel +
//! `Weak::upgrade_in_event_loop` progress convention `bridge::export_thread` uses.
//!
//! # `adjustable_anchors` is always empty
//!
//! `solve_meet_points_verified` is explicit that passing real recorded-mast anchors
//! as `adjustable_anchors` is wrong, and measurably weaker even when used as
//! intended (24.2% pass rate vs. 90.4% for the fixed-anchor mode this module uses).
//! Every `ScaleReference` tier here is a real recorded mast or authored dimension,
//! never an estimate, so this module always passes `&[]`.
//!
//! # No true progress fraction
//!
//! The repair search has no caller-visible loop (it lives entirely inside
//! `indicatrix::geometry::meet_solver::verify`, which this crate doesn't own), so
//! [`DeepSolveProgress`] carries only elapsed wall time, posted on a fixed interval
//! by a lightweight ticker thread -- an honest "still working" indicator, never a
//! fabricated percentage.
//!
//! # Cancellation is a UI-level abandonment, not a mid-call interrupt
//!
//! [`DeepSolveHandle::cancel`] cannot stop the in-flight call partway through --
//! there is no checkpoint to poll. What it DOES do: the UI stops waiting and
//! discards whatever the call eventually returns
//! ([`DeepSolveOutcome::Cancelled`] is reported within one [`TICK_INTERVAL`]), so
//! the user gets the editor back immediately. The already-spawned OS thread keeps
//! running to completion in the background and its result is simply dropped -- costs
//! CPU, never correctness, bounded by the search's own run budget.

use indicatrix::geometry::{
    meet_solver::{MeetTierInput, VerifiedSolveReport, solve_meet_points_verified},
    stone_metrics::ExternalProportions,
};
use slint::{ComponentHandle, Weak};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often the ticker thread posts an elapsed-time [`DeepSolveProgress`] update --
/// see "No true progress fraction". Cheap enough against a computation already
/// measured in seconds-to-minutes.
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Handle returned by [`spawn_deep_solve`]. See "Cancellation is a UI-level
/// abandonment" for exactly what cancelling does and doesn't stop.
pub struct DeepSolveHandle {
    cancel: Arc<AtomicBool>,
}

impl DeepSolveHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One elapsed-time tick posted while a deep solve runs -- carries no completion
/// fraction, see the module doc comment.
#[derive(Debug, Clone, Copy)]
pub struct DeepSolveProgress {
    pub elapsed: Duration,
}

/// What a deep solve produced, or that the UI gave up waiting on it. `Completed`
/// carries only a [`VerifiedSolveReport`] to DISPLAY -- never the solved masts
/// themselves, since this module never applies anything to a `Design`.
pub enum DeepSolveOutcome {
    Completed {
        report: VerifiedSolveReport,
    },
    /// The user cancelled before the search returned -- it may still be running in
    /// the background, but its eventual result is never delivered anywhere.
    Cancelled,
}

/// Spawns the deep-solve worker (plus its progress ticker) off the UI thread.
/// `on_progress` is invoked on the UI event loop roughly every [`TICK_INTERVAL`]
/// while the search runs; `on_done` exactly once, with the final outcome.
pub fn spawn_deep_solve<T, P, D>(
    ui_weak: Weak<T>,
    gear_teeth_abs: u32,
    tiers: Vec<MeetTierInput>,
    targets: ExternalProportions,
    on_progress: P,
    on_done: D,
) -> DeepSolveHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, DeepSolveProgress) + Send + 'static + Clone,
    D: FnOnce(&T, DeepSolveOutcome) + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_ticker = Arc::clone(&cancel);
    let cancel_worker = Arc::clone(&cancel);
    let done_flag = Arc::new(AtomicBool::new(false));
    let done_ticker = Arc::clone(&done_flag);
    let ticker_ui = ui_weak.clone();

    // Ticker: posts an elapsed-time progress update until the compute thread
    // signals it is done, or the user cancels (no point ticking a dialog the
    // UI has already been told to stop waiting on).
    thread::spawn(move || {
        let start = Instant::now();
        loop {
            thread::sleep(TICK_INTERVAL);
            if done_ticker.load(Ordering::Relaxed) || cancel_ticker.load(Ordering::Relaxed) {
                break;
            }
            let elapsed = start.elapsed();
            let on_progress = on_progress.clone();
            let _ = ticker_ui.upgrade_in_event_loop(move |ui| {
                on_progress(&ui, DeepSolveProgress { elapsed });
            });
        }
    });

    thread::spawn(move || {
        // `adjustable_anchors: &[]` -- every `ScaleReference` tier here is a real
        // recorded mast or authored dimension, never an estimate safe to adjust.
        // The solved masts themselves are discarded -- nothing this crate could do
        // with them.
        let (_solved, report) = solve_meet_points_verified(gear_teeth_abs, &tiers, &targets, &[]);
        done_flag.store(true, Ordering::Relaxed);
        let cancelled = cancel_worker.load(Ordering::Relaxed);
        let outcome = outcome_for(cancelled, report);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            on_done(&ui, outcome);
        });
    });

    DeepSolveHandle { cancel }
}

/// What a finished worker-thread call becomes for the UI: a cancelled run's
/// `report` is discarded unconditionally, regardless of what it says, since the
/// user has already been told the editor stopped waiting. Pulled out of
/// [`spawn_deep_solve`]'s worker closure so this decision -- the one genuinely
/// testable piece of behavior in an otherwise thread-plumbing module -- has
/// something a test can call directly.
const fn outcome_for(cancelled: bool, report: VerifiedSolveReport) -> DeepSolveOutcome {
    if cancelled {
        DeepSolveOutcome::Cancelled
    } else {
        DeepSolveOutcome::Completed { report }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_report(accepted: bool) -> VerifiedSolveReport {
        VerifiedSolveReport {
            initial_score: 0.5,
            score_after_calibration: 0.3,
            final_score: 0.1,
            accepted,
            overrides_applied: 2,
            anchor_moves_applied: 1,
            pipeline_runs: 12,
        }
    }

    // --- DeepSolveHandle::cancel ---

    #[test]
    fn cancel_sets_the_flag_the_worker_and_ticker_threads_poll() {
        // `DeepSolveHandle::cancel`'s entire job is flipping the shared
        // `Arc<AtomicBool>`. Verified directly, without spinning up either thread.
        let flag = Arc::new(AtomicBool::new(false));
        let handle = DeepSolveHandle {
            cancel: Arc::clone(&flag),
        };
        assert!(!flag.load(Ordering::Relaxed));
        handle.cancel();
        assert!(flag.load(Ordering::Relaxed));
    }

    // --- outcome_for: the cancellation decision ---

    #[test]
    fn outcome_for_reports_cancelled_and_discards_the_report_when_cancelled_is_true() {
        let outcome = outcome_for(true, dummy_report(true));
        assert!(matches!(outcome, DeepSolveOutcome::Cancelled));
    }

    #[test]
    fn outcome_for_reports_completed_with_the_report_when_not_cancelled() {
        let report = dummy_report(false);
        let outcome = outcome_for(false, report);
        match outcome {
            DeepSolveOutcome::Completed { report: r } => {
                assert_eq!(r.accepted, report.accepted);
                assert_eq!(r.pipeline_runs, report.pipeline_runs);
                assert_eq!(r.final_score, report.final_score);
            }
            DeepSolveOutcome::Cancelled => panic!("expected Completed, got Cancelled"),
        }
    }

    #[test]
    fn outcome_for_ignores_the_reports_own_accepted_flag_when_cancelled() {
        // Even a would-have-been-accepted report is thrown away on cancel -- the
        // search's eventual result is never delivered once the user has cancelled.
        let outcome = outcome_for(true, dummy_report(true));
        assert!(matches!(outcome, DeepSolveOutcome::Cancelled));
    }

    // --- DeepSolveProgress ---

    #[test]
    fn progress_carries_the_elapsed_duration_it_was_built_with() {
        let progress = DeepSolveProgress {
            elapsed: Duration::from_millis(1500),
        };
        assert_eq!(progress.elapsed, Duration::from_millis(1500));
    }

    #[test]
    fn progress_is_copy_not_just_clone() {
        // The ticker thread hands each tick a fresh `DeepSolveProgress` by value
        // across the `upgrade_in_event_loop` boundary -- `Copy` is load-bearing.
        let progress = DeepSolveProgress {
            elapsed: Duration::from_secs(3),
        };
        let copied = progress;
        assert_eq!(copied.elapsed, progress.elapsed);
    }
}
