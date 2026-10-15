//! The design viewport's Rust side: pointer events in, pictures out.
//!
//! - [`input`]: the `ViewportModel` callbacks. Each takes the raw event, hands it to the
//!   core (`indicatrix_mobile_core::viewport`) together with the shared session and
//!   selection, and acts on the [`Outcome`](indicatrix_mobile_core::viewport::Outcome):
//!   toast, selection, redraw, or the after-edit path when the design changed;
//! - [`runtime`]: the `viewport` background queue that plans and draws pictures, latest
//!   job wins, and the landing of finished frames on the UI thread;
//! - [`model`]: [`push`], which writes everything `ViewportModel` shows from the state
//!   and asks the runtime for a new picture when what it depends on changed.
//!
//! Camera moves, selection taps and handle drags call [`push`] (this surface only);
//! only a change of the design or the selection asks the other Design surfaces to
//! refresh as well, through `design::after_edit` and `design::refresh`.

mod input;
mod model;
mod runtime;

pub use runtime::shutdown;

use indicatrix_mobile_core::app_state::AppState;
use slint::ComponentHandle;

use super::Shared;
use crate::AppWindow;

/// Connects the viewport's callbacks and starts its raster queue.
pub fn wire(window: &AppWindow, state: &Shared) {
    runtime::install(window, state);
    input::wire(window, state);
}

/// Pushes the viewport from the state and, when the picture it shows is out of date,
/// queues a new one. Cheap when nothing changed.
///
/// The window's scale factor is read here on every push, not only when the viewport is
/// resized: a window can learn its real scale after the first layout without changing its
/// logical size, and a raster drawn at the wrong scale is blurry. A changed scale changes
/// the raster size, which `runtime::request` turns into a new picture.
///
/// In the Render mode the raster queue is idle (the picture is the render's) and the
/// render view is told that the camera may have moved.
pub fn push(window: &AppWindow, state: &AppState) {
    state
        .viewport
        .note_scale_factor(window.window().scale_factor());
    model::push(window, state);
    runtime::request(window, state);
    super::render::on_viewport_changed(window, state);
}
