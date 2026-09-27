//! The "thin callback" half of this group: calls [`super::diagram::render_cut_diagram`]/
//! [`super::html::cutting_sheet_html`], then does the plain file-system write that has
//! to live somewhere.

use super::{diagram::render_cut_diagram, html::cutting_sheet_html};
use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::Design;
use std::path::Path;

/// Three-panel diagram resolution embedded in a cutting sheet -- small enough
/// that the resulting `data:` URI keeps the HTML file a reasonable size (a few
/// hundred KB, not several MB) while still being legible printed at A4/Letter
/// width; a cutter wanting a larger image uses [`write_diagram_png`] instead.
const SHEET_DIAGRAM_WIDTH: u32 = 900;
/// See [`SHEET_DIAGRAM_WIDTH`].
const SHEET_DIAGRAM_HEIGHT: u32 = 360;

/// The standalone diagram export's resolution -- print-quality (roughly
/// 150 DPI at A4 landscape width) rather than the cutting sheet's own smaller
/// embed, since this is the one path meant to be viewed/printed at full size on
/// its own.
pub const DIAGRAM_EXPORT_WIDTH: u32 = 1800;
/// See [`DIAGRAM_EXPORT_WIDTH`].
pub const DIAGRAM_EXPORT_HEIGHT: u32 = 720;

/// Builds `design`/`solved`'s cutting sheet (with an embedded diagram, when the
/// design currently closes) and writes it to `dest`.
///
/// The save-as file-picker dialog is handled by `native_io`'s own job
/// (`native_io::pick_file`, on a background thread), so this stays callable with
/// no `EditorState`/UI dependency at all, and safely callable from a background
/// thread itself. The actual `std::fs::write` also belongs on a background thread.
///
/// `custom` is threaded straight through to [`super::html::cutting_sheet_html`] --
/// see that function's own doc comment.
///
/// # Errors
///
/// The write failure, ready to show as a toast.
pub fn write_cutting_sheet_html(
    design: &Design,
    solved: &[SolvedTier],
    dest: &Path,
    custom: &[GemMaterial],
) -> Result<(), String> {
    let diagram = render_cut_diagram(design, solved, SHEET_DIAGRAM_WIDTH, SHEET_DIAGRAM_HEIGHT);
    let html = cutting_sheet_html(design, solved, diagram.as_ref(), custom);

    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, html).map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}

/// The "thin callback" half of the standalone diagram export:
/// [`write_cutting_sheet_html`]'s narrower sibling -- the 2D diagram alone, at
/// print resolution ([`DIAGRAM_EXPORT_WIDTH`] x [`DIAGRAM_EXPORT_HEIGHT`]),
/// written to `dest`. Shares [`super::diagram::render_cut_diagram`] with the
/// cutting sheet rather than a second rendering path.
///
/// The save-as file-picker is handled by `native_io`, not by this function --
/// see [`write_cutting_sheet_html`]'s own doc comment.
///
/// # Errors
///
/// `Err` when `design` does not currently close to a solid (nothing to
/// diagram -- named explicitly rather than silently saving a blank/stale
/// image) or when the write itself fails.
pub fn write_diagram_png(
    design: &Design,
    solved: &[SolvedTier],
    dest: &Path,
) -> Result<(), String> {
    let Some(image) =
        render_cut_diagram(design, solved, DIAGRAM_EXPORT_WIDTH, DIAGRAM_EXPORT_HEIGHT)
    else {
        return Err(
            "This design does not currently close to a solid -- nothing to diagram.".to_string(),
        );
    };

    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, &image.png_bytes)
        .map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}
