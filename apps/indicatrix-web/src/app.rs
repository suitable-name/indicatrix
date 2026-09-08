//! Wires `ui/app.slint`'s callbacks to `scene`/`render`, and the wasm entry point
//! itself.
//!
//! # Why `Rc<RefCell<..>>` rather than `Arc<Mutex<..>>`
//!
//! `wasm32-unknown-unknown` (without the `atomics` target feature) is single-threaded:
//! every callback below, and every `.await` point inside [`render_loop`], runs on the
//! same and only "thread" [`slint::spawn_local`] has. `Rc`/`RefCell` are both correct
//! and cheaper than their atomic counterparts -- no second thread could ever contend
//! for these borrows, only a single task that might harmlessly re-enter itself (see
//! [`RenderCoordinator`]).

// Every `async fn` below captures an `Rc<RefCell<..>>` and so is checked `!Send`
// correctly, but there is no second thread on wasm32 a future could be sent to. One
// module-level allow rather than one per function.
#![allow(
    clippy::future_not_send,
    reason = "every async fn here captures Rc<RefCell<..>>, correctly !Send, but \
              wasm32-unknown-unknown has no second thread to send a future to regardless"
)]

use std::{cell::RefCell, rc::Rc, time::Duration};

use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, Weak};
use wasm_bindgen::prelude::*;

use crate::{
    AppWindow,
    render::{Accumulator, CHUNK_SPP, GpuState, RenderError, TARGET_SPP, accumulate_chunk},
    scene::{StoneGeometry, ViewState, clamp_render_dims, material_for_index, parse_uploaded_asc},
};

/// How long [`wire_callbacks`]'s render-surface resize handler waits after the last
/// `render-size-changed` event before applying a new render-target size and (via
/// [`request_render`]) resetting accumulation.
///
/// A drag-resize fires dozens of intermediate size events per gesture; feeding each
/// straight into [`crate::render::Accumulator::reset`] would restart a full
/// [`TARGET_SPP`]-sample pass on every tick, so the image would never progress.
/// [`RenderCoordinator`]'s existing coalescing only handles in-flight renders, not a
/// burst of nominally-distinct resize events, so this needs a real debounce timer to
/// collapse the whole burst down to the one size the drag actually settled on.
///
/// `150ms` is a reasoned middle ground: short enough the render catches up almost
/// immediately once dragging stops, long enough a dragged window edge never lets two
/// distinct sizes both survive to trigger a reset.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

/// The two pieces of state [`render_loop`] mutates across `.await` points --
/// bundled so exactly one `std::mem::take`/put-back pair (see that function's own
/// comment) covers both, instead of one per field.
#[derive(Default)]
struct RenderResources {
    gpu: GpuState,
    accum: Accumulator,
}

/// Everything a render needs that isn't recomputed from `ui/app.slint`'s own
/// properties: the parsed stone (`None` until a file loads) and the accumulation
/// state (device + running sum). Held behind one `Rc<RefCell<..>>` shared by every
/// callback and by [`render_loop`] -- see this module's doc comment.
struct AppState {
    stone: Option<StoneGeometry>,
    view: ViewState,
    resources: RenderResources,
}

/// Coalesces render requests: a burst of `camera-changed` callbacks (one per `moved`
/// event while dragging -- see `ui/app.slint`) must not spawn one overlapping
/// `render_loop` task per event, and must not let a stale accumulation keep running
/// once a newer one has something different to show. Only two states matter: whether
/// an accumulation pass is currently running, and whether a NEWER scene has arrived
/// since the running one started.
///
/// [`request_render`] sets `dirty` and starts [`render_loop`] only if nothing is
/// already running. [`render_loop`] clears `dirty` right before it resets the
/// accumulator and starts a fresh pass, and re-checks `dirty` before every chunk it
/// dispatches -- becoming `true` again aborts the in-flight pass and restarts from a
/// fresh accumulator rather than letting a stale pass finish or blending old and new
/// samples together (see [`Accumulator::reset`] for why that blend is a correctness
/// bug, not just a visual one).
#[derive(Default)]
struct RenderCoordinator {
    running: bool,
    dirty: bool,
    /// User-requested pause. Checked before every chunk dispatch, so pausing takes
    /// effect within one chunk rather than at the end of a full 256-spp pass.
    /// Deliberately NOT implemented by resetting: that would throw away up to
    /// `TARGET_SPP` samples of real tracing every time the button was pressed. The
    /// accumulator is left exactly as it is; `resume` below re-enters the same pass.
    paused: bool,
    /// Set by [`resume_render`] to re-enter accumulation WITHOUT the reset a `dirty`
    /// pass performs -- the one distinction between "the scene changed, start over"
    /// and "carry on where we left off".
    resume: bool,
}

