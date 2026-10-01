//! The live render loop: the page decides WHAT to render ([`sync`]), the render
//! Workers trace it (`indicatrix_web_core::host::RenderPool`), and the page shows it.
//!
//! # Design
//!
//! - **Scene.** [`sync`] builds a `SceneSpec` from `WebApp` with
//!   `indicatrix_web_core::settings::scene_spec` (the solved design's planes, the
//!   settings' finishes/material/lighting/bounces, the view's camera and size, the HDR
//!   id) and compares it with the one being rendered. Any difference is a new scene:
//!   `set_scene` + `start(target)`, a new epoch whose stale chunks the pool drops. A
//!   changed target alone just moves the goal. Leaving the Render tab pauses the pool
//!   (`cancel`); coming back resumes the same accumulation.
//! - **HDR map.** `hdr::sync_hdr` uploads the uploaded `.hdr` to the render Workers
//!   and the analysis Worker (so the metrics HUD is scored under it) and returns the id
//!   the scene names; it drops the map again when the setting is turned off.
//! - **Display.** Every merged pass calls [`on_progress`]. The picture is the
//!   desktop's live tone map (`display::live_rgba`, its `tonemap_running_average`),
//!   shown at most every [`MIN_DISPLAY_INTERVAL_MS`] -- or every three tone-map
//!   durations if the tone map is slow, so it never takes more than about a third of
//!   the page's time. Orbiting keeps accumulating progressively at full resolution
//!   (the desktop's default has no low-resolution preview either).
//! - **Denoise on settle.** Once the target is reached, or no pass has landed for
//!   [`SETTLE_MS`], the running sum goes to the pool's picture Worker (`request_picture
//!   (DenoisedLive)`), which runs the desktop's guide pass (once per scene) and
//!   `denoise_and_tonemap_frame`. It runs there, not on the page: measured natively
//!   on one thread, the denoise alone takes 0.3 s at 480x360, 0.8 s at 800x600 and
//!   1.2 s at 960x720 (plus 0.1 s of guide pass), and wasm is slower still -- far
//!   past the ~300 ms the page could spare. It is a Worker of its own, not one of the
//!   tracing Workers, so an orbit right after a settle finds every partition free. A
//!   denoise that fails is asked for once more, then left as traced (the pill says so).
//! - **Status.** The progress pill reads `samples/target spp · spp/s · N workers`;
//!   the status strip's backend slot reads `CPU · N workers`. A Worker that crashes or
//!   goes silent is replaced by the pool; one that keeps failing shows "Restart render
//!   workers".

mod hdr;
mod header_material;

use self::hdr::{HdrState, sync_hdr};
use super::{export, now_ms, panel, rgba_image};
use crate::{
    AppModel, RenderModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
        solve::{gave_up, is_solving, with_solved},
        state::SolveState,
    },
};
use indicatrix::geometry::GpuFacetPlane;
use indicatrix_editor::solve_policy::design_to_gpu_planes_from_solved;
use indicatrix_web_core::{
    display::{format_rate, live_rgba, samples_per_second},
    host::{PictureResult, WorkerPool},
    protocol::PictureKind,
    render::{Accumulator, clamp_live_spp},
    scene::SceneSpec,
    settings::{SceneInputs, render_material, scene_spec},
};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::RefCell, time::Duration};

/// The shortest gap between two displayed frames (about ten per second).
pub const MIN_DISPLAY_INTERVAL_MS: f64 = 100.0;

/// No pass for this long counts as settled (the denoise runs).
pub const SETTLE_MS: u64 = 1000;

