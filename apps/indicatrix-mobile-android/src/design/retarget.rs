//! Material Retarget workflow over the shared checked Shift/search engines.
use super::Shared;
use crate::{AppWindow, RetargetModel, push::toast};
use indicatrix_editor::{
    material::design_material_options,
    optimize_view::{parse_seed, preset_labels},
    retarget::{
        CrownShift, GirdleAllowance,
        search::SearchSettings,
        view::{CheckView, initial_target_index, plan_view, resolve_target_selection},
    },
    snapshot::{DesignSnapshot, compare_rows},
};
use indicatrix_mobile_core::{
    operations::{InputFingerprint, OperationIdentity, OperationPhase},
    retarget::{self, RetargetControls, RetargetJob, RetargetResult, RetargetWorker},
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

struct Runtime {
    active: Option<OperationIdentity>,
    shared: Shared,
    queue: BackgroundQueue,
    worker: Arc<Mutex<RetargetWorker>>,
    result: Option<RetargetResult>,
    selected: Option<usize>,
}
thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }

pub fn wire(window: &AppWindow, shared: &Shared) {
    RUNTIME.with(|value| {
        *value.borrow_mut() = Some(Runtime {
            active: None,
            shared: Rc::clone(shared),
            queue: BackgroundQueue::new("retarget"),
            worker: Arc::new(Mutex::new(RetargetWorker::default())),
            result: None,
            selected: None,
        });
    });
    window
        .global::<RetargetModel>()
        .set_release_ready(super::optimization::RELEASE_READY);
    // Retarget has preset objectives; the separate custom-weight form belongs to Optimize.
    let labels = preset_labels();
    window
        .global::<RetargetModel>()
        .set_presets(ModelRc::new(VecModel::from(
            labels[..labels.len().saturating_sub(1)]
                .iter()
                .map(|name| (*name).into())
                .collect::<Vec<slint::SharedString>>(),
        )));
    wire_form(window, shared);
    wire_select(window);
    wire_commit(window);
}

/// The form: show, start, cancel and close.
fn wire_form(window: &AppWindow, shared: &Shared) {
    let (weak, state) = (window.as_weak(), Rc::clone(shared));
    window.global::<RetargetModel>().on_show(move || {
        if let Some(window) = weak.upgrade() {
            let app = state.borrow();
            let options = design_material_options(&app.props.custom_materials);
            let model = window.global::<RetargetModel>();
            model.set_material(initial_target_index(&app.session.design.material, &options));
            model.set_materials(ModelRc::new(VecModel::from(
                options
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<slint::SharedString>>(),
            )));
            model.set_ri(
                app.session
                    .design
                    .material
                    .refractive_index_override
                    .map_or_else(slint::SharedString::new, |value| value.to_string().into()),
            );
            model.set_open(true);
        }
    });
    let weak = window.as_weak();
    window.global::<RetargetModel>().on_start(move || {
        if let Some(window) = weak.upgrade() {
            start(&window);
        }
    });
    let (weak, state) = (window.as_weak(), Rc::clone(shared));
    window.global::<RetargetModel>().on_cancel(move || {
        state.borrow_mut().operations.cancel();
        if let Some(window) = weak.upgrade() {
            window
                .global::<RetargetModel>()
                .set_status("Cancelling at the next engine checkpoint…".into());
        }
    });
    let weak = window.as_weak();
    window.global::<RetargetModel>().on_close(move || {
        if let Some(window) = weak.upgrade() {
            let model = window.global::<RetargetModel>();
            if model.get_running() {
                model.invoke_cancel();
            }
            model.set_open(false);
        }
    });
}