/// Marks the shared state dirty and ensures exactly one [`render_loop`] task is (or
/// becomes) running to service it.
fn request_render(
    app: Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    coord: Rc<RefCell<RenderCoordinator>>,
) {
    let already_running = {
        let mut c = coord.borrow_mut();
        c.dirty = true;
        std::mem::replace(&mut c.running, true)
    };
    if already_running {
        return;
    }
    wasm_bindgen_futures::spawn_local(render_loop(app, state, coord));
}

/// Clears the pause and re-enters accumulation WITHOUT resetting, so the samples
/// traced before the pause are kept and the pass carries on from where it stopped.
///
/// Contrast [`request_render`], which sets `dirty` and therefore restarts from zero --
/// correct when the scene changed, wrong when the user merely un-paused.
fn resume_render(
    app: Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    coord: Rc<RefCell<RenderCoordinator>>,
) {
    let already_running = {
        let mut c = coord.borrow_mut();
        c.paused = false;
        c.resume = true;
        std::mem::replace(&mut c.running, true)
    };
    if already_running {
        return;
    }
    wasm_bindgen_futures::spawn_local(render_loop(app, state, coord));
}

/// `"128 / 256 spp"` -- the one place that string is built, so `ui/app.slint`'s
/// `progress-text` property (see its own doc comment) never has to reconstruct it
/// from separate numbers and risk drifting from [`TARGET_SPP`].
fn progress_text(done: u32) -> String {
    format!("{done} / {TARGET_SPP} spp")
}

/// Applies one chunk's outcome to the window: repaint and progress on success, or a
/// worded, first-class error on either decline. Returns `false` when the pass must
/// stop (both error arms). Both error arms deliberately leave whatever image is
/// already on screen rather than clearing it: a partial accumulation is still a real,
/// if noisier, picture of the stone.
fn apply_chunk_result(
    handle: &AppWindow,
    result: Result<crate::render::ChunkOutcome, RenderError>,
    accum: &Accumulator,
) -> bool {
    match result {
        Ok(outcome) => {
            // Sized off the accumulator itself, the one value guaranteed to match
            // `buffer`'s (and therefore `to_rgba`'s output) actual dimensions.
            let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(accum.width(), accum.height());
            buffer.make_mut_bytes().copy_from_slice(&accum.to_rgba());
            handle.set_stone_image(Image::from_rgba8(buffer));
            handle.set_backend_label(outcome.backend_label.into());
            handle.set_progress_text(progress_text(accum.samples()).into());
        }
        // This build is WebGPU-only: a decline here means the browser tab genuinely
        // cannot render, not that a CPU path will pick it up a moment later. Both arms
        // set `has-error` and stop this pass -- the outer loop's next iteration exits
        // immediately since `dirty` is false, leaving the error message on screen
        // until a new scene change tries again.
        Err(RenderError::NoWebGpu(detail)) => {
            handle.set_status_text(
                format!(
                    "This browser has no usable WebGPU, which this build \
                     requires -- there is no CPU fallback. Try a recent \
                     Chrome, Edge, or Firefox with WebGPU enabled. ({detail})"
                )
                .into(),
            );
            handle.set_has_error(true);
            handle.set_backend_label("WebGPU unavailable".into());
            return false;
        }
        Err(RenderError::Declined(detail)) => {
            handle.set_status_text(format!("The GPU declined this scene: {detail}").into());
            handle.set_has_error(true);
            handle.set_backend_label("WebGPU: declined".into());
            return false;
        }
    }
    true
}

