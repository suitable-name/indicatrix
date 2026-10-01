//! Optical metrics in the browser: the Render tab's gemological-metrics HUD ([`hud`]) and
//! the Tilt Performance dialog ([`tilt`]), both computed in the analysis Worker
//! (`WorkerPool::analysis`) by the desktop's own functions (`indicatrix_web_core::solve`'s
//! `Metrics` and `Tilt` requests).
//!
//! # Scheduling
//!
//! - The design solve keeps its own Worker. A new job on a `SolveClient` supersedes the
//!   running one and terminates the Worker once that has run for 500 ms, so metrics on the
//!   solve Worker would kill a long solve every time the camera settled. They never
//!   touch it.
//! - The HUD's metrics are requested when the rendered pose or light has held still for
//!   [`hud::METRICS_DEBOUNCE`], and only when their inputs differ from the ones on screen
//!   or on their way (the desktop's `MetricsCacheKey` rule). A newer request supersedes an
//!   older one; the HUD dims meanwhile.
//! - The HUD is scored under the HDR map that lights the viewport, which the analysis
//!   Worker holds a copy of (`WorkerPool::set_hdr`); the map's id is one of the request's
//!   inputs. The tilt sweep is scored under the lighting preset only, as on the desktop.
//! - The tilt sweep shares the analysis Worker. It is the only job that must not be
//!   superseded by a HUD update, so while it runs the HUD's requests wait ([`hud::resume`]
//!   sends the newest one when the sweep ends) and the HUD keeps showing its last numbers,
//!   dimmed.

pub mod hud;
pub mod tilt;

use crate::{AppWindow, app::Ctx};

pub use hud::{scene_blocked, scene_changed, scene_gone};

/// Wires the Tilt dialog's callbacks. Call once at start-up.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    tilt::wire(ui, ctx);
}
