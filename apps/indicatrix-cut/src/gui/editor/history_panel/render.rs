//! Drawing the design at one step into a small picture, on the worker thread.
//!
//! The same flat solid raster the compare window and the Edit tab's Solid view use, at
//! one fixed three-quarter pose so the pictures of different steps can be compared at a
//! glance. The background is transparent: the tile the picture sits in follows the theme.
//! A concave tool cut is not drawn (the picture is the planar facets only, like the
//! compare window's).

use crate::gui::{
    render::camera_lighting::fit_distance_for_radius,
    solid_preview::{
        mesh_cache::MeshCache,
        raster::{SolidRasterizer, SolidStyle},
        to_pixel_buffer,
    },
};
use glam::Vec3;
use indicatrix::optics::raytracer::{Camera, DEFAULT_FOV_DEG, DEFAULT_POSE};
use indicatrix_editor::{HistorySnapshot, solve_policy::design_to_gpu_planes_from_solved};
use slint::{Rgba8Pixel, SharedPixelBuffer};

/// A finished picture's pixels, `Send` so they can cross back to the UI thread.
pub(super) type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// The solid style of a picture: the Solid view's defaults with the rough's own planes
/// tinted, no orientation caption and a transparent background.
fn thumb_style(preform_planes: usize) -> SolidStyle {
    SolidStyle {
        background: [0, 0, 0, 0],
        preform_plane_count: preform_planes,
        show_orientation_marker: false,
        ..SolidStyle::default()
    }
}

/// Owns the mesh cache and the rasterizer a worker draws every picture with.
pub(super) struct ThumbRenderer {
    cache: MeshCache,
    raster: SolidRasterizer,
}

impl Default for ThumbRenderer {
    fn default() -> Self {
        Self {
            cache: MeshCache::default(),
            raster: SolidRasterizer::new(1, 1),
        }
    }
}

impl ThumbRenderer {
    /// Draws the solid the half-spaces `planes` (`n . x <= m`, the rough's own
    /// `preform_planes` first) bound, framed to fill an `edge` by `edge` picture. `None`
    /// when the planes do not close into a solid.
    pub(super) fn render_planes(
        &mut self,
        planes: &[(Vec3, f32)],
        preform_planes: usize,
        edge: u32,
    ) -> Option<Pixels> {
        let prepared = self.cache.get_or_build(planes)?;
        let camera = Camera::new(
            DEFAULT_POSE.yaw,
            DEFAULT_POSE.pitch,
            fit_distance_for_radius(prepared.bounding_radius()),
            DEFAULT_FOV_DEG,
        );
        self.raster.resize(edge.max(1), edge.max(1));
        self.raster
            .render_prepared(prepared, &camera, &thumb_style(preform_planes));
        Some(to_pixel_buffer(&self.raster))
    }
}

/// The picture of the design at step `position` of `session`: replays the steps between,
/// solves, and draws. `None` when any of that cannot be done -- the step is out of range,
/// the design does not solve, or its facets do not close into a solid.
pub(super) fn render_step(
    renderer: &mut ThumbRenderer,
    session: &HistorySnapshot,
    position: usize,
    edge: u32,
) -> Option<Pixels> {
    let design = session.design_at(position).ok()?;
    let preform_planes = design.preform.planes().len();
    let solved = design.solve().ok()?;
    let planes: Vec<(Vec3, f32)> = design_to_gpu_planes_from_solved(&design, &solved)
        .iter()
        .map(|plane| (Vec3::from(plane.normal), -plane.d))
        .collect();
    renderer.render_planes(&planes, preform_planes, edge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::Edit;
    use indicatrix_editor::EditorSession;

    /// The six half-spaces of the cube `|x|, |y|, |z| <= 1`.
    fn cube() -> Vec<(Vec3, f32)> {
        [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .map(|normal| (normal, 1.0))
        .collect()
    }

    fn opaque_pixels(pixels: &Pixels) -> usize {
        pixels
            .as_slice()
            .iter()
            .filter(|pixel| pixel.a == 255)
            .count()
    }

    #[test]
    fn a_closed_solid_is_drawn_at_the_asked_size_on_a_transparent_background() {
        let mut renderer = ThumbRenderer::default();
        let pixels = renderer
            .render_planes(&cube(), 0, 48)
            .expect("a cube closes");
        assert_eq!((pixels.width(), pixels.height()), (48, 48));
        let opaque = opaque_pixels(&pixels);
        assert!(
            opaque > 100,
            "the cube covers part of the picture: {opaque}"
        );
        assert!(
            opaque < 48 * 48,
            "and the corners stay transparent: {opaque}"
        );
        let corner = pixels.as_slice()[0];
        assert_eq!(corner.a, 0, "the background is transparent");
    }

    #[test]
    fn planes_that_do_not_close_draw_nothing() {
        let mut renderer = ThumbRenderer::default();
        assert!(renderer.render_planes(&[(Vec3::Y, 1.0)], 0, 32).is_none());
        assert!(renderer.render_planes(&[], 0, 32).is_none());
    }

    #[test]
    fn the_renderer_can_be_reused_at_another_size() {
        let mut renderer = ThumbRenderer::default();
        let small = renderer.render_planes(&cube(), 0, 24).unwrap();
        let large = renderer.render_planes(&cube(), 0, 96).unwrap();
        assert_eq!((small.width(), large.width()), (24, 96));
    }

    #[test]
    fn a_step_of_a_real_session_is_drawn_and_an_unknown_one_is_not() {
        let mut session = EditorSession::fresh();
        session
            .apply(Edit::SetPreformYOffset { y_offset: 0.1 })
            .expect("edit must apply");
        let snapshot = session.history_snapshot();
        let mut renderer = ThumbRenderer::default();
        for position in 0..=1 {
            let pixels = render_step(&mut renderer, &snapshot, position, 40)
                .unwrap_or_else(|| panic!("step {position} draws"));
            assert_eq!(pixels.width(), 40);
            assert!(opaque_pixels(&pixels) > 0, "step {position} has a solid");
        }
        assert!(render_step(&mut renderer, &snapshot, 2, 40).is_none());
    }
}
