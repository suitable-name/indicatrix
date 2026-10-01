//! The Render tab: the live CPU-worker render loop ([`live`]), its settings panel
//! ([`panel`]), the PNG export ([`export`]) and the viewport-size tracking
//! ([`viewport`]).
//!
//! # How the rest of the app reaches the renderer
//!
//! - [`request_sync`] after any state change. `app::persist::schedule_save` calls it,
//!   and every state change already goes through that, so material, lighting, camera,
//!   design edits, loads, undo/redo and the view tab all reach the renderer without
//!   hooks of their own. The resize debounce and the HDR upload call it directly. The
//!   sync itself runs on the next event-loop turn, so a burst of changes is one
//!   comparison, and no caller's `WebApp` borrow can still be held.
//! - [`solve_finished`] when a solve lands (`app::solve`).
//! - [`open_export_dialog`] from File > Export PNG.

mod export;
mod live;
mod panel;
pub mod viewport;

use crate::{AppWindow, app::Ctx};

pub use export::open_export_dialog;
pub use live::{hdr_lights_viewport, optics_scene};

/// Wires the render panel, the export dialog and the worker actions, pushes the
/// panel's initial values and schedules the first sync (which runs once start-up has
/// restored the design). Call once at start-up, after `app::callbacks::wire`.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    live::install(ctx);
    panel::wire(ui, ctx);
    export::wire(ui, ctx);
    panel::push_panel(ui, &ctx.state.borrow().settings);
    live::request_sync(ctx);
}

/// Asks the renderer to compare the scene it would render now with the one it is
/// rendering, on the next event-loop turn (see the module doc comment).
pub fn request_sync(ctx: &Ctx) {
    live::request_sync(ctx);
}

/// A solve finished: the renderer may now have planes to trace.
pub fn solve_finished(ctx: &Ctx) {
    live::request_sync(ctx);
}

/// `performance.now()` in milliseconds (0 when unavailable).
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// An RGBA8 buffer as a Slint image.
fn rgba_image(width: u32, height: u32, rgba: &[u8]) -> slint::Image {
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() == rgba.len() {
        bytes.copy_from_slice(rgba);
    }
    slint::Image::from_rgba8(buffer)
}