/// Runs accumulation passes until no further scene change is pending -- see
/// [`RenderCoordinator`] for exactly what "pending" means and how a change mid-pass is
/// handled.
///
/// One call to this function owns `state.resources` (the WebGPU device and the
/// running sample sum) for as long as it runs, taking it out via `std::mem::take` at
/// the top and putting it back only when about to return -- never through a `RefCell`
/// borrow held across the `.await` inside [`accumulate_chunk`] (a live borrow spanning
/// an `.await` is exactly what turns "safe because only one task ever runs" into
/// "silently panics" the moment that invariant stops holding).
async fn render_loop(
    app: Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    coord: Rc<RefCell<RenderCoordinator>>,
) {
    let mut resources = std::mem::take(&mut state.borrow_mut().resources);

    'pass: loop {
        // Whether this iteration starts a FRESH pass (reset the accumulator) or
        // CONTINUES a paused one. Getting this wrong is a real bug: resetting on
        // resume silently discards work, and not resetting on a scene change blends
        // two scenes' radiance into one buffer.
        let fresh_pass = {
            let mut c = coord.borrow_mut();
            if c.paused {
                c.running = false;
                break 'pass;
            }
            if c.dirty {
                c.dirty = false;
                c.resume = false;
                true
            } else if c.resume {
                c.resume = false;
                false
            } else {
                c.running = false;
                break 'pass;
            }
        };

        let Some((planes, material, view, file_name)) = ({
            let s = state.borrow();
            s.stone.as_ref().map(|stone| {
                (
                    stone.planes.clone(),
                    material_for_index(s.view.material_index),
                    s.view.clone(),
                    stone.file_name.clone(),
                )
            })
        }) else {
            // No file loaded yet -- a control was touched before any `.asc` was
            // chosen. Nothing to render; drop the request rather than loop forever.
            coord.borrow_mut().running = false;
            break 'pass;
        };

        // A new pass: the camera/material/lighting/file/size this accumulator's
        // existing samples (if any) were traced against no longer matches
        // `view`/`material` -- see `Accumulator::reset`. `view.render_width`/
        // `render_height` were already clamped by `scene::clamp_render_dims` when
        // `wire_callbacks`'s (debounced) resize handler wrote them.
        if fresh_pass {
            resources.accum.reset(view.render_width, view.render_height);
        }

        if let Some(handle) = app.upgrade() {
            handle.set_busy(true);
            handle.set_has_error(false);
            handle.set_status_text(format!("Loaded {file_name}.").into());
            handle.set_progress_text(progress_text(resources.accum.samples()).into());
        }

        loop {
            // Re-checked before every chunk (not just once per pass): a scene change
            // that arrives mid-accumulation must abort this pass immediately rather
            // than finish dispatching a chunk against the now-stale `planes`/
            // `material`/`view` this loop already snapshotted.
            if coord.borrow().dirty {
                continue 'pass;
            }

            if coord.borrow().paused {
                break;
            }

            let remaining = TARGET_SPP.saturating_sub(resources.accum.samples());
            if remaining == 0 {
                break;
            }
            let chunk_spp = CHUNK_SPP.min(remaining);

            let result = accumulate_chunk(
                &mut resources.gpu,
                &planes,
                &material,
                &view,
                &mut resources.accum,
                chunk_spp,
            )
            .await;

            let Some(handle) = app.upgrade() else {
                // The window was dropped mid-render (page navigating away). Nothing
                // left to update, and no reason to keep looping.
                state.borrow_mut().resources = resources;
                return;
            };
            if !apply_chunk_result(&handle, result, &resources.accum) {
                break;
            }
        }

        if let Some(handle) = app.upgrade() {
            handle.set_busy(false);
        }
    }

    state.borrow_mut().resources = resources;
}