/// What is on screen for the current scene.
#[derive(Default)]
struct Shown {
    scene_id: u64,
    started_ms: f64,
    /// `(time_ms, samples)` of the first progress report that had samples: the start
    /// of the window the spp/s readout is measured over.
    rate_anchor: Option<(f64, u32)>,
    last_display_ms: f64,
    tonemap_ms: f64,
    samples: u32,
    denoise_requested: Option<(u64, u32)>,
    denoised: Option<(u64, u32)>,
    denoise_ms: Option<f64>,
    /// The sum whose denoise failed once and was asked for again.
    denoise_retried: Option<(u64, u32)>,
    /// The sum whose denoise failed twice: it stays as traced, with no more attempts.
    denoise_failed: Option<(u64, u32)>,
}

#[derive(Default)]
struct Live {
    installed: bool,
    sync_timer: Timer,
    display_timer: Timer,
    settle_timer: Timer,
    /// The live scene being rendered.
    current: Option<SceneSpec>,
    target_spp: u32,
    hdr: HdrState,
    shown: Shown,
    /// The last error reported for the current scene: every Worker reports a scene
    /// that cannot be built, and one message is enough.
    last_error: Option<String>,
}

thread_local! {
    static LIVE: RefCell<Live> = RefCell::new(Live::default());
}

/// Whether an HDR map lights the live view (the Workers hold it and the scene names it).
pub fn hdr_lights_viewport() -> bool {
    hdr::in_use()
}

/// Records that the pool must get the live scene again (after an export or a Worker
/// restart replaced it).
pub(super) fn forget_scene() {
    LIVE.with(|live| live.borrow_mut().current = None);
}

/// Wires the Worker actions of the Render tab.
pub(super) fn install(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let c = ctx.clone();
    ui.global::<RenderModel>()
        .on_restart_workers(move || restart_workers(&c));
}

/// Schedules a [`sync`] on the next event-loop turn (restarting the same zero-delay
/// timer, so a burst of requests is one sync).
pub(super) fn request_sync(ctx: &Ctx) {
    let c = ctx.clone();
    LIVE.with(|live| {
        live.borrow()
            .sync_timer
            .start(TimerMode::SingleShot, Duration::ZERO, move || sync(&c));
    });
}

/// The pool, created on first use, with this module's callbacks installed once.
pub(super) fn ensure_pool(ctx: &Ctx) -> Result<WorkerPool, String> {
    let pool = crate::workers::pool()?;
    let first = LIVE.with(|live| !std::mem::replace(&mut live.borrow_mut().installed, true));
    if first {
        let render = pool.render();
        let c = ctx.clone();
        render.on_progress(move |acc| on_progress(&c, acc));
        let c = ctx.clone();
        render.on_error(move |message| on_error(&c, message));
        let c = ctx.clone();
        render.on_picture(move |result| on_picture(&c, result));
    }
    Ok(pool)
}

/// What the live view should do now.
enum Desired {
    /// No design: nothing to render.
    Nothing,
    /// Another tab is showing: pause.
    Hidden,
    /// Cannot render yet or at all; the text says why.
    Blocked(String),
    /// Render this.
    Scene(Box<SceneSpec>),
}

/// The solved design's planes, or why there are none yet.
fn live_planes(ctx: &Ctx) -> Result<Vec<GpuFacetPlane>, String> {
    let needs_solve = {
        let app = ctx.state.borrow();
        let Some(design) = &app.design else {
            return Err(String::new());
        };
        let current = design.session.current_generation();
        if design.session.design.tiers.is_empty() {
            return Ok(design_to_gpu_planes_from_solved(
                &design.session.design,
                &[],
            ));
        }
        match &app.solve {
            SolveState::Solved {
                generation, planes, ..
            } if *generation == current => return Ok(planes.clone()),
            SolveState::Failed {
                generation,
                message,
            } if *generation == current => {
                return Err(format!(
                    "The design does not solve, so there is nothing to render: {message}"
                ));
            }
            _ => true,
        }
    };
    if needs_solve && !is_solving(ctx) {
        if let Some(message) = gave_up(ctx) {
            // The solve Worker failed repeatedly: nothing will start another attempt until
            // the design changes or Solve is pressed.
            return Err(format!(
                "The design could not be solved, so there is nothing to render: {message}. \
                 Edit the design or press Solve to try again."
            ));
        }
        // `app::solve` calls `crate::render::solve_finished` when it lands.
        with_solved(ctx, |_, _| {});
    }
    Err("Solving the design...".to_string())
}

