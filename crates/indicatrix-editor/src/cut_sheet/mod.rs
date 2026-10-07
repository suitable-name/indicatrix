//! A design's printable cutting sequence as a single self-contained HTML string.
//!
//! Printable HTML, no new dependency, prints straight from the browser -- and the
//! standalone 2D faceting diagram (PNG bytes) that shares its rendering pipeline.
//!
//! Everything here is a pure function of `&Design` + `&[SolvedTier]` (+ the caller's
//! [`SheetDetails`]): [`render_cut_diagram`] and [`render_cut_views`] (mesh ->
//! `indicatrix_solid::diagram2d` raster -> PNG bytes in memory) and
//! [`cutting_sheet_document_with`] (the views plus [`SheetLayout`], built from
//! [`indicatrix_cut_core::Design::cutting_sheet`]'s own rows -> one HTML string).
//! Nothing touches a file system, dialog or clock: the desktop writes the bytes to
//! a picked path, the web app hands them to a browser download, and the export date
//! is text the caller supplies.
//!
//! # Layout
//!
//! The printed sheet follows the owner's fantasy-cut template: header, Facet Data, Size
//! Data, Design Data, four views, comments, then a Pavilion and a Crown section. The
//! counts, ratios and rows are computed once in [`SheetLayout`]; the HTML and the text
//! sheet are both written from it.
//!
//! # Pavilion/Crown sections
//!
//! A row's section comes from the solver's own `classify_blocks`
//! (`design.meet_tier_inputs()` in, one `Block` per tier out, aligned with the cutting
//! sheet's one-row-per-tier order) -- never derived from the sign of a formatted angle
//! string. The girdle rows are part of the Pavilion section.
//!
//! # Model-unit vs. mm masts
//!
//! The "Mast (mm)" column only appears when
//! [`indicatrix_cut_core::Design::girdle_diameter_mm`] is set -- not when the mm
//! conversion actually resolves. A design with a stated girdle diameter that
//! currently fails to close still shows the column, with `"-"` cells, rather than
//! silently reverting to a model-units-only sheet.

mod details;
mod diagram;
mod html;
mod layout;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_layout;

pub use details::{SheetDetails, month_year_text};
pub use diagram::{CutViews, DiagramImage, render_cut_diagram, render_cut_views};
pub use html::{cutting_sheet_html, cutting_sheet_html_with};
pub use layout::{BlockCount, FacetData, SheetLayout, SizeData, UNTITLED_HEADING};

/// Three-panel diagram resolution of the standalone [`render_cut_diagram`] embed.
///
/// The printed sheet itself no longer embeds this picture: it shows the four views of
/// [`SHEET_VIEW_WIDTH`] x [`SHEET_VIEW_HEIGHT`].
pub const SHEET_DIAGRAM_WIDTH: u32 = 900;
/// See [`SHEET_DIAGRAM_WIDTH`].
pub const SHEET_DIAGRAM_HEIGHT: u32 = 360;

/// Resolution of each of the four views on a printed cutting sheet.
///
/// Two views per row on A4, so each prints about 90 mm wide at roughly 160 dpi, while the
/// four `data:` URIs together keep the HTML file a few hundred KB.
pub const SHEET_VIEW_WIDTH: u32 = 560;
/// See [`SHEET_VIEW_WIDTH`].
pub const SHEET_VIEW_HEIGHT: u32 = 440;

/// The standalone diagram export's resolution -- print-quality (roughly 150 DPI at
/// A4 landscape width) rather than the cutting sheet's own smaller embed.
pub const DIAGRAM_EXPORT_WIDTH: u32 = 1800;
/// See [`DIAGRAM_EXPORT_WIDTH`].
pub const DIAGRAM_EXPORT_HEIGHT: u32 = 720;

/// The complete cutting instructions as the HTML string a caller writes or downloads.
///
/// With the four views when the design currently closes, and the header words from
/// [`SheetDetails::from_design`]: exactly [`cutting_sheet_document_with`] with those details.
/// `custom` is threaded through to the printed refractive index.
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
    cutting_sheet_document_with(design, solved, custom, &SheetDetails::from_design(design))
}

/// [`cutting_sheet_document`] with explicit header `details` (title, author, date, shape,
/// comments, ranges). Pure: it reads no clock, so the date is whatever `details.date_text`
/// says.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
#[must_use]
pub fn cutting_sheet_document_with(
    design: &indicatrix_cut_core::Design,
    solved: &[indicatrix::geometry::meet_solver::SolvedTier],
    custom: &[indicatrix::optics::materials::GemMaterial],
    details: &SheetDetails,
) -> String {
    let views = render_cut_views(design, solved, SHEET_VIEW_WIDTH, SHEET_VIEW_HEIGHT);
    cutting_sheet_html_with(design, solved, views.as_ref(), custom, details)
}

/// The cutting instructions as plain text, under plain headings.
///
/// The blocks are the HTML sheet's (header, Facet Data, Size Data, Design Data, comments,
/// Pavilion, Crown). The four views are pictures and appear only in the HTML.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
#[must_use]
pub fn cutting_sheet_text_with(
    design: &indicatrix_cut_core::Design,
    solved: &[indicatrix::geometry::meet_solver::SolvedTier],
    custom: &[indicatrix::optics::materials::GemMaterial],
    details: &SheetDetails,
) -> String {
    SheetLayout::build(design, solved, custom, details).to_text()
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
