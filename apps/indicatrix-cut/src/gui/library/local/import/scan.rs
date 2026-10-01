//! Discovers which files a import request should read: walking a folder for
//! `.asc`, `.gem` and `.gcs` candidates (with an optional, guarded recursive
//! descent) and finding a `.asc`'s own native sidecar, if one sits beside it.

use super::foreign::ForeignFormat;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tracing::warn;

/// Backstop against a pathological filesystem when [`super::wiring::setup_import_callback`]'s
/// optional subfolder recursion is enabled -- a symlink/junction loop is already caught by
/// [`collect_design_files_recursive`]'s `visited` set regardless of depth, so this only
/// guards an absurdly deep (but non-cyclic) real tree.
pub(super) const MAX_RECURSE_DEPTH: usize = 32;

/// `true` for a file Import reads: `.asc` cutting instructions, a `GemCAD` `.gem`
/// save file, or a Gem Cut Studio `.gcs` file (extension compared
/// case-insensitively). The latter two are converted to `.asc` on import -- see
/// [`super::foreign`].
#[must_use]
fn is_importable_design(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("asc"))
        || ForeignFormat::from_path(path).is_some()
}

/// Walks one directory for importable design files ([`is_importable_design`]),
/// descending into subdirectories when `recurse` is true. `visited` records the
/// canonicalized (symlink-resolved) path of every directory already entered this
/// call tree -- a symlink or junction looping back to an ancestor canonicalizes to
/// a path already in that set, so the second visit is skipped rather than
/// recursing forever (comparing raw paths wouldn't catch this, since a symlink's
/// own path text never repeats even though its target does). `depth` is capped at
/// [`MAX_RECURSE_DEPTH`] as a second, independent backstop. Both guards fail open
/// (skip and log via `warn!`) -- one bad subfolder shouldn't stop design files
/// elsewhere in the tree from being found.
pub(super) fn collect_design_files_recursive(
    dir: &Path,
    recurse: bool,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    if depth > max_depth {
        warn!(
            "Import: not descending into '{}' -- exceeded max recursion depth ({max_depth})",
            dir.display()
        );
        return;
    }
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canon) {
        warn!(
            "Import: skipping '{}' -- already visited (symlink loop?)",
            dir.display()
        );
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && is_importable_design(&p) {
            out.push(p);
        } else if recurse && p.is_dir() {
            collect_design_files_recursive(&p, recurse, depth + 1, max_depth, visited, out);
        }
    }
}

/// Resolves what [`super::pipeline::import_path`] should actually import: the single
/// file `path` names, or every `.asc`/`.gem`/`.gcs` file directly inside it (plus,
/// when `recurse`, its subfolders; see [`collect_design_files_recursive`] for the
/// symlink-loop/depth guards).
///
/// `Err` carries the user-facing message `import_path` returns verbatim -- an
/// unreadable folder, a path that is neither file nor folder, a directly picked
/// native sidecar, or a folder with no design file in it. Split out purely so
/// `import_path` stays under clippy's `too_many_lines` limit.
pub(super) fn collect_import_candidates(
    path: &Path,
    recurse: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(entries) => {
                // The top-level folder the user picked isn't itself loop-guarded (it
                // can't be reached via a symlink pointing back to itself before this
                // point) -- only directories walked below it are, via `visited`.
                let mut visited: HashSet<PathBuf> = HashSet::new();
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && is_importable_design(&p) {
                        candidates.push(p);
                    } else if recurse && p.is_dir() {
                        collect_design_files_recursive(
                            &p,
                            recurse,
                            1,
                            MAX_RECURSE_DEPTH,
                            &mut visited,
                            &mut candidates,
                        );
                    }
                }
            }
            Err(e) => return Err(format!("Could not read folder '{}': {e}", path.display())),
        }
    } else if path.is_file() {
        // A directly picked Indicatrix native sidecar (current
        // `.indicatrix.toml` or legacy `.gemcut.toml`) is not a design file this
        // import path can do anything useful with -- reaching `local::import_asc`
        // with it would come back as an opaque "parse error", with nothing pointing
        // the cutter at the button that actually opens this kind of file. Detected
        // via `indicatrix_cut_core::native::asc_path_for_native`'s own suffix check
        // (a naming guess, not a parse -- see that function's own doc comment)
        // since no file has been read yet at this point.
        if indicatrix_cut_core::native::asc_path_for_native(path).is_some() {
            return Err(format!(
                "'{}' is an Indicatrix native design file, not a .asc -- open it with \"Open \
                 Native\" in the Edit tab instead of Import.",
                path.display()
            ));
        }
        candidates.push(path.to_path_buf());
    } else {
        return Err(format!("'{}' is not a file or folder.", path.display()));
    }

    if candidates.is_empty() {
        return Err(format!(
            "No .asc, .gem or .gcs files found at '{}'.",
            path.display()
        ));
    }
    Ok(candidates)
}

/// Looks for a native sidecar sitting beside `asc_path` -- the current
/// `<stem>.indicatrix.toml` suffix first, falling back to the legacy
/// `<stem>.gemcut.toml` suffix -- and reads its bytes when one exists. `None` when
/// neither file is present, which is the ordinary case for a bare `.asc` with no
/// Indicatrix-authored history.
///
/// `Path::set_extension` is used the same way
/// `indicatrix_formats::native::path::native_path_for_asc` builds the current-suffix
/// path (a multi-segment extension like `"indicatrix.toml"` replaces everything
/// after the LAST dot in the file name, giving `stem.indicatrix.toml`, not
/// `stem.asc.indicatrix.toml`).
pub(super) fn find_native_sidecar(asc_path: &Path) -> Option<(String, Vec<u8>)> {
    let mut current = asc_path.to_path_buf();
    current.set_extension(indicatrix_cut_core::native::NATIVE_EXTENSION_SUFFIX);
    let mut legacy = asc_path.to_path_buf();
    legacy.set_extension(indicatrix_cut_core::native::LEGACY_NATIVE_EXTENSION_SUFFIX);

    [current, legacy].into_iter().find_map(|candidate| {
        let bytes = std::fs::read(&candidate).ok()?;
        let name = candidate.file_name()?.to_string_lossy().into_owned();
        Some((name, bytes))
    })
}
