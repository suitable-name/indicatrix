//! The desktop stand-in for the Android platform shim, used by the preview binary.
//!
//! Files are picked with `rfd` dialogs on a background thread (so the UI never
//! blocks), saved with `std::fs`, and "shared" or "printed" by writing the file to
//! `<cache>/share/` and logging its path -- a desktop has no share sheet. The
//! clipboard, keep-screen-on and open-URL services are log lines. Thermal headroom is
//! not reported, and the incoming handler is stored but never called.
//!
//! On macOS `rfd` needs its dialogs on the main thread, which this stand-in does not
//! do; the preview is meant for Windows and Linux hosts.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use indicatrix_formats::native::design::{DESIGN_EXTENSION, LEGACY_DESIGN_EXTENSION};
use indicatrix_mobile_core::platform::{
    AppDirs, DocumentsCallback, IncomingCallback, OpenKind, OpenRequest, OutgoingFile, PickedFile,
    Platform, PlatformError, SaveCallback, SaveOutcome,
};
use rfd::FileDialog;

/// Extensions of design files (`.isdd`, the legacy `.indicatrix`, `.asc`, `.gem`, `.gcs`).
const DESIGN_EXTENSIONS: [&str; 5] = [
    DESIGN_EXTENSION,
    LEGACY_DESIGN_EXTENSION,
    "asc",
    "gem",
    "gcs",
];
/// A folder import reads at most this many files...
const MAX_FILES: usize = 2_000;
/// ...and at most this many bytes in all (the same caps as the Android shim).
const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// A folder import looks at most this many directory levels below the picked folder.
const MAX_DEPTH: usize = 4;

/// The preview's [`Platform`].
pub struct DesktopPlatform {
    dirs: AppDirs,
    incoming: Mutex<Option<IncomingCallback>>,
}

impl DesktopPlatform {
    /// A platform whose private directories are `dirs` (created if missing).
    #[must_use]
    pub fn new(dirs: AppDirs) -> Self {
        for dir in [&dirs.files, &dirs.cache] {
            if let Err(error) = fs::create_dir_all(dir) {
                tracing::warn!(path = %dir.display(), %error, "could not create an app directory");
            }
        }
        Self {
            dirs,
            incoming: Mutex::new(None),
        }
    }

    /// Writes `bytes` as `<cache>/share/<name>` and returns the path.
    fn write_to_share(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, PlatformError> {
        let dir = self.dirs.cache.join("share");
        fs::create_dir_all(&dir).map_err(|error| {
            PlatformError::new(format!("Could not create the share folder: {error}"))
        })?;
        let path = dir.join(safe_file_name(name, "shared-file"));
        fs::write(&path, bytes).map_err(|error| {
            PlatformError::new(format!("Could not write {}: {error}", path.display()))
        })?;
        Ok(path)
    }
}

impl Platform for DesktopPlatform {
    fn app_dirs(&self) -> &AppDirs {
        &self.dirs
    }

    fn open_documents(&self, request: OpenRequest, done: DocumentsCallback) {
        let slot = Arc::new(Mutex::new(Some(done)));
        let thread_slot = Arc::clone(&slot);
        let spawned = thread::Builder::new()
            .name("preview-open".to_owned())
            .spawn(move || {
                let result = pick(request);
                if let Some(done) = take_once(&thread_slot) {
                    done(result);
                }
            });
        if let Err(error) = spawned
            && let Some(done) = take_once(&slot)
        {
            done(Err(PlatformError::new(format!(
                "Could not open the file picker: {error}"
            ))));
        }
    }

    fn create_document(&self, file: OutgoingFile, done: SaveCallback) {
        let slot = Arc::new(Mutex::new(Some(done)));
        let thread_slot = Arc::clone(&slot);
        let spawned = thread::Builder::new()
            .name("preview-save".to_owned())
            .spawn(move || {
                let result = save(&file);
                if let Some(done) = take_once(&thread_slot) {
                    done(result);
                }
            });
        if let Err(error) = spawned
            && let Some(done) = take_once(&slot)
        {
            done(Err(PlatformError::new(format!(
                "Could not open the save dialog: {error}"
            ))));
        }
    }

    fn share(&self, file: OutgoingFile) -> Result<(), PlatformError> {
        let path = self.write_to_share(&file.suggested_name, &file.bytes)?;
        tracing::info!(
            path = %path.display(),
            mime = %file.mime,
            "share: there is no share sheet on the desktop; the file was written here"
        );
        Ok(())
    }

    fn print_html(&self, title: &str, html: &str) -> Result<(), PlatformError> {
        let path = self.write_to_share(&format!("{title}.html"), html.as_bytes())?;
        tracing::info!(
            path = %path.display(),
            "print: there is no print dialog on the desktop; the page was written here"
        );
        Ok(())
    }

    fn set_clipboard_text(&self, text: &str) -> Result<(), PlatformError> {
        let preview: String = text.chars().take(200).collect();
        tracing::info!(chars = text.chars().count(), %preview, "clipboard: text copied (logged only)");
        Ok(())
    }

    fn keep_screen_on(&self, on: bool) {
        tracing::info!(on, "keep screen on (logged only)");
    }

    fn thermal_headroom(&self) -> Option<f32> {
        None
    }

    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        tracing::info!(url, "open url (logged only)");
        Ok(())
    }

