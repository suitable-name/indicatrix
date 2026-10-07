//! Turning a design into what a variant keeps -- its `.indicatrix` text and a small solid
//! picture -- and a stored variant back into a design.
//!
//! The text is the design file without lighting, history or attached files (none of those are
//! part of a variant, and the library database stays small). The picture is drawn the way the
//! History tab draws its steps: the flat solid raster at one fixed three-quarter pose, on a
//! transparent background, so the tile it sits in follows the theme.

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
use indicatrix_cut_core::{
    Design,
    native::{DesignExtras, design_from_str, design_to_string},
};
use indicatrix_editor::{EditorSession, solve_policy::design_to_gpu_planes_from_solved};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::io::Cursor;

/// A decoded picture's pixels, `Send` so they can cross back to the UI thread.
pub(super) type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// The edge of a stored picture in pixels: sharp enough for the 72-pixel tile on a
/// high-density screen.
pub(super) const PICTURE_EDGE_PX: u32 = 160;

/// A copy of `session`'s design as it stands at history step `position` (`None` for as it
/// stands now).
///
/// # Errors
///
/// A plain sentence when the step is out of range or cannot be replayed.
pub(super) fn design_at_step(
    session: &EditorSession,
    position: Option<usize>,
) -> Result<Design, String> {
    match position {
        Some(step) if step != session.history_position() => session
            .design_at(step)
            .map_err(|error| format!("The design at that step could not be worked out. {error}")),
        _ => Ok(session.design.clone()),
    }
}

/// The `.indicatrix` text a variant of `design` keeps.
///
/// # Errors
///
/// A plain sentence when the design cannot be written (this is not expected for a design that
/// is open in the editor).
pub(super) fn design_text(design: &Design) -> Result<String, String> {
    design_to_string(design, None, &DesignExtras::default())
        .map_err(|error| format!("The design could not be written. {error}"))
}

/// The design a variant's text holds.
///
/// # Errors
///
/// A plain sentence when the text is not a design this version can read.
pub(super) fn design_from_text(text: &str) -> Result<Design, String> {
    design_from_str(text)
        .map(|loaded| loaded.design)
        .map_err(|error| format!("The saved variant could not be read. {error}"))
}

/// `design` drawn into an `edge` by `edge` picture, or `None` when it does not solve or its
/// facets do not close into a solid.
fn draw(design: &Design, edge: u32) -> Option<Pixels> {
    let solved = design.solve().ok()?;
    let planes: Vec<(Vec3, f32)> = design_to_gpu_planes_from_solved(design, &solved)
        .iter()
        .map(|plane| (Vec3::from(plane.normal), -plane.d))
        .collect();
    let mut cache = MeshCache::default();
    let prepared = cache.get_or_build(&planes)?;
    let camera = Camera::new(
        DEFAULT_POSE.yaw,
        DEFAULT_POSE.pitch,
        fit_distance_for_radius(prepared.bounding_radius()),
        DEFAULT_FOV_DEG,
    );
    let mut raster = SolidRasterizer::new(edge.max(1), edge.max(1));
    let style = SolidStyle {
        background: [0, 0, 0, 0],
        preform_plane_count: design.preform.planes().len(),
        show_orientation_marker: false,
        ..SolidStyle::default()
    };
    raster.render_prepared(prepared, &camera, &style);
    Some(to_pixel_buffer(&raster))
}

/// PNG bytes for `pixels`.
fn encode_png(pixels: &Pixels) -> Option<Vec<u8>> {
    let image =
        image::RgbaImage::from_raw(pixels.width(), pixels.height(), pixels.as_bytes().to_vec())?;
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .ok()?;
    Some(bytes)
}

/// The PNG a variant of `design` keeps, or `None` when the design cannot be drawn (it does
/// not solve): the variant is then saved without a picture.
pub(super) fn picture_png(design: &Design) -> Option<Vec<u8>> {
    encode_png(&draw(design, PICTURE_EDGE_PX)?)
}

/// The pixels of a stored PNG, or `None` when it cannot be read (a damaged row shows "No
/// picture" instead of failing the list).
pub(super) fn decode_png(png: &[u8]) -> Option<Pixels> {
    let decoded = image::load_from_memory(png).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    let mut buffer = Pixels::new(width, height);
    let source: &[Rgba8Pixel] = bytemuck::cast_slice(decoded.as_raw());
    buffer.make_mut_slice().copy_from_slice(source);
    Some(buffer)
}