/// The live scene for the current state (ignoring the view tab), or why there is
/// none. Also used by the export, which renders the same scene at its own size.
pub(super) fn live_scene(ctx: &Ctx, pool: &WorkerPool) -> Result<SceneSpec, String> {
    let planes = live_planes(ctx)?;
    if planes.is_empty() {
        return Err("Nothing to render: the design has no facets yet.".to_string());
    }
    let hdr_id = sync_hdr(ctx, pool);
    let app = ctx.state.borrow();
    let design = app.design.as_ref().map(|d| &d.session.design);
    let material = render_material(&app.settings, design, &app.custom_materials)?;
    Ok(scene_spec(
        &app.settings,
        &SceneInputs {
            planes: &planes,
            material: &material,
            custom_materials: &app.custom_materials,
            width: app.view.render_width,
            height: app.view.render_height,
            hdr_id,
        },
    ))
}

/// The stone, material and light of the live view, without a frame size or an HDR map:
/// what the Tilt dialog's sweep measures (`crate::metrics::tilt`), which the desktop
/// scores under the lighting preset whatever lights the viewport. (The metrics HUD takes
/// the live scene itself, whose `hdr_id` names the map it is scored under.) The planes
/// are the solved design's, so this starts a solve when there is none for the current
/// design and answers "Solving..." until it lands.
pub fn optics_scene(ctx: &Ctx) -> Result<SceneSpec, String> {
    let planes = live_planes(ctx)?;
    if planes.is_empty() {
        return Err("Nothing to measure: the design has no facets yet.".to_string());
    }
    let app = ctx.state.borrow();
    let design = app.design.as_ref().map(|d| &d.session.design);
    let material = render_material(&app.settings, design, &app.custom_materials)?;
    Ok(scene_spec(
        &app.settings,
        &SceneInputs {
            planes: &planes,
            material: &material,
            custom_materials: &app.custom_materials,
            width: app.view.render_width,
            height: app.view.render_height,
            hdr_id: None,
        },
    ))
}

fn desired(ctx: &Ctx) -> Result<(Desired, Option<WorkerPool>), String> {
    let (has_design, tab) = {
        let app = ctx.state.borrow();
        (app.design.is_some(), app.settings.view_tab)
    };
    if !has_design {
        return Ok((Desired::Nothing, crate::workers::existing()));
    }
    if tab != 0 {
        // Another tab is showing: pause a pool that exists, but do not start Workers just
        // to hold them idle (a session restored on the Solid tab creates them when the
        // Render tab is first shown, or when a solve needs the solve Worker).
        let existing = crate::workers::existing();
        return Ok((Desired::Hidden, existing));
    }
    let pool = ensure_pool(ctx)?;
    let desired = match live_scene(ctx, &pool) {
        Ok(spec) => Desired::Scene(Box::new(spec)),
        Err(reason) => Desired::Blocked(reason),
    };
    Ok((desired, Some(pool)))
}

