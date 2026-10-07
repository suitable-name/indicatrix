//! An explicit, off-thread, cancellable "Optimize" action, following `deep_solve.rs`'s
//! own conventions (`thread::spawn` + `Arc<AtomicBool>` cancel +
//! `Weak::upgrade_in_event_loop` progress) exactly -- see that module's own doc comment
//! for why this shape (not `bridge::export_thread`'s, not a bespoke one) is the
//! established pattern for a long-running, cancellable editor action in this crate.
//!
//! [`spawn_optimize_run`] is the Optimize tab's entry point (a full [`OptimizeOptions`]
//! request: anchored tiers, angle ranges, the girdle guard, several candidates, an
//! [`OptimizeResult`] back). Retarget's Optimize mode has its own worker in
//! `callbacks::retarget_actions::optimize_run`.
//!
//! # Why this is not `gui::batch::tilt`/`gui::batch::batch_queue`'s shape
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
//! # Hinges are measured on the worker
//!
//! A run that varies anchored tiers needs each tier's hinge, which needs a solved design.
//! The worker solves once and measures them (see [`prepare_anchor_hinges`], which the
//! command line's `optimize` shares) instead of the UI thread, which must never solve
//! synchronously and whose cached solve may be a frame behind the design.
//!
//! # Cancellation is a real mid-search checkpoint
//!
//! `optimize_design` polls `cancel` once per tier decision, so a cancellation here
//! typically takes effect within one `evaluate_candidate_pair` call's latency
//! (milliseconds to a few seconds) -- the same kind of real checkpoint
//! `deep_solve::DeepSolveHandle::cancel` polls per pipeline run (see that module's
//! own doc comment, "Cancellation is a real mid-search checkpoint"), just at a
//! different grain. [`OptimizeRunOutcome::Cancelled`] differs from
//! `deep_solve::DeepSolveOutcome::Cancelled` in what it carries, though: a real
//! partial result (whatever the search found before the checkpoint fired), not
//! `deep_solve`'s empty, discarded variant.

// Wired via `gui::editor::setup_optimize_callback`/`setup_optimize_cancel_callback`/
// `setup_optimize_apply_callback`, which call `spawn_optimize_run` below the exact
// same way `gui::editor`'s own `setup_deep_solve_callback` calls
// `deep_solve::spawn_deep_solve`. Unlike `deep_solve` (a read-only diagnostic that
// never mutates `Design`), an Optimize result the user chooses to keep is applied
// through `indicatrix_editor::optimize_view::apply_candidate` (`History::apply`, one
// `Edit::ModifyTier` per changed tier) by `setup_optimize_apply_callback`, never
// straight into `Design` -- so it stays undoable exactly like every other edit this
// crate makes, and `optimize_design` itself never needs to know `History` exists.

use super::material_lookup::{EditorMaterialLookup, resolved_gem_material};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{
    Design, DesignSolveError, MaterialSelection, OptimizeConfig, OptimizeOptions, OptimizeResult,
    SearchHooks, free_tier_indices_with,
    optimize::{SearchStage, StartProgress, inclusive_max_evaluations_for},
    optimize_design_with,
};
use indicatrix_editor::{
    material_lookup::sized_material_for_optimize, optimize_view::measure_anchor_hinges,
    solve_policy::design_to_gpu_planes_from_solved,
};
use slint::{ComponentHandle, Weak};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Matches `deep_solve::TICK_INTERVAL`'s own value and rationale: cheap and infrequent
/// enough to cost nothing against a computation already measured in the hundreds of
/// milliseconds to hundreds of seconds (see `indicatrix_cut_core::optimize`'s own "Cost first" doc
/// section).
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Handle returned by [`spawn_optimize_run`]. See the module
/// doc comment's "Cancellation is a REAL mid-search checkpoint" section for exactly what
/// cancelling does.
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
/// `max_evaluations` is [`inclusive_max_evaluations_for`]'s figure (screening, coordinate
/// stage PLUS the polish stages' own caps), not [`OptimizeConfig::max_evaluations`] alone.
/// This prevents the evaluation counter sailing past `max_evaluations` once the
/// polish stage starts. `stage` is the true search stage at each progress report,
/// so a caller can show a stage name instead of a stalled fraction while
/// [`SearchStage::BaselineFull`]/[`SearchStage::FinalFull`] are running.
#[derive(Debug, Clone, Copy)]
pub struct OptimizeSolveProgress {
    pub evaluations: usize,
    pub max_evaluations: usize,
    pub stage: SearchStage,
    /// The several-starts search's latest "start i of n, best ..." report; `None` for a
    /// single start and before the first report.
    pub start: Option<StartProgress>,
    pub elapsed: Duration,
}