/// Opens the browser's file picker, reads and parses whatever `.asc` file the user
/// chooses, and updates the UI and shared state accordingly -- success or failure.
///
/// `rfd::AsyncFileDialog` is used rather than the blocking `rfd::FileDialog`
/// `apps/indicatrix-cut` calls natively: there is no OS thread here for a blocking
/// dialog call to run on without freezing the tab, and `rfd`'s wasm32 backend only
/// offers the async form regardless (see this crate's `Cargo.toml` for exactly how it
/// implements that on this target).
async fn choose_and_load_file(
    app: Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    coord: Rc<RefCell<RenderCoordinator>>,
) {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .add_filter("GemCAD cutting schedule", &["asc"])
        .pick_file()
        .await
    else {
        // User dismissed the picker -- not an error, nothing to report.
        return;
    };

    let file_name = handle.file_name();
    let bytes = handle.read().await;

    match parse_uploaded_asc(&file_name, &bytes) {
        Ok(geometry) => {
            if let Some(w) = app.upgrade() {
                w.set_status_text(format!("Loaded {file_name}.").into());
                w.set_has_error(false);
                w.set_has_stone(true);
            }
            state.borrow_mut().stone = Some(geometry);
            request_render(app, state, coord);
        }
        Err(message) => {
            if let Some(w) = app.upgrade() {
                w.set_status_text(message.into());
                w.set_has_error(true);
                w.set_has_stone(false);
            }
        }
    }
}

/// Registers every `ui/app.slint` callback against `state`/`coord`.
///
/// `resize_timer` is threaded in from `main` (rather than created here) for the same
/// reason `state`/`coord` are: the render-size-changed handler below restarts it on
/// every event (see [`RESIZE_DEBOUNCE`]'s doc comment), which requires the SAME
/// `Timer` instance across every call, not a fresh one per registration.
fn wire_callbacks(
    app: &AppWindow,
    state: &Rc<RefCell<AppState>>,
    coord: &Rc<RefCell<RenderCoordinator>>,
    resize_timer: &Rc<Timer>,
) {
    let weak = app.as_weak();

    {
        let weak = weak.clone();
        let state = state.clone();
        let coord = coord.clone();
        app.on_choose_file(move || {
            wasm_bindgen_futures::spawn_local(choose_and_load_file(
                weak.clone(),
                state.clone(),
                coord.clone(),
            ));
        });
    }
    {
        let weak = weak.clone();
        let state = state.clone();
        let coord = coord.clone();
        app.on_camera_changed(move |yaw, pitch, distance| {
            {
                let mut s = state.borrow_mut();
                s.view.yaw = yaw;
                s.view.pitch = pitch;
                s.view.distance = distance;
            }
            request_render(weak.clone(), state.clone(), coord.clone());
        });
    }
    {
        let weak = weak.clone();
        let state = state.clone();
        let coord = coord.clone();
        app.on_material_changed(move |index| {
            state.borrow_mut().view.material_index = index;
            request_render(weak.clone(), state.clone(), coord.clone());
        });
    }
    {
        let weak = weak.clone();
        let state = state.clone();
        let coord = coord.clone();
        app.on_lighting_changed(move |index| {
            state.borrow_mut().view.lighting_index = index;
            request_render(weak.clone(), state.clone(), coord.clone());
        });
    }
    {
        let state = state.clone();
        let coord = coord.clone();
        let weak_pause = weak.clone();
        app.on_toggle_pause(move || {
            let Some(handle) = weak_pause.upgrade() else {
                return;
            };
            let now_paused = !handle.get_paused();
            handle.set_paused(now_paused);
            if now_paused {
                // Only raise the flag. `render_loop` checks it before its next chunk
                // and parks itself there, which keeps the accumulator intact -- see
                // `RenderCoordinator::paused`'s doc comment for why pausing must never
                // reset. Nothing is torn down, so resuming is genuinely a continuation.
                coord.borrow_mut().paused = true;
            } else {
                // `resume_render`, NOT `request_render`: the latter sets `dirty`, which
                // makes the next pass reset and re-trace from zero -- correct for a
                // scene change, but it would silently discard everything accumulated
                // before the pause.
                resume_render(weak_pause.clone(), state.clone(), coord.clone());
            }
        });
    }
    {
        let state = state.clone();
        let coord = coord.clone();
        app.on_exposure_changed(move |exposure| {
            state.borrow_mut().view.exposure = exposure;
            request_render(weak.clone(), state.clone(), coord.clone());
        });
    }
    // `weak` is moved into the closure above rather than cloned: it's this function's
    // final use of the outer binding (the resize handler below gets its OWN `Weak`
    // from `app.as_weak()`).
    //
    // Factored into its own function: the debounce logic below (a nested closure
    // creating a further-nested closure) is enough machinery on its own that inlining
    // it here would make `wire_callbacks` hard to scan.
    wire_render_size_changed(app, state, coord, resize_timer);
}

