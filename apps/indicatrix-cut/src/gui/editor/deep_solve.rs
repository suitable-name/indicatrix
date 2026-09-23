//! "Deep Solve" -- an explicit, off-thread, cancellable action layered next to the
//! ordinary "Solve" button (which only calls the cheap `solve_meet_points`). This one
//! calls `solve_meet_points_verified_with` instead: the externally-verified repair
//! search that scores candidate mast configurations against a design's own printed
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
//! UI thread outright. Runs on [`super::solve_service::SolveService`]'s single worker
//! thread rather than a bespoke `thread::spawn` -- one `SolveService` per
//! [`spawn_deep_solve`] call, since each Deep Solve run has its own caller-supplied
//! `on_progress`/`on_done` closures; see that type's own doc comment for the
//! mailbox/cancellation machinery this reuses.
//!
//! # `adjustable_anchors` is always empty
//!
//! `solve_meet_points_verified_with` is explicit that passing real recorded-mast
//! anchors as `adjustable_anchors` is wrong, and measurably weaker even when used as
//! intended (24.2% pass rate vs. 90.4% for the fixed-anchor mode this module uses).
//! Every `ScaleReference` tier here is a real recorded mast or authored dimension,
//! never an estimate, so [`super::solve_service::SolveKind::Verified`] always passes
//! `&[]` for that parameter.
//!
//! # No true progress fraction
//!
//! The repair search has no caller-visible loop of its own (every pipeline run
//! forwards whichever plain-solve-shaped [`indicatrix::geometry::meet_solver::SolveProgress`]
//! its current run is at -- see `solve_meet_points_verified_with`'s own doc
//! comment), so [`DeepSolveProgress`] still carries only elapsed wall time, exactly
//! as before: a caller wanting the raw `SolveProgress` can read it from
//! `SolveService` directly, but this module's own `EditorModel.deep_solve_status`
//! consumer only ever wanted "still working, N seconds in."
//!
//! # Cancellation is a real mid-search checkpoint
//!
//! [`DeepSolveHandle::cancel`] sets the SAME `SolveControl::with_cancel` flag
//! `solve_meet_points_verified_with` checks from inside whichever pipeline run is
//! in flight -- typically single-digit milliseconds, matching `solve_service`'s
//! own cancellation guarantee, not whenever the in-flight OS thread happens to
//! finish its multi-minute run. A cancelled run's [`DeepSolveOutcome::Cancelled`]
//! carries nothing to show -- see [`outcome_for`]'s own doc comment for why.

use super::solve_service::{
    SolveKind, SolveOutcome, SolveRequest, SolveResult, SolveService, VerifiedSolve,
};
use indicatrix::geometry::{
    meet_solver::{SolveError, SolvedTier, VerifiedSolveReport},
    stone_metrics::ExternalProportions,
};
use indicatrix_cut_core::Design;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// Handle returned by [`spawn_deep_solve`]. See the module doc comment,
/// "Cancellation is a real mid-search checkpoint".
pub struct DeepSolveHandle {
    handle: super::solve_service::SolveHandle,
    /// Keeps this run's dedicated [`SolveService`] (and its one worker thread)
    /// alive for as long as anything could still call [`Self::cancel`] -- see the
    /// module doc comment, "Why this needs its own thread". The worker parks
    /// forever on its empty mailbox once this one-shot request completes; dropping
    /// this field (with the handle) simply lets that parked thread be reclaimed
    /// like any other -- there is nothing left for it to do either way.
    _service: SolveService,
}