/// Choosing a candidate: the check's diagnostics and the comparison with the original.
fn wire_select(window: &AppWindow) {
    let weak = window.as_weak();
    window.global::<RetargetModel>().on_select(move |index| {
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
                .ok_or_else(|| "Check Shift first.".to_owned())?;
            let check = result
                .checks
                .get(index)
                .ok_or_else(|| "Select a candidate first.".to_owned())?;
            let view = CheckView::from_check(check);
            // Invalid geometry still has readable diagnostics; it never grants Apply.
            let candidate =
                retarget::preview(&app, result, index, current_fingerprint(&window, &app)?);
            let mut lines = vec![view.headline, view.strategy];
            lines.extend(view.details);
            lines.push(
                "Metrics: original material → unchanged in target → retargeted in target".into(),
            );
            for (label, cells) in ["Windowing", "Brilliance", "Extinction"]
                .iter()
                .zip(view.metrics.chunks(3))
            {
                lines.push(format!("{label}: {}", cells.join(" → ")));
            }
            let allowed = if let Ok(candidate) = candidate {
                let snapshot = DesignSnapshot {
                    design: app.session.design.clone(),
                    solved: Some(result.original_solved.clone()),
                    label: "Original".into(),
                    from_retarget_apply: false,
                };
                lines.extend(
                    compare_rows(
                        &snapshot,
                        &candidate,
                        result
                            .previews
                            .get(index)
                            .and_then(|preview| preview.as_ref().ok())
                            .map(|preview| preview.1.as_slice()),
                    )
                    .into_iter()
                    .filter(|row| row.status_label != "Same")
                    .map(|row| {
                        format!(
                            "{}: {} → {} · {} · mast delta {}",
                            row.name,
                            row.old_angle,
                            row.new_angle,
                            row.status_label,
                            row.mast_delta
                        )
                    }),
                );
                true
            } else {
                false
            };
            runtime.selected = Some(index);
            Ok::<_, String>((lines.join("\n"), allowed))
        });
        let model = window.global::<RetargetModel>();
        match answer {
            Ok((text, allowed)) => {
                model.set_comparison(text.into());
                model.set_can_apply(allowed);
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
    let weak = window.as_weak();
    window.global::<RetargetModel>().on_apply(move || {
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
                .ok_or_else(|| "Check a candidate first.".to_owned())?;
            let index = runtime
                .selected
                .ok_or_else(|| "Select a candidate first.".to_owned())?;
            let inputs = current_fingerprint(&window, &app)?;
            retarget::apply(&mut app, result, index, inputs)?;
            Ok::<_, String>(Rc::clone(&runtime.shared))
        });
        match answer {
            Ok(shared) => {
                window.global::<RetargetModel>().set_can_apply(false);
                super::after_edit(&window, &shared);
            }
            Err(error) => toast::show(&window, &error),
        }
    });
    let weak = window.as_weak();
    window.global::<RetargetModel>().on_save_variant(move || {
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
                .ok_or_else(|| "Check a candidate first.".to_owned())?;
            let index = runtime
                .selected
                .ok_or_else(|| "Select a candidate first.".to_owned())?;
            let design =
                retarget::preview(&app, result, index, current_fingerprint(&window, &app)?)?;
            Ok::<_, String>((Rc::clone(&runtime.shared), design))
        });
        match answer {
            Ok((shared, design)) => super::variants::save_candidate(
                &window,
                &shared,
                &design,
                &window.global::<RetargetModel>().get_variant_name(),
            ),
            Err(error) => toast::show(&window, &error),
        }
    });
}

fn controls(
    window: &AppWindow,
    app: &indicatrix_mobile_core::app_state::AppState,
) -> Result<RetargetControls, String> {
    let model = window.global::<RetargetModel>();
    let target = resolve_target_selection(
        &app.session.design,
        &app.props.custom_materials,
        model.get_material(),
        &model.get_ri(),
    )?;
    let crown = match model.get_crown() {
        1 => CrownShift::fixed(),
        2 => CrownShift {
            scale_by_ratio: true,
            ..CrownShift::default()
        },
        3 => {
            let fraction =
                indicatrix_editor::loading::eval_number(&model.get_crown_fraction(), None)
                    .map_err(|error| error.to_string())?;
            if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
                return Err("Crown fraction must be between 0 and 1.".into());
            }
            CrownShift {
                fraction,
                follow_pavilion: false,
                scale_by_ratio: false,
            }
        }
        _ => CrownShift::default(),
    };
    let girdle = model.get_girdle().then_some(GirdleAllowance::standard());
    let search = if model.get_search() {
        let mut settings = SearchSettings::from_choices(
            usize::try_from(model.get_preset()).unwrap_or(0),
            usize::try_from(model.get_range()).unwrap_or(1),
            usize::try_from(model.get_effort()).unwrap_or(1),
        );
        settings.keep_look = model.get_keep_look();
        settings.girdle = girdle;
        settings.seed = parse_seed(&model.get_seed())?;
        settings.keep = usize::try_from(model.get_candidates())
            .unwrap_or(3)
            .clamp(1, 5);
        Some(settings)
    } else {
        None
    };
    Ok(RetargetControls {
        target,
        crown,
        girdle,
        search,
    })
}

