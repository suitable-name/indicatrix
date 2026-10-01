//! Viewport-size tracking and the camera pose the render view is traced with.
//!
//! - [`track_browser_viewport`] sizes the Slint window to the browser viewport,
//!   now and on every browser resize. Without it Slint's winit backend pins the
//!   canvas at the window's preferred size and the responsive layout never reacts.
//! - [`clamp_render_dims`] turns the render view's logical size into a physical
//!   render-target size (DPR-aware, capped); `AppModel.render-size-changed` is
//!   debounced by [`RESIZE_DEBOUNCE`] before it lands in [`ViewState`].

use crate::{AppWindow, app::settings::RenderSettings};
use indicatrix_solid::preview::view::with_min_aspect;
use slint::ComponentHandle;
use std::time::Duration;

/// How long after the last `render-size-changed` event a new render-target size
/// is applied: a drag-resize fires dozens of events, and each would otherwise
/// restart accumulation.
pub const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

/// The smallest render-target edge, in physical pixels (the render view can
/// report zero on its first layout pass).
pub const MIN_RENDER_DIM: u32 = 64;

/// The largest render-target edge, in physical pixels, however large the view
/// lays out; the displayed image is scaled up with `image-fit: contain`.
pub const MAX_RENDER_DIM: u32 = 960;

/// The largest device pixel ratio applied: DPR is a quadratic cost multiplier.
pub const MAX_DEVICE_PIXEL_RATIO: f32 = 1.5;

/// The narrowest aspect (width over height) the render raster is traced at: 16:10, the
/// same as the Solid view's raster (`views::refresh`). The camera's field of view is
/// vertical, so a portrait or square view (a phone-width window) would crop the stone's
/// sides out of the raster itself; a narrower view instead gets a 16:10 raster that the
/// Render tab letterboxes with `image-fit: contain`. The PNG export follows the raster's
/// aspect, so it frames the stone the way the view shows it.
pub const MIN_RENDER_ASPECT: f32 = 1.6;

/// The render view's logical size and the window's scale factor, as a physical
/// render-target size clamped to `MIN_RENDER_DIM..=MAX_RENDER_DIM`, and no narrower than
/// [`MIN_RENDER_ASPECT`]. A negative or NaN size saturates to the minimum.
#[must_use]
pub fn clamp_render_dims(logical_width: f32, logical_height: f32, scale_factor: f32) -> (u32, u32) {
    let dpr = scale_factor.clamp(1.0, MAX_DEVICE_PIXEL_RATIO);
    let physical_width = (logical_width.max(0.0) * dpr).round() as u32;
    let physical_height = (logical_height.max(0.0) * dpr).round() as u32;
    with_min_aspect(
        (
            physical_width.clamp(MIN_RENDER_DIM, MAX_RENDER_DIM),
            physical_height.clamp(MIN_RENDER_DIM, MAX_RENDER_DIM),
        ),
        MIN_RENDER_ASPECT,
        MIN_RENDER_DIM,
    )
}

/// The camera pose and render-target size the renderer renders from.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    /// Camera yaw, radians.
    pub yaw: f32,
    /// Camera pitch, radians.
    pub pitch: f32,
    /// Camera distance.
    pub distance: f32,
    /// Render-target width, physical pixels (already clamped).
    pub render_width: u32,
    /// Render-target height, physical pixels (already clamped).
    pub render_height: u32,
}

impl ViewState {
    /// The camera from `settings`, and the desktop's 800 x 600 until the render
    /// view reports its real size.
    #[must_use]
    pub const fn from_settings(settings: &RenderSettings) -> Self {
        Self {
            yaw: settings.camera_yaw,
            pitch: settings.camera_pitch,
            distance: settings.camera_distance,
            render_width: 800,
            render_height: 600,
        }
    }
}

/// The browser viewport in CSS pixels: `documentElement.client*` first (it
/// excludes scrollbar gutters), `window.inner*` second, 1100 x 700 when both read
/// zero (before the first layout, or in an offscreen embedding).
fn viewport_logical_size() -> slint::LogicalSize {
    let (mut w, mut h) = (1100.0_f64, 700.0_f64);
    if let Some(win) = web_sys::window() {
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

/// Sizes the Slint window to the browser viewport, now (deferred to the first
/// event-loop turn: the winit window does not exist before `run()`) and on every
/// browser `resize`. The `resize` closure lives for the page's lifetime, so it is
/// leaked with `forget()`.
pub fn track_browser_viewport(app: &AppWindow) {
    let weak_initial = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(app) = weak_initial.upgrade() {
            app.window().set_size(viewport_logical_size());
        }
    });

    let weak = app.as_weak();
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
