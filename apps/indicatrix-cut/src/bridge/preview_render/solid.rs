//! The solid-renderer catalogue preview: a flat-shaded, edge-outlined picture of a
//! design drawn by the software rasterizer in a few milliseconds, as a stand-in until
//! the traced preview is rendered.

use super::{PREVIEW_DISTANCE, PREVIEW_YAW, PreviewView, encode_png};
use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
};
use indicatrix_solid::{
    mesh_cache::MeshCache,
    raster::{SolidRasterizer, SolidStyle},
};

/// Draws `planes` from `view`'s pose into a `size` x `size` PNG, on an opaque black
/// background like the traced previews. `None` for a zero size, an arrangement that does
/// not close into a solid, or a failed PNG encode.
///
/// Uses the same camera as [`super::render_view`], so a solid picture and the traced one
/// that replaces it frame the stone identically. CPU only; it never touches the GPU.
#[must_use]
pub fn render_view_solid(
    planes: &[GpuFacetPlane],
    size: u32,
    view: PreviewView,
) -> Option<Vec<u8>> {
    if size == 0 {
        return None;
    }
    // `GpuFacetPlane` is `n . x + d <= 0`; the mesh cache takes `n . x <= m`.
    let half_spaces: Vec<(Vec3, f32)> = planes
        .iter()
        .map(|plane| (Vec3::from(plane.normal), -plane.d))
        .collect();
    let mut cache = MeshCache::default();
    let mesh = cache.get_or_build(&half_spaces)?;
    let camera = Camera::new(PREVIEW_YAW, view.pitch(), PREVIEW_DISTANCE, DEFAULT_FOV_DEG);
    let style = SolidStyle {
        background: [0, 0, 0, 255],
        ..SolidStyle::default()
    };
    let mut raster = SolidRasterizer::new(size, size);
    raster.render_prepared(mesh, &camera, &style);
    encode_png(size, size, &raster.color)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube's six faces.
    fn cube() -> Vec<GpuFacetPlane> {
        [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .map(|n| GpuFacetPlane::new(n, -0.5))
        .collect()
    }

    #[test]
    fn a_closed_stone_draws_a_png_of_the_requested_size_with_painted_pixels() {
        let png = render_view_solid(&cube(), 48, PreviewView::Top).expect("a cube closes");
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (48, 48));
        assert!(
            decoded.pixels().any(|p| p.0[..3] != [0, 0, 0]),
            "something other than the background was painted"
        );
    }

    #[test]
    fn an_open_arrangement_and_a_zero_size_draw_nothing() {
        assert!(render_view_solid(&cube()[..3], 32, PreviewView::Front).is_none());
        assert!(render_view_solid(&cube(), 0, PreviewView::Front).is_none());
    }
}
