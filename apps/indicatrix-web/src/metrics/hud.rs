//! The Render tab's metrics HUD: brilliance, fire, scintillation, windowing and extinction
//! of the pose on screen, as the desktop's HUD shows them.
//!
//! [`scene_changed`] is called by the live render sync (`crate::render`) with the scene it
//! would render. Two scenes with equal `MetricsParams` (planes, material with its
//! render-time overrides, camera yaw/pitch, light yaw/pitch, preset and HDR map id) need
//! no new numbers, so exposure, size, bounces and finishes never recompute anything.
//!
//! # Under the HDR map
//!
//! The metrics are scored under the radiance the viewport is lit with, as on the desktop:
//! a scene lit by a loaded HDR map names its id, the analysis Worker holds the same map
//! (`WorkerPool::set_hdr`), and the HUD says "Scored under the HDR map". The map's id is
//! part of the params, so loading, replacing or dropping a map recomputes the numbers. A
//! Worker that does not hold the map (it did not decode, or the memory budget left it no
//! copy) scores under the preset and the result says so; the HUD then names the preset
//! and that the map is not loaded.

use super::tilt;
use crate::{MetricsModel, app::Ctx};
use indicatrix_web_core::{
    host::WorkerPool,
    scene::SceneSpec,
    solve::{MetricsParams, MetricsResultData, SolveRequest, SolveResponse},
    solve_error::SolveError,
};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::RefCell, time::Duration};

/// How long the pose and light must hold still before new metrics are requested: an
/// orbit drag changes them dozens of times a second, and each request would supersede the
/// last.
pub const METRICS_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Runtime {
    debounce: Timer,
    /// The inputs of the newest scene; what the HUD should end up showing.
    target: Option<MetricsParams>,
    /// The inputs of the numbers on screen.
    shown: Option<MetricsParams>,
    /// The inputs of the job in the Worker.
    in_flight: Option<MetricsParams>,
    /// Bumped by every dispatch and reset; an answer of an older one is ignored.
    epoch: u64,
    /// A request was due while a tilt sweep held the Worker ([`resume`] sends it).
    deferred: bool,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
}

/// What a new scene needs from the Worker.
enum Next {
    /// Its numbers are on screen.
    Shown,
    /// A job for exactly these inputs is running.
    OnItsWay,
    /// Other inputs: request after the debounce.
    Compute,
}

/// The live sync has a scene to render: make sure the HUD shows (or will show) its
/// metrics.
pub fn scene_changed(ctx: &Ctx, spec: &SceneSpec) {
    let params = MetricsParams::from_scene(spec);
    let next = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        let next = if rt.in_flight.as_ref() == Some(&params) {
            Next::OnItsWay
        } else if rt.in_flight.is_none() && rt.shown.as_ref() == Some(&params) {
            Next::Shown
        } else {
            Next::Compute
        };
        rt.target = Some(params);
        if !matches!(next, Next::Compute) {
            rt.debounce.stop();
            rt.deferred = false;
        }
        next
    });
    match next {
        Next::Shown => set_pending(ctx, false),
        Next::OnItsWay => {}
        Next::Compute => {
            set_pending(ctx, true);
            let c = ctx.clone();
            RUNTIME.with(|rt| {
                rt.borrow()
                    .debounce
                    .start(TimerMode::SingleShot, METRICS_DEBOUNCE, move || {
                        dispatch(&c);
                    });
            });
        }
    }
}

/// The live sync cannot render for now (the design is solving, or does not solve): the
/// numbers on screen describe an older stone, so they dim, and nothing is requested until
/// a scene comes back.
pub fn scene_blocked(ctx: &Ctx) {
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.debounce.stop();
        rt.target = None;
        rt.deferred = false;
    });
    set_pending(ctx, true);
}

/// There is no design: the HUD is cleared and any job in the Worker is forgotten.
pub fn scene_gone(ctx: &Ctx) {
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.debounce.stop();
        rt.target = None;
        rt.shown = None;
        rt.in_flight = None;
        rt.deferred = false;
        rt.epoch += 1;
    });
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<MetricsModel>();
        model.set_has_metrics(false);
        model.set_pending(false);
        model.set_status_text(slint::SharedString::new());
        model.set_scored_under_text(slint::SharedString::new());
    }
}