/// Brings the pool in line with the state -- see the module doc comment.
fn sync(ctx: &Ctx) {
    if export::is_running() {
        return;
    }
    header_material::follow(ctx);
    header_material::show_linked(ctx);
    if let Some(ui) = ctx.ui.upgrade() {
        panel::push_panel(&ui, &ctx.state.borrow().settings);
    }
    let (desired, pool) = match desired(ctx) {
        Ok(pair) => pair,
        Err(error) => {
            set_overlay(ctx, &error);
            set_backend(ctx, "CPU \u{b7} unavailable");
            return;
        }
    };
    let Some(pool) = pool else {
        // No design, or another tab with no Workers started yet.
        set_overlay(ctx, "");
        return;
    };
    set_backend(
        ctx,
        &format!("CPU \u{b7} {} workers", pool.render().worker_count()),
    );
    match desired {
        Desired::Nothing => {
            pool.render().cancel();
            forget_scene();
            clear_frame(ctx);
            crate::metrics::scene_gone(ctx);
        }
        Desired::Hidden => pool.render().cancel(),
        Desired::Blocked(reason) => {
            pool.render().cancel();
            set_overlay(ctx, &reason);
            crate::metrics::scene_blocked(ctx);
        }
        Desired::Scene(spec) => {
            set_overlay(ctx, "");
            crate::metrics::scene_changed(ctx, &spec);
            render_scene(ctx, &pool, *spec);
        }
    }
}

/// Starts `spec` if it differs from the scene being rendered, and (re)starts the pool
/// towards the target.
fn render_scene(ctx: &Ctx, pool: &WorkerPool, spec: SceneSpec) {
    let target = clamp_live_spp(ctx.state.borrow().settings.target_spp);
    let changed = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let changed = live.current.as_ref() != Some(&spec);
        if changed {
            live.current = Some(spec.clone());
            live.last_error = None;
        }
        live.target_spp = target;
        changed
    });
    let render = pool.render();
    if changed {
        render.set_scene(spec);
        let scene_id = render.scene_id();
        LIVE.with(|live| {
            live.borrow_mut().shown = Shown {
                scene_id,
                started_ms: now_ms(),
                ..Shown::default()
            };
        });
    }
    render.start(target);
    if !changed {
        // A new target or denoise setting may settle an unchanged scene.
        maybe_denoise(ctx, pool);
        show_progress_text(ctx, pool);
    }
}

/// A pass merged. Hands export scenes to the export; displays live ones (throttled).
fn on_progress(ctx: &Ctx, acc: &Accumulator) {
    if export::is_running() {
        export::on_progress(ctx, acc);
        return;
    }
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    if acc.scene_id() != pool.render().scene_id() {
        return;
    }
    let c = ctx.clone();
    LIVE.with(|live| {
        live.borrow().settle_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(SETTLE_MS),
            move || {
                if let Some(pool) = crate::workers::existing() {
                    settle(&c, &pool);
                }
            },
        );
    });
    let (last, tonemap_ms) = LIVE.with(|live| {
        let live = live.borrow();
        (live.shown.last_display_ms, live.shown.tonemap_ms)
    });
    let interval = MIN_DISPLAY_INTERVAL_MS.max(3.0 * tonemap_ms);
    let wait = last + interval - now_ms();
    let done = pool.render().is_done();
    if wait <= 0.0 || done {
        display(ctx, &pool);
    } else {
        let c = ctx.clone();
        LIVE.with(|live| {
            live.borrow().display_timer.start(
                TimerMode::SingleShot,
                Duration::from_secs_f64(wait / 1000.0),
                move || {
                    if let Some(pool) = crate::workers::existing() {
                        display(&c, &pool);
                    }
                },
            );
        });
    }
}

/// Shows the current running sum (unless its denoised picture is already up).
fn display(ctx: &Ctx, pool: &WorkerPool) {
    if export::is_running() {
        return;
    }
    let render = pool.render();
    let acc = render.accumulator();
    let acc = acc.borrow();
    let samples = acc.sample_count();
    let scene_id = acc.scene_id();
    let denoised = LIVE.with(|live| live.borrow().shown.denoised);
    if samples == 0 || denoised == Some((scene_id, samples)) {
        return;
    }
    let start = now_ms();
    let rgba = live_rgba(acc.sum(), samples);
    let (width, height) = (acc.width(), acc.height());
    drop(acc);
    let elapsed = now_ms() - start;
    set_frame(ctx, width, height, &rgba);
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let shown = &mut live.shown;
        shown.last_display_ms = now_ms();
        shown.tonemap_ms = if shown.tonemap_ms > 0.0 {
            0.5f64.mul_add(elapsed - shown.tonemap_ms, shown.tonemap_ms)
        } else {
            elapsed
        };
        shown.samples = samples;
    });
    show_progress_text(ctx, pool);
    if render.is_done() {
        maybe_denoise(ctx, pool);
    }
}

