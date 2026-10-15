//! File-name rules and media-type constants for design files. Pure path arithmetic,
//! no filesystem access.
//!
//! Every save writes `.isdd` (Indicatrix Suite Design Document). The earlier
//! `.indicatrix` extension is still read, exactly like `.isdd`, but never written.

use super::codec::FileKind;
use std::path::{Path, PathBuf};

/// The design file extension every save writes, without the leading dot.
pub const DESIGN_EXTENSION: &str = "isdd";

/// [`DESIGN_EXTENSION`] with its leading dot, as a file picker filter or a file name
/// suffix wants it.
pub const DESIGN_EXTENSION_DOTTED: &str = ".isdd";

/// The extension design files carried before `.isdd`, without the leading dot. Read
/// exactly like [`DESIGN_EXTENSION`], never written.
pub const LEGACY_DESIGN_EXTENSION: &str = "indicatrix";

/// [`LEGACY_DESIGN_EXTENSION`] with its leading dot.
pub const LEGACY_DESIGN_EXTENSION_DOTTED: &str = ".indicatrix";

/// Every extension that names a design file when READING (file dialog filters, drop
/// and scan accept lists): the current one first, then the legacy one.
pub const DESIGN_READ_EXTENSIONS: [&str; 2] = [DESIGN_EXTENSION, LEGACY_DESIGN_EXTENSION];

/// The media type of a design file (TOML text under a vendor type).
pub const DESIGN_MIME_TYPE: &str = "application/vnd.indicatrix.design+toml";

/// An HTML `accept` attribute value that admits a design file by either extension or
/// by media type.
pub const DESIGN_ACCEPT: &str = ".isdd,.indicatrix,application/vnd.indicatrix.design+toml";

/// The human-readable name of the file type, for dialogs. It carries no extension: a
/// filter label appends one, e.g. `"Indicatrix design file (*.isdd)"`.
pub const DESIGN_FILE_DESCRIPTION: &str = "Indicatrix design file";

/// `true` iff `path` names a design file: its last extension is `isdd` or the legacy
/// `indicatrix` (case-insensitive). An `.indicatrix.toml` name (the removed overlay
/// sidecar) is not one.
#[must_use]
pub fn is_design_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            DESIGN_READ_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// `true` iff `path` names a design file under the legacy `.indicatrix` extension
/// (case-insensitive).
#[must_use]
pub fn is_legacy_design_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(LEGACY_DESIGN_EXTENSION))
}

/// The name endings of the retired overlay sidecar (`<name>.indicatrix.toml`, and the
/// older `<name>.gemcut.toml`). Nothing reads them any more.
const RETIRED_SIDECAR_SUFFIXES: [&str; 2] = [".indicatrix.toml", ".gemcut.toml"];

/// `true` iff `name` is a retired overlay sidecar (`.indicatrix.toml` or `.gemcut.toml`,
/// case-insensitive).
///
/// Such files are hidden wherever an old library record still carries one as an attachment,
/// so they never show up or travel with a design again.
#[must_use]
pub fn is_retired_sidecar_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    RETIRED_SIDECAR_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// The design file name for a design called `stem`: `stem` -> `stem.isdd`.
#[must_use]
pub fn design_path_for(stem: &str) -> PathBuf {
    PathBuf::from(format!("{stem}{DESIGN_EXTENSION_DOTTED}"))
}

/// The design file path next to an existing file: `dir/foo.asc` and
/// `dir/foo.indicatrix` both give `dir/foo.isdd`.
///
/// `None` when `path` has no file stem.
#[must_use]
pub fn design_path_for_sibling(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    Some(path.with_file_name(format!("{stem}{DESIGN_EXTENSION_DOTTED}")))
}

/// Tells a design file from anything else by its file name alone.
///
/// `.isdd` and the legacy `.indicatrix` are a [`FileKind::Design`]; any other name is
/// [`FileKind::Unknown`]. The name is only a hint: [`super::detect_kind`] judges the
/// content.
#[must_use]
pub fn detect_kind_of_path(path: &Path) -> FileKind {
    if is_design_path(path) {
        FileKind::Design
    } else {
        FileKind::Unknown
    }
}
