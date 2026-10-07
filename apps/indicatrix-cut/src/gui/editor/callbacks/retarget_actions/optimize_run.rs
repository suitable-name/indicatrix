//! Retarget "Optimize": the search the Optimize tab starts, run off the UI thread.
//!
//! The editor crate owns the search (`indicatrix_editor::retarget::search`); this module owns
//! everything around it that touches the window:
//!
//! - [`start_search`] gathers the inputs, starts a worker thread and a progress timer. The
//!   worker reads a cancel flag between steps and writes its step count into atomics; the
//!   timer turns them into the progress line on the UI thread.
//! - [`finish_search`] receives the report on the UI thread, keeps it in a results slot and
//!   fills the candidate list. The best option is picked at once.
//! - [`select_candidate`] makes one option the dialog's proposal: its rows, verdict, masts and
//!   optical table are copied to the places Apply, the embedded comparison and the compare
//!   window already read for a Shift result. Apply then commits it as one undo step.
//! - [`begin_probe`] times one evaluation on a worker so the dialog can say how long a search
//!   will take before it is started. The editor crate has no clock; the time is taken here.
//!
//! A search is cancelled, and its results forgotten, by `RetargetAsyncRun::cancel_and_supersede`
//! (in the parent module), which every change of input (mode, target, crown) goes through.

use super::{
    RETARGET_ASYNC,
    check_run::{current_gem, reset_check, set_external_check},
    material::{resolve_target_selection, resolved_material_from_selection},
    proposal_view::{
        candidate_row_views, plan_view, push_candidates, push_retarget_view, push_target_error,
        push_target_readout, search_summary_line, string_model,
    },
    sync_embedded_comparison,
};
use crate::{
    MainWindow, RetargetCandidateItem, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::editor::{
        auto_solve,
        retarget::{
            CrownShift, GirdleAllowance, RetargetPlan, build_plan,
            check::RetargetCheck,
            search::{
                EFFORT_CHOICES, RANGE_CHOICES_DEG, SearchError, SearchInputs, SearchReport,
                SearchSettings, estimate_text, probe_evaluation, run_search,
            },
        },
        stale::{self, ResultKind},
        state::EditorState,
    },
};
use indicatrix::optics::{LightingPreset, materials::GemMaterial};
use indicatrix_cut_core::{
    CANONICAL_LIGHTING_PRESET, Design, ObjectivePreset, optimize::SearchStage,
};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};

/// How often the progress line is refreshed while a search runs.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// The names of the effort choices, in the order of [`EFFORT_CHOICES`].
const EFFORT_NAMES: [&str; 3] = ["Quick", "Normal", "Thorough"];

/// Shown when no crown or pavilion facet could vary.
const NOTHING_TO_SEARCH: &str =
    "Nothing to search: no crown or pavilion facet has an angle of its own to vary.";

/// What identifies one timing of the probe: the design generation, the lighting and the
/// target's refractive index (as bits).
type ProbeKey = (u64, i32, u64);

/// What a finished search left, kept until an input changes.
struct FinishedSearch {
    report: SearchReport,
    /// The Shift plan the search started from; a picked option re-lists its rows.
    plan: RetargetPlan,
    /// The live design the search ran against.
    design: Design,
    /// The design generation it ran against.
    generation: u64,
}

thread_local! {
    /// The options of the last finished search; `None` after any change of input.
    static FINISHED: RefCell<Option<FinishedSearch>> = const { RefCell::new(None) };
    /// Refreshes the progress line while a search runs.
    static POLL: Timer = Timer::default();
    /// The measured cost of one evaluation, in milliseconds, once the probe has run.
    static STEP_MS: Cell<Option<f64>> = const { Cell::new(None) };
    /// What the last finished probe measured.
    static PROBED: Cell<Option<ProbeKey>> = const { Cell::new(None) };
}

/// Bumped by every probe; a probe whose number is no longer current drops its result.
static PROBE_SERIAL: AtomicU64 = AtomicU64::new(0);

/// What the worker and the UI thread share.
#[derive(Debug, Default)]
pub(super) struct SearchProgress {
    cancel: AtomicBool,
    evaluations: AtomicUsize,
    stage: AtomicU8,
}

