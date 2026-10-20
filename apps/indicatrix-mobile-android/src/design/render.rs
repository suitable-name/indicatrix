//! The Render view's Rust side: the ray-traced picture of the Design viewport's Render
//! mode.
//!
//! The decisions (when a trace starts, restarts and stops, the status line, the light
//! drag, the export options) live in `indicatrix_mobile_core::viewport::render_view` and
//! the engine in `indicatrix_mobile_core::render`; this module ties them to the UI:
//!
//! | Submodule | Job |
//! |---|---|
//! | `runtime` | the poll timer (frames out of the engine onto `RenderModel`), the deferred restart, the 10 s governor, the state handle for timers |
//! | `model` | `RenderModel`'s overlay properties from the state |
//! | `export` | the export sheet: options, the worker thread, share and save |
//!
//! Camera changes reach the trace through `viewport::push` (see [`on_viewport_changed`]);
//! settings and design changes through `design::refresh` (see [`push`]); a poll every
//! 100 ms catches the rest and stops the trace when the Design destination is left.

mod export;
mod model;
mod runtime;

use indicatrix_mobile_core::{
    app_state::AppState,
    render::{PauseReason, RenderStatus},
    viewport::render_view,
};
use slint::ComponentHandle;

use super::Shared;
use crate::{AppWindow, RenderModel, push::toast};

pub use runtime::{on_touch_released, on_viewport_changed};

/// Connects `RenderModel`'s callbacks and gives the timers their handle on the state.
pub fn wire(window: &AppWindow, state: &Shared) {
    runtime::install(window, state);
    export::wire(window, state);

    let model = window.global::<RenderModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_engine_selected(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let engine = indicatrix::renderer::engine::RenderEngine::from_index(
            u8::try_from(index).unwrap_or(0),
        );
        {
            let mut app = shared.borrow_mut();
            app.update_settings(|settings| settings.render.engine = engine.index());
            app.render.stop();
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_backend_selected(move |index| {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            let Some(backend) = indicatrix_mobile_core::settings::RenderBackend::from_index(index)
            else {
                return;
            };
            if app.settings.render.backend == backend {
                return;
            }
            app.update_settings(|settings| settings.render.backend = backend);
            app.render.set_user_paused(false);
            app.render.stop();
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_restart_requested({
        let weak = window.as_weak();
        let shared = state.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                shared.borrow_mut().render.stop();
                crate::design::refresh(&window, &shared);
            }
        }
    });
    model.on_pause_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            if app.settings.render.backend
                == indicatrix_mobile_core::settings::RenderBackend::Remote
            {
                app.render.cancel_remote();
                drop(app);
                model::push(&window, &shared.borrow());
                toast::show(
                    &window,
                    "Remote session cancelled. Retry starts a new session at sample zero.",
                );
                return;
            }
            let paused = app.render.status() == RenderStatus::Paused(PauseReason::User);
            app.render.set_user_paused(!paused);
        }
        model::push(&window, &shared.borrow());
    });

    wire_settings(window, state);
}

/// The quality, denoise and light chips, and the Export button.
fn wire_settings(window: &AppWindow, state: &Shared) {
    let model = window.global::<RenderModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_quality_cycled(move || {
        let Some(window) = weak.upgrade() else { return };
        let next = {
            let mut app = shared.borrow_mut();
            let next = render_view::next_quality(app.settings.render.quality);
            app.update_settings(|settings| settings.render.quality = next);
            next
        };
        toast::show(
            &window,
            &format!("Render quality: {}", render_view::quality_label(next)),
        );
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_denoise_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            let on = !app.settings.render.denoise;
            app.update_settings(|settings| settings.render.denoise = on);
            if app.settings.render.backend
                == indicatrix_mobile_core::settings::RenderBackend::Remote
            {
                app.render.stop();
            }
            app.render.set_denoise(on);
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_light_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        let available = render_view::light_drag_available(&shared.borrow());
        if available {
            let mut app = shared.borrow_mut();
            let on = !app.viewport.light_drag.on;
            app.viewport.light_drag.on = on;
            app.viewport.light_drag.end();
        } else {
            toast::show(
                &window,
                "This lighting rig has no movable light. Pick another on the Light tab.",
            );
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_export_requested(move || {
        let Some(window) = weak.upgrade() else { return };
        export::open(&window, &shared);
    });
}

/// Pushes the render view's properties from the state and makes sure the timers that
/// follow the Render mode run (or stop) with it. Cheap.
pub fn push(window: &AppWindow, state: &AppState) {
    export::refresh_document(window, state);
    model::push(window, state);
    runtime::sync(window, state);
}

/// Lifecycle notifications arrive without a window handle, after releasing `AppState`.
pub fn refresh_editor() {
    if let Some((window, shared)) = runtime::context() {
        super::refresh(&window, &shared);
    }
}

/// Cancels a picture export or Send To Server upload that is talking to the remote
/// endpoint (Forget Server calls this, so no connection outlives the credentials it uses).
/// An export that renders on this phone keeps running.
pub fn cancel_remote_export() {
    export::cancel_remote();
}

/// Stops the timers and the export thread and drops every thread-local handle (the state,
/// the window). Call once on the UI thread when the event loop has ended.
pub fn shutdown() {
    export::shutdown();
    runtime::shutdown();
}