/// The tilt sweep ended: sends the request it held back, if any.
pub fn resume(ctx: &Ctx) {
    let due = RUNTIME.with(|rt| {
        let rt = rt.borrow();
        rt.deferred && !rt.debounce.running()
    });
    if due {
        dispatch(ctx);
    }
}

/// Sends the newest inputs to the analysis Worker, unless they are already shown or on
/// their way, or a tilt sweep holds the Worker.
fn dispatch(ctx: &Ctx) {
    if tilt::is_running() {
        RUNTIME.with(|rt| rt.borrow_mut().deferred = true);
        return;
    }
    let job = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        let params = rt.target.clone()?;
        let current = rt.in_flight.as_ref() == Some(&params)
            || (rt.in_flight.is_none() && rt.shown.as_ref() == Some(&params));
        if current {
            rt.deferred = false;
            return None;
        }
        rt.epoch += 1;
        rt.deferred = false;
        rt.in_flight = Some(params.clone());
        Some((params, rt.epoch))
    });
    let Some((params, epoch)) = job else {
        return;
    };
    let client = match crate::workers::pool().and_then(|pool| WorkerPool::analysis(&pool)) {
        Ok(client) => client,
        Err(message) => {
            finish(ctx, epoch, &params, Err(SolveError::Failed(message)));
            return;
        }
    };
    let future = client.solve(
        String::new(),
        SolveRequest::Metrics {
            params: params.clone(),
        },
    );
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await;
        finish(&ctx, epoch, &params, result);
    });
}

/// A job ended: show its numbers, or say why there are none.
fn finish(
    ctx: &Ctx,
    epoch: u64,
    params: &MetricsParams,
    result: Result<SolveResponse, SolveError>,
) {
    if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
        // Superseded by a newer dispatch, or the design went away.
        return;
    }
    RUNTIME.with(|rt| rt.borrow_mut().in_flight = None);
    match result {
        Ok(SolveResponse::Metrics(metrics)) => {
            let up_to_date = RUNTIME.with(|rt| {
                let mut rt = rt.borrow_mut();
                rt.shown = Some(params.clone());
                rt.target.as_ref() == Some(params)
            });
            let note = metrics.scored_under.hud_note(params.hdr_id.is_some());
            push_numbers(ctx, &metrics, !up_to_date, &note);
        }
        Ok(SolveResponse::AnalysisFailed { message, .. }) => {
            show_unavailable(ctx, &format!("Metrics unavailable: {message}"));
        }
        // The tilt sweep took the Worker: the request is sent again when it ends.
        Err(SolveError::Superseded) => {
            RUNTIME.with(|rt| rt.borrow_mut().deferred = true);
        }
        Err(error) => {
            crate::app::diagnostics::console_warn(&format!("metrics failed: {error}"));
            show_unavailable(ctx, &format!("Metrics unavailable: {error}"));
        }
        Ok(_) => {}
    }
}

fn set_pending(ctx: &Ctx, pending: bool) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<MetricsModel>().set_pending(pending);
    }
}

/// Shows `metrics` with `note` (what they were scored under; empty for the preset the
/// viewport shows) under the numbers.
fn push_numbers(ctx: &Ctx, metrics: &MetricsResultData, pending: bool, note: &str) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<MetricsModel>();
    model.set_brilliance_pct(metrics.brilliance_pct);
    model.set_fire_index(metrics.fire_index);
    model.set_scintillation_pct(metrics.scintillation_pct);
    model.set_windowing_pct(metrics.windowing_pct);
    model.set_extinction_pct(metrics.extinction_pct);
    model.set_status_text(slint::SharedString::new());
    model.set_scored_under_text(note.into());
    model.set_has_metrics(true);
    model.set_pending(pending);
}

fn show_unavailable(ctx: &Ctx, text: &str) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<MetricsModel>();
        model.set_has_metrics(false);
        model.set_pending(false);
        model.set_status_text(text.into());
        model.set_scored_under_text(slint::SharedString::new());
    }
}
