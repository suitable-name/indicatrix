//! The "Both" view mode's solid-edges overlay: a render whose non-edge pixels are
//! fully transparent, composited as an `Image` layer over the path-traced render.
//!
//! [`raster::SolidRasterizer::render_prepared`] already produces exactly this when
//! styled with [`raster::FillMode::Transparent`]: the fill pass writes alpha `0`
//! for every filled pixel while the edge pass always writes alpha `255` (it draws a
//! line, not a fill span). [`render_edges_layer`] just forces that one style field.

use super::{
    mesh_cache::CachedMesh,
    raster::{FillMode, SolidRasterizer, SolidStyle},
};
use indicatrix::optics::raytracer::Camera;

/// Renders `prepared` into `rasterizer` for the "Both" view's solid-edges layer.
///
/// Identical to [`SolidRasterizer::render_prepared`] with `style.fill_mode` forced
/// to [`FillMode::Transparent`]. Clones `style` internally so the SAME `SolidStyle`
/// used for the ordinary opaque Solid-mode render can be reused here unchanged.
///
/// The orientation marker and the facet labels are forced OFF as well. Both belong
/// to the opaque solid render: this layer is composited over the path-traced image,
/// where the marker would either double up with the solid view's own copy or appear
/// over a render that never asked for it. Forcing them off keeps this layer's
/// promise that every pixel it leaves opaque is an edge pixel, which is what the
/// compositing in `solid_viewport.slint` relies on.
pub fn render_edges_layer(
    rasterizer: &mut SolidRasterizer,
    prepared: &CachedMesh,
    camera: &Camera,
    style: &SolidStyle,
) {
    let transparent_style = SolidStyle {
        fill_mode: FillMode::Transparent,
        show_orientation_marker: false,
        facet_labels: Vec::new(),
        ..style.clone()
    };
    rasterizer.render_prepared(prepared, camera, &transparent_style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::solid_preview::mesh_cache::MeshCache;
    use glam::Vec3;

    /// The canonical standard round brilliant plane set. `MeshCache` (the only
    /// PUBLIC way to build a [`CachedMesh`]) works in the same `f32` `(Vec3,
    /// offset)` convention `GpuFacetPlane` carries, so this skips widening to `f64`.
    fn rbc_planes_f32() -> Vec<(Vec3, f32)> {
        use indicatrix::geometry::cuts::StandardGemCuts;
        StandardGemCuts::standard_round_brilliant()
            .into_iter()
            .map(|plane| (Vec3::from(plane.normal), -plane.d))
            .collect()
    }

    #[test]
    fn edges_layer_is_transparent_except_at_the_opaque_renders_edge_pixels() {
        let mut mesh_cache = MeshCache::default();
        let prepared = mesh_cache
            .get_or_build(&rbc_planes_f32())
            .expect("RBC-445 must close");
        let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
        // The orientation marker is drawn over the opaque render's own edges (and
        // is forced off in the overlay by `render_edges_layer`), so leaving it on
        // would make a marker pixel differ for a reason this test is not about.
        let style = SolidStyle {
            show_orientation_marker: false,
            ..SolidStyle::default()
        };

        let mut opaque = SolidRasterizer::new(200, 150);
        opaque.render_prepared(prepared, &camera, &style);

        let mut edges = SolidRasterizer::new(200, 150);
        render_edges_layer(&mut edges, prepared, &camera, &style);

        let pixel_count = 200 * 150;
        let mut transparent = 0usize;
        let mut opaque_edge_pixels = 0usize;
        for i in 0..pixel_count {
            let o = i * 4;
            let edge_alpha = edges.color[o + 3];
            if edge_alpha == 0 {
                transparent += 1;
                continue;
            }
            assert_eq!(
                edge_alpha, 255,
                "edges_layer alpha must be 0 or 255, pixel {i}"
            );
            opaque_edge_pixels += 1;
            // Both passes draw edges identically and only differ in fill alpha, so
            // the opaque render must show the same edge color at the same pixel.
            let edge_rgb = [edges.color[o], edges.color[o + 1], edges.color[o + 2]];
            assert_eq!(edge_rgb, style.edge_color, "pixel {i}");
            let opaque_rgb = [opaque.color[o], opaque.color[o + 1], opaque.color[o + 2]];
            assert_eq!(
                opaque_rgb, style.edge_color,
                "pixel {i}: opaque render must show the same edge color here"
            );
        }

        assert!(
            transparent > pixel_count * 9 / 10,
            "expected the large majority of pixels to be fully transparent, got \
             {transparent}/{pixel_count}"
        );
        assert!(
            opaque_edge_pixels > 0,
            "expected at least some edge pixels to be fully opaque"
        );
    }
}