/// Registers `ui/app.slint`'s `render-size-changed` callback: DPR-aware (via
/// [`clamp_render_dims`]) and debounced (see [`RESIZE_DEBOUNCE`]'s doc comment)
/// before it ever reaches [`request_render`]/[`Accumulator::reset`].
fn wire_render_size_changed(
    app: &AppWindow,
    state: &Rc<RefCell<AppState>>,
    coord: &Rc<RefCell<RenderCoordinator>>,
    resize_timer: &Rc<Timer>,
) {
    let weak = app.as_weak();
    let state = state.clone();
    let coord = coord.clone();
    let resize_timer = resize_timer.clone();
    app.on_render_size_changed(move |logical_width, logical_height| {
        // Fresh clones on every call, not just once at registration: this outer
        // closure is `FnMut` and fires once per `changed width`/`changed height`
        // event, potentially dozens of times during one drag-resize.
        let weak = weak.clone();
        let state = state.clone();
        let coord = coord.clone();

        // Restarting the SAME `Timer` on every event, not spawning a new one, turns a
        // whole burst of resize events into a single deferred action: `Timer::start`
        // cancels whatever this timer was previously waiting to run, so only the LAST
        // event within any `RESIZE_DEBOUNCE` window ever fires.
        resize_timer.start(TimerMode::SingleShot, RESIZE_DEBOUNCE, move || {
            let Some(handle) = weak.upgrade() else {
                // The window is gone before the debounce settled -- page navigating
                // away mid-resize. Nothing left to resize.
                return;
            };
            // Read fresh here, at the moment the resize settles: a page can move
            // between monitors with different device pixel ratios with no
            // `render-size-changed` event of its own.
            let scale_factor = handle.window().scale_factor();
            let (width, height) = clamp_render_dims(logical_width, logical_height, scale_factor);

            let changed = {
                let mut s = state.borrow_mut();
                let changed = s.view.render_width != width || s.view.render_height != height;
                s.view.render_width = width;
                s.view.render_height = height;
                changed
            };
            // A no-op guard: `changed width`/`changed height` can fire for a
            // logical-pixel delta that rounds to the same physical size once DPR
            // scaling and clamping are applied. Calling `request_render`
            // unconditionally would restart accumulation for an unchanged target.
            if changed {
                request_render(weak.clone(), state.clone(), coord.clone());
            }
        });
    });
}

/// The browser viewport, in CSS pixels, as a Slint logical size.
///
/// Falls back to the root window's own `preferred-width`/`preferred-height` if either
/// query fails -- the same size Slint would have picked unaided, so a failure here
/// degrades to the previous behaviour rather than to a zero-sized window.
#[cfg(target_arch = "wasm32")]
fn viewport_logical_size() -> slint::LogicalSize {
    let (mut w, mut h) = (980.0_f64, 620.0_f64);
    if let Some(win) = web_sys::window() {
        // `document.documentElement.clientWidth/Height` first, `window.innerWidth/
        // Height` second. They usually agree; where they differ, the former excludes
        // scrollbar gutters. Both are tried since both can legitimately read 0 --
        // before first layout, and (measured) inside an offscreen/headless embedding.
        // A 0 is rejected rather than trusted: the fallback below just reproduces the
        // pre-viewport-tracking size until the first real `resize` event corrects it.
        let doc_el = win.document().and_then(|d| d.document_element());
        let from_doc = doc_el.map(|e| (f64::from(e.client_width()), f64::from(e.client_height())));
        let from_win = || {
            let iw = win.inner_width().ok().and_then(|v| v.as_f64());
            let ih = win.inner_height().ok().and_then(|v| v.as_f64());
            iw.zip(ih)
        };
        if let Some((dw, dh)) = from_doc.filter(|(dw, dh)| *dw > 0.0 && *dh > 0.0) {
            w = dw;
            h = dh;
        } else if let Some((iw, ih)) = from_win().filter(|(iw, ih)| *iw > 0.0 && *ih > 0.0) {
            w = iw;
            h = ih;
        }
    }
    slint::LogicalSize::new(w as f32, h as f32)
}