fn current_fingerprint(
    window: &AppWindow,
    app: &indicatrix_mobile_core::app_state::AppState,
) -> Result<InputFingerprint, String> {
    retarget::fingerprint(app, &controls(window, app)?)
}

fn start(window: &AppWindow) {
    if !super::optimization::RELEASE_READY {
        return;
    }
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        let job = {
            let mut app = runtime.shared.borrow_mut();
            controls(window, &app).and_then(|controls| RetargetJob::capture(&mut app, controls))
        };
        let job = match job {
            Ok(job) => job,
            Err(error) => {
                window.global::<RetargetModel>().set_status(error.into());
                return;
            }
        };
        let identity = job.token.identity;
        runtime.active = Some(identity);
        let worker = Arc::clone(&runtime.worker);
        runtime.result = None;
        runtime.selected = None;
        let model = window.global::<RetargetModel>();
        model.set_running(true);
        model.set_can_apply(false);
        model.set_status("Checking original geometry and Shift on the operation worker…".into());
        model.set_results(ModelRc::new(VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        model.set_comparison("".into());
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
            let result = catch_unwind(AssertUnwindSafe(|| {
                job.run(
                    &mut worker.lock().unwrap_or_else(PoisonError::into_inner),
                    &progress,
                )
            }))
            .unwrap_or_else(|_| Err("Retarget stopped unexpectedly.".into()));
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
            window.global::<RetargetModel>().set_status(status.into());
        }
    });
}

fn finish(window: &AppWindow, identity: OperationIdentity, result: Result<RetargetResult, String>) {
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if runtime.active != Some(identity) {
            return;
        }
        runtime.active = None;
        let model = window.global::<RetargetModel>();
        model.set_running(false);
        model.set_can_apply(false);
        let mut app = runtime.shared.borrow_mut();
        let generation = app.session.current_generation();
        let inputs = match current_fingerprint(window, &app) {
            Ok(inputs) => inputs,
            Err(error) => {
                model.set_status(format!("Result discarded: {error}").into());
                return;
            }
        };
        if !app.operations.accepts(identity, generation, inputs) {
            model.set_status("Result discarded: cancelled, interrupted or inputs changed.".into());
            return;
        }
        app.operations
            .publish(identity, generation, inputs, OperationPhase::Running);
        let phase = match &result {
            Ok(_) => OperationPhase::Completed,
            Err(error) => OperationPhase::Failed(error.clone()),
        };
        if !app.operations.publish(identity, generation, inputs, phase) {
            return;
        }
        match result {
            Ok(result) => {
                let view = plan_view(&result.plan).with_tier_names(&app.session.design);
                let mut lines = result.notes.clone();
                lines.extend(view.rows.into_iter().map(|row| {
                    format!(
                        "{}: {} → {} · margin {} · {}",
                        row.name, row.old_angle, row.new_angle, row.margin, row.risk_label
                    )
                }));
                model.set_status(lines.join("\n").into());
                model.set_results(ModelRc::new(VecModel::from(
                    result
                        .labels
                        .iter()
                        .map(|label| label.as_str().into())
                        .collect::<Vec<slint::SharedString>>(),
                )));
                runtime.result = Some(result);
            }
            Err(error) => model.set_status(error.into()),
        }
    });
}

pub fn push(window: &AppWindow, app: &indicatrix_mobile_core::app_state::AppState) {
    if matches!(
        app.operations.phase(),
        Some(OperationPhase::Stale | OperationPhase::Cancelling)
    ) {
        let model = window.global::<RetargetModel>();
        model.set_running(false);
        model.set_can_apply(false);
    }
}
pub fn shutdown() {
    RUNTIME.with(|value| drop(value.borrow_mut().take()));
}
