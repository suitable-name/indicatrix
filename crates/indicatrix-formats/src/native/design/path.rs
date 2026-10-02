//! File-name rules and media-type constants for design files. Pure path arithmetic,
//! no filesystem access.

use super::codec::FileKind;
use crate::native::{LEGACY_NATIVE_EXTENSION_SUFFIX, NATIVE_EXTENSION_SUFFIX};
use std::path::{Path, PathBuf};

/// The design file extension, without the leading dot.
pub const DESIGN_EXTENSION: &str = "indicatrix";

/// The design file extension with its leading dot, as a file picker filter wants it.
pub const DESIGN_EXTENSION_DOTTED: &str = ".indicatrix";

/// The media type of a design file (TOML text under a vendor type).
pub const DESIGN_MIME_TYPE: &str = "application/vnd.indicatrix.design+toml";

/// An HTML `accept` attribute value that admits a design file by extension or media
/// type.
pub const DESIGN_ACCEPT: &str = ".indicatrix,application/vnd.indicatrix.design+toml";

/// The human-readable name of the file type, for dialogs.
pub const DESIGN_FILE_DESCRIPTION: &str = "Indicatrix design";

/// `true` iff `path` names a design file: its last extension is `indicatrix`
/// (case-insensitive). A `name.indicatrix.toml` overlay sidecar is not one.
#[must_use]
pub fn is_design_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(DESIGN_EXTENSION))
}

/// The design file name for a design called `stem`: `stem` -> `stem.indicatrix`.
#[must_use]
pub fn design_path_for(stem: &str) -> PathBuf {
    PathBuf::from(format!("{stem}{DESIGN_EXTENSION_DOTTED}"))
}

/// The design file path next to an existing file: `dir/foo.asc` and
/// `dir/foo.indicatrix.toml` both give `dir/foo.indicatrix`.
///
/// `None` when `path` has no file name.
#[must_use]
pub fn design_path_for_sibling(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let lower = name.to_ascii_lowercase();
    let mut stem = None;
    for suffix in [NATIVE_EXTENSION_SUFFIX, LEGACY_NATIVE_EXTENSION_SUFFIX] {
        let dotted = format!(".{suffix}");
        if lower.ends_with(&dotted) {
            stem = name.get(..name.len() - dotted.len());
            break;
        }
    }
    let stem = match stem {
        Some(stem) => stem,
        None => path.file_stem()?.to_str()?,
    };
    Some(path.with_file_name(format!("{stem}{DESIGN_EXTENSION_DOTTED}")))
}

/// Tells a design file from an overlay sidecar by its file name alone.
///
/// `.indicatrix` is a [`FileKind::Design`]; `.indicatrix.toml` and `.gemcut.toml` are a
/// [`FileKind::OverlaySidecar`]; any other name is [`FileKind::Unknown`]. The name
/// is only a hint: [`super::detect_kind`] judges the content.
#[must_use]
pub fn detect_kind_of_path(path: &Path) -> FileKind {
    if is_design_path(path) {
        return FileKind::Design;
    }
    let lower = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if [NATIVE_EXTENSION_SUFFIX, LEGACY_NATIVE_EXTENSION_SUFFIX]
        .iter()
        .any(|suffix| lower.ends_with(&format!(".{suffix}")))
    {
        FileKind::OverlaySidecar
    } else {
        FileKind::Unknown
    }
}
