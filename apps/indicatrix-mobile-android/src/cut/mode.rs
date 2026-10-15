//! Cutting Mode's Rust side: the `CutModel.mode-*` callbacks, the screen-awake request,
//! and the highlighted diagram.
//!
//! The model lives in the core (`indicatrix_mobile_core::cutting::mode`); this file
//! connects it to Slint:
//!
//! - [`open`] starts the mode at a step (`CutModel.mode-open` goes true, the screen is
//!   kept on), [`close`] ends it (the screen may sleep again);
//! - the `mode-next`, `mode-previous`, `mode-jump`, `mode-toggle-done` and `mode-close`
//!   callbacks move through the steps and mark them;
//! - after every move the step's texts are written to `CutModel` and the diagram of the
//!   step is drawn on a `cutting-mode` background queue, latest request wins; the queue
//!   exists only while the mode is open;
//! - [`push`] keeps the page in step with the design, and restarts or closes the mode
//!   when the design changed under it (with a toast);
//! - [`on_lifecycle`] lets the screen sleep while the app is paused.
//!
//! The first [`open`] wires the callbacks if [`wire`] has not been called yet, so the
//! mode works whichever the Cut page calls first.

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

use indicatrix_mobile_core::{
    app_state::{AppState, Lifecycle},
    cutting::mode::{
        self as model, DESIGN_CHANGED_MESSAGE, Staleness,
        diagram::{DiagramPixels, DiagramScene, panel_for},
    },
    tasks::{BackgroundQueue, Generation},
};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, Timer};

use crate::{AppWindow, CutModel, design::Shared};

/// What the toast says when Cutting Mode is asked for without a current solve.
const NOT_SOLVED_MESSAGE: &str = "Solve the design to see cutting instructions";
/// Width and height of the diagram picture in pixels (one square panel).
const DIAGRAM_PIXELS: u32 = 800;
/// How long to wait before looking at the design change again when the state is busy.
const RETRY: Duration = Duration::from_millis(16);

/// What the UI thread keeps between callbacks.
#[derive(Default)]
struct Runtime {
    /// The diagram thread; present only while the mode is open.
    queue: Option<BackgroundQueue>,
    /// Names the newest diagram request; an older one is dropped.
    generation: Generation,
    /// The diagram's fixed part, built once per opening.
    scene: Option<Arc<DiagramScene>>,
    /// Whether building `scene` was tried since the mode opened.
    scene_tried: bool,
    /// The `(step, design generation)` the picture was last requested for.
    requested: Option<(usize, u64)>,
    /// The app state, to revalidate after a design change seen by [`push`].
    state: Weak<RefCell<AppState>>,
    /// Whether a revalidation is already scheduled.
    revalidating: bool,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
    static WIRED: Cell<bool> = const { Cell::new(false) };
}

/// Connects the `CutModel.mode-*` callbacks. Safe to call more than once.
pub fn wire(window: &AppWindow, state: &Shared) {
    if WIRED.replace(true) {
        return;
    }
    RUNTIME.with(|cell| cell.borrow_mut().state = Rc::downgrade(state));
    let cut = window.global::<CutModel>();
    cut.on_mode_next(step_callback(window, state, model::next));
    cut.on_mode_previous(step_callback(window, state, model::previous));
    cut.on_mode_jump({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move |step| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Ok(step) = usize::try_from(step) else {
                return;
            };
            if change(&state, |app| model::jump(app, step)) {
                show(&window, &state);
            }
        }
    });
    cut.on_mode_toggle_done({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if change(&state, |app| model::toggle_done(app).is_some()) {
                show(&window, &state);
                // The Cut page's list and progress card show the marks too.
                if let Ok(app) = state.try_borrow() {
                    super::push(&window, &app);
                }
            }
        }
    });
    cut.on_mode_close({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move || {
            if let Some(window) = weak.upgrade() {
                close(&window, &state);
            }
        }
    });
}

/// A callback that moves with `go` (next or previous) and redraws when it moved.
fn step_callback(
    window: &AppWindow,
    state: &Shared,
    go: fn(&mut AppState) -> bool,
) -> impl Fn() + 'static {
    let (state, weak) = (Rc::clone(state), window.as_weak());
    move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if change(&state, go) {
            show(&window, &state);
        }
    }
}

