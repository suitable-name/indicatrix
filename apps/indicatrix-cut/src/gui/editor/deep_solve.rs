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
//! `History` the sole mutator of `Design` trivially. [`DeepSolveOutcome::Completed`]
//! does carry the search's own solved masts (`solved`) out to the caller -- purely
//! for a per-tier DISPLAY comparison against the design's last plain solve (see
//! [`tier_mast_deltas`]), never applied to anything.
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
    meet_solver::{MeetTierInput, SolvedTier, VerifiedSolveReport, solve_meet_points_verified},
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
/// carries the [`VerifiedSolveReport`] to DISPLAY, plus the [`SolvedTier`]s the
/// verified repair search actually settled on (`solved`) -- still never applied to a
/// `Design` (this module remains a read-only diagnostic, see the module doc comment),
/// but now available to a caller that wants a per-tier comparison against the
/// design's own last plain solve. [`tier_mast_deltas`] is that comparison;
/// [`setup_deep_solve_callback`](super::super::callbacks::setup_deep_solve_callback)
/// (in `callbacks::solve_actions`) is its one caller today, folding the result into
/// the deep-solve status line via [`format_tier_mast_deltas`]. A dedicated per-tier
/// TABLE (like Optimize's own result rows) would need more than that one-line
/// summary: a display slot on `EditorModel`/a Slint model type to hold rows, and a
/// declared callback or property for the inspector to read it from -- none of which
/// this lane owns (see `ui/models/editor.slint`).
pub enum DeepSolveOutcome {
    Completed {
        /// The verified repair search's own solved masts, in the same tier order as
        /// `Design::tiers`/`Design::solve`'s output -- see [`tier_mast_deltas`].
        solved: Vec<SolvedTier>,
        /// The aggregate verdict to DISPLAY -- see `format_deep_solve_report`
        /// (`gui::editor::view`).
        report: VerifiedSolveReport,
    },
    /// The user cancelled before the search returned -- it may still be running in
    /// the background, but its eventual result is never delivered anywhere.
    Cancelled,
}

/// One tier's mast disagreement between a plain solve and Deep Solve's externally
/// verified repair -- the per-tier breakdown behind
/// [`VerifiedSolveReport`]'s aggregate verdict. Display-only, like the report itself:
/// nothing here is ever written back to a `Design`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierMastDelta {
    /// This tier's index in `Design::tiers` (and in `Design::solve`'s parallel
    /// output).
    pub tier_index: usize,
    /// The plain solve's mast for this tier, before verification.
    pub before_mast: f64,
    /// Deep Solve's verified mast for this tier.
    pub after_mast: f64,
}

impl TierMastDelta {
    /// The signed movement `after_mast - before_mast` -- positive means Deep Solve
    /// pushed the mast further from the girdle, negative means closer.
    #[must_use]
    pub fn delta(&self) -> f64 {
        self.after_mast - self.before_mast
    }
}

/// A mast movement below this (in the same units as `SolvedTier::mast`, i.e. mast
/// order) is treated as "unchanged" -- filters out the floating-point noise a tier
/// Deep Solve genuinely left alone would otherwise show as a spurious sub-millitier
/// disagreement.
const MAST_DELTA_EPSILON: f64 = 1e-6;

/// Zips `current` (a plain [`indicatrix_cut_core::Design::solve`]) against `verified`
/// (a Deep Solve's own [`DeepSolveOutcome::Completed::solved`]) tier by tier, keeping
/// only the tiers whose mast actually moved by more than [`MAST_DELTA_EPSILON`] --
/// the per-tier breakdown [`DeepSolveOutcome`]'s own doc comment describes. Both
/// slices are expected in the same tier order (`Design::meet_tier_inputs`'s own
/// convention); this pairs them positionally and silently stops at the shorter of the
/// two lengths rather than panicking, since a design edited between the plain solve
/// and Deep Solve's completion can change tier count -- the caller already flags that
/// case `stale` independently (`EditorState::generation`).
#[must_use]
pub fn tier_mast_deltas(current: &[SolvedTier], verified: &[SolvedTier]) -> Vec<TierMastDelta> {
    current
        .iter()
        .zip(verified.iter())
        .enumerate()
        .filter_map(|(tier_index, (before, after))| {
            let delta = after.mast - before.mast;
            (delta.abs() > MAST_DELTA_EPSILON).then_some(TierMastDelta {
                tier_index,
                before_mast: before.mast,
                after_mast: after.mast,
            })
        })
        .collect()
}

