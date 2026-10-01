//! A design's printable cutting sequence as a single self-contained HTML string.
//!
//! Printable HTML, no new dependency, prints straight from the browser -- and the
//! standalone 2D faceting diagram (PNG bytes) that shares its rendering pipeline.
//!
//! Everything here is a pure function of `&Design` + `&[SolvedTier]`:
//! [`render_cut_diagram`] (mesh -> `indicatrix_solid::diagram2d` raster -> PNG
//! bytes in memory) and [`cutting_sheet_html`] (that diagram plus
//! [`indicatrix_cut_core::Design::cutting_sheet`]'s own rows -> one HTML string).
//! Nothing touches a file system, dialog or clock: the desktop writes the bytes to
//! a picked path, the web app hands them to a browser download.
//!
//! # Crown/Pavilion labelling
//!
//! The per-row Crown/Pavilion/Girdle label comes from the solver's own
//! `classify_blocks` (`design.meet_tier_inputs()` in, one `Block` per tier out,
//! aligned with the cutting sheet's one-row-per-tier order) -- never derived from
//! the sign of a formatted angle string.
//!
//! # Model-unit vs. mm masts
//!
//! The "Mast (mm)" column only appears when
//! [`indicatrix_cut_core::Design::girdle_diameter_mm`] is set -- not when the mm
//! conversion actually resolves. A design with a stated girdle diameter that
//! currently fails to close still shows the column, with `"-"` cells, rather than
//! silently reverting to a model-units-only sheet.

mod diagram;
mod html;
#[cfg(test)]
mod tests;

pub use diagram::{DiagramImage, render_cut_diagram};
pub use html::cutting_sheet_html;

/// Three-panel diagram resolution embedded in a cutting sheet.
///
/// Small enough that the resulting `data:` URI keeps the HTML file a reasonable size (a
/// few hundred KB) while still legible printed at A4/Letter width.
pub const SHEET_DIAGRAM_WIDTH: u32 = 900;
/// See [`SHEET_DIAGRAM_WIDTH`].
pub const SHEET_DIAGRAM_HEIGHT: u32 = 360;

/// The standalone diagram export's resolution -- print-quality (roughly 150 DPI at
/// A4 landscape width) rather than the cutting sheet's own smaller embed.
pub const DIAGRAM_EXPORT_WIDTH: u32 = 1800;
/// See [`DIAGRAM_EXPORT_WIDTH`].
pub const DIAGRAM_EXPORT_HEIGHT: u32 = 720;

/// `design`/`solved`'s complete cutting sheet (with an embedded diagram at
/// [`SHEET_DIAGRAM_WIDTH`] x [`SHEET_DIAGRAM_HEIGHT`] when the design currently
/// closes) as the HTML string a caller writes or downloads.
///
/// `custom` is threaded
/// through to [`cutting_sheet_html`].
///
/// # Panics
///
/// Same alignment contract as [`indicatrix_cut_core::Design::cutting_sheet`]:
/// `solved` must have one entry per tier `design` currently has, in the same order.
#[must_use]
pub fn cutting_sheet_document(
    design: &indicatrix_cut_core::Design,
    solved: &[indicatrix::geometry::meet_solver::SolvedTier],
    custom: &[indicatrix::optics::materials::GemMaterial],
) -> String {
    let diagram = render_cut_diagram(design, solved, SHEET_DIAGRAM_WIDTH, SHEET_DIAGRAM_HEIGHT);
    cutting_sheet_html(design, solved, diagram.as_ref(), custom)
}

/// The standalone diagram export: the 2D diagram alone at print resolution
/// ([`DIAGRAM_EXPORT_WIDTH`] x [`DIAGRAM_EXPORT_HEIGHT`]), PNG-encoded.
///
/// # Errors
///
/// When `design` does not currently close to a solid (nothing to diagram -- named
/// explicitly rather than silently producing a blank/stale image).
pub fn diagram_export_png(
    design: &indicatrix_cut_core::Design,
    solved: &[indicatrix::geometry::meet_solver::SolvedTier],
) -> Result<Vec<u8>, String> {
    render_cut_diagram(design, solved, DIAGRAM_EXPORT_WIDTH, DIAGRAM_EXPORT_HEIGHT)
        .map(|image| image.png_bytes)
        .ok_or_else(|| {
            "This design does not currently close to a solid -- nothing to diagram.".to_string()
        })
}
