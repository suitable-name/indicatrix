//! Local, cancellable verified solve on a separate governed operation worker.

use super::Shared;
use crate::{AppWindow, DeepSolveModel, push::toast};
use indicatrix_editor::{
    deep_solve::{
        availability, format_deep_solve_report, format_tier_mast_deltas, tier_mast_deltas,
    },
    printed_proportions::parse_printed_proportions_form,
};
use indicatrix_mobile_core::{
    deep_solve::{self, DeepSolveJob, DeepSolveResult},
    operations::{OperationIdentity, OperationPhase},
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::{Duration, Instant},
};

struct Runtime {
    shared: Shared,
    queue: BackgroundQueue,
    result: Option<DeepSolveResult>,
    identity: Option<OperationIdentity>,
}
thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }

pub fn wire(window: &AppWindow, state: &Shared) {
    RUNTIME.with(|runtime| {
        *runtime.borrow_mut() = Some(Runtime {
            shared: Rc::clone(state),
            queue: BackgroundQueue::new("editor-operation"),
            result: None,
            identity: None,
        });
    });
    let (weak, shared) = (window.as_weak(), Rc::clone(state));
    window.global::<DeepSolveModel>().on_show(move || {
        let Some(window) = weak.upgrade() else { return };
        let app = shared.borrow();
        let targets = app.document.printed_proportions.unwrap_or_default();
        let text = |value: Option<f64>| {
            value.map_or_else(slint::SharedString::new, |value| value.to_string().into())
        };
        let model = window.global::<DeepSolveModel>();
        model.set_vol(text(targets.vol_w3));
        model.set_lw(text(targets.lw));
        model.set_cw(text(targets.cw));
        model.set_pw(text(targets.pw));
        model.set_hw(text(targets.hw));
        push(&window, &app);
        model.set_open(true);
    });
    let weak = window.as_weak();
    window.global::<DeepSolveModel>().on_start(move || {
        if let Some(window) = weak.upgrade() {
            start(&window);
        }
    });
    let (weak, shared) = (window.as_weak(), Rc::clone(state));
    window.global::<DeepSolveModel>().on_cancel(move || {
        shared.borrow_mut().operations.cancel();
        if let Some(window) = weak.upgrade() {
            window.global::<DeepSolveModel>().set_running(false);
            window.global::<DeepSolveModel>().set_can_pin(false);
            window
                .global::<DeepSolveModel>()
                .set_status("Cancelled. A new run starts a new search.".into());
        }
    });
    let weak = window.as_weak();
    window.global::<DeepSolveModel>().on_close(move || {
        if let Some(window) = weak.upgrade() {
            window.global::<DeepSolveModel>().invoke_cancel();
            window.global::<DeepSolveModel>().set_open(false);
        }
    });
    wire_targets(window, state);
}

/// The printed figures and Pin: the two commands that change the design.
fn wire_targets(window: &AppWindow, state: &Shared) {
    let (weak, shared) = (window.as_weak(), Rc::clone(state));
    window.global::<DeepSolveModel>().on_save_targets(move || {
        let Some(window) = weak.upgrade() else { return };
        let model = window.global::<DeepSolveModel>();
        let targets = parse_printed_proportions_form(
            &model.get_vol(),
            &model.get_lw(),
            &model.get_cw(),
            &model.get_pw(),
            &model.get_hw(),
        );
        match targets {
            Ok(targets) => {
                let mut app = shared.borrow_mut();
                app.document.printed_proportions = Some(targets);
                app.session.mark_unsaved();
                app.operations.invalidate();
                model.set_can_pin(false);
                model.set_status(
                    "Using externally entered printed proportions. Save the design to keep them."
                        .into(),
                );
                drop(app);
                super::after_edit(&window, &shared);
            }
            Err(error) => model.set_status(error.into()),
        }
    });
    let (weak, shared) = (window.as_weak(), Rc::clone(state));
    window.global::<DeepSolveModel>().on_pin(move |tier| {
        let (Some(window), Ok(tier)) = (weak.upgrade(), usize::try_from(tier)) else {
            return;
        };
        let result = RUNTIME.with(|runtime| {
            let guard = runtime.borrow();
            let result = guard
                .as_ref()
                .and_then(|runtime| runtime.result.as_ref())
                .ok_or_else(|| "Run Deep Solve first.".to_owned())?;
            deep_solve::pin(
                &mut shared.borrow_mut(),
                result,
                tier,
                super::solve::clock(),
            )
        });
        if let Err(error) = result {
            toast::show(&window, &error);
        }
        super::after_edit(&window, &shared);
    });
}

