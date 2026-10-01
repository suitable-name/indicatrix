//! The Tilt Performance dialog (the desktop's `gui::tilt::tilt_profile` +
//! `performance_graph_dialog.slint`): brilliance, windowing and extinction against tilt
//! away from table-up, four camera azimuths of 181 points each.
//!
//! # Flow
//!
//! Opening the dialog sweeps automatically when the curves on screen were not swept for
//! the current inputs (the desktop does the same); Run sweeps again. The sweep runs in
//! the analysis Worker as `SolveRequest::Tilt`, streaming a stage line and a fraction, and
//! Cancel stops it between points (`SolveClient::request_cancel`) -- the curves already on
//! screen stay. The inputs are the desktop's key: the stone's solved planes, the
//! material, and the light; the camera is not one (the sweep moves it). While the dialog is
//! open a short poll compares them with the swept ones and marks the curves stale.
//!
//! # Lighting
//!
//! The sweep is scored under the lighting preset's rig at the current light, as the
//! desktop's dialog does: a loaded HDR map lights the viewport and the metrics HUD, but
//! not this sweep. The dialog says so while a map is loaded
//! (`indicatrix_web_core::solve::tilt_lighting_note`).
//!
//! # What differs from the desktop
//!
//! No hover preview thumbnail (a render of the stone at the hovered tilt): the render pool
//! is busy with the live view, and a second scene per hover would stall it. No "Copy data
//! table" and no tilt-video export.

use super::hud;
use crate::{AppWindow, TiltModel, app::Ctx, workers};
use indicatrix::color::metrics::AxisProfile;
use indicatrix_editor::view_model::curve_path::full_axis_curve_path;
use indicatrix_web_core::{
    host::{SolveClient, WorkerPool},
    solve::{
        SolveRequest, SolveResponse, TiltAxisData, TiltParams, TiltResultData, tilt_lighting_note,
    },
    solve_error::SolveError,
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, time::Duration};

/// How often an open dialog compares the swept inputs with the current ones.
const STALE_POLL: Duration = Duration::from_millis(400);

#[derive(Default)]
struct Runtime {
    /// Bumped by every Run; an answer of an older sweep is ignored.
    epoch: u64,
    running: bool,
    /// The inputs the curves on screen were swept for.
    computed: Option<TiltParams>,
    /// The dialog opened before there was a stone to sweep: sweep as soon as there is.
    wants_run: bool,
    poll: Timer,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
}

/// Whether a sweep is running in the analysis Worker (the HUD then waits).
#[must_use]
pub fn is_running() -> bool {
    RUNTIME.with(|rt| rt.borrow().running)
}

/// The current inputs and the traced material's name, or why there is nothing to sweep.
fn current_inputs(ctx: &Ctx) -> Result<(TiltParams, String), String> {
    let spec = crate::render::optics_scene(ctx)?;
    let name = spec.material.name.clone();
    Ok((TiltParams::from_scene(&spec), name))
}

fn set_status(ctx: &Ctx, text: &str) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<TiltModel>().set_status_text(text.into());
    }
}

/// The dialog opened: start the stale poll and sweep if the curves are not current.
fn opened(ctx: &Ctx) {
    start_poll(ctx);
    match current_inputs(ctx) {
        Ok((params, name)) => {
            show_inputs(ctx, &params, &name);
            let current = RUNTIME.with(|rt| rt.borrow().computed.as_ref() == Some(&params));
            if current {
                set_stale(ctx, false);
            } else {
                run(ctx);
            }
        }
        Err(reason) => {
            RUNTIME.with(|rt| rt.borrow_mut().wants_run = true);
            set_status(ctx, &reason);
        }
    }
}

/// Names the traced material and what the sweep for `params` is scored under.
fn show_inputs(ctx: &Ctx, params: &TiltParams, material: &str) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<TiltModel>();
        model.set_material_name(material.into());
        model.set_lighting_note(
            tilt_lighting_note(params.preset_index, crate::render::hdr_lights_viewport()).into(),
        );
    }
}

fn set_stale(ctx: &Ctx, stale: bool) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<TiltModel>().set_curves_stale(stale);
    }
}

/// While the dialog is open: marks the curves stale when the inputs moved, and sweeps
/// once a stone appears if opening found none.
fn start_poll(ctx: &Ctx) {
    let c = ctx.clone();
    RUNTIME.with(|rt| {
        rt.borrow()
            .poll
            .start(TimerMode::Repeated, STALE_POLL, move || poll(&c));
    });
}

fn poll(ctx: &Ctx) {
    let open = ctx
        .ui
        .upgrade()
        .is_some_and(|ui| ui.global::<TiltModel>().get_open());
    if !open {
        RUNTIME.with(|rt| {
            let mut rt = rt.borrow_mut();
            rt.poll.stop();
            rt.wants_run = false;
        });
        return;
    }
    if is_running() {
        return;
    }
    let Ok((params, name)) = current_inputs(ctx) else {
        return;
    };
    show_inputs(ctx, &params, &name);
    let (computed, wants_run) = RUNTIME.with(|rt| {
        let rt = rt.borrow();
        (rt.computed.clone(), rt.wants_run)
    });
    if wants_run {
        run(ctx);
        return;
    }
    set_stale(ctx, computed.is_some_and(|c| c != params));
}

