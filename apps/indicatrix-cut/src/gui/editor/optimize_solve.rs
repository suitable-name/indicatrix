//! An explicit, off-thread, cancellable "Optimize" action, following `deep_solve.rs`'s
//! own conventions (`thread::spawn` + `Arc<AtomicBool>` cancel +
//! `Weak::upgrade_in_event_loop` progress) exactly -- see that module's own doc comment
//! for why this shape (not `bridge::export_thread`'s, not a bespoke one) is the
//! established pattern for a long-running, cancellable editor action in this crate.
//!
//! # Why this is not `gui::tilt_batch`/`gui::batch_queue`'s shape
//!
//! Those modules distribute independent whole-design work items (a catalogue design's
//! render, or its tilt-curve sweep) across local lanes and a remote dispatcher. A
//! single design's own coordinate search is a different shape: one design, one
//! evolving state, sweeps that depend on each other's outcome. What is directly reused
//! from that class of infrastructure is narrower and lives inside
//! `indicatrix_cut_core::optimize` itself: `evaluate_candidate_pair`'s two-OS-thread
//! parallel evaluation of a tier's `+step`/`-step` candidates -- remote dispatch
//! (unlike the local parallelism) is not achievable without extending
//! `indicatrix-net`'s wire protocol.
//!
//! # Real progress, not just elapsed time
//!
//! `deep_solve.rs`'s own [`deep_solve::DeepSolveProgress`](super::deep_solve::DeepSolveProgress)
//! carries only elapsed wall time, because `solve_meet_points_verified`'s repair
//! search has no caller-visible loop this crate controls.
//! `indicatrix_cut_core::optimize::optimize_design` is the opposite: this crate wrote
//! its own search loop, so [`OptimizeSolveProgress`] carries the real running
//! evaluation count (via [`indicatrix_cut_core::optimize::SearchHooks::on_progress`])
//! alongside elapsed time -- an honest "N of your M-evaluation budget so far", not an
//! estimate.
//!
//! A dedicated ticker thread still exists (same `TICK_INTERVAL` idea as
//! `deep_solve.rs`), not to fabricate progress but to throttle how often the UI thread
//! is asked to redraw: `on_progress` is called after every one of potentially hundreds
//! of tier decisions, cheaply updating a shared [`AtomicUsize`] with no thread hop; the
//! ticker reads that counter on its own schedule and is the only thing that ever calls
//! `Weak::upgrade_in_event_loop`.
//!
//! # Cancellation is a real mid-search checkpoint
//!
//! `optimize_design` polls `cancel` once per tier decision, so a cancellation here
//! typically takes effect within one `evaluate_candidate_pair` call's latency
//! (milliseconds to a few seconds) -- the same kind of real checkpoint
//! `deep_solve::DeepSolveHandle::cancel` polls per pipeline run (see that module's
//! own doc comment, "Cancellation is a real mid-search checkpoint"), just at a
//! different grain. [`OptimizeSolveOutcome::Cancelled`] differs from
//! `deep_solve::DeepSolveOutcome::Cancelled` in what it carries, though: a real
//! partial `OptimizeOutcome` (whatever the search found before the checkpoint
//! fired), not `deep_solve`'s empty, discarded variant.

// Wired via `gui::editor::setup_optimize_callback`/`setup_optimize_cancel_callback`/
// `setup_optimize_apply_callback`, which call `spawn_optimize_solve` below the exact
// same way `gui::editor`'s own `setup_deep_solve_callback` calls
// `deep_solve::spawn_deep_solve`. Unlike `deep_solve` (a read-only diagnostic that
// never mutates `Design`), an Optimize result the user chooses to keep is applied
// through `indicatrix_cut_core::apply_optimize_outcome` (`History::apply`, one
// `Edit::ModifyTier` per changed tier) by `setup_optimize_apply_callback`, never
// straight into `Design` -- so it stays undoable exactly like every other edit this
// crate makes, and `optimize_design` itself never needs to know `History` exists.