/// The latest [`StartProgress`] as atomics the worker's hook writes and the ticker reads.
/// `count == 0` means no report yet.
#[derive(Default)]
struct StartCell {
    index: AtomicUsize,
    count: AtomicUsize,
    best_bits: AtomicU32,
}

impl StartCell {
    fn store(&self, progress: StartProgress) {
        self.index.store(progress.index, Ordering::Relaxed);
        self.best_bits
            .store(progress.best_fast_score.to_bits(), Ordering::Relaxed);
        // Written last: a reader that sees a count sees an index no older than it.
        self.count.store(progress.count, Ordering::Relaxed);
    }

    fn load(&self) -> Option<StartProgress> {
        let count = self.count.load(Ordering::Relaxed);
        (count > 0).then(|| StartProgress {
            index: self.index.load(Ordering::Relaxed),
            count,
            best_fast_score: f32::from_bits(self.best_bits.load(Ordering::Relaxed)),
        })
    }
}

/// The "start K of N" report worth showing at `stage`: dropped once the polish (and the final
/// scoring after it) is running, where the stored report is stale; kept for screening and the
/// coordinate stage.
fn start_for_stage(stage: SearchStage, start: Option<StartProgress>) -> Option<StartProgress> {
    match stage {
        SearchStage::Polish | SearchStage::FinalFull => None,
        _ => start,
    }
}

/// What a run of the Optimize tab produced: the whole [`OptimizeResult`] (the best outcome,
/// the ranked candidates and the mast changes).
pub enum OptimizeRunOutcome {
    /// The search finished.
    Completed { result: OptimizeResult },
    /// The user cancelled before the search's own loop observed it; `result` is the real,
    /// honest partial result (whatever was found before the checkpoint fired), never
    /// discarded -- see the module doc comment's "Cancellation is a real mid-search
    /// checkpoint" section.
    Cancelled { result: OptimizeResult },
    /// `design` itself did not solve at all -- no
    /// [`indicatrix_cut_core::design::MeetConstraint::ScaleReference`] anchor for one or
    /// more blocks, or a [`indicatrix_cut_core::TierTarget`] could not be resolved -- so the
    /// search was never even started.
    Failed { error: DesignSolveError },
}

/// Everything [`spawn_optimize_run`]'s worker thread needs, cloned onto it -- bundled
/// purely so the function signature itself stays short (the same reasoning
/// `indicatrix::color::metrics`'s per-evaluation context structs document on themselves).
pub struct OptimizeRunRequest {
    pub design: Design,
    pub material_selection: MaterialSelection,
    /// The catalogue's own custom materials at the moment Optimize was launched -- so
    /// the objective is scored against the same resolved material (built-ins,
    /// customs, and any `refractive_index_override`, via [`resolved_gem_material`])
    /// the viewport and tilt curves use for this exact design, not a built-ins-only,
    /// override-blind fallback.
    pub custom_materials: Vec<GemMaterial>,
    pub config: OptimizeConfig,
    /// What may change beyond the search knobs. With `vary_anchored` on, the worker fills
    /// `anchor_hinges` itself (see [`prepare_anchor_hinges`]).
    pub options: OptimizeOptions,
    /// The tiers anchored variation is limited to ("Only selected tiers"); `None` means
    /// every tier that may vary.
    pub only_tiers: Option<BTreeSet<usize>>,
}

