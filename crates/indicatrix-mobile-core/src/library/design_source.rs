//! Which stored file an entry opens from, and reading it as a [`Design`].
//!
//! The rule is the vault's: a self-contained `.isdd` (or legacy `.indicatrix`) design file
//! first, else the first `.asc`, else the first `.gem`, else the first `.gcs` (converted to `.asc`).
//! Other attachments (PDFs, images) are never a design.

use indicatrix_cut_core::{Design, native::design_from_str};
use indicatrix_editor::loading::design_from_asc_text;
use indicatrix_vault::{
    local::{
        DesignFileKind, design_attachment_position, design_file_to_asc_text, is_native_design_name,
        native_design_attachment_position,
    },
    model::file::AttachedFile,
};

/// The attachment of `files` an entry opens from, if there is one.
pub(super) fn primary_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    let names = || files.iter().map(|file| file.name.as_str());
    native_design_attachment_position(names())
        .or_else(|| design_attachment_position(names()).map(|(position, _)| position))
        .and_then(|position| files.get(position))
}

/// The design the attachments of one entry describe.
pub(super) fn design_of(files: &[AttachedFile]) -> Result<Design, String> {
    let attachment =
        primary_attachment(files).ok_or_else(|| "This entry holds no design file.".to_owned())?;
    design_from_attachment(attachment)
}

/// The design one stored file describes.
fn design_from_attachment(file: &AttachedFile) -> Result<Design, String> {
    let name = &file.name;
    if is_native_design_name(name) {
        let text = std::str::from_utf8(&file.content)
            .map_err(|_| format!("\"{name}\" is not a text (UTF-8) design file."))?;
        return design_from_str(text)
            .map(|loaded| loaded.design)
            .map_err(|error| format!("\"{name}\" cannot be read: {error}"));
    }
    let kind = DesignFileKind::from_file_name(name)
        .ok_or_else(|| format!("\"{name}\" is not a design file."))?;
    let converted = design_file_to_asc_text(name, kind, &file.content)?;
    design_from_asc_text(&converted.asc_file_name, &converted.asc_text, None)
        .map(|loaded| loaded.design)
        .map_err(|error| format!("\"{name}\" cannot be read: {error}"))
}