use super::material_lookup::{EditorMaterialLookup, resolved_gem_material};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{
    Design, DesignSolveError, MaterialSelection, OptimizeConfig, OptimizeOutcome, SearchHooks,
    free_tier_indices,
    optimize::{SearchStage, inclusive_max_evaluations},
    optimize_design,
};
use slint::{ComponentHandle, Weak};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Matches `deep_solve::TICK_INTERVAL`'s own value and rationale: cheap and infrequent
/// enough to cost nothing against a computation already measured in the hundreds of
/// milliseconds to hundreds of seconds (see `indicatrix_cut_core::optimize`'s own "Cost first" doc
/// section).
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Handle returned by [`spawn_optimize_solve`]. See the module doc comment's
/// "Cancellation is a REAL mid-search checkpoint" section for exactly what cancelling
/// does.
pub struct OptimizeSolveHandle {
    cancel: Arc<AtomicBool>,
}

impl OptimizeSolveHandle {
    /// Requests cancellation -- see the module doc comment, "Cancellation is a
    /// real mid-search checkpoint".
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One progress tick posted while an optimize run is in progress -- see the module doc
/// comment's "Real progress, not just elapsed time" section.
///
/// `max_evaluations` is [`inclusive_max_evaluations`]'s figure (coordinate stage
/// PLUS the polish stage's own cap), not [`OptimizeConfig::max_evaluations`] alone.
/// This prevents the evaluation counter sailing past `max_evaluations` once the
/// polish stage starts. `stage` is the true search stage at each progress report,
/// so a caller can show a stage name instead of a stalled fraction while
/// [`SearchStage::BaselineFull`]/[`SearchStage::FinalFull`] are running.
#[derive(Debug, Clone, Copy)]
pub struct OptimizeSolveProgress {
    pub evaluations: usize,
    pub max_evaluations: usize,
    pub stage: SearchStage,
    pub elapsed: Duration,
}

/// What an optimize run produced.
pub enum OptimizeSolveOutcome {
    Completed {
        outcome: OptimizeOutcome,
    },
    /// The user cancelled before the search's own loop observed it -- see the module
    /// doc comment's "Cancellation is a REAL mid-search checkpoint" section.
    /// `outcome` is the REAL, honest partial result (whatever was found before the
    /// checkpoint fired), never discarded.
    Cancelled {
        outcome: OptimizeOutcome,
    },
    /// `design` itself did not solve at all -- no
    /// [`indicatrix_cut_core::design::MeetConstraint::ScaleReference`] anchor for
    /// one or more blocks, or a [`indicatrix_cut_core::TierTarget`] could not be
    /// resolved -- so the search was never even started.
    Failed {
        error: DesignSolveError,
    },
}

/// Everything [`spawn_optimize_solve`]'s worker thread needs, cloned onto it -- bundled
/// purely so the function signature itself stays short (the same reasoning
/// `indicatrix::color::metrics`'s per-evaluation context structs document on themselves).
struct OptimizeJob {
    design: Design,
    material_selection: MaterialSelection,
    /// The catalogue's own custom materials at the moment Optimize was launched -- so
    /// the objective is scored against the same resolved material (built-ins,
    /// customs, and any `refractive_index_override`, via [`resolved_gem_material`])
    /// the viewport and tilt curves use for this exact design, not a built-ins-only,
    /// override-blind fallback.
    custom_materials: Vec<GemMaterial>,
    config: OptimizeConfig,
}

/// Spawns the optimize worker (plus its progress ticker) off the UI thread. `on_progress`
/// is invoked on the UI event loop roughly every [`TICK_INTERVAL`] while the search
/// runs; `on_done` exactly once, with the final outcome. Mirrors
/// `deep_solve::spawn_deep_solve`'s generic-handle shape exactly (same reasoning: no
/// direct dependency on `crate::MainWindow`).
pub fn spawn_optimize_solve<T, P, D>(
    ui_weak: Weak<T>,
    design: Design,
    material_selection: MaterialSelection,
    custom_materials: Vec<GemMaterial>,
    config: OptimizeConfig,
    on_progress: P,
    on_done: D,
) -> OptimizeSolveHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, OptimizeSolveProgress) + Send + 'static + Clone,
    D: FnOnce(&T, OptimizeSolveOutcome) + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_ticker = Arc::clone(&cancel);
    let cancel_worker = Arc::clone(&cancel);
    let done_flag = Arc::new(AtomicBool::new(false));
    let done_ticker = Arc::clone(&done_flag);
    let evaluations_done = Arc::new(AtomicUsize::new(0));
    let evaluations_ticker = Arc::clone(&evaluations_done);
    // `SearchStage::BaselineFull` (`0`) is also the correct stage to show before
    // the worker thread's first real report ever lands.
    let stage_done = Arc::new(AtomicU8::new(SearchStage::BaselineFull.to_code()));
    let stage_ticker = Arc::clone(&stage_done);
    let ticker_ui = ui_weak.clone();
    // Include both coordinate and polish stage evaluation budgets in the reported cap,
    // so the total never appears to exceed `max_evaluations`.
    let max_evaluations = inclusive_max_evaluations(&config, free_tier_indices(&design).len());

    // Ticker: throttles how often the UI thread is asked to redraw progress -- see the
    // module doc comment's "Real progress, not just elapsed time" section for why this
    // reads (never fabricates) the real running evaluation count.
    thread::spawn(move || {
        let start = Instant::now();
        loop {
            thread::sleep(TICK_INTERVAL);
            if done_ticker.load(Ordering::Relaxed) || cancel_ticker.load(Ordering::Relaxed) {
                break;
            }
            let progress = OptimizeSolveProgress {
                evaluations: evaluations_ticker.load(Ordering::Relaxed),
                max_evaluations,
                stage: SearchStage::from_code(stage_ticker.load(Ordering::Relaxed)),
                elapsed: start.elapsed(),
            };
            let on_progress = on_progress.clone();
            let _ = ticker_ui.upgrade_in_event_loop(move |ui| {
                on_progress(&ui, progress);
            });
        }
    });

    let job = OptimizeJob {
        design,
        material_selection,
        custom_materials,
        config,
    };

    thread::spawn(move || {
        let lookup = EditorMaterialLookup::new(&job.custom_materials);
        let material = resolved_gem_material(&job.material_selection, &lookup);
        let hooks = SearchHooks {
            cancel: Some(&cancel_worker),
            on_progress: Some(&|evaluations: usize, stage: SearchStage| {
                evaluations_done.store(evaluations, Ordering::Relaxed);
                stage_done.store(stage.to_code(), Ordering::Relaxed);
            }),
        };
        let result = optimize_design(&job.design, &material, &job.config, &hooks);
        done_flag.store(true, Ordering::Relaxed);
        let cancelled = cancel_worker.load(Ordering::Relaxed);
        let outcome = outcome_for(cancelled, result);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            on_done(&ui, outcome);
        });
    });

    OptimizeSolveHandle { cancel }
}

