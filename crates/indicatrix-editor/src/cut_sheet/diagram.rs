//! Renders a design's solved masts into a 2D crown/pavilion/profile faceting diagram
//! and PNG-encodes it, with no Slint/file/dialog dependency of any kind.

use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use indicatrix_cut_core::Design;
use indicatrix_solid::diagram2d::{self, DiagramConfig, DiagramStyle};

/// A rendered 2D faceting diagram, already PNG-encoded.
///
/// [`render_cut_diagram`]'s output, shared by [`super::html::cutting_sheet_html`]
/// (embedded as a `data:` URI) and the desktop's diagram export (written to disk as-is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagramImage {
    /// Pixel width, matching what was requested of [`render_cut_diagram`].
    pub width: u32,
    /// Pixel height, matching what was requested of [`render_cut_diagram`].
    pub height: u32,
    /// The frame, already PNG-encoded (RGBA8, no ICC profile -- an untagged PNG
    /// is read as sRGB by every viewer/printer, matching this diagram's fixed,
    /// non-photographic color palette).
    pub png_bytes: Vec<u8>,
}

/// Builds `design`'s three-panel (crown/pavilion/profile) 2D faceting diagram at
/// `width` x `height` from its already-[`Design::solve`]'d (or
/// [`Design::resolve_dirty`]'d) `solved` masts, and PNG-encodes it.
///
/// `None` when the design's current planes don't close to a real solid
/// ([`build_solid_mesh`] reports [`SolidStatus::Unbounded`]/[`SolidStatus::Degenerate`]
/// rather than [`SolidStatus::Closed`]) -- there is no meaningful diagram to draw
/// for a design that isn't currently a closed stone, the same condition the Edit
/// tab's own Solid/Diagram viewport already handles by showing its last-good
/// frame instead. A caller (a printed sheet, or a standalone diagram export)
/// shows this design-not-closed message itself rather than a blank or stale
/// image.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`]: `solved` must
/// have one entry per tier `design` currently has, in the same order.
#[must_use]
pub fn render_cut_diagram(
    design: &Design,
    solved: &[SolvedTier],
    width: u32,
    height: u32,
) -> Option<DiagramImage> {
    let planes = design.planes_from_solved(solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return None;
    };
    let config = DiagramConfig {
        width,
        height,
        gear_teeth: design.meta.gear_teeth_abs(),
        // `f32`: `DiagramConfig`'s own field, matching every other in-app
        // consumer of `ScheduleMeta::gear_reference_angle` (an `f64`) narrowed
        // to feed this Slint-free rendering path.
        gear_reference_angle: design.meta.gear_reference_angle as f32,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
    };
    let frame = diagram2d::render_diagram(&mesh, &config, &DiagramStyle::default());
    let png_bytes = encode_png(frame.width, frame.height, &frame.color).ok()?;
    Some(DiagramImage {
        width: frame.width,
        height: frame.height,
        png_bytes,
    })
}

/// PNG-encodes a raw RGBA8 `width * height * 4`-byte buffer -- the same
/// `PngEncoder`/`ExtendedColorType::Rgba8` call
/// `bridge::export_thread::tonemap_png::save_png` uses for its own untagged
/// (sRGB) branch, just encoding to an in-memory buffer instead of a file (this
/// diagram's bytes end up embedded in HTML or written out by
/// the desktop's diagram export, never saved directly by this function).
///
/// # Errors
///
/// The underlying encoder's error, stringified -- unreachable in practice for a
/// well-formed RGBA8 buffer matching `width`/`height`, but threaded through
/// rather than unwrapped.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}
