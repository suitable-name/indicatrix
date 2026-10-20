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
use indicatrix::geometry::plane::GpuFacetPlane;
use indicatrix_solid::preview::{
    StoneGeometryBuf,
    camera::{fit_distance_for_view, orbit_distance_bounds, planar_bounding_radius},
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};

/// The planar radius of the last plane set Fit measured.
///
/// Measuring builds the whole solid mesh, so a repeated Fit (or the harness' repeated
/// framing reads) on the same stone reuses the answer. The key is the plane set's `Arc`
/// itself, the identity `SceneIdentity` already uses: every writer of
/// `RenderContext::active_planes` installs a new `Arc`. The cache holds a clone of the
/// key, so its address cannot be reused by another allocation, and an in-place
/// `Arc::make_mut` writer copies instead of mutating the measured planes.
struct PlanarRadiusCache<T> {
    measured: Option<(Arc<T>, Option<f64>)>,
}

impl<T> PlanarRadiusCache<T> {
    const fn new() -> Self {
        Self { measured: None }
    }

    /// The cached radius when `planes` is the set measured last, otherwise `measure()`
    /// (remembered for the next call).
    fn get_or_measure(
        &mut self,
        planes: &Arc<T>,
        measure: impl FnOnce() -> Option<f64>,
    ) -> Option<f64> {
        let hit = match &self.measured {
            Some((held, radius)) if Arc::ptr_eq(held, planes) => Some(*radius),
            _ => None,
        };
        if let Some(radius) = hit {
            return radius;
        }
        let radius = measure();
        self.measured = Some((Arc::clone(planes), radius));
        radius
    }
}

thread_local! {
    /// Fit runs on the UI thread only.
    static PLANAR_RADIUS: RefCell<PlanarRadiusCache<Vec<GpuFacetPlane>>> =
        const { RefCell::new(PlanarRadiusCache::new()) };
}

/// The Fit distance for the picture on screen now.
///
/// In the Live view's path-traced mode the measured radius is also stored as the
/// frame radius, so the zoom clamp that reads it afterwards does not pull the fitted
/// camera back in.
///
/// The stone is measured at most once per plane set ([`PlanarRadiusCache`]), outside the
/// `RenderContext` lock, which the render thread shares.
pub fn fit_distance(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    frame_radius: &Mutex<f64>,
) -> f32 {
    let (planes, render_size) = {
        let ctx = RenderContext::lock(render_ctx);
        (Arc::clone(&ctx.active_planes), (ctx.width, ctx.height))
    };
    let solid = ui.global::<SolidPreviewModel>();
    let on_live_tab = ui.get_render_view_tab() == 0;
    let traced_only = on_live_tab
        && crate::gui::view_modes::LiveViewMode::from_raw(
            ui.global::<ViewportModel>().get_live_view_mode(),
        )
        .is_traced();
    // The path-traced image is letterboxed to the render resolution (`image-fit:
    // contain`), so its own aspect is what has to hold the stone.
    let traced = traced_only
        || (!on_live_tab
            && crate::gui::view_modes::EditViewMode::from_raw(solid.get_view_mode()).is_traced());
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
    // Concave tools only remove material, so the planes alone bound the stone.
    let current = PLANAR_RADIUS.with_borrow_mut(|cache| {
        cache.get_or_measure(&planes, || {
            planar_bounding_radius(&StoneGeometryBuf {
                planes: planes.as_ref().clone(),
                ..StoneGeometryBuf::default()
            })
        })
    });
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
    use super::{PlanarRadiusCache, fit_radius, ratio};
    use std::{cell::Cell, sync::Arc};

    #[test]
    fn the_same_plane_set_is_measured_once() {
        let planes = Arc::new(vec![1_u8]);
        let mut cache = PlanarRadiusCache::new();
        let calls = Cell::new(0);
        let measure = |cache: &mut PlanarRadiusCache<Vec<u8>>, planes: &Arc<Vec<u8>>| {
            cache.get_or_measure(planes, || {
                calls.set(calls.get() + 1);
                Some(1.5)
            })
        };
        assert_eq!(measure(&mut cache, &planes), Some(1.5));
        assert_eq!(measure(&mut cache, &planes), Some(1.5));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_new_plane_set_is_measured_again_even_when_equal() {
        let first = Arc::new(vec![1_u8]);
        let second = Arc::new(vec![1_u8]);
        let mut cache = PlanarRadiusCache::new();
        assert_eq!(cache.get_or_measure(&first, || Some(1.0)), Some(1.0));
        assert_eq!(cache.get_or_measure(&second, || Some(2.0)), Some(2.0));
        // The cache now describes the second set.
        assert_eq!(
            cache.get_or_measure(&second, || panic!("cached")),
            Some(2.0)
        );
    }

    #[test]
    fn a_plane_set_that_closes_nothing_is_cached_too() {
        let planes = Arc::new(Vec::<u8>::new());
        let mut cache = PlanarRadiusCache::new();
        assert_eq!(cache.get_or_measure(&planes, || None), None);
        assert_eq!(cache.get_or_measure(&planes, || panic!("cached")), None);
    }

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