/// No pass for [`SETTLE_MS`]: denoise what there is.
fn settle(ctx: &Ctx, pool: &WorkerPool) {
    if !export::is_running() {
        maybe_denoise(ctx, pool);
    }
}

/// Asks a Worker for the denoised picture of the current sum, once per sample count.
fn maybe_denoise(ctx: &Ctx, pool: &WorkerPool) {
    let enabled = ctx.state.borrow().settings.denoise;
    let render = pool.render();
    let (scene_id, samples) = {
        let acc = render.accumulator();
        let acc = acc.borrow();
        (acc.scene_id(), acc.sample_count())
    };
    let key = Some((scene_id, samples));
    let (requested, denoised, failed) = LIVE.with(|live| {
        let live = live.borrow();
        (
            live.shown.denoise_requested,
            live.shown.denoised,
            live.shown.denoise_failed,
        )
    });
    if !enabled || samples == 0 || requested == key || denoised == key || failed == key {
        return;
    }
    if render.request_picture(PictureKind::DenoisedLive).is_ok() {
        LIVE.with(|live| live.borrow_mut().shown.denoise_requested = key);
        show_progress_text(ctx, pool);
    }
}

/// A Worker finished a picture: export PNGs go to the export, denoised frames here.
fn on_picture(ctx: &Ctx, result: PictureResult) {
    if matches!(result.kind, PictureKind::Png { .. }) {
        export::on_picture(ctx, result);
        return;
    }
    if let Err(error) = &result.bytes {
        denoise_failed(ctx, result.scene_id, error);
        return;
    }
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        if live.shown.denoise_requested == Some((result.scene_id, result.sample_count)) {
            live.shown.denoise_requested = None;
        }
    });
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    let render = pool.render();
    let (scene_id, samples, width, height) = {
        let acc = render.accumulator();
        let acc = acc.borrow();
        (
            acc.scene_id(),
            acc.sample_count(),
            acc.width(),
            acc.height(),
        )
    };
    let wanted = ctx.state.borrow().settings.denoise;
    if export::is_running() || !wanted || result.scene_id != scene_id {
        return;
    }
    let Ok(rgba) = result.bytes else {
        return;
    };
    if result.sample_count != samples {
        // Newer samples arrived meanwhile; the settled sum is denoised again.
        if render.is_done() {
            maybe_denoise(ctx, &pool);
        }
        return;
    }
    set_frame(ctx, width, height, &rgba);
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.shown.denoised = Some((scene_id, samples));
        live.shown.denoise_ms = Some(result.elapsed_ms);
    });
    show_progress_text(ctx, &pool);
}

/// A denoise failed (the picture Worker crashed, or could not make it).
///
/// The same sum is asked for once more; a second failure leaves the frame as traced and
/// says so, instead of showing "denoising..." forever.
fn denoise_failed(ctx: &Ctx, scene_id: u64, error: &str) {
    crate::app::diagnostics::console_warn(&format!("denoise failed: {error}"));
    let retry = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let shown = &mut live.shown;
        let key = shown
            .denoise_requested
            .filter(|(scene, _)| *scene == scene_id)?;
        shown.denoise_requested = None;
        if shown.denoise_retried == Some(key) {
            shown.denoise_failed = Some(key);
            Some(false)
        } else {
            shown.denoise_retried = Some(key);
            Some(true)
        }
    });
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    match retry {
        Some(true) => maybe_denoise(ctx, &pool),
        Some(false) => show_progress_text(ctx, &pool),
        None => {}
    }
}

