//! Writes an `.asc`/native TOML pair to disk as one atomic-as-possible operation:
//! stage both files under same-directory temp names, back up whatever already sits
//! at each destination, then rename both temp files into place.

use std::path::{Path, PathBuf};

/// Writes `asc_text`/`native_toml` to `dest_path`/`native_path` as one
/// atomic-as-possible pair: a read-only folder, a full disk, or an
/// antivirus lock must never leave a plausible-looking `.asc` on disk with its
/// authored constraints nowhere to be found (this module's own stated rule, see
/// [`setup_save_native_callback`]'s doc comment).
///
/// Both files are staged under same-directory temp sibling names first ([`temp_sibling`]
/// -- always the same filesystem as the real target, so the rename that follows is a
/// cheap, effectively-atomic same-volume operation, never one that could silently
/// fall back to copy+delete across volumes). Before either temp file is renamed into
/// place, any file ALREADY at that destination is copied to a `.bak` sibling first
/// ([`backup_existing`]): a bad save (or this very save, if the design
/// regressed since the last one) must never overwrite the only copy of a design that
/// was already on disk. Only once both backups (when needed) and both writes have
/// already succeeded are the real renames attempted. The one residual failure window
/// -- the second rename failing after the first already landed -- is reported as
/// exactly that (`.asc` on disk, sidecar not yet written) rather than silently
/// claimed as a full success.
///
/// # Errors
///
/// A ready-to-toast message naming exactly what's on disk afterward.
pub(super) fn write_pair_atomically(
    dest_path: &Path,
    native_path: &Path,
    asc_text: &str,
    native_toml: &str,
) -> Result<(), String> {
    let tmp_asc = temp_sibling(dest_path);
    let tmp_native = temp_sibling(native_path);

    if let Err(e) = std::fs::write(&tmp_asc, asc_text) {
        let _ = std::fs::remove_file(&tmp_asc);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            dest_path.display()
        ));
    }
    if let Err(e) = std::fs::write(&tmp_native, native_toml) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            native_path.display()
        ));
    }
    if let Err(message) = backup_existing(dest_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(message);
    }
    if let Err(message) = backup_existing(native_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(message);
    }
    if let Err(e) = std::fs::rename(&tmp_asc, dest_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(format!(
            "Failed to finalize {}: {e}. Nothing was saved.",
            dest_path.display()
        ));
    }
    if let Err(e) = std::fs::rename(&tmp_native, native_path) {
        return Err(format!(
            "Saved '{}' but failed to finalize its native sidecar {}: {e}. The .asc is on \
             disk without its native sidecar -- re-save once the problem is fixed.",
            dest_path.display(),
            native_path.display()
        ));
    }
    Ok(())
}

/// A same-directory temp sibling of `path`, used to stage a write before the final
/// atomic-as-possible rename -- see [`write_pair_atomically`]. Always a sibling
/// (never `std::env::temp_dir()`), so the rename that follows never crosses
/// filesystems.
pub(super) fn temp_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.tmp"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".tmp");
            s
        },
    );
    path.with_file_name(file_name)
}

/// `path`'s `.bak` sibling -- e.g. `design.asc` -> `design.asc.bak`, `design.
/// indicatrix.toml` -> `design.indicatrix.toml.bak`. One generation of backup only:
/// a second save in a row overwrites the `.bak` from the first, matching "keep the
/// PREVIOUS version" rather than an ever-growing history.
fn backup_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.bak"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".bak");
            s
        },
    );
    path.with_file_name(file_name)
}

/// Never overwrites the only copy: copies `path` to its [`backup_sibling`]
/// before [`write_pair_atomically`] renames a freshly staged temp file over it. A
/// no-op (`Ok(())`) when `path` doesn't exist yet -- a design's first save has
/// nothing to back up. Copies rather than renames `path` itself: `path` is left
/// completely untouched by this step either way, so a failed backup aborts the whole
/// save (see [`write_pair_atomically`]) without having disturbed the file that was
/// already there.
///
/// # Errors
///
/// A ready-to-toast message naming the file that could not be backed up.
fn backup_existing(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let backup = backup_sibling(path);
    std::fs::copy(path, &backup).map_err(|e| {
        format!(
            "Failed to back up {} to {} before overwriting it: {e}. Nothing was saved.",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}