/// What the UI thread keeps of a running search.
pub(super) struct SearchHandle {
    progress: Arc<SearchProgress>,
    started: Instant,
    total_steps: usize,
}

/// A reading of a running search.
#[derive(Debug, Clone, Copy)]
struct Snapshot {
    evaluations: usize,
    stage: SearchStage,
    elapsed: Duration,
    total_steps: usize,
}

impl SearchHandle {
    /// Asks the worker to stop at its next step.
    pub(super) fn cancel(&self) {
        self.progress.cancel.store(true, AtomicOrdering::Relaxed);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            evaluations: self.progress.evaluations.load(AtomicOrdering::Relaxed),
            stage: SearchStage::from_code(self.progress.stage.load(AtomicOrdering::Relaxed)),
            elapsed: self.started.elapsed(),
            total_steps: self.total_steps,
        }
    }
}

// --- Text ---

/// `seconds` as short words: `a second`, `40 s`, `3 min`.
fn duration_text(seconds: f64) -> String {
    if seconds < 1.5 {
        "a second".to_string()
    } else if seconds < 90.0 {
        format!("{seconds:.0} s")
    } else {
        format!("{:.0} min", seconds / 60.0)
    }
}

/// The progress line of a running search.
fn progress_text(
    stage: SearchStage,
    evaluations: usize,
    total_steps: usize,
    elapsed: Duration,
) -> String {
    let what = match stage {
        SearchStage::BaselineFull => "Scoring the starting stone",
        SearchStage::Coordinate => "Trying angles",
        SearchStage::Polish => "Fine-tuning",
        SearchStage::FinalFull => "Scoring the results",
        SearchStage::Screening => "Choosing starting points",
    };
    let seconds = elapsed.as_secs_f64();
    let left = if evaluations >= 10 && seconds >= 2.0 && evaluations < total_steps {
        let remaining = seconds * (total_steps - evaluations) as f64 / evaluations as f64;
        format!(" At most about {} left.", duration_text(remaining))
    } else {
        String::new()
    };
    format!("{what}: step {evaluations} of up to {total_steps}.{left}")
}

/// The name of effort choice `index`.
fn effort_label(index: usize, steps: usize) -> String {
    let name = EFFORT_NAMES.get(index).copied().unwrap_or("Custom");
    format!("{name} ({steps} steps)")
}

// --- Settings, options and the estimate ---

/// The dialog's three Optimize combo boxes as search settings.
fn settings_from_ui(ui: &MainWindow) -> SearchSettings {
    let model = ui.global::<RetargetModel>();
    let index = |value: i32| usize::try_from(value).unwrap_or(0);
    SearchSettings {
        keep_look: model.get_keep_look(),
        girdle: model.get_girdle_allowance().then(GirdleAllowance::standard),
        ..SearchSettings::from_choices(
            index(model.get_objective_index()),
            index(model.get_range_index()),
            index(model.get_effort_index()),
        )
    }
}

/// Fills the three combo boxes' option lists and puts them at their defaults. Called when the
/// dialog opens.
pub(super) fn push_options(ui: &MainWindow) {
    let model = ui.global::<RetargetModel>();
    model.set_objective_options(string_model(
        ObjectivePreset::ALL.into_iter().map(ObjectivePreset::label),
    ));
    model.set_range_options(string_model(
        RANGE_CHOICES_DEG
            .iter()
            .map(|degrees| format!("{degrees:.0} degrees either side")),
    ));
    model.set_effort_options(string_model(
        EFFORT_CHOICES
            .iter()
            .enumerate()
            .map(|(index, steps)| effort_label(index, *steps)),
    ));
    let defaults = SearchSettings::default();
    model.set_objective_index(i32::try_from(defaults.preset.index()).unwrap_or(0));
    model.set_range_index(
        i32::try_from(
            RANGE_CHOICES_DEG
                .iter()
                .position(|degrees| (*degrees - defaults.range_deg).abs() < 1e-9)
                .unwrap_or(0),
        )
        .unwrap_or(0),
    );
    model.set_effort_index(
        i32::try_from(
            EFFORT_CHOICES
                .iter()
                .position(|steps| *steps == defaults.evaluations)
                .unwrap_or(0),
        )
        .unwrap_or(0),
    );
    refresh_estimate(ui);
}

