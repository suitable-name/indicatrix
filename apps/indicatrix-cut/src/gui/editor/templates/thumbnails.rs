//! The solid thumbnails on the New Design dialog's template cards.
//!
//! Each template is drawn once, by a worker thread, the first time the dialog opens:
//! the template at its own gear on its own preform, solved, turned into a mesh and
//! rasterised with the same CPU solid renderer the Edit tab's Solid view uses (flat
//! grey, lit, facet edges drawn) on a transparent background, seen from above and a
//! little to the side so the outline and the crown facets both read. The finished
//! pictures are kept for the life of the app -- opening the dialog again asks for
//! nothing it already has.
//!
//! The pieces:
//!
//! - [`render_thumbnail`] is pure (a design in, a pixel buffer out), so a test draws every
//!   card's picture without a window;
//! - [`Thumbnails`] owns the worker (a [`LatestWorker`], whose one-slot mailbox means a
//!   request lists every template still missing, never one per request) and the cache,
//!   and posts each finished picture to the UI thread as it is done, shapes first.
//!
//! A `slint::Image` is not `Send`, so the worker only ever holds a
//! `SharedPixelBuffer`; `Image::from_rgba8` is called on the UI thread, in
//! [`super::set_thumbnail`].

use super::{gallery_design, set_thumbnail};
use crate::{
    MainWindow,
    gui::{
        latest_worker::LatestWorker,
        render::camera_lighting::fit_distance_for_radius,
        solid_preview::{
            mesh_cache::MeshCache,
            raster::{SolidRasterizer, SolidStyle},
            to_pixel_buffer,
        },
    },
};
use glam::Vec3;
use indicatrix::optics::raytracer::{Camera, DEFAULT_FOV_DEG};
use indicatrix_cut_core::Design;
use indicatrix_editor::solve_policy::design_to_gpu_planes_from_solved;
use slint::{Rgba8Pixel, SharedPixelBuffer, Weak};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
use tracing::warn;

/// A finished picture, `Send` so it can cross back to the UI thread.
pub(super) type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// A thumbnail's edge in pixels. The card draws it at about 94 logical pixels, so this
/// stays sharp on a 150 % display.
pub(super) const THUMBNAIL_EDGE: u32 = 160;

/// The viewing direction: a quarter turn short of straight ahead, and about 49 degrees
/// above the girdle plane -- the table, the crown facets and the outline all show.
const THUMBNAIL_YAW: f32 = 0.6;
const THUMBNAIL_PITCH: f32 = 0.85;

/// Draws `design` as a [`THUMBNAIL_EDGE`] square thumbnail, or `None` when it does not
/// solve into a closed solid. The background is transparent, so the card shows through.
#[must_use]
pub(super) fn render_thumbnail(design: &Design) -> Option<Pixels> {
    let solved = design.solve().ok()?;
    let planes: Vec<(Vec3, f32)> = design_to_gpu_planes_from_solved(design, &solved)
        .iter()
        .map(|plane| (Vec3::from(plane.normal), -plane.d))
        .collect();
    let mut cache = MeshCache::default();
    let mesh = cache.get_or_build(&planes)?;
    let camera = Camera::new(
        THUMBNAIL_YAW,
        THUMBNAIL_PITCH,
        fit_distance_for_radius(mesh.bounding_radius()),
        DEFAULT_FOV_DEG,
    );
    let style = SolidStyle {
        show_orientation_marker: false,
        show_preform: false,
        preform_plane_count: design.preform.planes().len(),
        ..SolidStyle::default()
    };
    let mut raster = SolidRasterizer::new(THUMBNAIL_EDGE, THUMBNAIL_EDGE);
    raster.render_prepared(mesh, &camera, &style);
    Some(to_pixel_buffer(&raster))
}

