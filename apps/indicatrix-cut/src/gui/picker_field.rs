//! Resolving a native file/folder picker's starting directory from a text field's
//! current value.

/// Resolves the starting directory for a native `rfd` file/folder picker from a path
/// text field's current value: the field's own value if it already names an existing
/// directory, that path's parent if it names an existing file, otherwise `None`
/// (callers then leave `rfd`'s directory unset). Shared by every picker call site that
/// still fills a text field this way (`camera_lighting`'s HDR environment-map path,
/// `remote::worker_callbacks`'s certificate-bundle-folder field) so "what counts as a
/// path already in the field" stays one rule.
pub(super) fn starting_dir_from_picker_field(current: &str) -> Option<std::path::PathBuf> {
    if current.is_empty() {
        return None;
    }
    let path = std::path::Path::new(current);
    if path.is_dir() {
        Some(path.to_path_buf())
    } else if path.is_file() {
        path.parent().map(std::path::Path::to_path_buf)
    } else {
        None
    }
}