/// Re-reads the three combo boxes: the objective's description and the time estimate.
pub(super) fn refresh_estimate(ui: &MainWindow) {
    let model = ui.global::<RetargetModel>();
    let settings = settings_from_ui(ui);
    let free = usize::try_from(model.get_free_tier_count()).unwrap_or(0);
    let steps = settings.total_steps(free);
    model.set_objective_description(settings.preset.description().into());
    model.set_estimate_text(estimate_text(steps, STEP_MS.with(Cell::get)).into());
}

/// Called whenever Optimize mode has (re)built the Shift plan it starts from: shows how many
/// angles can vary and starts timing one evaluation.
pub(super) fn prepare_for_plan(
    ui: &MainWindow,
    design: &Design,
    plan: &RetargetPlan,
    generation: u64,
) {
    ui.global::<RetargetModel>()
        .set_free_tier_count(i32::try_from(plan.moving_count()).unwrap_or(i32::MAX));
    begin_probe(ui, design, plan, generation);
    refresh_estimate(ui);
}

/// Times one evaluation of `design` in the target material on a worker thread (twice: the
/// first run pays for one-off setup) and shows the estimate when it is done. Skipped when the
/// same design, lighting and target were timed before.
fn begin_probe(ui: &MainWindow, design: &Design, plan: &RetargetPlan, generation: u64) {
    // Retarget is always timed and scored under the canonical lighting, not the viewport's.
    let key: ProbeKey = (generation, 0, plan.n_to.to_bits());
    if PROBED.with(Cell::get) == Some(key) {
        return;
    }
    let serial = PROBE_SERIAL.fetch_add(1, AtomicOrdering::SeqCst) + 1;
    let design = design.clone();
    let gem = plan.target.gem.clone();
    let lighting = CANONICAL_LIGHTING_PRESET;
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let time_one = || {
            let started = Instant::now();
            let solved = probe_evaluation(&design, &gem, lighting);
            (solved, started.elapsed())
        };
        let _warm_up = time_one();
        if PROBE_SERIAL.load(AtomicOrdering::SeqCst) != serial {
            return;
        }
        let (solved, took) = time_one();
        let step_ms = solved.then_some(took.as_secs_f64() * 1000.0);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| finish_probe(&ui, serial, key, step_ms));
    });
}

/// A probe's result, back on the UI thread. Dropped when a newer probe was started.
fn finish_probe(ui: &MainWindow, serial: u64, key: ProbeKey, step_ms: Option<f64>) {
    if PROBE_SERIAL.load(AtomicOrdering::SeqCst) != serial {
        return;
    }
    if step_ms.is_some() {
        STEP_MS.with(|cell| cell.set(step_ms));
        PROBED.with(|cell| cell.set(Some(key)));
    }
    refresh_estimate(ui);
}

// --- Resetting ---

/// Drops the options of a finished search and stops the progress timer.
pub(super) fn forget_results() {
    FINISHED.with(|cell| cell.borrow_mut().take());
    POLL.with(Timer::stop);
}

/// Clears everything the Optimize tab shows about a search: the option list, the summary, the
/// notes, the error and the progress. Leaves the three combo boxes alone.
pub(super) fn reset_search_ui(ui: &MainWindow) {
    let model = ui.global::<RetargetModel>();
    model.set_candidates(ModelRc::new(VecModel::from(
        Vec::<RetargetCandidateItem>::new(),
    )));
    model.set_selected_candidate(-1);
    model.set_search_summary("".into());
    model.set_search_notes(string_model(Vec::<String>::new()));
    model.set_search_error("".into());
    model.set_search_progress_text("".into());
    model.set_search_done(false);
    model.set_is_busy(false);
    model.set_optimize_evaluations(0);
    model.set_optimize_max_evaluations(0);
}

/// "Cancel" while a search runs (the dialog's button or the status strip's chip): stops the
/// worker, forgets what it had and leaves the Shift angles listed.
pub(super) fn cancel_optimize_run(ui: &MainWindow) {
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
    stale::clear(ResultKind::Retarget);
    reset_search_ui(ui);
    reset_check(ui);
    ui.global::<RetargetModel>()
        .set_search_summary("The search was cancelled.".into());
    // No option is held any more, so the comparison pane must not keep showing one.
    sync_embedded_comparison(ui, false);
}

// --- Running a search ---

