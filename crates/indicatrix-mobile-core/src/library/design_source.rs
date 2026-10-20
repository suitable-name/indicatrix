//! Which stored file an entry opens from, and reading it as a [`Design`].
//!
//! The rule is the vault's: a self-contained `.isdd` (or legacy `.indicatrix`) design file
//! first, else the first `.asc`, else the first `.gem`, else the first `.gcs` (converted to `.asc`).
//! Other attachments (PDFs, images) are never a design.
//!
//! `.asc` has no notation for frosting, so the frosting of an entry read from one of those
//! comes from the rows the library stored with the entry (what the import of a `.gcs` or a
//! saved design file recorded), or from the `.gcs` itself; a design file states its own.

use indicatrix_cut_core::{
    Design,
    native::{DesignExtras, design_from_str, design_to_string},
};
use indicatrix_editor::loading::design_from_asc_text;
use indicatrix_vault::{
    local::{
        DesignFileKind, design_attachment_position, design_file_to_asc_text, is_native_design_name,
        native_design_attachment_position,
    },
    model::{file::AttachedFile, frosting::StoredTierFrosting},
};

use crate::cutting::progress_file_stems;

/// The attachment of `files` an entry opens from, if there is one.
pub(super) fn primary_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    let names = || files.iter().map(|file| file.name.as_str());
    native_design_attachment_position(names())
        .or_else(|| design_attachment_position(names()).map(|(position, _)| position))
        .and_then(|position| files.get(position))
}

/// The design the attachments of one entry describe, polished: for a place that has no
/// frosting to apply (measuring a file before it is stored).
pub(super) fn design_of(files: &[AttachedFile]) -> Result<Design, String> {
    design_of_with_frosting(files, &[])
}

/// The design the attachments of one entry describe, with the `frosting` the library stored
/// for the entry applied when the design is read from `.asc` cutting instructions (a design
/// file carries its own).
pub(super) fn design_of_with_frosting(
    files: &[AttachedFile],
    frosting: &[StoredTierFrosting],
) -> Result<Design, String> {
    let attachment =
        primary_attachment(files).ok_or_else(|| "This entry holds no design file.".to_owned())?;
    design_from_attachment(attachment, frosting)
}

/// The design one stored file describes.
fn design_from_attachment(
    file: &AttachedFile,
    stored: &[StoredTierFrosting],
) -> Result<Design, String> {
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
    // The stored rows win; a `.gcs` read here states its own.
    let frosting = if stored.is_empty() {
        converted.frosting
    } else {
        StoredTierFrosting::to_gcs(stored)
    };
    design_from_asc_text(&converted.asc_file_name, &converted.asc_text, None)
        .map(|loaded| {
            let mut design = loaded.design;
            design.apply_imported_frosting(&frosting);
            design
        })
        .map_err(|error| format!("\"{name}\" cannot be read: {error}"))
}

/// The names of the done-mark files the design behind `files` and `frosting` keeps its
/// cutting progress in (see `cutting::progress_file_stems`). Empty when the entry holds no
/// readable design: there is then nothing to name.
///
/// The names are the ones the open design gets, because they are computed from the text
/// the editor opens: the stored native file itself, the `.asc` a `.gem` or `.gcs` import
/// converts to, or, for a frosted entry stored as `.asc`, the native text that
/// `LibraryHandle::load` completes it to.
pub(super) fn progress_stems(
    files: &[AttachedFile],
    frosting: &[StoredTierFrosting],
) -> Vec<String> {
    let Some(attachment) = primary_attachment(files) else {
        return Vec::new();
    };
    if is_native_design_name(&attachment.name) {
        return std::str::from_utf8(&attachment.content)
            .map_or_else(|_| Vec::new(), stems_of_native_text);
    }
    let Ok(design) = design_from_attachment(attachment, frosting) else {
        return Vec::new();
    };
    if frosting.is_empty() {
        return progress_file_stems("", &design);
    }
    design_to_string(&design, None, &DesignExtras::default())
        .map_or_else(|_| Vec::new(), |text| stems_of_native_text(&text))
}

/// The done-mark file names of the design a native design file's `text` holds.
fn stems_of_native_text(text: &str) -> Vec<String> {
    design_from_str(text).map_or_else(
        |_| Vec::new(),
        |loaded| progress_file_stems(&loaded.metadata.id, &loaded.design),
    )
}