impl DeepSolveHandle {
    /// Requests cancellation -- see the module doc comment, "Cancellation is a
    /// real mid-search checkpoint".
    pub fn cancel(&self) {
        self.handle.cancel();
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
/// declared callback or property for the inspector to read it from (see
/// `ui/models/editor.slint`).
pub enum DeepSolveOutcome {
    Completed {
        /// The verified repair search's own solved masts, in the same tier order as
        /// `Design::tiers`/`Design::solve`'s output -- see [`tier_mast_deltas`].
        solved: Vec<SolvedTier>,
        /// The aggregate verdict to DISPLAY -- see `format_deep_solve_report`
        /// (`gui::editor::view`).
        report: VerifiedSolveReport,
        /// This design's own plain solve, computed on the background worker
        /// alongside `solved` when [`spawn_deep_solve`]'s caller had no valid
        /// cached one -- `None` either because the caller already had one (nothing
        /// to compute) or because the plain solve itself failed. Whichever it is,
        /// `callbacks::solve_actions::apply_deep_solve_outcome` falls back to its
        /// own cached copy first and only reads this when that cache was missing,
        /// so exactly one of the two is ever `Some` for a given run -- see that
        /// function's own comment.
        baseline: Option<Vec<SolvedTier>>,
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
/// status line. Only the ones the catalogue actually printed are shown --
/// `ExternalProportions`' fields are each independently optional -- so a design
/// missing, say, `C/W` simply omits it rather than showing a placeholder. Empty
/// string (nothing appended) only if every field is `None`, which cannot happen on
/// [`setup_deep_solve_callback`](super::super::callbacks::setup_deep_solve_callback)'s
/// one call site (it already requires `Some(targets)` to launch Deep Solve at all)
/// but keeps this safe to call generally.
///
/// `source` is `EditorState::asc_filename` at the moment Deep Solve was launched.
/// `printed_proportions` is captured once at catalogue load and never re-derived,
/// so the paired `.asc`'s own file name is the honest source for output formatting --
/// `None` prints no source clause at all rather than inventing one.
#[must_use]
pub fn format_verification_targets(targets: &ExternalProportions, source: Option<&str>) -> String {
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
    let source_clause = source.map_or_else(String::new, |name| format!(" (from \"{name}\")"));
    format!(
        " Verified against the printed proportions{source_clause}: {}.",
        parts.join(", ")
    )
}

/// `EditorState::printed_proportions` is captured once, at catalogue-load time,
/// and never re-derived from the design's own edits -- so a Deep Solve verdict
/// against it after any edit is a verdict against what the design was when printed,
/// not what it is now. `edited_since_load` should be the caller's
/// `EditorState::history.can_undo()` at the moment the run was launched: a fresh
/// `History` is created on every load/new/replace, so "can undo" means "at least one
/// edit has landed since load". Deliberately NOT `EditorState::is_dirty`
/// (`generation` vs. `saved_generation`): that goes back to `false` when the cutter
/// saves, even though the printed-proportions targets still describe the design as
/// it was when printed, not what it is post-edit -- saving does not un-invalidate
/// them.
#[must_use]
pub const fn edited_since_load_caveat(edited_since_load: bool) -> &'static str {
    if edited_since_load {
        " Note: this design has been edited since it was loaded -- these targets \
         may no longer describe the design you are holding."
    } else {
        ""
    }
}

/// Spawns a dedicated [`SolveService`] and submits one `Verified` request to it --
/// see the module doc comment, "Why this needs its own thread". `on_progress` is
/// invoked on the UI event loop, throttled to `solve_service`'s own ~10 Hz (an
/// honest "still working, N seconds in", not a fabricated percentage -- see "No
/// true progress fraction"); `on_done` exactly once, with the final outcome.
///
/// `need_baseline` is the caller's own answer to "do I already have a valid plain
/// solve for this design cached?" (`false` when it does) -- forwarded unchanged as
/// [`SolveKind::Verified::compute_baseline`], so a caller whose cache was missing
/// or stale gets that baseline computed on THIS worker thread, alongside the
/// verified search, rather than falling back to a synchronous `Design::solve` on
/// the UI thread. See [`DeepSolveOutcome::Completed::baseline`]'s own doc comment
/// for how the two possible sources reconcile at the one call site that reads
/// either.
pub fn spawn_deep_solve<T, P, D>(
    ui_weak: Weak<T>,
    design: Design,
    targets: ExternalProportions,
    need_baseline: bool,
    on_progress: P,
    on_done: D,
) -> DeepSolveHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, DeepSolveProgress) + Send + 'static + Clone,
    D: FnOnce(&T, DeepSolveOutcome) + Send + 'static,
{
    let start = Instant::now();
    // `on_done` is a one-shot `FnOnce`, but `SolveService::new` needs a `Fn` it can
    // clone onto every completion (this module's own `SolveService` only ever
    // completes once -- see the module doc comment -- but the type itself does not
    // know that). `Mutex<Option<D>>` lets the single real call `.take()` it; a
    // second call (which cannot happen here) would silently do nothing rather than
    // panic, the safer failure mode for a bound this loose.
    let on_done = Arc::new(Mutex::new(Some(on_done)));
    let service = SolveService::new(
        ui_weak,
        move |ui: &T, report: super::solve_service::SolveProgressReport| {
            debug_assert_eq!(
                report.generation, 0,
                "this module always submits generation 0"
            );
            // An honest trace of which pipeline-run phase/sweep the search is
            // currently on, at `solve_service`'s own ~10 Hz throttle -- cheap, and
            // the only place this module ever sees the raw `SolveProgress`. The module
            // uses only elapsed time, not a progress fraction (see doc comment).
            tracing::trace!(
                phase = ?report.progress.phase,
                sweep = report.progress.sweep,
                "deep solve progress"
            );
            on_progress(
                ui,
                DeepSolveProgress {
                    elapsed: start.elapsed(),
                },
            );
        },
        move |ui: &T, result: SolveResult| {
            debug_assert_eq!(
                result.generation, 0,
                "this module always submits generation 0"
            );
            tracing::info!(
                elapsed_ms = result.elapsed.as_millis(),
                "deep solve finished"
            );
            let SolveOutcome::Verified(verified) = result.outcome else {
                unreachable!("spawn_deep_solve only ever submits SolveKind::Verified");
            };
            let outcome = outcome_for(verified);
            let done = on_done
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(done) = done {
                done(ui, outcome);
            }
        },
    );
    // `adjustable_anchors: &[]` (inside `SolveKind::Verified`'s handling) -- every
    // `ScaleReference` tier here is a real recorded mast or authored dimension,
    // never an estimate safe to adjust. `generation: 0` -- see the closure above.
    let handle = service.submit(SolveRequest {
        design: Arc::new(design),
        generation: 0,
        kind: SolveKind::Verified {
            targets,
            compute_baseline: need_baseline,
        },
    });
    debug_assert_eq!(
        handle.generation, 0,
        "this module always submits generation 0"
    );
    DeepSolveHandle {
        handle,
        _service: service,
    }
}

/// What a finished run becomes for the UI: a cancelled run's `solved`/`report` are
/// discarded unconditionally, regardless of what they say, since the user has
/// already been told the editor stopped waiting. Pulled out of [`spawn_deep_solve`]
/// so this decision -- the one genuinely testable piece of behavior in an otherwise
/// thread-plumbing module -- has something a test can call directly.
fn outcome_for(result: Result<VerifiedSolve, SolveError>) -> DeepSolveOutcome {
    match result {
        Ok(VerifiedSolve {
            solved,
            report,
            baseline,
        }) => DeepSolveOutcome::Completed {
            solved,
            report,
            baseline,
        },
        // `TooManyPlanes` cannot occur in practice: Deep Solve is only offered once
        // the ordinary "Solve" button has already solved this SAME design
        // successfully, and the verified search never adds planes of its own -- so
        // the plane count already cleared `indicatrix::geometry::meet_solver::MAX_PLANES`
        // before this run started. Reported the same as a real cancellation
        // (nothing to show) rather than adding a third `DeepSolveOutcome` variant
        // for a case this module's one call site cannot reach.
        Err(SolveError::Cancelled | SolveError::TooManyPlanes { .. }) => {
            DeepSolveOutcome::Cancelled
        }
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

    // `DeepSolveHandle::cancel` delegates to `solve_service::SolveHandle::cancel` --
    // see `solve_service::tests::handle_cancel_sets_the_flag_a_worker_would_read`
    // for the underlying flag behavior. `DeepSolveHandle` itself can only be
    // constructed by `spawn_deep_solve` (its `handle`/`_service` fields require a
    // real `SolveService`), so there is nothing left to test bare here.

    // --- outcome_for: the cancellation decision ---

    #[test]
    fn outcome_for_reports_completed_with_the_report_when_ok() {
        let report = dummy_report(false);
        let solved = vec![dummy_solved_tier(1.0)];
        let outcome = outcome_for(Ok(VerifiedSolve {
            solved,
            report,
            baseline: None,
        }));
        match outcome {
            DeepSolveOutcome::Completed {
                solved: s,
                report: r,
                baseline,
            } => {
                assert_eq!(r.accepted, report.accepted);
                assert_eq!(r.pipeline_runs, report.pipeline_runs);
                assert_eq!(r.final_score, report.final_score);
                assert_eq!(s.len(), 1);
                assert!(baseline.is_none());
            }
            DeepSolveOutcome::Cancelled => panic!("expected Completed, got Cancelled"),
        }
    }

    // The baseline plain solve `solve_service::run_solve` computes on the
    // background worker (never on the UI thread -- see `spawn_deep_solve`'s own
    // doc comment) must reach `DeepSolveOutcome::Completed` unchanged, so the
    // completion handler can use it in place of a missing cache without any
    // further solving of its own.
    #[test]
    fn outcome_for_carries_a_worker_computed_baseline_through_to_completed() {
        let report = dummy_report(true);
        let solved = vec![dummy_solved_tier(1.0)];
        let baseline = vec![dummy_solved_tier(0.9)];
        let outcome = outcome_for(Ok(VerifiedSolve {
            solved,
            report,
            baseline: Some(baseline.clone()),
        }));
        match outcome {
            DeepSolveOutcome::Completed {
                baseline: Some(b), ..
            } => {
                assert_eq!(b.len(), baseline.len());
                assert!((b[0].mast - baseline[0].mast).abs() < 1e-9);
            }
            DeepSolveOutcome::Completed { baseline: None, .. } => {
                panic!("expected the worker-computed baseline to survive");
            }
            DeepSolveOutcome::Cancelled => panic!("expected Completed, got Cancelled"),
        }
    }

    #[test]
    fn outcome_for_reports_cancelled_on_a_cancelled_solve_error() {
        let outcome = outcome_for(Err(SolveError::Cancelled));
        assert!(matches!(outcome, DeepSolveOutcome::Cancelled));
    }

    #[test]
    fn outcome_for_reports_cancelled_on_the_unreachable_in_practice_too_many_planes_case() {
        // See `outcome_for`'s own doc comment for why this cannot happen from
        // `spawn_deep_solve`'s one call site, and why `Cancelled` (nothing to
        // show) is the honest fallback rather than a panic.
        let outcome = outcome_for(Err(SolveError::TooManyPlanes {
            planes: 999,
            max: 400,
        }));
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
        let text = format_verification_targets(&targets, None);
        assert!(text.contains("Vol/W3 0.6013"));
        assert!(text.contains("C/W 0.1489"));
        assert!(!text.contains("L/W"));
        assert!(!text.contains("P/W"));
        assert!(!text.contains("H/W"));
    }

    #[test]
    fn format_verification_targets_is_empty_when_nothing_is_set() {
        assert_eq!(
            format_verification_targets(&ExternalProportions::default(), Some("RBC-445.asc")),
            "",
            "no source clause should print when there are no targets to report at all"
        );
    }

    #[test]
    fn format_verification_targets_names_the_source_when_given_one() {
        // The verdict must say where the printed figures it verified against came
        // from.
        let targets = ExternalProportions {
            vol_w3: Some(0.6013),
            ..ExternalProportions::default()
        };
        let text = format_verification_targets(&targets, Some("RBC-445.asc"));
        assert!(text.contains("RBC-445.asc"), "got: {text}");
    }

    #[test]
    fn format_verification_targets_omits_the_source_clause_when_none() {
        let targets = ExternalProportions {
            vol_w3: Some(0.6013),
            ..ExternalProportions::default()
        };
        let text = format_verification_targets(&targets, None);
        assert!(!text.contains("(from"), "got: {text}");
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
