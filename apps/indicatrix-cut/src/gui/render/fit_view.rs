//! The pose pills' "Fit" (`ViewportModel.set_view(5)`): the orbit distance that shows the
//! whole stone in the picture on screen.
//!
//! Two inputs used to make Fit frame the wrong size:
//!
//! - The radius. The solid worker's last frame (`SlintSolidSink::mesh_bounding_radius`)
//!   is only current while the solid raster is on screen. The Live view's path-traced
//!   image draws `RenderContext::active_planes` directly, and a library selection there
//!   redraws no solid frame (`resubmit_live_solid` runs in Solid mode only), so the
//!   stored radius still described whatever stone the worker drew last. Fit therefore
//!   measures the stone the tracer holds now ([`planar_bounding_radius`]) and, where the
//!   solid raster is on screen, keeps the drawn frame's radius when that is larger (the
//!   Cut slider and Cutting mode draw the partly cut stone, which can be bigger than the
//!   finished one the tracer holds).
//! - The image. The camera's field of view is the vertical one, so a picture taller than
//!   wide needs the stone further away ([`fit_distance_for_view`]).

use crate::{MainWindow, SolidPreviewModel, ViewportModel, bridge::render_thread::RenderContext};
use indicatrix_solid::preview::camera::{
    fit_distance_for_view, orbit_distance_bounds, planar_bounding_radius,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError};

/// The Fit distance for the picture on screen now.
///
/// In the Live view's path-traced mode the measured radius is also stored as the
/// frame radius, so the zoom clamp that reads it afterwards does not pull the fitted
/// camera back in.
///
/// The mesh build runs outside the `RenderContext` lock, which the render thread shares.
pub(super) fn fit_distance(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    frame_radius: &Mutex<f64>,
) -> f32 {
    let (stone, render_size) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            super::camera_lighting::active_stone(&ctx),
            (ctx.width, ctx.height),
        )
    };
    let solid = ui.global::<SolidPreviewModel>();
    let on_live_tab = ui.get_render_view_tab() == 0;
    let traced_only = on_live_tab && ui.global::<ViewportModel>().get_live_view_mode() != 0;
    // The path-traced image is letterboxed to the render resolution (`image-fit:
    // contain`), so its own aspect is what has to hold the stone.
    let traced = traced_only || (!on_live_tab && matches!(solid.get_view_mode(), 1 | 2));
    let aspect = if traced {
        ratio(render_size.0 as f32, render_size.1 as f32)
    } else if on_live_tab {
        ratio(
            solid.get_live_viewport_width(),
            solid.get_live_viewport_height(),
        )
    } else {
        ratio(solid.get_viewport_width(), solid.get_viewport_height())
    };
    let current = planar_bounding_radius(&stone);
    let mut stored = frame_radius.lock().unwrap_or_else(PoisonError::into_inner);
    let radius = fit_radius(current, *stored, traced_only);
    if traced_only {
        *stored = radius;
    }
    drop(stored);
    let (min, _) = orbit_distance_bounds(radius);
    // Not clamped to the far end: a very tall picture needs the stone further out than
    // the zoom range allows, and clamping would crop it again.
    fit_distance_for_view(radius, aspect).max(min)
}

/// The radius to fit: the stone the tracer holds (`current`), or the drawn frame's
/// (`drawn`) where the solid raster is on screen and its stone is larger. `drawn`
/// alone when the planes do not close.
const fn fit_radius(current: Option<f64>, drawn: f64, traced_only: bool) -> f64 {
    match current {
        Some(current) if traced_only => current,
        Some(current) => current.max(drawn),
        None => drawn,
    }
}

/// `width / height`, or `1.0` before the picture has a size.
fn ratio(width: f32, height: f32) -> f32 {
    if width > 0.0 && height > 0.0 {
        width / height
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::{fit_radius, ratio};

    #[test]
    fn the_traced_picture_fits_the_stone_the_tracer_holds() {
        // A stale, smaller frame radius (the worker drew another stone last) loses ...
        assert!((fit_radius(Some(1.4), 0.8, true) - 1.4).abs() < 1e-12);
        // ... and so does a stale, larger one.
        assert!((fit_radius(Some(1.4), 2.0, true) - 1.4).abs() < 1e-12);
    }

    #[test]
    fn the_solid_picture_keeps_the_larger_drawn_stone() {
        // The Cut slider's partly cut stone is bigger than the finished one.
        assert!((fit_radius(Some(1.4), 2.0, false) - 2.0).abs() < 1e-12);
        assert!((fit_radius(Some(1.4), 0.8, false) - 1.4).abs() < 1e-12);
    }

    #[test]
    fn planes_that_close_nothing_fall_back_to_the_drawn_frame() {
        assert!((fit_radius(None, 0.9, true) - 0.9).abs() < 1e-12);
        assert!((fit_radius(None, 0.9, false) - 0.9).abs() < 1e-12);
    }

    #[test]
    fn an_unsized_picture_counts_as_square() {
        assert!((ratio(0.0, 400.0) - 1.0).abs() < 1e-6);
        assert!((ratio(800.0, 400.0) - 2.0).abs() < 1e-6);
    }
}
