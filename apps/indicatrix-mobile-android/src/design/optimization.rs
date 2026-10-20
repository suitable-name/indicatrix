//! Governed Optimize form, immutable candidate previews and atomic Apply.
use super::Shared;
use crate::{AppWindow, OptimizeModel, OptimizeRange, push::toast};
use indicatrix_cut_core::CANONICAL_LIGHTING_PRESET;
use indicatrix_editor::{
    optimize_view::{self, RangeInput, RunForm, RunPlan},
    snapshot::{DesignSnapshot, compare_rows},
};
use indicatrix_mobile_core::{
    operations::{InputFingerprint, OperationIdentity, OperationPhase},
    optimization::{self, OptimizationJob, OptimizationResult},
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::{Duration, Instant},
};

pub const RELEASE_READY: bool = cfg!(debug_assertions) || cfg!(feature = "validated-design-search");
struct Runtime {
    active: Option<OperationIdentity>,
    shared: Shared,
    queue: BackgroundQueue,
    result: Option<OptimizationResult>,
    selected: Option<usize>,
}
thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }

pub fn wire(window: &AppWindow, state: &Shared) {
    RUNTIME.with(|value| {
        *value.borrow_mut() = Some(Runtime {
            active: None,
            shared: Rc::clone(state),
            queue: BackgroundQueue::new("optimization"),
            result: None,
            selected: None,
        });
    });
    let model = window.global::<OptimizeModel>();
    model.set_release_ready(RELEASE_READY);
    model.set_presets(ModelRc::new(VecModel::from(
        optimize_view::preset_labels()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<slint::SharedString>>(),
    )));
    wire_form(window, state);
    wire_select(window);
    wire_commit(window);
}

/// The form: show, reset, start, cancel and close.
fn wire_form(window: &AppWindow, state: &Shared) {
    let model = window.global::<OptimizeModel>();
    let (weak, shared) = (window.as_weak(), Rc::clone(state));
    model.on_show(move || {
        let Some(window) = weak.upgrade() else { return };
        window
            .global::<OptimizeModel>()
            .set_vary_anchored(optimize_view::default_vary_anchored(
                &shared.borrow().session.design,
            ));
        reset_ranges(&window, &shared);
        window.global::<OptimizeModel>().set_open(true);
    });
    let (weak, shared) = (window.as_weak(), Rc::clone(state));
    model.on_reset_ranges(move || {
        if let Some(window) = weak.upgrade() {
            reset_ranges(&window, &shared);
        }
    });
    let weak = window.as_weak();
    model.on_start(move || {
        if let Some(window) = weak.upgrade() {
            start(&window);
        }
    });
    let (weak, shared) = (window.as_weak(), Rc::clone(state));
    model.on_cancel(move || {
        shared.borrow_mut().operations.cancel();
        if let Some(window) = weak.upgrade() {
            window
                .global::<OptimizeModel>()
                .set_status("Cancelling at the next engine checkpoint…".into());
            window.global::<OptimizeModel>().set_can_apply(false);
        }
    });
    let weak = window.as_weak();
    model.on_close(move || {
        if let Some(window) = weak.upgrade() {
            if window.global::<OptimizeModel>().get_running() {
                window.global::<OptimizeModel>().invoke_cancel();
            }
            window.global::<OptimizeModel>().set_open(false);
        }
    });
}