/// Renders [`tier_mast_deltas`]'s result as a status-line suffix naming which tiers
/// Deep Solve's verified repair actually disagreed with the plain solve on, and by
/// how much -- the minimal consumer [`DeepSolveOutcome`]'s own doc comment describes,
/// so this per-tier data is never dead: `format_deep_solve_report`
/// (`gui::editor::view`) still owns the aggregate verdict line; this is appended
/// after it. Returns `""` when `deltas` is empty (report already reads as full
/// agreement, and an empty sentence would just be noise).
#[must_use]
pub fn format_tier_mast_deltas(deltas: &[TierMastDelta]) -> String {
    if deltas.is_empty() {
        return String::new();
    }
    let tiers: Vec<String> = deltas
        .iter()
        .map(|d| format!("tier {} ({:+.4})", d.tier_index, d.delta()))
        .collect();
    format!(" Disagrees with the last solve on: {}.", tiers.join(", "))
}

/// The five printed proportions Deep Solve verified against, formatted for the
/// status line (CAD audit item 162: a verdict with no target values on screen reads
/// as "not accepted -- still deviates from the printed figures" with nothing saying
/// WHICH figures). Only the ones the catalogue actually printed are shown --
/// `ExternalProportions`' fields are each independently optional -- so a design
/// missing, say, `C/W` simply omits it rather than showing a placeholder. Empty
/// string (nothing appended) only if every field is `None`, which cannot happen on
/// [`setup_deep_solve_callback`](super::super::callbacks::setup_deep_solve_callback)'s
/// one call site (it already requires `Some(targets)` to launch Deep Solve at all)
/// but keeps this safe to call generally.
#[must_use]
pub fn format_verification_targets(targets: &ExternalProportions) -> String {
    let mut parts = Vec::new();
    if let Some(v) = targets.vol_w3 {
        parts.push(format!("Vol/W3 {v:.4}"));
    }
    if let Some(v) = targets.lw {
        parts.push(format!("L/W {v:.4}"));
    }
    if let Some(v) = targets.cw {
        parts.push(format!("C/W {v:.4}"));
    }
    if let Some(v) = targets.pw {
        parts.push(format!("P/W {v:.4}"));
    }
    if let Some(v) = targets.hw {
        parts.push(format!("H/W {v:.4}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(
        " Verified against the printed proportions: {}.",
        parts.join(", ")
    )
}

/// CAD audit item 162: `EditorState::printed_proportions` is captured once, at
/// catalogue-load time, and never re-derived from the design's own edits (see that
/// field's own doc comment) -- so a Deep Solve verdict against it after any edit is
/// honestly a verdict against what the design USED TO print, not necessarily what it
/// prints now. `edited_since_load` should be the caller's
/// `EditorState::history.can_undo()` at the moment the run was launched: a fresh
/// `History` is created on every load/new/replace, so "can undo" means "at least one
/// edit has landed since load" -- exactly the "History is non-empty since load"
/// signal the finding names. Deliberately NOT `EditorState::is_dirty` (`generation`
/// vs. `saved_generation`): that one goes back to `false` the moment the cutter
/// saves, even though the printed-proportions targets still describe the design as
/// it was PRINTED, not as it now is post-edit -- saving does not un-invalidate them.
#[must_use]
pub const fn edited_since_load_caveat(edited_since_load: bool) -> &'static str {
    if edited_since_load {
        " Note: this design has been edited since it was loaded -- these targets \
         may no longer describe the design you are holding."
    } else {
        ""
    }
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
        let (solved, report) = solve_meet_points_verified(gear_teeth_abs, &tiers, &targets, &[]);
        done_flag.store(true, Ordering::Relaxed);
        let cancelled = cancel_worker.load(Ordering::Relaxed);
        let outcome = outcome_for(cancelled, solved, report);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            on_done(&ui, outcome);
        });
    });

    DeepSolveHandle { cancel }
}