/// The picture of `template_index`: the cached one, else a fresh drawing (cached for next
/// time). `None` when the template does not draw.
fn picture(cache: &Mutex<HashMap<i32, Pixels>>, template_index: i32) -> Option<Pixels> {
    let cached = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&template_index)
        .cloned();
    if cached.is_some() {
        return cached;
    }
    let pixels = render_thumbnail(&gallery_design(template_index))?;
    cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(template_index, pixels.clone());
    Some(pixels)
}

/// The worker that draws the thumbnails, and what it has drawn.
pub(super) struct Thumbnails {
    worker: LatestWorker<Vec<i32>>,
}

impl Thumbnails {
    /// Starts the worker thread. Each picture it finishes is handed to the UI thread
    /// through `ui_weak` (a window that is gone by then is ignored).
    pub(super) fn spawn(ui_weak: Weak<MainWindow>) -> Self {
        let cache: Arc<Mutex<HashMap<i32, Pixels>>> = Arc::default();
        let worker = LatestWorker::spawn_with_panic_hook(
            "template-thumbnails",
            move |wanted: Vec<i32>| {
                for template_index in wanted {
                    let Some(pixels) = picture(&cache, template_index) else {
                        warn!("template {template_index} did not draw a thumbnail");
                        continue;
                    };
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        set_thumbnail(&ui, template_index, &pixels);
                    });
                }
            },
            || {},
        );
        Self { worker }
    }

    /// Asks for the pictures of `wanted` (template indices, drawn in this order). A
    /// request replaces any not yet started one, so `wanted` should list everything still
    /// missing; templates drawn already are served from the cache.
    pub(super) fn request(&self, wanted: Vec<i32>) {
        if !wanted.is_empty() && !self.worker.submit(wanted) {
            warn!("the template thumbnail worker is not running");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_editor::templates::gallery_cards;
    use std::collections::BTreeSet;

    /// How many pixels of `pixels` are painted (alpha above zero).
    fn painted(pixels: &Pixels) -> usize {
        pixels.as_slice().iter().filter(|pixel| pixel.a > 0).count()
    }

    #[test]
    fn every_template_draws_a_stone_on_a_transparent_background() {
        let total = (THUMBNAIL_EDGE * THUMBNAIL_EDGE) as usize;
        for card in gallery_cards()
            .into_iter()
            .filter(|card| card.template_index >= 1)
        {
            let pixels = render_thumbnail(&gallery_design(card.template_index))
                .unwrap_or_else(|| panic!("{} did not draw", card.name));
            assert_eq!(pixels.width(), THUMBNAIL_EDGE, "{}", card.name);
            assert_eq!(pixels.height(), THUMBNAIL_EDGE, "{}", card.name);
            let painted = painted(&pixels);
            assert!(
                painted > total / 20,
                "{}: only {painted} of {total} pixels are painted",
                card.name
            );
            assert!(
                painted < total * 9 / 10,
                "{}: {painted} of {total} pixels are painted -- the stone fills the frame",
                card.name
            );
            assert_eq!(
                pixels.as_slice()[0].a,
                0,
                "{}: the corner is not transparent",
                card.name
            );
        }
    }

    /// Different shapes draw different pictures (not every card the same stone).
    #[test]
    fn the_featured_shapes_draw_different_pictures() {
        let counts: BTreeSet<usize> = gallery_cards()
            .into_iter()
            .filter(|card| card.template_index >= 1 && card.group.code() == 1)
            .map(|card| {
                painted(
                    &render_thumbnail(&gallery_design(card.template_index))
                        .expect("a featured shape draws"),
                )
            })
            .collect();
        assert!(counts.len() >= 4, "painted-pixel counts: {counts:?}");
    }

    /// The blank design has no stone to draw: it is a bare preform and gets no card
    /// picture (the dialog never asks for index 0), but drawing it must not fail either.
    #[test]
    fn the_blank_design_still_draws_without_failing() {
        assert!(render_thumbnail(&gallery_design(0)).is_some());
    }
}