/// Sizes the Slint window to the browser viewport, now and on every browser resize.
///
/// # Why this is required, and why the responsive layout is inert without it
///
/// Slint's winit backend writes an INLINE `width`/`height` style onto the `<canvas>` it
/// binds to, derived from the window's size. Left alone, that size is the root
/// element's `preferred-width`/`preferred-height` -- so the canvas is pinned at a fixed
/// 980x620 forever, overriding whatever `index.html`'s stylesheet asks for (measured
/// directly against the built page before this existed: `canvas.style` read `980px` /
/// `620px` at every viewport size tried).
///
/// That makes `AppWindow`'s two layout branches (side-by-side vs. stacked) and the
/// `render-size-changed` debounce unreachable in practice: they react to the SLINT
/// window's size, which without this never changes. Driving `set_size` from the real
/// viewport is what connects them to the browser.
///
/// The `Closure` is deliberately leaked with `forget()`: it must outlive this function
/// and stay callable for the whole life of the page, and there is no later point at
/// which a browser tab hands this module a chance to drop it.
#[cfg(target_arch = "wasm32")]
fn track_browser_viewport(app: &AppWindow) {
    // Deferred to the first event-loop turn, not applied inline here: this function
    // necessarily runs before `AppWindow::run()`, and on web the winit window does not
    // exist until `run()` starts the event loop. A `set_size` issued now would land on
    // a window that doesn't exist yet, reproducing the pinned-980x620 symptom this is
    // meant to fix. A zero-duration single-shot timer fires on the first loop turn, by
    // which point the window genuinely exists.
    let weak_initial = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(app) = weak_initial.upgrade() {
            app.window().set_size(viewport_logical_size());
        }
    });

    let weak = app.as_weak();
    // `Closure::wrap(Box::new(..))`, not `Closure::new(..)`: the latter builds a
    // lifetime-scoped closure that cannot be handed to JS as a long-lived `onresize`
    // handler. `wrap` produces the owned form that `forget()` below can leak.
    let on_resize = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
        if let Some(app) = weak.upgrade() {
            app.window().set_size(viewport_logical_size());
        }
    }) as Box<dyn FnMut()>);
    if let Some(win) = web_sys::window() {
        win.set_onresize(Some(wasm_bindgen::JsCast::unchecked_ref(
            on_resize.as_ref(),
        )));
    }
    on_resize.forget();
}

/// The wasm entry point. `#[wasm_bindgen(start)]` runs this once, automatically, as
/// soon as the module the browser fetched has finished instantiating -- `index.html`
/// calls no exported function of its own to kick things off.
#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
    // Without this, a Rust panic surfaces in the browser console as an opaque
    // "unreachable executed" WebAssembly trap -- no message, no file, no line. This
    // hook replaces that with the actual panic message via `console.error`.
    console_error_panic_hook::set_once();

    let app = AppWindow::new().map_err(|e| JsValue::from_str(&e.to_string()))?;

    let state = Rc::new(RefCell::new(AppState {
        stone: None,
        view: ViewState::default(),
        resources: RenderResources::default(),
    }));
    let coord = Rc::new(RefCell::new(RenderCoordinator::default()));
    // One `Timer` for the lifetime of the page, shared (via `Rc`, not recreated) by
    // every `render-size-changed` event -- reusing the SAME instance is what makes the
    // debounce actually debounce (see `RESIZE_DEBOUNCE`).
    let resize_timer = Rc::new(Timer::default());

    wire_callbacks(&app, &state, &coord, &resize_timer);

    // Must run AFTER `wire_callbacks`: sizing the window fires `render-size-changed`,
    // and that callback has to already be wired for the first size to reach the
    // renderer rather than being dropped on the floor.
    #[cfg(target_arch = "wasm32")]
    track_browser_viewport(&app);

    // On `wasm32`, `run()` hands control to the browser's own event loop and does not
    // return in practice. The `Result` return type stays meaningful anyway: a
    // `PlatformError` here means the event loop could not even start (no
    // `<canvas>`/WebGL context available), worth surfacing to `console.error`.
    app.run().map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(())
}