/// Applies `edit` to the state if it is free. False when it is busy or `edit` says nothing
/// changed.
fn change(state: &Shared, edit: impl FnOnce(&mut AppState) -> bool) -> bool {
    state.try_borrow_mut().is_ok_and(|mut app| edit(&mut app))
}

/// Opens Cutting Mode at `step` (a position in the cutting order; a negative or unknown
/// step starts at the first step not done). Without a current solve it only says so.
pub fn open(window: &AppWindow, state: &Shared, step: i32) {
    wire(window, state);
    let opened = state.try_borrow_mut().is_ok_and(|mut app| {
        let opened = model::open(&mut app, usize::try_from(step).ok()).is_ok();
        if opened {
            app.platform().keep_screen_on(true);
        }
        opened
    });
    let cut = window.global::<CutModel>();
    if !opened {
        cut.set_mode_open(false);
        crate::push::toast::show(window, NOT_SOLVED_MESSAGE);
        return;
    }
    start_runtime();
    cut.set_mode_open(true);
    show(window, state);
}

/// Closes Cutting Mode: the screen may sleep, the diagram thread ends, and the Cut page
/// refreshes its marks.
pub fn close(window: &AppWindow, state: &Shared) {
    if let Ok(mut app) = state.try_borrow_mut() {
        model::close(&mut app);
        app.platform().keep_screen_on(false);
    }
    stop_runtime();
    let cut = window.global::<CutModel>();
    cut.set_mode_open(false);
    cut.set_mode_diagram(Image::default());
    if let Ok(app) = state.try_borrow() {
        super::push(window, &app);
    }
}

/// Ends the diagram thread. Call on the UI thread once the event loop has ended, with
/// the other shutdown steps.
pub fn shutdown() {
    stop_runtime();
    RUNTIME.with(|cell| cell.borrow_mut().state = Weak::new());
}

/// Keeps the page in step with the design. Call from the Cut page's `push`, which runs on
/// every design refresh. Cheap when the mode is closed. A design that changed since the
/// mode opened is dealt with right after this call returns (the state is only borrowed
/// shared here).
pub fn push(window: &AppWindow, state: &AppState) {
    if !model::is_open(state) {
        return;
    }
    if model::is_stale(state) {
        schedule_revalidation(window);
        return;
    }
    write_view(window, state);
}

/// Lets the screen sleep while the app is paused or stopping, and keeps it on again after
/// it resumes, while Cutting Mode is open. Call from the lifecycle handler.
pub fn on_lifecycle(state: &AppState, event: Lifecycle) {
    if !model::is_open(state) {
        return;
    }
    match event {
        Lifecycle::Paused | Lifecycle::Stopping => state.platform().keep_screen_on(false),
        Lifecycle::Resumed => state.platform().keep_screen_on(true),
    }
}

/// Writes the step's texts and asks for its diagram.
fn show(window: &AppWindow, state: &Shared) {
    let Ok(app) = state.try_borrow() else {
        return;
    };
    write_view(window, &app);
    request_diagram(window, &app);
}

/// Writes the `CutModel.mode-*` properties of the current step.
fn write_view(window: &AppWindow, state: &AppState) {
    let Some(view) = model::view(state) else {
        return;
    };
    let cut = window.global::<CutModel>();
    cut.set_mode_step(to_i32(view.step));
    cut.set_mode_step_count(to_i32(view.count));
    cut.set_mode_code(view.code.into());
    cut.set_mode_angle(view.angle_text.into());
    cut.set_mode_index(view.index_text.into());
    cut.set_mode_name(view.name.into());
    cut.set_mode_notes(view.notes.into());
    cut.set_mode_done(view.done);
    cut.set_mode_section(view.section.into());
}

/// `value` as a Slint `int`.
fn to_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Starts the diagram thread and forgets the last opening's scene.
fn start_runtime() {
    RUNTIME.with(|cell| {
        let mut runtime = cell.borrow_mut();
        let _superseded = runtime.generation.advance();
        runtime.queue = Some(BackgroundQueue::new("cutting-mode"));
        runtime.scene = None;
        runtime.scene_tried = false;
        runtime.requested = None;
    });
}

/// Ends the diagram thread (dropping the queue lets a render in flight finish first) and
/// drops the scene. Pending pictures are discarded.
fn stop_runtime() {
    let queue = RUNTIME.with(|cell| {
        let mut runtime = cell.borrow_mut();
        let _superseded = runtime.generation.advance();
        runtime.scene = None;
        runtime.scene_tried = false;
        runtime.requested = None;
        runtime.queue.take()
    });
    // Joined outside the borrow: the thread never touches the runtime, but be safe.
    drop(queue);
}