/// Spawns the Optimize tab's worker (plus its progress ticker) off the UI thread.
/// `on_progress` is invoked on the UI event loop roughly every [`TICK_INTERVAL`] while the
/// search runs; `on_done` exactly once, with the final outcome. Mirrors
/// `deep_solve::spawn_deep_solve`'s generic-handle shape exactly (same reasoning: no
/// direct dependency on `crate::MainWindow`).
pub fn spawn_optimize_run<T, P, D>(
    ui_weak: Weak<T>,
    request: OptimizeRunRequest,
    on_progress: P,
    on_done: D,
) -> OptimizeSolveHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, OptimizeSolveProgress) + Send + 'static + Clone,
    D: FnOnce(&T, OptimizeRunOutcome) + Send + 'static,
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
    let start_done = Arc::new(StartCell::default());
    let start_ticker = Arc::clone(&start_done);
    let ticker_ui = ui_weak.clone();
    // Include screening, the coordinate stage and every kept start's polish in the reported
    // cap, so the total never appears to exceed `max_evaluations`. The free-tier count is
    // refined by the worker once the hinges are known.
    let max_evaluations = Arc::new(AtomicUsize::new(inclusive_max_evaluations_for(
        &request.config,
        request.options.keep_candidates,
        free_tier_indices_with(&request.design, &request.options).len(),
    )));
    let max_evaluations_ticker = Arc::clone(&max_evaluations);

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
            let stage = SearchStage::from_code(stage_ticker.load(Ordering::Relaxed));
            let progress = OptimizeSolveProgress {
                evaluations: evaluations_ticker.load(Ordering::Relaxed),
                max_evaluations: max_evaluations_ticker.load(Ordering::Relaxed),
                stage,
                start: start_for_stage(stage, start_ticker.load()),
                elapsed: start.elapsed(),
            };
            let on_progress = on_progress.clone();
            let _ = ticker_ui.upgrade_in_event_loop(move |ui| {
                on_progress(&ui, progress);
            });
        }
    });

    thread::spawn(move || {
        let OptimizeRunRequest {
            design,
            material_selection,
            custom_materials,
            config,
            mut options,
            only_tiers,
        } = request;
        let lookup = EditorMaterialLookup::new(&custom_materials);
        let material = resolved_gem_material(&material_selection, &lookup);
        let hooks = SearchHooks {
            cancel: Some(&cancel_worker),
            on_progress: Some(&|evaluations: usize, stage: SearchStage| {
                evaluations_done.store(evaluations, Ordering::Relaxed);
                stage_done.store(stage.to_code(), Ordering::Relaxed);
            }),
            on_start: Some(&|progress: StartProgress| start_done.store(progress)),
        };
        let result = prepare_run_inputs(&design, material, &mut options, only_tiers.as_ref())
            .and_then(|material| {
                max_evaluations.store(
                    inclusive_max_evaluations_for(
                        &config,
                        options.keep_candidates,
                        free_tier_indices_with(&design, &options).len(),
                    ),
                    Ordering::Relaxed,
                );
                optimize_design_with(&design, &material, &config, &options, &hooks)
            });
        done_flag.store(true, Ordering::Relaxed);
        let cancelled = cancel_worker.load(Ordering::Relaxed);
        let outcome = run_outcome_for(cancelled, result);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            on_done(&ui, outcome);
        });
    });

    OptimizeSolveHandle { cancel }
}

/// The worker's one solve: measures the hinges a run varying anchored tiers needs, and sizes
/// `material` by the design's girdle diameter like the render does (see
/// [`sized_material_for_optimize`]), so the face-up colour the tone objective works out
/// matches the Live Render. The solve is skipped when neither is wanted.
///
/// # Errors
///
/// [`DesignSolveError`] when the design does not solve, the same error the search would give.
fn prepare_run_inputs(
    design: &Design,
    material: GemMaterial,
    options: &mut OptimizeOptions,
    only: Option<&BTreeSet<usize>>,
) -> Result<GemMaterial, DesignSolveError> {
    let wants_size = design
        .girdle_diameter_mm
        .is_some_and(|mm| mm > 0.0 && mm.is_finite());
    if !options.vary_anchored && !wants_size {
        return Ok(material);
    }
    let solved = design.solve()?;
    if options.vary_anchored {
        measure_anchor_hinges(options, design, &solved, only);
    }
    let planes = design_to_gpu_planes_from_solved(design, &solved);
    Ok(sized_material_for_optimize(material, design, &planes))
}