/// What a finished worker-thread call becomes for the UI -- see [`OptimizeSolveOutcome`]'s
/// own doc comment. Unlike `deep_solve`'s `outcome_for`, a cancelled run's OWN partial
/// [`OptimizeOutcome`] is preserved (see the module doc comment's "Cancellation is a
/// REAL mid-search checkpoint" section), not discarded -- only a genuine solve failure
/// (checked first, since it can happen regardless of `cancelled`) takes priority.
fn outcome_for(
    cancelled: bool,
    result: Result<OptimizeOutcome, DesignSolveError>,
) -> OptimizeSolveOutcome {
    match result {
        Err(error) => OptimizeSolveOutcome::Failed { error },
        Ok(outcome) if cancelled => OptimizeSolveOutcome::Cancelled { outcome },
        Ok(outcome) => OptimizeSolveOutcome::Completed { outcome },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{AngleChange, MissingAnchor, ObjectiveComponents};

    fn dummy_outcome(changed: bool) -> OptimizeOutcome {
        let components = ObjectiveComponents {
            windowing_pct: 10.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        };
        OptimizeOutcome {
            before: components,
            before_score: 10.0,
            before_yield_loss_pct: 0.0,
            after: components,
            after_score: 10.0,
            after_yield_loss_pct: 0.0,
            evaluations: 4,
            changes: if changed {
                vec![AngleChange {
                    index: 0,
                    from_deg: 30.0,
                    to_deg: 31.0,
                }]
            } else {
                Vec::new()
            },
            cancelled: false,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        }
    }

    // --- OptimizeSolveHandle::cancel ---

    #[test]
    fn cancel_sets_the_flag_the_worker_and_ticker_threads_poll() {
        let flag = Arc::new(AtomicBool::new(false));
        let handle = OptimizeSolveHandle {
            cancel: Arc::clone(&flag),
        };
        assert!(!flag.load(Ordering::Relaxed));
        handle.cancel();
        assert!(flag.load(Ordering::Relaxed));
    }

    // --- outcome_for ---

    #[test]
    fn outcome_for_reports_failed_regardless_of_cancelled_when_the_design_does_not_solve() {
        // A `MissingAnchor` can happen whether or not the user also cancelled --
        // solve failure takes priority since there is no partial search outcome to
        // report at all in that case.
        let error = DesignSolveError::MissingAnchor(MissingAnchor {
            blocks: vec![indicatrix::geometry::meet_solver::Block::Crown],
        });
        for cancelled in [true, false] {
            let outcome = outcome_for(cancelled, Err(error.clone()));
            assert!(matches!(outcome, OptimizeSolveOutcome::Failed { .. }));
        }
    }

    #[test]
    fn outcome_for_preserves_the_real_partial_outcome_when_cancelled() {
        let outcome = outcome_for(true, Ok(dummy_outcome(true)));
        match outcome {
            OptimizeSolveOutcome::Cancelled { outcome } => {
                assert_eq!(outcome.changes.len(), 1);
                assert_eq!(outcome.evaluations, 4);
            }
            _ => panic!("expected Cancelled"),
        }
    }

    #[test]
    fn outcome_for_reports_completed_when_not_cancelled() {
        let outcome = outcome_for(false, Ok(dummy_outcome(false)));
        assert!(matches!(outcome, OptimizeSolveOutcome::Completed { .. }));
    }

    // --- OptimizeSolveProgress ---

    #[test]
    fn progress_is_copy_not_just_clone() {
        // `spawn_optimize_solve`'s ticker thread hands each tick a fresh
        // `OptimizeSolveProgress` by value across the `upgrade_in_event_loop`
        // boundary -- `Copy` (not just `Clone`) is load-bearing there, same as
        // `deep_solve::DeepSolveProgress`.
        let progress = OptimizeSolveProgress {
            evaluations: 12,
            max_evaluations: 200,
            stage: SearchStage::Coordinate,
            elapsed: Duration::from_secs(1),
        };
        let copied = progress;
        assert_eq!(copied.evaluations, progress.evaluations);
    }

    // --- material resolution (via `super::material_lookup::resolved_gem_material`) ---

    #[test]
    fn optimize_job_resolves_no_material_selection_to_diamond() {
        // Mirrors `material_lookup`'s own
        // `resolved_gem_material_uses_the_real_dispersion_curve_with_no_override`
        // fallback, exercised here through the exact path `spawn_optimize_solve`'s
        // worker thread now takes -- a brand-new design's `MaterialSelection::none()`
        // has no name at all, so the objective must still resolve to SOMETHING
        // real (diamond, via `MaterialSelection::resolve`'s own documented
        // fallback), never fail the whole run over a display concern.
        let lookup = EditorMaterialLookup::new(&[]);
        let material = resolved_gem_material(&MaterialSelection::none(), &lookup);
        assert_eq!(material.name, GemMaterial::diamond().name);
    }
}
