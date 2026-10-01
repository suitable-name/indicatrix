//! The "thin callback" half of the cutting-sheet/diagram exports: builds the bytes
//! through `indicatrix_editor::cut_sheet`, then does the plain file-system write
//! that has to live somewhere.

use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::Design;
use indicatrix_editor::cut_sheet::{cutting_sheet_document, diagram_export_png};
use std::path::Path;

/// Builds `design`/`solved`'s cutting sheet (with an embedded diagram, when the
/// design currently closes -- see `indicatrix_editor::cut_sheet::
/// cutting_sheet_document`) and writes it to `dest`.
///
/// The save-as file-picker dialog is handled by `native_io`'s own job
/// (`native_io::pick_file`, on a background thread), so this stays callable with
/// no `EditorState`/UI dependency at all, and safely callable from a background
/// thread itself. The actual `std::fs::write` also belongs on a background thread.
///
/// `custom` is threaded straight through to the sheet's "Refractive index" row.
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
    let html = cutting_sheet_document(design, solved, custom);

    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, html).map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}

/// The "thin callback" half of the standalone diagram export: the 2D diagram alone,
/// at print resolution (`indicatrix_editor::cut_sheet::diagram_export_png`),
/// written to `dest`.
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
    let png_bytes = diagram_export_png(design, solved)?;

    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, &png_bytes).map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}
