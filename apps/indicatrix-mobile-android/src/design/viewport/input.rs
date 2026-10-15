//! Slint to the core: the `ViewportModel` callbacks.
//!
//! Every callback takes the raw event, builds an [`Env`] from the shared session and
//! selection, hands both to the core and acts on the [`Outcome`] it returns.

use std::{cell::RefCell, time::Duration};

use indicatrix_editor::manipulate::DiscardReason;
use indicatrix_mobile_core::{
    app_state::AppState,
    layout::WindowClass,
    viewport::{
        Env, Outcome, Pinch, Pointer, ViewMode, ViewportState, gesture::Button, render_view,
    },
};
use slint::{ComponentHandle, Timer, TimerMode};

use super::{model, runtime};
use crate::{AppWindow, ViewportModel, design::Shared};

/// How long a finger must rest before the long-press card opens.
const LONG_PRESS: Duration = Duration::from_millis(450);

/// How often a pending Slice cut is checked for having gone stale.
const SLICE_POLL: Duration = Duration::from_millis(150);

thread_local! {
    static LONG_PRESS_TIMER: Timer = Timer::default();
    static SLICE_TIMER: Timer = Timer::default();
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Connects every `ViewportModel` callback.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    WINDOW.with(|cell| *cell.borrow_mut() = Some(window.as_weak()));
    wire_pointer(window, state);
    wire_view(window, state);
    wire_slice(window, state);
}