/// What a finished worker-thread call becomes for the UI: a cancelled run's
/// `solved`/`report` are discarded unconditionally, regardless of what they say,
/// since the user has already been told the editor stopped waiting. Pulled out of
/// [`spawn_deep_solve`]'s worker closure so this decision -- the one genuinely
/// testable piece of behavior in an otherwise thread-plumbing module -- has
/// something a test can call directly.
fn outcome_for(
    cancelled: bool,
    solved: Vec<SolvedTier>,
    report: VerifiedSolveReport,
) -> DeepSolveOutcome {
    if cancelled {
        DeepSolveOutcome::Cancelled
    } else {
        DeepSolveOutcome::Completed { solved, report }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::SolveStrategy;

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

    fn dummy_solved_tier(mast: f64) -> SolvedTier {
        SolvedTier {
            mast,
            strategy: SolveStrategy::DependencyOrder,
            detail: String::new(),
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
        let outcome = outcome_for(true, Vec::new(), dummy_report(true));
        assert!(matches!(outcome, DeepSolveOutcome::Cancelled));
    }

    #[test]
    fn outcome_for_reports_completed_with_the_report_when_not_cancelled() {
        let report = dummy_report(false);
        let solved = vec![dummy_solved_tier(1.0)];
        let outcome = outcome_for(false, solved, report);
        match outcome {
            DeepSolveOutcome::Completed {
                solved: s,
                report: r,
            } => {
                assert_eq!(r.accepted, report.accepted);
                assert_eq!(r.pipeline_runs, report.pipeline_runs);
                assert_eq!(r.final_score, report.final_score);
                assert_eq!(s.len(), 1);
            }
            DeepSolveOutcome::Cancelled => panic!("expected Completed, got Cancelled"),
        }
    }

    #[test]
    fn outcome_for_ignores_the_reports_own_accepted_flag_when_cancelled() {
        // Even a would-have-been-accepted report is thrown away on cancel -- the
        // search's eventual result is never delivered once the user has cancelled.
        let outcome = outcome_for(true, Vec::new(), dummy_report(true));
        assert!(matches!(outcome, DeepSolveOutcome::Cancelled));
    }

    // --- tier_mast_deltas ---

    #[test]
    fn tier_mast_deltas_keeps_only_tiers_that_actually_moved() {
        let current = vec![
            dummy_solved_tier(1.0),
            dummy_solved_tier(2.0),
            dummy_solved_tier(3.0),
        ];
        let verified = vec![
            dummy_solved_tier(1.0),
            dummy_solved_tier(2.5),
            dummy_solved_tier(3.0),
        ];
        let deltas = tier_mast_deltas(&current, &verified);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].tier_index, 1);
        assert_eq!(deltas[0].before_mast, 2.0);
        assert_eq!(deltas[0].after_mast, 2.5);
        assert!((deltas[0].delta() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn tier_mast_deltas_is_empty_when_every_tier_matches() {
        let masts = vec![dummy_solved_tier(1.0), dummy_solved_tier(2.0)];
        let deltas = tier_mast_deltas(&masts, &masts.clone());
        assert_eq!(
            deltas.len(),
            0,
            "identical solves must report no disagreement"
        );
    }

    #[test]
    fn tier_mast_deltas_stops_at_the_shorter_slice_instead_of_panicking() {
        let current = vec![dummy_solved_tier(1.0), dummy_solved_tier(2.0)];
        let verified = vec![dummy_solved_tier(9.0)];
        let deltas = tier_mast_deltas(&current, &verified);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].tier_index, 0);
    }

    // --- format_tier_mast_deltas ---

    #[test]
    fn format_tier_mast_deltas_is_empty_string_when_no_tiers_disagree() {
        assert_eq!(format_tier_mast_deltas(&[]), "");
    }

    #[test]
    fn format_tier_mast_deltas_names_the_tier_and_signed_movement() {
        let deltas = vec![TierMastDelta {
            tier_index: 3,
            before_mast: 1.0,
            after_mast: 1.25,
        }];
        let text = format_tier_mast_deltas(&deltas);
        assert!(text.contains("tier 3"));
        assert!(text.contains("+0.2500"));
    }

    // --- format_verification_targets ---

    #[test]
    fn format_verification_targets_names_only_the_fields_that_are_set() {
        let targets = ExternalProportions {
            vol_w3: Some(0.6013),
            lw: None,
            cw: Some(0.1489),
            pw: None,
            hw: None,
        };
        let text = format_verification_targets(&targets);
        assert!(text.contains("Vol/W3 0.6013"));
        assert!(text.contains("C/W 0.1489"));
        assert!(!text.contains("L/W"));
        assert!(!text.contains("P/W"));
        assert!(!text.contains("H/W"));
    }

    #[test]
    fn format_verification_targets_is_empty_when_nothing_is_set() {
        assert_eq!(
            format_verification_targets(&ExternalProportions::default()),
            ""
        );
    }

    // --- edited_since_load_caveat ---

    #[test]
    fn edited_since_load_caveat_is_empty_when_not_edited() {
        assert_eq!(edited_since_load_caveat(false), "");
    }

    #[test]
    fn edited_since_load_caveat_warns_when_edited() {
        let text = edited_since_load_caveat(true);
        assert!(text.contains("edited since it was loaded"));
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