/// Draws the diagram of the current step on the background queue, unless it was already
/// asked for.
fn request_diagram(window: &AppWindow, state: &AppState) {
    let Some(view) = model::view(state) else {
        return;
    };
    let Some(design_generation) = state.cutting.mode.as_ref().map(|mode| mode.generation) else {
        return;
    };
    let has_scene = RUNTIME.with(|cell| {
        let Ok(mut runtime) = cell.try_borrow_mut() else {
            return true;
        };
        if runtime.requested == Some((view.step, design_generation)) {
            return true;
        }
        runtime.requested = Some((view.step, design_generation));
        if !runtime.scene_tried {
            runtime.scene_tried = true;
            runtime.scene = model::diagram_scene(state).map(Arc::new);
        }
        let (Some(scene), Some(queue)) = (runtime.scene.as_ref(), runtime.queue.as_ref()) else {
            return runtime.queue.is_none();
        };
        let token = runtime.generation.advance();
        let (scene, generation) = (Arc::clone(scene), runtime.generation.clone());
        let (tier, panel) = (view.tier_index, panel_for(view.side));
        let weak = window.as_weak();
        queue.submit(move || {
            if !generation.is_current(token) {
                return;
            }
            let pixels = scene.render(tier, panel, DIAGRAM_PIXELS, DIAGRAM_PIXELS);
            let Some(buffer) = buffer_of(&pixels) else {
                return;
            };
            let _queued = weak.upgrade_in_event_loop(move |window| {
                if token_is_current(token) {
                    window
                        .global::<CutModel>()
                        .set_mode_diagram(Image::from_rgba8(buffer));
                }
            });
        });
        true
    });
    if !has_scene {
        // The stone does not close (or there is no solve): no picture.
        window
            .global::<CutModel>()
            .set_mode_diagram(Image::default());
    }
}

/// The pixels as a Slint buffer, `None` when the sizes do not agree.
fn buffer_of(pixels: &DiagramPixels) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(pixels.width, pixels.height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() != pixels.rgba.len() {
        return None;
    }
    bytes.copy_from_slice(&pixels.rgba);
    Some(buffer)
}

/// Whether `token` names the newest diagram request.
fn token_is_current(token: u64) -> bool {
    RUNTIME.with(|cell| match cell.try_borrow() {
        Ok(runtime) => runtime.generation.is_current(token),
        // Busy: show the picture rather than lose it.
        Err(_) => true,
    })
}

/// Looks at the design change on the next turn of the event loop (the state is borrowed
/// shared by the caller now).
fn schedule_revalidation(window: &AppWindow) {
    let already = RUNTIME.with(|cell| {
        let Ok(mut runtime) = cell.try_borrow_mut() else {
            return true;
        };
        std::mem::replace(&mut runtime.revalidating, true)
    });
    if already {
        return;
    }
    let weak = window.as_weak();
    Timer::single_shot(Duration::ZERO, move || {
        if let Some(window) = weak.upgrade() {
            revalidate(&window);
        }
    });
}

/// Restarts or closes the mode after a design change, and tells the user.
fn revalidate(window: &AppWindow) {
    let state = RUNTIME.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|runtime| runtime.state.upgrade())
    });
    let Some(state) = state else {
        RUNTIME.with(|cell| cell.borrow_mut().revalidating = false);
        return;
    };
    let Ok(mut app) = state.try_borrow_mut() else {
        // Busy right now: try again in a moment.
        let weak = window.as_weak();
        Timer::single_shot(RETRY, move || {
            if let Some(window) = weak.upgrade() {
                revalidate(&window);
            }
        });
        return;
    };
    let outcome = model::restart_if_stale(&mut app);
    drop(app);
    RUNTIME.with(|cell| cell.borrow_mut().revalidating = false);
    match outcome {
        Staleness::Fresh => {}
        Staleness::Restarted => {
            RUNTIME.with(|cell| {
                let mut runtime = cell.borrow_mut();
                runtime.scene = None;
                runtime.scene_tried = false;
                runtime.requested = None;
            });
            crate::push::toast::show(window, DESIGN_CHANGED_MESSAGE);
            show(window, &state);
        }
        Staleness::Closed => {
            close(window, &state);
            crate::push::toast::show(window, DESIGN_CHANGED_MESSAGE);
        }
    }
}