/// Connects the resize, pointer, wheel and pinch callbacks.
fn wire_pointer(window: &AppWindow, state: &Shared) {
    let model = window.global::<ViewportModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_view_resized(move |width, height| {
        let Some(window) = weak.upgrade() else { return };
        let changed = {
            let Ok(mut app) = shared.try_borrow_mut() else {
                return;
            };
            let compact = app.window_class == WindowClass::Compact;
            let scale = window.window().scale_factor();
            app.viewport.set_view(width, height, scale, compact)
        };
        if changed {
            super::push(&window, &shared.borrow());
        }
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_pointer(move |kind, button, finger, x, y| {
        let Some(window) = weak.upgrade() else { return };
        note_pointer(&shared, kind);
        if light_drag(&window, &shared, kind, x, y) {
            return;
        }
        let event = match kind {
            0 => Pointer::Down {
                finger,
                button: match button {
                    0 => Button::Left,
                    1 => Button::Right,
                    _ => Button::Other,
                },
                x,
                y,
            },
            1 => Pointer::Up { finger, x, y },
            2 => Pointer::Move { x, y },
            _ => Pointer::Cancel,
        };
        match kind {
            0 => arm_long_press(),
            1 | 3 => LONG_PRESS_TIMER.with(Timer::stop),
            _ => {}
        }
        act(&window, &shared, |viewport, env| {
            viewport.pointer(event, env)
        });
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_scrolled(move |delta_y, x, y| {
        let Some(window) = weak.upgrade() else { return };
        act(&window, &shared, |viewport, env| {
            viewport.wheel(delta_y, x, y, env)
        });
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_pinch(move |phase, scale, x, y| {
        let Some(window) = weak.upgrade() else { return };
        let event = match phase {
            0 => Pinch::Begin { x, y },
            1 => Pinch::Update { scale, x, y },
            _ => Pinch::End,
        };
        if phase == 0 {
            // A second finger makes it a pinch, never a long press.
            LONG_PRESS_TIMER.with(Timer::stop);
        }
        if let Ok(mut app) = shared.try_borrow_mut() {
            app.viewport.touch.set_pinch(phase < 2);
        }
        act(&window, &shared, |viewport, env| viewport.pinch(event, env));
    });
}

/// Connects the view-mode, camera and handle callbacks.
fn wire_view(window: &AppWindow, state: &Shared) {
    let model = window.global::<ViewportModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_view_mode_chosen(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let changed = shared
            .try_borrow_mut()
            .is_ok_and(|mut app| app.viewport.set_mode(ViewMode::from_index(index)));
        if changed {
            // The Render view starts or stops its trace with the mode: refresh every
            // surface (the render view's push is one of them).
            crate::design::refresh(&window, &shared);
        }
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_camera_chosen(move |kind| {
        let Some(window) = weak.upgrade() else { return };
        if let Ok(mut app) = shared.try_borrow_mut() {
            app.viewport.set_standard_view(kind);
        }
        super::push(&window, &shared.borrow());
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_handles_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        if let Ok(mut app) = shared.try_borrow_mut() {
            let on = !app.viewport.handles_on;
            app.viewport.set_handles_on(on);
        }
        super::push(&window, &shared.borrow());
    });
}

/// Connects the Slice callbacks and the info card.
fn wire_slice(window: &AppWindow, state: &Shared) {
    let model = window.global::<ViewportModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_slice_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        act(&window, &shared, |viewport, _| {
            let on = !viewport.slice.mode;
            viewport.set_slice_mode(on)
        });
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_symmetric_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        act(&window, &shared, |viewport, env| {
            let on = !viewport.slice.symmetric;
            viewport.set_slice_symmetric(on, env)
        });
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_slice_keep(move || {
        let Some(window) = weak.upgrade() else { return };
        act(&window, &shared, ViewportState::slice_keep);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_slice_flip(move || {
        let Some(window) = weak.upgrade() else { return };
        act(&window, &shared, ViewportState::slice_flip);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_slice_discard(move || {
        let Some(window) = weak.upgrade() else { return };
        act(&window, &shared, |viewport, _| {
            viewport.slice_discard(DiscardReason::User)
        });
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_info_closed(move || {
        let Some(window) = weak.upgrade() else { return };
        let closed = shared
            .try_borrow_mut()
            .is_ok_and(|mut app| app.viewport.close_info());
        if closed {
            super::push(&window, &shared.borrow());
        }
    });
}

/// Records whether a pointer is down (`kind` 0 down, 1 up, 3 cancel), so the Render view
/// can coalesce trace restarts while a finger moves the camera.
fn note_pointer(state: &Shared, kind: i32) {
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    match kind {
        0 => app.viewport.touch.set_pointer(true),
        1 | 3 => app.viewport.touch.set_pointer(false),
        _ => {}
    }
}

/// While the Render view's Light chip is on, one finger moves the light instead of the
/// camera: the events go to the light drag and `true` says they were taken.
fn light_drag(window: &AppWindow, state: &Shared, kind: i32, x: f32, y: f32) -> bool {
    let moved = {
        let Ok(mut app) = state.try_borrow_mut() else {
            return false;
        };
        if !(app.viewport.mode.is_render() && app.viewport.light_drag.on) {
            return false;
        }
        match kind {
            0 => {
                app.viewport.light_drag.begin(x, y);
                None
            }
            2 => app.viewport.light_drag.moved(x, y),
            _ => {
                app.viewport.light_drag.end();
                None
            }
        }
    };
    if let Some((dx, dy)) = moved {
        render_view::drag_light(&mut state.borrow_mut(), dx, dy);
        // The settings changed: the Light tab, the render view and the trace follow.
        crate::design::refresh(window, state);
    }
    true
}

/// Runs `action` on the viewport with the shared session and selection, then acts on
/// what it asks for. Skipped when the state is borrowed (a re-entrant callback).
fn act(
    window: &AppWindow,
    state: &Shared,
    action: impl FnOnce(&mut ViewportState, &mut Env<'_>) -> Outcome,
) {
    let outcome = {
        let Ok(mut guard) = state.try_borrow_mut() else {
            return;
        };
        let app: &mut AppState = &mut guard;
        model::sync_options(app);
        let mut env = Env {
            session: &mut app.session,
            selected_tier: &mut app.selected_tier,
            now: runtime::since_start(),
        };
        action(&mut app.viewport, &mut env)
    };
    apply(window, state, &outcome);
}

/// Carries out an [`Outcome`] on the UI.
fn apply(window: &AppWindow, state: &Shared, outcome: &Outcome) {
    if let Some(text) = &outcome.toast {
        crate::push::toast::show(window, text);
    }
    if let Some(tier) = outcome.select {
        let index = tier.map_or(-1, |tier| i32::try_from(tier).unwrap_or(-1));
        window.global::<ViewportModel>().invoke_tier_picked(index);
    }
    if outcome.edited {
        crate::design::after_edit(window, state);
    } else if outcome.refresh || outcome.select.is_some() {
        crate::design::refresh(window, state);
    } else if outcome.redraw || outcome.live_edit {
        super::push(window, &state.borrow());
    }
}

/// Starts the long-press countdown for the finger that just went down.
fn arm_long_press() {
    LONG_PRESS_TIMER.with(|timer| {
        timer.start(TimerMode::SingleShot, LONG_PRESS, || {
            let Some(window) = WINDOW.with(|cell| cell.borrow().as_ref()?.upgrade()) else {
                return;
            };
            let Some(state) = runtime::shared() else {
                return;
            };
            act(&window, &state, |viewport, env| {
                viewport.long_press_tick(env)
            });
        });
    });
}

/// Keeps the Slice expiry poll running exactly while a cut is pending.
pub(super) fn watch_slice(state: &AppState) {
    let pending = state.viewport.slice.session.is_some();
    SLICE_TIMER.with(|timer| {
        if pending && !timer.running() {
            timer.start(TimerMode::Repeated, SLICE_POLL, slice_tick);
        } else if !pending && timer.running() {
            timer.stop();
        }
    });
}

/// One poll: discards a pending cut when the design or the selection moved on.
fn slice_tick() {
    let Some(window) = WINDOW.with(|cell| cell.borrow().as_ref()?.upgrade()) else {
        return;
    };
    let Some(state) = runtime::shared() else {
        return;
    };
    act(&window, &state, |viewport, env| {
        viewport.check_expiry(env.session.current_generation(), *env.selected_tier)
    });
}