/// Pushes `RetargetModel.optimize_evaluations`/`optimize_max_evaluations` and feeds the same
/// fraction to this run's status-strip chip, if one is registered (`max_evaluations == 0`,
/// not yet known, reports indeterminate).
fn set_optimize_progress(ui: &MainWindow, evaluations: usize, max_evaluations: usize) {
    ui.global::<RetargetModel>()
        .set_optimize_evaluations(i32::try_from(evaluations).unwrap_or(i32::MAX));
    ui.global::<RetargetModel>()
        .set_optimize_max_evaluations(i32::try_from(max_evaluations).unwrap_or(i32::MAX));
    if let (Some(activity), Some(id)) = (
        auto_solve::activity(),
        RETARGET_ASYNC.with(|cell| cell.borrow().activity_id),
    ) {
        let fraction = if max_evaluations == 0 {
            crate::gui::editor::activity::INDETERMINATE
        } else {
            (evaluations as f32 / max_evaluations as f32).min(1.0)
        };
        activity.progress(id, fraction);
    }
}

/// The progress timer's tick: shows how far the running search has got.
fn poll_progress(ui: &MainWindow) {
    let Some(snapshot) =
        RETARGET_ASYNC.with(|cell| cell.borrow().handle.as_ref().map(SearchHandle::snapshot))
    else {
        return;
    };
    set_optimize_progress(ui, snapshot.evaluations, snapshot.total_steps);
    ui.global::<RetargetModel>().set_search_progress_text(
        progress_text(
            snapshot.stage,
            snapshot.evaluations,
            snapshot.total_steps,
            snapshot.elapsed,
        )
        .into(),
    );
}

/// Registers the run in the status strip; cancelling from there runs [`cancel_optimize_run`].
fn register_activity(ui: &MainWindow) -> Option<u64> {
    auto_solve::activity().map(|activity| {
        let ui_weak = ui.as_weak();
        activity.start(
            "retarget_optimize",
            "Retarget Optimize",
            Some(Box::new(move || {
                // `cancel_optimize_run` finishes this very activity, and this closure runs
                // from inside the registry's own borrow, so the call waits one event-loop
                // tick (a `Timer::single_shot` needs no handle to keep it alive).
                let ui_weak = ui_weak.clone();
                Timer::single_shot(Duration::ZERO, move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        cancel_optimize_run(&ui);
                    }
                });
            })),
        )
    })
}

/// The Search button: builds the Shift plan the search starts from, starts the worker and the
/// progress timer, and returns at once. [`finish_search`] hears the result.
pub(super) fn start_search(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // A new search replaces whatever the last one left.
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
    reset_search_ui(ui);
    reset_check(ui);
    stale::clear(ResultKind::Retarget);
    sync_embedded_comparison(ui, false);

    let (design, generation) = {
        let st = state.borrow();
        (
            st.design.clone(),
            st.generation.load(AtomicOrdering::Relaxed),
        )
    };
    let custom = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let model = ui.global::<RetargetModel>();
    let selection = match resolve_target_selection(
        &design,
        &custom,
        model.get_target_material_index(),
        &model.get_target_ri_override_text(),
    ) {
        Ok(selection) => selection,
        Err(message) => {
            push_target_error(ui, &message);
            return;
        }
    };
    let target = resolved_material_from_selection(&selection, &custom);
    push_target_readout(ui, &selection, &target);
    let crown = CrownShift {
        fraction: f64::from(model.get_crown_fraction()),
        scale_by_ratio: model.get_scale_crown_by_ratio(),
        follow_pavilion: model.get_crown_follows_pavilion(),
    };
    let plan = build_plan(&design, &target, crown, &custom);
    if plan.moving_count() == 0 {
        model.set_search_error(NOTHING_TO_SEARCH.into());
        return;
    }

    let settings = settings_from_ui(ui);
    let lighting = CANONICAL_LIGHTING_PRESET;
    let total_steps = settings.total_steps(plan.moving_count());
    let progress = Arc::new(SearchProgress::default());
    let run_id = RETARGET_ASYNC.with(|cell| {
        let mut run = cell.borrow_mut();
        run.run_id = run.run_id.wrapping_add(1);
        run.run_id
    });

    model.set_is_busy(true);
    model.set_search_progress_text("Starting the search...".into());
    let activity_id = register_activity(ui);
    RETARGET_ASYNC.with(|cell| {
        let mut run = cell.borrow_mut();
        run.activity_id = activity_id;
        run.handle = Some(SearchHandle {
            progress: Arc::clone(&progress),
            started: Instant::now(),
            total_steps,
        });
    });
    set_optimize_progress(ui, 0, total_steps);
    POLL.with(|timer| {
        let ui_weak = ui.as_weak();
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || {
            if let Some(ui) = ui_weak.upgrade() {
                poll_progress(&ui);
            }
        });
    });

    spawn_worker(
        SearchJob {
            design,
            plan,
            custom,
            settings,
            lighting,
            generation,
            run_id,
            progress,
        },
        ui.as_weak(),
    );
}

