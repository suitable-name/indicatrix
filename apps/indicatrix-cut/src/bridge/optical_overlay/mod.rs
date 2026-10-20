//! Separate interactive diagnostic layer. Beauty pixels and accumulation are never modified.
pub mod export;
mod hover;
mod labels;
mod lifecycle;
mod status;
use status::Status;
pub mod snapshot;
mod summary;
mod worker;
use crate::{
    MainWindow, OpticalModel, ViewportModel,
    bridge::{frame_cache::stone_width::StoneWidthCache, render_thread::RenderContext},
};
use indicatrix::{
    color::metrics::OpticalImage,
    renderer::optical_overlay::{OpticalPresentation, optical_rgba_with_extent},
};
pub use labels::facet_labels;
use labels::{captured_labels, same_labels};
use lifecycle::{Displayed, may_publish, record_displayed};
use slint::ComponentHandle;
use snapshot::OpticalSnapshot;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use worker::{Channels, Job, Worker};
/// The trace the worker was last asked for.
struct Requested {
    scene: Arc<OpticalSnapshot>,
    /// The beauty frame the trace was requested for; its size decides where it may land.
    stamp: Displayed,
    channels: Channels,
}
struct Controller {
    worker: Worker,
    serial: u64,
    displayed: Option<Displayed>,
    requested: Option<Requested>,
    cached: Option<Arc<OpticalImage>>,
    /// The capture of the latest scene generation. Every input of a capture is part of the
    /// scene generation, so the same generation needs no second capture.
    last_capture: Option<Arc<OpticalSnapshot>>,
    /// Persistent across captures: a per-millimeter material measures the stone once.
    width_cache: StoneWidthCache,
    /// Why the last trace failed; shown in place of the scene's limitations until the
    /// next request.
    failure: Option<String>,
    style: OpticalPresentation,
    settled: Instant,
}
impl Controller {
    /// Abandons everything in flight and everything cached for the old input.
    fn reset(&mut self) {
        self.serial += 1;
        self.worker.cancel(self.serial);
        self.requested = None;
        self.cached = None;
        self.failure = None;
    }
}
thread_local! {
    static STATE: RefCell<Option<Controller>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}
