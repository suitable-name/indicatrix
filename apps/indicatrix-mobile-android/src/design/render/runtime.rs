//! The Render view's timers and the handle they use on the app state.
//!
//! - The **poll** runs while the Render mode is chosen: every 100 ms while it is on show
//!   (Design destination, a design, a viewport size), every 500 ms otherwise. Each tick
//!   brings the trace in line with the state (`render_view::maintain`: start, restart,
//!   stop) and moves the newest frame from the engine onto `RenderModel`. Leaving the
//!   Design destination therefore stops the trace within half a second.
//! - The **kick** is a single-shot timer that runs the same maintenance right after a
//!   change (a camera move, a setting) instead of waiting for the next tick. While a
//!   finger is down the core's gate answers "wait", and the kick is re-armed for the rest
//!   of the 150 ms, so a drag restarts the trace at most about seven times a second.
//! - The **governor** runs every 10 s while a trace is active and applies the thermal and
//!   battery rules to the worker pool.
//!
//! No timer closure holds the app state: they capture a `slint::Weak` and find the state
//! through [`RUNTIME`], which [`shutdown`] empties (the state must be dropped while the
//! engine's threads exist, not by the thread-local teardown).

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use indicatrix_mobile_core::{
    app_state::AppState,
    viewport::render_view::{self, Maintain},
};
use slint::{ComponentHandle, Timer, TimerMode, Weak};

use super::model;
use crate::{AppWindow, design::Shared};

/// The poll while the Render mode is on show.
const POLL_ACTIVE: Duration = Duration::from_millis(100);
/// The poll while the mode is chosen but not on show.
const POLL_IDLE: Duration = Duration::from_millis(500);
/// How often the governor looks at the thermal headroom.
const GOVERN_INTERVAL: Duration = Duration::from_secs(10);
/// How long a kick waits when the state is borrowed at the moment it fires.
const RETRY: Duration = Duration::from_millis(16);

/// What the timers need besides themselves.
struct Runtime {
    shared: Shared,
    window: Weak<AppWindow>,
}

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
    /// The frame poll and maintenance tick.
    static POLL: Timer = Timer::default();
    /// The deferred maintenance after a change.
    static KICK: Timer = Timer::default();
    /// The thermal and battery governor.
    static GOVERN: Timer = Timer::default();
    /// The interval the poll runs at, in ms (0 when it is stopped).
    static POLL_MS: Cell<u64> = const { Cell::new(0) };
}

/// Remembers the window and the state for the timers. Call once on the UI thread.
pub(super) fn install(window: &AppWindow, state: &Shared) {
    RUNTIME.with(|cell| {
        *cell.borrow_mut() = Some(Runtime {
            shared: Rc::clone(state),
            window: window.as_weak(),
        });
    });
}

/// Stops every timer and forgets the state and the window.
pub(super) fn shutdown() {
    POLL.with(Timer::stop);
    KICK.with(Timer::stop);
    GOVERN.with(Timer::stop);
    POLL_MS.with(|ms| ms.set(0));
    let runtime = RUNTIME.with(|cell| cell.borrow_mut().take());
    drop(runtime);
}

/// The window and the state, on the UI thread, while the view is installed.
pub(super) fn context() -> Option<(AppWindow, Shared)> {
    RUNTIME.with(|cell| {
        let guard = cell.try_borrow().ok()?;
        let runtime = guard.as_ref()?;
        Some((runtime.window.upgrade()?, Rc::clone(&runtime.shared)))
    })
}

/// The time since the app started, the clock the restart gate runs on.
fn now() -> Duration {
    crate::design::solve::clock()
}

/// The viewport (camera, size) may have changed: have the trace follow it right away.
pub fn on_viewport_changed(_window: &AppWindow, state: &AppState) {
    if state.viewport.mode.is_render() || state.viewport.driver.is_active() {
        schedule_kick(Duration::ZERO);
    }
}

/// Makes the timers follow the Render mode: the poll runs while the mode is chosen, and a
/// trace that is still active when the mode is left is stopped by a kick.
pub(super) fn sync(_window: &AppWindow, state: &AppState) {
    if state.viewport.mode.is_render() {
        set_poll_rate(render_view::wants(state));
        schedule_kick(Duration::ZERO);
    } else {
        stop_poll();
        if state.viewport.driver.is_active() {
            schedule_kick(Duration::ZERO);
        }
    }
}

/// Runs the maintenance once, `delay` from now (replacing a kick already waiting).
fn schedule_kick(delay: Duration) {
    KICK.with(|timer| {
        timer.start(TimerMode::SingleShot, delay, || {
            if let Some((window, shared)) = context() {
                maintain(&window, &shared);
            }
        });
    });
}

/// Starts, restarts or stops the trace as the state asks, then pushes the overlay.
pub(super) fn maintain(window: &AppWindow, shared: &Shared) {
    let outcome = {
        let Ok(mut app) = shared.try_borrow_mut() else {
            // The state is busy (an edit is being applied): try again in a moment.
            schedule_kick(RETRY);
            return;
        };
        render_view::maintain(&mut app, now())
    };
    match outcome {
        Maintain::Wait(delay) => schedule_kick(delay),
        Maintain::Started => start_governor(),
        Maintain::Stopped => GOVERN.with(Timer::stop),
        Maintain::Idle | Maintain::Blocked(_) => {}
    }
    if let Ok(app) = shared.try_borrow() {
        model::push(window, &app);
    }
}

/// Runs the poll at the rate that fits `on_show`, restarting its timer only on a change.
fn set_poll_rate(on_show: bool) {
    let interval = if on_show { POLL_ACTIVE } else { POLL_IDLE };
    let millis = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
    if POLL_MS.with(Cell::get) == millis {
        return;
    }
    POLL_MS.with(|ms| ms.set(millis));
    POLL.with(|timer| timer.start(TimerMode::Repeated, interval, tick));
}

/// Stops the poll.
fn stop_poll() {
    POLL.with(Timer::stop);
    POLL_MS.with(|ms| ms.set(0));
}

/// One poll: maintenance, then the newest frame.
fn tick() {
    let Some((window, shared)) = context() else {
        return;
    };
    maintain(&window, &shared);
    let (frame, on_show, render) = {
        let Ok(mut app) = shared.try_borrow_mut() else {
            return;
        };
        let on_show = render_view::wants(&app);
        let frame = if on_show {
            app.render.take_frame()
        } else {
            None
        };
        (frame, on_show, app.viewport.mode.is_render())
    };
    if let Some(frame) = frame {
        model::show_frame(&window, &frame);
    }
    if render {
        set_poll_rate(on_show);
    } else {
        stop_poll();
    }
}

/// Starts the 10 s governor if it is not running.
fn start_governor() {
    GOVERN.with(|timer| {
        if !timer.running() {
            timer.start(TimerMode::Repeated, GOVERN_INTERVAL, govern);
        }
    });
}

/// One governor step: the platform's thermal headroom and the battery saver setting go to
/// the worker pool.
fn govern() {
    let Some((_window, shared)) = context() else {
        return;
    };
    let platform = Arc::clone(shared.borrow().platform());
    // The platform call may take a moment (JNI); the state is not borrowed meanwhile.
    let headroom = platform.thermal_headroom();
    let Ok(mut app) = shared.try_borrow_mut() else {
        return;
    };
    let battery_saver = app.settings.battery_saver;
    app.render.govern(headroom, battery_saver);
}