/// Everything the worker thread needs.
struct SearchJob {
    design: Design,
    plan: RetargetPlan,
    custom: Vec<GemMaterial>,
    settings: SearchSettings,
    lighting: LightingPreset,
    generation: u64,
    run_id: u64,
    progress: Arc<SearchProgress>,
}

/// Runs `job` on a worker thread; the report comes back through [`finish_search`].
fn spawn_worker(job: SearchJob, ui_weak: slint::Weak<MainWindow>) {
    std::thread::spawn(move || {
        let SearchJob {
            design,
            plan,
            custom,
            settings,
            lighting,
            generation,
            run_id,
            progress,
        } = job;
        let current = current_gem(&design, &custom);
        // A panic inside the search must not leave the dialog busy forever.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let inputs = SearchInputs {
                design: &design,
                plan: &plan,
                current_gem: current.as_ref(),
                lighting,
                settings: &settings,
            };
            run_search(&inputs, &progress.cancel, &|evaluations, stage| {
                progress
                    .evaluations
                    .store(evaluations, AtomicOrdering::Relaxed);
                progress
                    .stage
                    .store(stage.to_code(), AtomicOrdering::Relaxed);
            })
        }))
        .unwrap_or_else(|_| {
            Err(SearchError::Solve(
                "the search stopped unexpectedly".to_string(),
            ))
        });
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            finish_search(
                &ui,
                run_id,
                FinishedInputs {
                    plan,
                    design,
                    generation,
                },
                outcome,
            );
        });
    });
}

/// What [`finish_search`] needs to keep with a report.
struct FinishedInputs {
    plan: RetargetPlan,
    design: Design,
    generation: u64,
}

/// A search's result, back on the UI thread. Dropped when a newer request has superseded the
/// run (cancel, a change of input, a new search).
fn finish_search(
    ui: &MainWindow,
    run_id: u64,
    inputs: FinishedInputs,
    outcome: Result<SearchReport, SearchError>,
) {
    if RETARGET_ASYNC.with(|cell| cell.borrow().run_id) != run_id {
        return;
    }
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().handle = None);
    POLL.with(Timer::stop);
    // Finishing on its own; a cancelled run's chip was finished by the cancel, so `take()` is
    // a harmless no-op there.
    if let (Some(activity), Some(id)) = (
        auto_solve::activity(),
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().activity_id.take()),
    ) {
        activity.finish(id);
    }
    let model = ui.global::<RetargetModel>();
    model.set_is_busy(false);
    model.set_search_progress_text("".into());
    model.set_search_done(true);

    let report = match outcome {
        Ok(report) => report,
        Err(SearchError::Cancelled) => {
            model.set_search_summary("The search was cancelled.".into());
            return;
        }
        Err(error) => {
            model.set_search_error(error.to_string().as_str().into());
            stale::clear(ResultKind::Retarget);
            return;
        }
    };

    push_candidates(ui, candidate_row_views(&report.candidates));
    model.set_search_summary(search_summary_line(&report).as_str().into());
    model.set_search_notes(string_model(report.notes()));
    let found = !report.candidates.is_empty();
    FINISHED.with(|cell| {
        *cell.borrow_mut() = Some(FinishedSearch {
            report,
            plan: inputs.plan,
            design: inputs.design,
            generation: inputs.generation,
        });
    });
    if found {
        select_candidate(ui, 0);
    } else {
        stale::clear(ResultKind::Retarget);
    }
}

// --- Picking an option ---