    fn set_incoming_handler(&self, handler: IncomingCallback) {
        if let Ok(mut slot) = self.incoming.lock() {
            *slot = Some(handler);
        } else {
            tracing::warn!("the incoming-files handler slot is poisoned");
        }
    }
}

/// Takes the value out of a once-slot; `None` when it is already taken or the lock
/// is poisoned.
fn take_once<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().ok()?.take()
}

/// A file-name-only, filesystem-safe version of `name` (never a path, never empty).
fn safe_file_name(name: &str, fallback: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['.', ' ']);
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Runs the dialog for `request` and reads what the user picked.
fn pick(request: OpenRequest) -> Result<Vec<PickedFile>, PlatformError> {
    match request.kind {
        OpenKind::Designs => {
            let dialog = FileDialog::new()
                .set_title("Open Designs")
                .add_filter("Designs", &DESIGN_EXTENSIONS);
            let paths: Vec<PathBuf> = if request.allow_multiple {
                dialog.pick_files().unwrap_or_default()
            } else {
                dialog.pick_file().into_iter().collect()
            };
            read_files(&paths)
        }
        OpenKind::HdrMap => {
            let path = FileDialog::new()
                .set_title("Open an Environment Map")
                .add_filter("Radiance HDR", &["hdr"])
                .pick_file();
            read_files(&path.into_iter().collect::<Vec<_>>())
        }
        OpenKind::DesignFolder => {
            let folder = FileDialog::new()
                .set_title("Import a Folder of Designs")
                .pick_folder();
            match folder {
                Some(folder) => read_files(&collect_design_files(&folder)?),
                None => Ok(Vec::new()),
            }
        }
    }
}

/// Reads every file in `paths`, refusing more than [`MAX_BYTES`] in total.
fn read_files(paths: &[PathBuf]) -> Result<Vec<PickedFile>, PlatformError> {
    let mut total: u64 = 0;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let name = display_name(path);
        let size = fs::metadata(path)
            .map_err(|error| PlatformError::new(format!("Could not read {name}: {error}")))?
            .len();
        total = total.saturating_add(size);
        if total > MAX_BYTES {
            return Err(too_large());
        }
        let bytes = fs::read(path)
            .map_err(|error| PlatformError::new(format!("Could not read {name}: {error}")))?;
        files.push(PickedFile {
            display_name: name,
            bytes,
        });
    }
    Ok(files)
}

/// The error for a folder or selection beyond the caps.
fn too_large() -> PlatformError {
    PlatformError::new("That is more than 2,000 design files or 256 MiB; pick a smaller folder.")
}

/// The file name of `path` as the user sees it.
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Whether `path` has one of the [`DESIGN_EXTENSIONS`] (case-insensitive).
fn is_design_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_lowercase();
        DESIGN_EXTENSIONS.contains(&extension.as_str())
    })
}

/// Every design file below `root`, at most [`MAX_DEPTH`] levels down, in path order.
/// Symbolic links are not followed. Fails when there are more than [`MAX_FILES`].
fn collect_design_files(root: &Path) -> Result<Vec<PathBuf>, PlatformError> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(path = %dir.display(), %error, "skipping an unreadable folder");
                continue;
            }
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    pending.push((path, depth + 1));
                }
            } else if kind.is_file() && is_design_file(&path) {
                if found.len() >= MAX_FILES {
                    return Err(too_large());
                }
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Shows the save dialog for `file` and writes it where the user chose.
fn save(file: &OutgoingFile) -> Result<SaveOutcome, PlatformError> {
    let mut dialog = FileDialog::new()
        .set_title("Save")
        .set_file_name(safe_file_name(&file.suggested_name, "design"));
    if let Some(extension) = Path::new(&file.suggested_name)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        dialog = dialog.add_filter(extension.to_uppercase(), &[extension]);
    }
    let Some(path) = dialog.save_file() else {
        return Ok(SaveOutcome::Cancelled);
    };
    fs::write(&path, &file.bytes).map_err(|error| {
        PlatformError::new(format!("Could not save {}: {error}", display_name(&path)))
    })?;
    Ok(SaveOutcome::Saved {
        display_name: display_name(&path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_file_names_are_plain_names() {
        assert_eq!(safe_file_name("Barion.asc", "x"), "Barion.asc");
        assert_eq!(safe_file_name("../../etc/passwd", "x"), "passwd");
        assert_eq!(safe_file_name("C:\\temp\\a b.html", "x"), "a b.html");
        assert_eq!(safe_file_name("a:b*c?.txt", "x"), "a_b_c_.txt");
        assert_eq!(safe_file_name("..", "fallback"), "fallback");
        assert_eq!(safe_file_name("", "fallback"), "fallback");
    }

    #[test]
    fn design_extensions_are_case_insensitive() {
        assert!(is_design_file(Path::new("a/Barion.ASC")));
        assert!(is_design_file(Path::new("x.isdd")));
        assert!(is_design_file(Path::new("x.indicatrix")));
        assert!(!is_design_file(Path::new("x.indicatrix.toml")));
        assert!(!is_design_file(Path::new("x.png")));
        assert!(!is_design_file(Path::new("noextension")));
    }

    #[test]
    fn display_name_is_the_last_component() {
        assert_eq!(display_name(Path::new("a/b/c.gem")), "c.gem");
    }

    #[test]
    fn take_once_yields_the_value_once() {
        let slot = Mutex::new(Some(7));
        assert_eq!(take_once(&slot), Some(7));
        assert_eq!(take_once(&slot), None);
    }
}