/// Choosing a candidate: its comparison with the original.
fn wire_select(window: &AppWindow) {
    let model = window.global::<OptimizeModel>();
    let weak = window.as_weak();
    model.on_select(move |index| {
        let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
            return;
        };
        let answer = RUNTIME.with(|value| {
            let mut guard = value.borrow_mut();
            let runtime = guard
                .as_mut()
                .ok_or_else(|| "The editor is closed.".to_owned())?;
            let app = runtime.shared.borrow();
            let result = runtime
                .result
                .as_ref()
                .ok_or_else(|| "Run Optimize first.".to_owned())?;
            let candidate =
                optimization::preview(&app, result, index, current_fingerprint(&window, &app)?)?;
            let solved = result
                .previews
                .get(index)
                .and_then(|preview| preview.as_ref().ok())
                .map(|preview| preview.1.as_slice());
            let snapshot = DesignSnapshot {
                design: app.session.design.clone(),
                solved: Some(result.original_solved.clone()),
                label: "Original".into(),
                from_retarget_apply: false,
            };
            let comparison = compare_rows(&snapshot, &candidate, solved)
                .into_iter()
                .filter(|row| row.status_label != "Same")
                .map(|row| {
                    format!(
                        "{} · {} → {} · mast delta {} · {}",
                        row.name, row.old_angle, row.new_angle, row.mast_delta, row.status_label
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            runtime.selected = Some(index);
            Ok::<_, String>(comparison)
        });
        let model = window.global::<OptimizeModel>();
        match answer {
            Ok(text) => {
                model.set_comparison(text.into());
                model.set_can_apply(true);
            }
            Err(error) => {
                model.set_comparison(error.into());
                model.set_can_apply(false);
            }
        }
    });
}

/// Apply and Save Variant of the chosen candidate.
fn wire_commit(window: &AppWindow) {
    let model = window.global::<OptimizeModel>();
    let weak = window.as_weak();
    model.on_apply(move || {
        let Some(window) = weak.upgrade() else { return };
        let answer = RUNTIME.with(|value| {
            let guard = value.borrow();
            let runtime = guard
                .as_ref()
                .ok_or_else(|| "The editor is closed.".to_owned())?;
            let mut app = runtime.shared.borrow_mut();
            let result = runtime
                .result
                .as_ref()
                .ok_or_else(|| "Run Optimize first.".to_owned())?;
            let index = runtime
                .selected
                .ok_or_else(|| "Select a candidate first.".to_owned())?;
            let inputs = current_fingerprint(&window, &app)?;
            optimization::apply(&mut app, result, index, inputs)?;
            Ok::<_, String>(Rc::clone(&runtime.shared))
        });
        match answer {
            Ok(shared) => {
                window.global::<OptimizeModel>().set_can_apply(false);
                super::after_edit(&window, &shared);
            }
            Err(error) => toast::show(&window, &error),
        }
    });
    let weak = window.as_weak();
    model.on_save_variant(move || {
        let Some(window) = weak.upgrade() else { return };
        let answer = RUNTIME.with(|value| {
            let guard = value.borrow();
            let runtime = guard
                .as_ref()
                .ok_or_else(|| "The editor is closed.".to_owned())?;
            let app = runtime.shared.borrow();
            let result = runtime
                .result
                .as_ref()
                .ok_or_else(|| "Run Optimize first.".to_owned())?;
            let index = runtime
                .selected
                .ok_or_else(|| "Select a candidate first.".to_owned())?;
            let design =
                optimization::preview(&app, result, index, current_fingerprint(&window, &app)?)?;
            Ok::<_, String>((Rc::clone(&runtime.shared), design))
        });
        match answer {
            Ok((shared, design)) => super::variants::save_candidate(
                &window,
                &shared,
                &design,
                &window.global::<OptimizeModel>().get_variant_name(),
            ),
            Err(error) => toast::show(&window, &error),
        }
    });
}

fn reset_ranges(window: &AppWindow, shared: &Shared) {
    let model = window.global::<OptimizeModel>();
    let rows =
        optimize_view::range_rows(&shared.borrow().session.design, model.get_vary_anchored())
            .into_iter()
            .map(|row| OptimizeRange {
                tier: i32::try_from(row.tier).unwrap_or(i32::MAX),
                name: row.name.into(),
                minimum: optimize_view::format_range_value(
                    row.min_deg.abs().min(row.max_deg.abs()),
                )
                .into(),
                maximum: optimize_view::format_range_value(
                    row.min_deg.abs().max(row.max_deg.abs()),
                )
                .into(),
                locked: row.driven,
            })
            .collect::<Vec<OptimizeRange>>();
    model.set_ranges(ModelRc::new(VecModel::from(rows)));
}

fn controls(
    window: &AppWindow,
    app: &indicatrix_mobile_core::app_state::AppState,
) -> Result<(RunPlan, Option<BTreeSet<usize>>), String> {
    let model = window.global::<OptimizeModel>();
    let ranges = model
        .get_ranges()
        .iter()
        .map(|row| {
            Ok(RangeInput {
                tier: usize::try_from(row.tier).map_err(|_| "Invalid tier range".to_owned())?,
                min_text: row.minimum.to_string(),
                max_text: row.maximum.to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let window_weight = model.get_window_weight();
    let extinction_weight = model.get_extinction_weight();
    let tilt_weight = model.get_tilt_weight();
    let budget = model.get_budget();
    let seed = model.get_seed();
    let number = |text: slint::SharedString| {
        text.parse::<f32>()
            .map_err(|_| "Custom weight/tone must be a number.".to_owned())
    };
    let form = RunForm {
        preset_index: usize::try_from(model.get_preset()).unwrap_or(0),
        weight_windowing: &window_weight,
        weight_extinction: &extinction_weight,
        weight_tilt_brilliance: &tilt_weight,
        yield_weight: number(model.get_yield_weight())?,
        tone: number(model.get_tone())?,
        vary_anchored: model.get_vary_anchored(),
        keep_girdle: model.get_keep_girdle(),
        budget_text: &budget,
        starts: usize::try_from(model.get_starts()).unwrap_or(1),
        seed_text: &seed,
        polish: model.get_polish(),
        candidates: usize::try_from(model.get_candidates()).unwrap_or(1),
        ranges: &ranges,
    };
    let plan =
        optimize_view::build_run_plan(&app.session.design, &form, CANONICAL_LIGHTING_PRESET)?;
    let only = model.get_only_selected().then(|| {
        let mut tiers = app.session.multi_selected.clone();
        if tiers.is_empty() {
            tiers.extend(app.selected_tier);
        }
        tiers
    });
    Ok((plan, only))
}
fn current_fingerprint(
    window: &AppWindow,
    app: &indicatrix_mobile_core::app_state::AppState,
) -> Result<InputFingerprint, String> {
    let (plan, only) = controls(window, app)?;
    optimization::fingerprint(app, &plan, only.as_ref())
}
fn start(window: &AppWindow) {
    if !RELEASE_READY {
        return;
    }
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        let job = {
            let mut app = runtime.shared.borrow_mut();
            controls(window, &app)
                .and_then(|(plan, only)| OptimizationJob::capture(&mut app, plan, only))
        };
        let job = match job {
            Ok(job) => job,
            Err(error) => {
                window.global::<OptimizeModel>().set_status(error.into());
                return;
            }
        };
        let identity = job.token.identity;
        runtime.active = Some(identity);
        runtime.result = None;
        runtime.selected = None;
        let model = window.global::<OptimizeModel>();
        model.set_running(true);
        model.set_can_apply(false);
        model.set_comparison("".into());
        model.set_status("Preparing the original geometry on the operation worker…".into());
        model.set_results(ModelRc::new(VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let started = Instant::now();
            let last = Cell::new(None::<Instant>);
            let progress = |count, stage| {
                if last
                    .get()
                    .is_some_and(|time| time.elapsed() < Duration::from_millis(100))
                {
                    return;
                }
                last.set(Some(Instant::now()));
                let status = format!(
                    "{count} evaluations · {stage:?} · {:.1}s",
                    started.elapsed().as_secs_f32()
                );
                let _queued = weak.upgrade_in_event_loop(move |window| {
                    progress_message(&window, identity, status);
                });
            };
            let result = catch_unwind(AssertUnwindSafe(|| job.run(&progress)))
                .unwrap_or_else(|_| Err("Optimize stopped unexpectedly.".into()));
            let _queued =
                weak.upgrade_in_event_loop(move |window| finish(&window, identity, result));
        });
    });
}
fn progress_message(window: &AppWindow, identity: OperationIdentity, status: String) {
    RUNTIME.with(|value| {
        let guard = value.borrow();
        let Some(runtime) = guard.as_ref() else {
            return;
        };
        let mut app = runtime.shared.borrow_mut();
        let generation = app.session.current_generation();
        let Ok(inputs) = current_fingerprint(window, &app) else {
            return;
        };
        if app
            .operations
            .publish(identity, generation, inputs, OperationPhase::Running)
        {
            window.global::<OptimizeModel>().set_status(status.into());
        }
    });
}
fn finish(
    window: &AppWindow,
    identity: OperationIdentity,
    result: Result<OptimizationResult, String>,
) {
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut(); let Some(runtime) = guard.as_mut() else { return };
        if runtime.active != Some(identity) { return; }
        runtime.active = None;
        let model = window.global::<OptimizeModel>(); model.set_running(false); model.set_can_apply(false);
        let mut app = runtime.shared.borrow_mut(); let generation = app.session.current_generation();
        let inputs = match current_fingerprint(window, &app) {
            Ok(inputs) => inputs,
            Err(error) => { model.set_status(format!("Result discarded: {error}").into()); return; }
        };
        let partial = result.as_ref().is_ok_and(|result| result.partial);
        let accepted = if partial { app.operations.finish_partial(identity, generation, inputs) } else {
            app.operations.publish(identity, generation, inputs, OperationPhase::Running);
            let phase = match &result { Ok(_) => OperationPhase::Completed, Err(error) => OperationPhase::Failed(error.clone()) };
            app.operations.publish(identity, generation, inputs, phase)
        };
        // An old worker must not clear the running flag of a replacement operation.
        if !accepted { model.set_status("Result discarded: cancelled, interrupted or inputs changed.".into()); return; }
        match result {
            Ok(result) => {
                model.set_status(if partial { "Stopped; validated partial candidates are available.".into() } else { optimize_view::optimize_run_status(&result.result, None).into() });
                model.set_results(ModelRc::new(VecModel::from(optimize_view::candidate_lines(&result.result).into_iter().map(|row| format!("{} · score {} · window {} · brilliance {} · extinction {} · yield {} · {}", row.rank, row.score, row.windowing, row.brilliance, row.extinction, row.yield_loss, row.tone).into()).collect::<Vec<slint::SharedString>>())));
                runtime.result = Some(result);
            }, Err(error) => model.set_status(error.into()),
        }
    });
}
pub fn push(window: &AppWindow, app: &indicatrix_mobile_core::app_state::AppState) {
    if matches!(
        app.operations.phase(),
        Some(OperationPhase::Stale | OperationPhase::Cancelling)
    ) {
        window.global::<OptimizeModel>().set_running(false);
        window.global::<OptimizeModel>().set_can_apply(false);
    }
}
pub fn shutdown() {
    RUNTIME.with(|value| drop(value.borrow_mut().take()));
}