/// Everything picking an option changes, worked out before any of it is applied.
struct Pick {
    proposal: indicatrix_editor::retarget::RetargetProposal,
    view: indicatrix_editor::retarget::view::RetargetView,
    check: RetargetCheck,
    generation: u64,
}

/// Makes option `index` of the finished search the dialog's proposal: its rows replace the
/// table, its verdict, masts and optical table replace the validity block, and the embedded
/// comparison is pointed at it. Apply then commits it, as one undo step.
///
/// The caller must not hold a borrow of the editor state: the comparison's handler reads it.
pub(super) fn select_candidate(ui: &MainWindow, index: usize) {
    let pick = FINISHED.with(|cell| {
        let finished = cell.borrow();
        let finished = finished.as_ref()?;
        let candidate = finished.report.candidates.get(index)?;
        let plan = finished
            .plan
            .with_angles(&finished.design, &candidate.angles);
        Some(Pick {
            proposal: plan.proposal(),
            view: plan_view(&plan).with_tier_names(&finished.design),
            check: RetargetCheck {
                validity: candidate.validity.clone(),
                anchors: candidate.anchors.clone(),
                metrics: candidate.metrics,
            },
            generation: finished.generation,
        })
    });
    let Some(pick) = pick else {
        return;
    };
    RETARGET_ASYNC.with(|cell| {
        cell.borrow_mut().pending = Some((pick.proposal, pick.generation));
    });
    stale::stamp(ResultKind::Retarget, pick.generation);
    push_retarget_view(ui, pick.view);
    set_external_check(ui, pick.generation, pick.check);
    ui.global::<RetargetModel>()
        .set_selected_candidate(i32::try_from(index).unwrap_or(0));
    sync_embedded_comparison(ui, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_in_short_words() {
        assert_eq!(duration_text(0.2), "a second");
        assert_eq!(duration_text(12.4), "12 s");
        assert_eq!(duration_text(89.0), "89 s");
        assert_eq!(duration_text(150.0), "2 min");
    }

    #[test]
    fn the_progress_line_names_the_stage_and_the_steps() {
        let line = progress_text(SearchStage::Coordinate, 5, 335, Duration::from_secs(1));
        assert_eq!(line, "Trying angles: step 5 of up to 335.");
        let line = progress_text(SearchStage::Polish, 300, 335, Duration::from_secs(60));
        assert!(
            line.starts_with("Fine-tuning: step 300 of up to 335."),
            "{line}"
        );
        assert!(line.contains("At most about"), "{line}");
        assert!(
            progress_text(SearchStage::FinalFull, 335, 335, Duration::from_secs(90))
                .ends_with("335.")
        );
        assert!(
            progress_text(SearchStage::BaselineFull, 0, 335, Duration::ZERO)
                .starts_with("Scoring the starting stone")
        );
    }

    #[test]
    fn the_time_left_is_a_ceiling_from_the_pace_so_far() {
        // 100 steps in 20 s leaves 235 steps at 0.2 s each: about 47 s.
        let line = progress_text(SearchStage::Coordinate, 100, 335, Duration::from_secs(20));
        assert!(line.contains("about 47 s left"), "{line}");
    }

    #[test]
    fn effort_choices_are_named_with_their_steps() {
        assert_eq!(effort_label(1, 300), "Normal (300 steps)");
        assert_eq!(effort_label(9, 5), "Custom (5 steps)");
        assert_eq!(EFFORT_NAMES.len(), EFFORT_CHOICES.len());
    }

    #[test]
    fn a_handle_cancels_the_worker_and_reads_its_progress() {
        let progress = Arc::new(SearchProgress::default());
        let handle = SearchHandle {
            progress: Arc::clone(&progress),
            started: Instant::now(),
            total_steps: 50,
        };
        assert!(!progress.cancel.load(AtomicOrdering::Relaxed));
        progress.evaluations.store(7, AtomicOrdering::Relaxed);
        progress
            .stage
            .store(SearchStage::Polish.to_code(), AtomicOrdering::Relaxed);
        let reading = handle.snapshot();
        assert_eq!(reading.evaluations, 7);
        assert_eq!(reading.stage, SearchStage::Polish);
        assert_eq!(reading.total_steps, 50);
        handle.cancel();
        assert!(progress.cancel.load(AtomicOrdering::Relaxed));
    }
}