/// Run: sweeps the current inputs in the analysis Worker.
fn run(ctx: &Ctx) {
    if is_running() {
        return;
    }
    let (params, name) = match current_inputs(ctx) {
        Ok(inputs) => inputs,
        Err(reason) => {
            RUNTIME.with(|rt| rt.borrow_mut().wants_run = true);
            set_status(ctx, &reason);
            return;
        }
    };
    let client = match workers::pool().and_then(|pool| WorkerPool::analysis(&pool)) {
        Ok(client) => client,
        Err(message) => {
            set_status(ctx, &message);
            return;
        }
    };
    let epoch = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.epoch += 1;
        rt.running = true;
        rt.wants_run = false;
        rt.epoch
    });
    show_inputs(ctx, &params, &name);
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<TiltModel>();
        model.set_running(true);
        model.set_cancelling(false);
        model.set_progress(0.0);
        model.set_status_text("Starting the tilt sweep...".into());
    }
    let weak = ctx.ui.clone();
    let future = client.solve_with_progress(
        String::new(),
        SolveRequest::Tilt {
            params: params.clone(),
        },
        move |progress| {
            if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                let model = ui.global::<TiltModel>();
                model.set_status_text(progress.message.into());
                model.set_progress(progress.fraction.unwrap_or(-1.0));
            }
        },
    );
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await;
        finish(&ctx, epoch, params, result);
    });
}

/// A sweep ended: shows its curves, or says why not.
fn finish(ctx: &Ctx, epoch: u64, params: TiltParams, result: Result<SolveResponse, SolveError>) {
    if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
        return;
    }
    RUNTIME.with(|rt| rt.borrow_mut().running = false);
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<TiltModel>();
        model.set_running(false);
        model.set_cancelling(false);
        model.set_progress(-1.0);
    }
    let status = match result {
        Ok(SolveResponse::TiltCurves(data)) => install(ctx, params, &data),
        Ok(SolveResponse::Cancelled) | Err(SolveError::Cancelled) => cancelled_text(),
        Ok(SolveResponse::AnalysisFailed { message, .. }) => {
            format!("The tilt sweep could not run: {message}")
        }
        Ok(_) => "The analysis worker answered a different request.".to_string(),
        Err(error) => {
            crate::app::diagnostics::console_warn(&format!("tilt sweep failed: {error}"));
            format!("The tilt sweep failed: {error}")
        }
    };
    set_status(ctx, &status);
    // The inputs may have moved while it ran.
    poll_now(ctx);
    hud::resume(ctx);
}

fn cancelled_text() -> String {
    "Cancelled -- the curves on screen (if any) are unchanged.".to_string()
}

/// One stale check outside the timer.
fn poll_now(ctx: &Ctx) {
    if ctx
        .ui
        .upgrade()
        .is_some_and(|ui| ui.global::<TiltModel>().get_open())
    {
        poll(ctx);
    }
}

/// Puts a finished sweep on screen; the status line it leaves.
fn install(ctx: &Ctx, params: TiltParams, data: &TiltResultData) -> String {
    let profiles: Option<Vec<AxisProfile>> =
        data.axes.iter().map(TiltAxisData::to_profile).collect();
    let Some(profiles) = profiles.filter(|p| p.len() == 4) else {
        return "The analysis worker returned malformed curves.".to_string();
    };
    let Some(ui) = ctx.ui.upgrade() else {
        return String::new();
    };
    let model = ui.global::<TiltModel>();
    model.set_brilliance_axes(rows(&profiles, |p| &p.brilliance));
    model.set_extinction_axes(rows(&profiles, |p| &p.extinction));
    model.set_windowing_axes(rows(&profiles, |p| &p.windowing));
    model.set_brilliance_paths(paths(&profiles, |p| &p.brilliance));
    model.set_extinction_paths(paths(&profiles, |p| &p.extinction));
    model.set_windowing_paths(paths(&profiles, |p| &p.windowing));
    model.set_curves_stale(false);
    RUNTIME.with(|rt| rt.borrow_mut().computed = Some(params));
    "Swept 4 axes of 181 points.".to_string()
}

/// One model row of 181 values per axis, the shape the dialog's `[[float]]` wants.
fn rows(
    profiles: &[AxisProfile],
    pick: impl Fn(&AxisProfile) -> &[f32; 181],
) -> ModelRc<ModelRc<f32>> {
    ModelRc::new(VecModel::from(
        profiles
            .iter()
            .map(|p| ModelRc::new(VecModel::from(pick(p).to_vec())))
            .collect::<Vec<_>>(),
    ))
}

/// One SVG path string per axis (`full_axis_curve_path`, the desktop's drawing).
fn paths(
    profiles: &[AxisProfile],
    pick: impl Fn(&AxisProfile) -> &[f32; 181],
) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        profiles
            .iter()
            .map(|p| SharedString::from(full_axis_curve_path(pick(p))))
            .collect::<Vec<_>>(),
    ))
}

/// Cancel: asks the sweep to stop between points; it then answers `Cancelled`.
fn cancel(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<TiltModel>();
    if !model.get_running() || model.get_cancelling() {
        return;
    }
    let asked = workers::pool()
        .and_then(|pool| WorkerPool::analysis(&pool))
        .and_then(|client| SolveClient::request_cancel(&client));
    match asked {
        Ok(()) => {
            model.set_cancelling(true);
            model.set_status_text("Cancelling...".into());
        }
        Err(message) => {
            // The replacement Worker could not start: the sweep is gone either way.
            RUNTIME.with(|rt| {
                let mut rt = rt.borrow_mut();
                rt.epoch += 1;
                rt.running = false;
            });
            model.set_running(false);
            model.set_progress(-1.0);
            model.set_status_text(format!("Could not cancel cleanly: {message}").into());
            hud::resume(ctx);
        }
    }
}

/// Registers the dialog's callbacks.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<TiltModel>();
    let c = ctx.clone();
    model.on_opened(move || opened(&c));
    let c = ctx.clone();
    model.on_run(move || run(&c));
    let c = ctx.clone();
    model.on_cancel(move || cancel(&c));
}
