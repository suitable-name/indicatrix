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

pub use runtime::on_viewport_changed;

/// Connects `RenderModel`'s callbacks and gives the timers their handle on the state.
pub fn wire(window: &AppWindow, state: &Shared) {
    runtime::install(window, state);
    export::wire(window, state);

    let model = window.global::<RenderModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_pause_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            let paused = app.render.status() == RenderStatus::Paused(PauseReason::User);
            app.render.set_user_paused(!paused);
        }
        model::push(&window, &shared.borrow());
    });

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
    model::push(window, state);
    runtime::sync(window, state);
}

/// Stops the timers and the export thread and drops every thread-local handle (the state,
/// the window). Call once on the UI thread when the event loop has ended.
pub fn shutdown() {
    export::shutdown();
    runtime::shutdown();
}