/// A Worker, scene or HDR failure.
fn on_error(ctx: &Ctx, message: &str) {
    let repeated = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let repeated = live.last_error.as_deref() == Some(message);
        live.last_error = Some(message.to_string());
        repeated
    });
    if !repeated {
        show_message(ctx, MessageKind::Error, &format!("Render: {message}"));
    }
    let failed = crate::workers::existing().is_some_and(|p| p.render().failed_worker_count() > 0);
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<RenderModel>().set_workers_failed(failed);
    }
    if failed {
        export::on_worker_failure(ctx);
    }
}

/// "Restart render workers".
fn restart_workers(ctx: &Ctx) {
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    match pool.render().restart_workers() {
        Ok(()) => {
            if let Some(ui) = ctx.ui.upgrade() {
                ui.global::<RenderModel>().set_workers_failed(false);
            }
            show_message(ctx, MessageKind::Info, "Render workers restarted.");
            forget_scene();
            request_sync(ctx);
        }
        Err(error) => show_message(
            ctx,
            MessageKind::Error,
            &format!("Could not restart the render workers: {error}"),
        ),
    }
}

/// `samples/target spp · spp/s · N workers`, plus the settle state.
fn show_progress_text(ctx: &Ctx, pool: &WorkerPool) {
    let render = pool.render();
    let samples = render.accumulator().borrow().sample_count();
    let now = now_ms();
    let (target, started, anchor, key, denoise) = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let target = live.target_spp;
        let s = &mut live.shown;
        // The first report with samples starts the rate window, so the Workers' start-up
        // does not count against the throughput.
        if s.rate_anchor.is_none() && samples > 0 {
            s.rate_anchor = Some((now, samples));
        }
        (
            target,
            s.started_ms,
            s.rate_anchor,
            (s.scene_id, samples),
            (
                s.denoise_requested,
                s.denoised,
                s.denoise_failed,
                s.denoise_ms,
            ),
        )
    });
    let (requested, denoised, failed, denoise_ms) = denoise;
    let rate = samples_per_second(samples, started, anchor, now);
    let mut text = format!(
        "{samples}/{target} spp \u{b7} {} spp/s \u{b7} {} workers",
        format_rate(rate),
        render.worker_count()
    );
    if denoised == Some(key) {
        text.push_str(&denoise_ms.map_or_else(
            || " \u{b7} denoised".to_string(),
            |ms| format!(" \u{b7} denoised ({:.1} s)", ms / 1000.0),
        ));
    } else if requested == Some(key) {
        text.push_str(" \u{b7} denoising...");
    } else if failed == Some(key) {
        text.push_str(" \u{b7} denoise failed");
    }
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<RenderModel>().set_progress_text(text.into());
    }
}

/// Shows the denoise setting's effect at once: the raw frame when it was turned off.
pub(super) fn denoise_setting_changed(ctx: &Ctx) {
    let enabled = ctx.state.borrow().settings.denoise;
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    if enabled {
        maybe_denoise(ctx, &pool);
    } else {
        LIVE.with(|live| live.borrow_mut().shown.denoised = None);
        display(ctx, &pool);
    }
}

fn set_frame(ctx: &Ctx, width: u32, height: u32, rgba: &[u8]) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RenderModel>();
        model.set_frame(rgba_image(width, height, rgba));
        model.set_has_frame(true);
    }
}

fn clear_frame(ctx: &Ctx) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RenderModel>();
        model.set_frame(slint::Image::default());
        model.set_has_frame(false);
        model.set_progress_text(slint::SharedString::new());
    }
}

fn set_overlay(ctx: &Ctx, text: &str) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<RenderModel>().set_overlay_text(text.into());
    }
}

fn set_backend(ctx: &Ctx, text: &str) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<AppModel>().set_backend_text(text.into());
    }
}
