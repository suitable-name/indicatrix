//! File-name arithmetic for the self-contained `.indicatrix` design file: the name a
//! design is offered for saving under, the schedule name recorded for it, and the
//! autosave naming. Pure path logic, no file access.

use indicatrix_formats::native::design::{
    DESIGN_EXTENSION_DOTTED, design_path_for, is_design_path,
};
use std::path::{Path, PathBuf};

/// The infix that marks a recovery snapshot: `<name>.autosave.indicatrix`.
pub(super) const AUTOSAVE_INFIX: &str = ".autosave";

/// The older recovery-snapshot suffix, still read at startup: `<name>.indicatrix.autosave.toml`.
pub(super) const LEGACY_AUTOSAVE_SUFFIX: &str = ".indicatrix.autosave.toml";

/// `name` without its extension: `foo.asc` and `foo.indicatrix` both give `foo`.
/// A name with no extension is returned as it is.
fn stem_of(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
}

/// The file name a design recorded as `schedule_name` (`foo.asc`) is offered for
/// saving under: `foo.indicatrix`.
pub(super) fn design_file_name_for(schedule_name: &str) -> String {
    design_path_for(stem_of(schedule_name))
        .to_string_lossy()
        .into_owned()
}

/// `path` as a design file path: unchanged when it already ends in `.indicatrix`, with
/// `.indicatrix` appended otherwise (a save dialog on some platforms returns the name
/// exactly as typed).
pub(super) fn ensure_design_extension(path: PathBuf) -> PathBuf {
    if is_design_path(&path) {
        return path;
    }
    let mut name = path.file_name().map_or_else(
        || std::ffi::OsString::from("design"),
        std::ffi::OsStr::to_os_string,
    );
    name.push(DESIGN_EXTENSION_DOTTED);
    path.with_file_name(name)
}

/// `true` when `path` names a recovery snapshot: a design file whose stem ends in
/// `.autosave`.
pub(super) fn is_autosave_design_path(path: &Path) -> bool {
    is_design_path(path)
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.to_ascii_lowercase().ends_with(AUTOSAVE_INFIX))
}

/// The design name a design file at `path` stands for: its file stem, minus the
/// `.autosave` infix of a recovery snapshot (so a recovered `foo.autosave.indicatrix`
/// is offered for saving as `foo.indicatrix`).
pub(super) fn design_stem_of_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let trimmed = if stem.to_ascii_lowercase().ends_with(AUTOSAVE_INFIX) {
        &stem[..stem.len() - AUTOSAVE_INFIX.len()]
    } else {
        stem
    };
    Some(trimmed.to_string())
}

/// The schedule name (`foo.asc`) recorded for a design opened from the design file at
/// `path`: its stem plus `.asc`. The editor keys its window title, catalogue write-back
/// and "Export .asc" default name on this, though no `.asc` exists on disk.
pub(super) fn schedule_name_for_design_path(path: &Path) -> String {
    format!(
        "{}.asc",
        design_stem_of_path(path).unwrap_or_else(|| "design".to_string())
    )
}

/// Whether `path` is the design file that `schedule_name` (the design's recorded
/// `.asc` name) belongs to: same stem, compared case-insensitively. A quick Save
/// writes straight to the remembered path only when this holds, so a design replaced
/// by another one never overwrites the previous design's file.
pub(super) fn path_belongs_to_schedule_name(path: &Path, schedule_name: &str) -> bool {
    is_design_path(path)
        && !is_autosave_design_path(path)
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case(stem_of(schedule_name)))
}

/// The `.autosave.indicatrix` file name an autosave of the design recorded as
/// `schedule_name` (`foo.asc`, or `None` for one never saved) is filed under.
pub(super) fn autosave_file_name(schedule_name: Option<&str>) -> String {
    format!(
        "{}{AUTOSAVE_INFIX}{DESIGN_EXTENSION_DOTTED}",
        autosave_base_name(schedule_name)
    )
}

/// The older recovery file name for the same design: `foo.indicatrix.autosave.toml`.
pub(super) fn legacy_autosave_file_name(schedule_name: Option<&str>) -> String {
    format!(
        "{}{LEGACY_AUTOSAVE_SUFFIX}",
        autosave_base_name(schedule_name)
    )
}

/// The base name an autosave is filed under: the design's own schedule name with the
/// `.asc` extension stripped, or `"untitled"` for a design never yet paired with one.
pub(super) fn autosave_base_name(schedule_name: Option<&str>) -> String {
    let name = schedule_name.unwrap_or("untitled");
    name.strip_suffix(".asc").unwrap_or(name).to_string()
}

/// `true` when `file_name` is a recovery snapshot in either the current or the
/// older naming.
pub(super) fn is_autosave_file_name(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(LEGACY_AUTOSAVE_SUFFIX)
        || lower.ends_with(&format!("{AUTOSAVE_INFIX}{DESIGN_EXTENSION_DOTTED}"))
}