/// What a finished worker-thread call becomes for the UI -- see [`OptimizeRunOutcome`]'s
/// own doc comment. Unlike `deep_solve`'s `outcome_for`, a cancelled run's OWN partial
/// [`OptimizeResult`] is preserved (see the module doc comment's "Cancellation is a
/// REAL mid-search checkpoint" section), not discarded -- only a genuine solve failure
/// (checked first, since it can happen regardless of `cancelled`) takes priority.
fn run_outcome_for(
    cancelled: bool,
    result: Result<OptimizeResult, DesignSolveError>,
) -> OptimizeRunOutcome {
    match result {
        Err(error) => OptimizeRunOutcome::Failed { error },
        Ok(result) if cancelled => OptimizeRunOutcome::Cancelled { result },
        Ok(result) => OptimizeRunOutcome::Completed { result },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{AngleChange, MissingAnchor, ObjectiveComponents, OptimizeOutcome};

    #[test]
    fn the_start_prefix_is_dropped_during_polish_and_kept_for_coordinate() {
        let start = Some(StartProgress {
            index: 2,
            count: 5,
            best_fast_score: 1.0,
        });
        assert!(start_for_stage(SearchStage::Coordinate, start).is_some());
        assert!(start_for_stage(SearchStage::Screening, start).is_some());
        assert!(start_for_stage(SearchStage::Polish, start).is_none());
        assert!(start_for_stage(SearchStage::FinalFull, start).is_none());
    }

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

    fn dummy_result(changed: bool) -> OptimizeResult {
        OptimizeResult {
            outcome: dummy_outcome(changed),
            mast_changes: Vec::new(),
            candidates: Vec::new(),
            tone_before: None,
            tone_goal: None,
            lighting: indicatrix_cut_core::CANONICAL_LIGHTING_PRESET,
            starts_run: 1,
            best_start: 0,
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

    // --- run_outcome_for ---

    #[test]
    fn run_outcome_for_reports_failed_regardless_of_cancelled_when_the_design_does_not_solve() {
        // A `MissingAnchor` can happen whether or not the user also cancelled --
        // solve failure takes priority since there is no partial search outcome to
        // report at all in that case.
        let error = DesignSolveError::MissingAnchor(MissingAnchor {
            blocks: vec![indicatrix::geometry::meet_solver::Block::Crown],
        });
        for cancelled in [true, false] {
            let outcome = run_outcome_for(cancelled, Err(error.clone()));
            assert!(matches!(outcome, OptimizeRunOutcome::Failed { .. }));
        }
    }

    #[test]
    fn run_outcome_for_preserves_the_real_partial_result_when_cancelled() {
        let outcome = run_outcome_for(true, Ok(dummy_result(true)));
        match outcome {
            OptimizeRunOutcome::Cancelled { result } => {
                assert_eq!(result.outcome.changes.len(), 1);
                assert_eq!(result.outcome.evaluations, 4);
            }
            _ => panic!("expected Cancelled"),
        }
    }

    #[test]
    fn run_outcome_for_reports_completed_when_not_cancelled() {
        let outcome = run_outcome_for(false, Ok(dummy_result(false)));
        assert!(matches!(outcome, OptimizeRunOutcome::Completed { .. }));
    }

    // The hinge measurement itself (`prepare_anchor_hinges`) is shared with the command line
    // and is tested in `indicatrix_editor::optimize_view`.

    // --- the girdle-diameter size rule ---

    #[test]
    fn optimize_job_sizes_the_material_by_the_girdle_diameter() {
        let mut design = indicatrix_editor::EditorSession::fresh().design;
        let coloured = || {
            GemMaterial::sapphire().with_body_color(
                indicatrix::optics::materials::body_color::BODY_COLOR_PRESETS[1].absorption_rgb,
            )
        };
        let mut options = OptimizeOptions::default();
        let unsized_material = prepare_run_inputs(&design, coloured(), &mut options, None)
            .expect("the fresh design solves");
        assert_eq!(unsized_material.absorption_path_scale, 1.0);

        design.girdle_diameter_mm = Some(6.5);
        let sized = prepare_run_inputs(&design, coloured(), &mut options, None)
            .expect("the fresh design solves");
        assert!(
            sized.absorption_path_scale > 1.0,
            "{}",
            sized.absorption_path_scale
        );
    }

    // --- OptimizeSolveProgress ---

    #[test]
    fn progress_is_copy_not_just_clone() {
        // `spawn_optimize_run`'s ticker thread hands each tick a fresh
        // `OptimizeSolveProgress` by value across the `upgrade_in_event_loop`
        // boundary -- `Copy` (not just `Clone`) is load-bearing there, same as
        // `deep_solve::DeepSolveProgress`.
        let progress = OptimizeSolveProgress {
            evaluations: 12,
            max_evaluations: 200,
            stage: SearchStage::Coordinate,
            start: None,
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
        // fallback, exercised here through the exact path `spawn_optimize_run`'s
        // worker thread now takes -- a brand-new design's `MaterialSelection::none()`
        // has no name at all, so the objective must still resolve to SOMETHING
        // real (diamond, via `MaterialSelection::resolve`'s own documented
        // fallback), never fail the whole run over a display concern.
        let lookup = EditorMaterialLookup::new(&[]);
        let material = resolved_gem_material(&MaterialSelection::none(), &lookup);
        assert_eq!(material.name, GemMaterial::diamond().name);
    }
}