/// Called by both actual UI image setters with the immutable producer's scene stamp.
///
/// The setters show **every** frame they receive, including one whose scene generation
/// has already moved on (an orbit or slider drag outpaces the render): dropping those
/// froze the viewport for the whole drag. A stale frame only hides the overlay (its
/// stamp is recorded here and `tick` refuses to publish against it) until a frame of the
/// current generation arrives. A previous map is cleared before the new beauty image can
/// appear.
pub fn displayed(ui: &MainWindow, generation: u64, width: u32, height: u32) {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let Some(c) = state.as_mut() else {
            return;
        };
        let stamp = Displayed {
            generation,
            width,
            height,
        };
        if record_displayed(&mut c.displayed, stamp) {
            let m = ui.global::<OpticalModel>();
            m.set_aspect(width as f32 / height.max(1) as f32);
            // An overlay that is off has nothing to hide, and `tick` leaves its Off
            // status alone once it is shown.
            if m.get_enabled() {
                clear(ui, Status::Updating, "");
            }
        }
    });
}
/// Hides the overlay and sets the status line; a repeat of what is already shown writes nothing.
fn clear(ui: &MainWindow, status: Status, detail: &str) {
    let m = ui.global::<OpticalModel>();
    if !m.get_ready() && m.get_hover_text().is_empty() && status::is_current(ui, status, detail) {
        return;
    }
    m.set_ready(false);
    status::set(ui, status, detail);
    m.set_hover_text("".into());
}
/// Capture once for exports; the worker never reads mutable UI properties.
pub fn presentation(ui: &MainWindow) -> OpticalPresentation {
    let m = ui.global::<OpticalModel>();
    OpticalPresentation {
        layers: [
            m.get_windowing(),
            m.get_extinction(),
            m.get_brilliance(),
            m.get_fire(),
        ],
        opacity: m.get_opacity(),
        map_mode: m.get_map_mode(),
        high_contrast: ui.global::<crate::PreferencesModel>().get_high_contrast(),
        scintillation: m.get_scintillation(),
    }
}
/// `beauty` is the size of the beauty frame the map was traced for: the map may be a
/// lower-resolution draft, and its patterns are laid out on beauty pixels so a draft and its
/// refinement draw the same hatching.
fn publish(ui: &MainWindow, c: &Controller, map: &OpticalImage, refined: bool, beauty: Displayed) {
    let m = ui.global::<OpticalModel>();
    let bytes = optical_rgba_with_extent(map, c.style, [beauty.width, beauty.height]);
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(map.width, map.height);
    buffer.make_mut_bytes().copy_from_slice(&bytes);
    m.set_image(crate::bridge::pixel_buffer::rgba_image(buffer));
    m.set_summary(summary::summary_text(map, c.style.scintillation).into());
    status::set(
        ui,
        if refined {
            Status::Ready
        } else {
            Status::Draft
        },
        &format!(
            "{} x {} - {} Samples/Pixel",
            map.width, map.height, map.samples
        ),
    );
    m.set_ready(true);
}
/// The overlay is Off, or enabled but not shown (hidden tab, Solid view): nothing is
/// traced and nothing is displayed. An Off overlay never takes the render lock.
fn inactive(ui: &MainWindow, c: &mut Controller, enabled: bool) {
    if c.requested.take().is_some() {
        c.serial += 1;
        c.worker.cancel(c.serial);
        c.cached = None;
    }
    c.last_capture = None;
    let (status, detail) = if enabled {
        (
            Status::Suspended,
            "Solid Hatching Shows Critical-Angle Risk",
        )
    } else {
        (Status::Off, "")
    };
    let m = ui.global::<OpticalModel>();
    if m.get_ready() || !status::is_current(ui, status, detail) {
        m.set_image(slint::Image::default());
        clear(ui, status, detail);
    }
}
/// What `tick` needs once the scene is known and a matching beauty frame is on screen.
struct Ready {
    scene: Arc<OpticalSnapshot>,
    displayed: Displayed,
    generation: u64,
}
/// The capture of the current scene generation, reused while the generation holds.
fn current_scene(
    c: &mut Controller,
    ctx: &mut RenderContext,
    generation: u64,
) -> Result<Arc<OpticalSnapshot>, String> {
    if let Some(reason) = OpticalSnapshot::refusal(ctx) {
        c.last_capture = None;
        return Err(reason);
    }
    let labels = captured_labels(ctx);
    if let Some(scene) = c.last_capture.as_mut()
        && scene.generation == generation
    {
        // The solid worker can publish facet names after the capture; names are the one
        // part of a capture that moves without a new scene generation.
        if !same_labels(scene.labels.as_ref(), labels.as_ref()) {
            Arc::make_mut(scene).labels = labels;
        }
        return Ok(Arc::clone(scene));
    }
    let scene = Arc::new(OpticalSnapshot::capture(ctx, &mut c.width_cache)?);
    c.last_capture = Some(Arc::clone(&scene));
    Ok(scene)
}
/// Everything up to a capture of the current scene on a matching beauty frame; `None`
/// (with the overlay hidden and the reason shown) while any of that is missing.
fn ready_scene(ui: &MainWindow, c: &mut Controller, ctx: &mut RenderContext) -> Option<Ready> {
    let m = ui.global::<OpticalModel>();
    if ui.global::<ViewportModel>().get_trace_stale() {
        c.reset();
        clear(ui, Status::Unavailable, "Solve the Current Design");
        return None;
    }
    let generation = ctx.scene_generation();
    if ctx.camera_moving {
        c.reset();
        c.settled = Instant::now();
        clear(ui, Status::Updating, "Camera Moving");
        return None;
    }
    let Some(displayed) = c.displayed.filter(|d| d.generation == generation) else {
        // Hide immediately, but retain the bounded cached/requested data until
        // the new beauty frame arrives. Exposure and sample-target changes
        // advance its generation without changing the optical input. The
        // comparison in `request_if_needed` either reuses that data or supersedes its serial.
        clear(ui, Status::Updating, "Waiting for a Matching Beauty Frame");
        return None;
    };
    match current_scene(c, ctx, generation) {
        Ok(scene) => Some(Ready {
            scene,
            displayed,
            generation,
        }),
        Err(reason) => {
            c.reset();
            clear(ui, Status::Unavailable, "");
            if m.get_limitations().as_str() != reason {
                m.set_limitations(reason.into());
            }
            None
        }
    }
}
/// Submits a trace when the optical input, the frame size or a channel request changed.
/// Returns `false` while a fresh input is still settling (nothing may be published yet).
fn request_if_needed(
    ui: &MainWindow,
    c: &mut Controller,
    scene: &Arc<OpticalSnapshot>,
    displayed: Displayed,
    channels: Channels,
) -> bool {
    // A channel already traced stays valid when its layer is switched off; only a newly
    // requested channel needs a new trace.
    let needs_trace = c.requested.as_ref().is_none_or(|r| {
        !(Arc::ptr_eq(&r.scene, scene) || r.scene.same_input(scene))
            || r.stamp.width != displayed.width
            || r.stamp.height != displayed.height
            || (!r.channels.fire && channels.fire)
            || (!r.channels.scintillation && channels.scintillation)
    });
    if needs_trace {
        clear(ui, Status::Updating, "");
        if c.settled.elapsed() < Duration::from_millis(120) {
            return false;
        }
        c.serial += 1;
        c.cached = None;
        c.failure = None;
        c.worker.submit(Job {
            serial: c.serial,
            scene: Arc::clone(scene),
            width: displayed.width,
            height: displayed.height,
            channels,
        });
        c.requested = Some(Requested {
            scene: Arc::clone(scene),
            stamp: displayed,
            channels,
        });
    } else if let Some(requested) = c.requested.as_mut()
        && !Arc::ptr_eq(&requested.scene, scene)
    {
        // The same optical input under a newer generation (exposure, sample targets) or
        // with newly published facet names: attribution must not need a second trace.
        requested.scene = Arc::clone(scene);
    }
    true
}
/// Publishes the finished pass, or the cached map under a changed presentation.
fn publish_if_ready(
    ui: &MainWindow,
    c: &mut Controller,
    displayed: Displayed,
    generation: u64,
    style_changed: bool,
) {
    let m = ui.global::<OpticalModel>();
    let requested = c.requested.as_ref().map_or(displayed, |r| r.stamp);
    // `tick` only gets here while the overlay is enabled and shown.
    if let Some(done) = c
        .worker
        .take()
        .filter(|r| may_publish(c.serial, r.serial, requested, c.displayed, generation, true))
    {
        match done.result {
            Ok(map) => {
                publish(ui, c, &map, done.refined, requested);
                c.cached = Some(map);
            }
            Err(error) => {
                clear(ui, Status::Unavailable, "");
                let text = format!("Diagnostic Pass: {error}");
                m.set_limitations(text.as_str().into());
                c.failure = Some(text);
            }
        }
    } else if (style_changed || !m.get_ready())
        && let Some(map) = &c.cached
    {
        publish(ui, c, map, map.samples > 1, requested);
    }
}
fn tick(ui: &MainWindow, ctx: &Arc<Mutex<RenderContext>>) {
    STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let Some(c) = state.as_mut() else {
            return;
        };
        let m = ui.global::<OpticalModel>();
        if !m.get_enabled() {
            inactive(ui, c, false);
            return;
        }
        let mut ctx = RenderContext::lock(ctx);
        if ctx.engine.is_ray()
            || !(ctx.tab_visible && ui.global::<ViewportModel>().get_live_view_mode() == 1)
        {
            drop(ctx);
            inactive(ui, c, true);
            return;
        }
        let ready = ready_scene(ui, c, &mut ctx);
        drop(ctx);
        let Some(Ready {
            scene,
            displayed,
            generation,
        }) = ready
        else {
            return;
        };
        let shown = c.failure.as_deref().unwrap_or(&scene.limitations);
        if m.get_limitations().as_str() != shown {
            m.set_limitations(shown.into());
        }
        let style = presentation(ui);
        let channels = Channels {
            fire: style.layers[3],
            scintillation: style.scintillation,
        };
        if !request_if_needed(ui, c, &scene, displayed, channels) {
            return;
        }
        let style_changed = c.style != style;
        c.style = style;
        publish_if_ready(ui, c, displayed, generation, style_changed);
    });
}
/// Installs one bounded worker and a UI-owned polling timer. Dropping the window stops both.
pub fn setup(ui: &MainWindow, ctx: &Arc<Mutex<RenderContext>>) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(Controller {
            worker: Worker::new(),
            serial: 0,
            displayed: None,
            requested: None,
            cached: None,
            last_capture: None,
            width_cache: StoneWidthCache::new(),
            failure: None,
            style: OpticalPresentation::default(),
            settled: Instant::now(),
        });
    });
    let weak = ui.as_weak();
    let ctx = ctx.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(40),
        move || {
            if let Some(ui) = weak.upgrade() {
                tick(&ui, &ctx);
            } else {
                STATE.with(|s| {
                    s.borrow_mut().take();
                });
                TIMER.with(|t| {
                    if let Some(t) = t.borrow().as_ref() {
                        t.stop();
                    }
                });
            }
        },
    );
    TIMER.with(|t| *t.borrow_mut() = Some(timer));
    let refresh_weak = ui.as_weak();
    ui.global::<OpticalModel>().on_refresh(move || {
        STATE.with(|s| {
            if let Some(c) = s.borrow_mut().as_mut() {
                c.reset();
            }
        });
        if let Some(ui) = refresh_weak.upgrade() {
            clear(&ui, Status::Updating, "");
        }
    });
    let invalid_weak = ui.as_weak();
    ui.global::<OpticalModel>().on_invalidate(move || {
        if let Some(ui) = invalid_weak.upgrade() {
            clear(&ui, Status::Updating, "");
        }
    });
    let hover_weak = ui.as_weak();
    ui.global::<OpticalModel>().on_hover(move |u, v| {
        let Some(ui) = hover_weak.upgrade() else {
            return;
        };
        let m = ui.global::<OpticalModel>();
        if !m.get_ready() {
            return;
        }
        STATE.with(|s| {
            let state = s.borrow();
            let Some(c) = state.as_ref() else {
                return;
            };
            let Some(map) = &c.cached else {
                return;
            };
            let scene = c.requested.as_ref().map(|r| &*r.scene);
            m.set_hover_text(hover::hover_text(map, scene, u, v).into());
        });
    });
}