pub fn push(window: &AppWindow, app: &indicatrix_mobile_core::app_state::AppState) {
    let (available, hint) = availability(
        &app.session.design,
        app.document.printed_proportions.as_ref(),
    );
    let model = window.global::<DeepSolveModel>();
    model.set_available(available);
    model.set_hint(hint.into());
    if matches!(
        app.operations.phase(),
        Some(OperationPhase::Stale | OperationPhase::Cancelling)
    ) {
        model.set_running(false);
        model.set_can_pin(false);
    }
}

fn start(window: &AppWindow) {
    RUNTIME.with(|runtime| {
        let mut guard = runtime.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        let job = match DeepSolveJob::capture(&mut runtime.shared.borrow_mut()) {
            Ok(job) => job,
            Err(error) => {
                window.global::<DeepSolveModel>().set_status(error.into());
                return;
            }
        };
        let identity = job.token.identity;
        runtime.identity = Some(identity);
        runtime.result = None;
        let model = window.global::<DeepSolveModel>();
        model.set_running(true);
        model.set_can_pin(false);
        model.set_status("Preparing; waiting for the device compute budget…".into());
        model.set_tiers(ModelRc::new(VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let start = Instant::now();
            let last = Cell::new(None::<Instant>);
            let progress = |progress: indicatrix::geometry::meet_solver::SolveProgress| {
                if last
                    .get()
                    .is_some_and(|instant| instant.elapsed() < Duration::from_millis(100))
                {
                    return;
                }
                last.set(Some(Instant::now()));
                let status = format!(
                    "Working for {:.1}s · {:?} · sweep {}",
                    start.elapsed().as_secs_f32(),
                    progress.phase,
                    progress.sweep
                );
                let _queued = weak.upgrade_in_event_loop(move |window| {
                    update_progress(&window, identity, status);
                });
            };
            let result = catch_unwind(AssertUnwindSafe(|| job.run(&progress)))
                .unwrap_or_else(|_| Err("Deep Solve stopped unexpectedly.".into()));
            let _queued =
                weak.upgrade_in_event_loop(move |window| finish(&window, identity, result));
        });
    });
}

fn update_progress(window: &AppWindow, identity: OperationIdentity, status: String) {
    RUNTIME.with(|runtime| {
        let guard = runtime.borrow();
        let Some(runtime) = guard.as_ref() else {
            return;
        };
        let mut app = runtime.shared.borrow_mut();
        let Ok(inputs) = deep_solve::fingerprint(&app) else {
            return;
        };
        let generation = app.session.current_generation();
        if app
            .operations
            .publish(identity, generation, inputs, OperationPhase::Running)
        {
            window.global::<DeepSolveModel>().set_status(status.into());
        }
    });
}

fn finish(
    window: &AppWindow,
    identity: OperationIdentity,
    result: Result<DeepSolveResult, String>,
) {
    RUNTIME.with(|runtime| {
        let mut guard = runtime.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        let mut app = runtime.shared.borrow_mut();
        let Ok(inputs) = deep_solve::fingerprint(&app) else {
            return;
        };
        let generation = app.session.current_generation();
        if !app.operations.accepts(identity, generation, inputs) {
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
        let model = window.global::<DeepSolveModel>();
        model.set_running(false);
        match result {
            Ok(result) => {
                let deltas = tier_mast_deltas(&result.baseline, &result.solved);
                model.set_status(
                    (format_deep_solve_report(&result.report, false)
                        + &format_tier_mast_deltas(&deltas))
                        .into(),
                );
                let tiers = result
                    .solved
                    .iter()
                    .enumerate()
                    .map(|(index, tier)| {
                        let before = result
                            .baseline
                            .get(index)
                            .map_or(f64::NAN, |tier| tier.mast);
                        format!("Pin tier {} · {:.6} → {:.6}", index + 1, before, tier.mast).into()
                    })
                    .collect::<Vec<slint::SharedString>>();
                model.set_tiers(ModelRc::new(VecModel::from(tiers)));
                model.set_can_pin(result.report.accepted);
                runtime.result = Some(result);
            }
            Err(error) => {
                model.set_status(error.into());
                model.set_can_pin(false);
            }
        }
    });
}

pub fn shutdown() {
    RUNTIME.with(|runtime| drop(runtime.borrow_mut().take()));
}
