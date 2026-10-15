//! The autosave: the open design as a `.isdd` file plus a small JSON note, under
//! `<files>/autosave/`, so a design survives the system killing the app.
//!
//! - `current.isdd` is the design as "Save design" writes it, except that the
//!   `[meta]` table is written unstamped (the autosave does not change the modification
//!   time).
//! - `current.json` says where the design came from, its file name and whether it had
//!   unsaved changes.
//!
//! [`autosave`] is cheap when nothing changed: it compares the session's generation and
//! dirty flag with the ones it last wrote and returns. Both files are written to a
//! temporary name and renamed, so a crash never leaves half a file. [`restore`] reads
//! them back at start-up; files that do not parse are renamed aside (never a panic, never
//! a lost file).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use indicatrix_cut_core::native::{DesignExtras, design_from_str, design_to_string};
use indicatrix_formats::native::design::DESIGN_EXTENSION_DOTTED;
use serde::{Deserialize, Serialize};

use super::{DocumentOrigin, display_title, install, open::from_design_file, unix_now};
use crate::app_state::AppState;

/// The directory below the app's files directory.
const DIR: &str = "autosave";
/// The stem of the design file in it (the extension is `DESIGN_EXTENSION_DOTTED`).
const DESIGN_STEM: &str = "current";
/// The note beside it.
const NOTE_FILE: &str = "current.json";
/// The version of [`Note`].
const NOTE_VERSION: u32 = 1;

/// What the last autosave wrote: the session generation and whether it was dirty. `None`
/// until the first write or restore.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Mark(Option<(u64, bool)>);

/// The JSON note.
#[derive(Debug, Serialize, Deserialize)]
struct Note {
    /// [`NOTE_VERSION`].
    version: u32,
    /// Where the design came from.
    origin: DocumentOrigin,
    /// The file name the user saw.
    display_name: Option<String>,
    /// The recorded `.asc` name.
    asc_filename: Option<String>,
    /// Whether the design had unsaved changes.
    dirty: bool,
    /// The session generation at the write (informational).
    generation: u64,
}

/// What [`restore`] brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// The title of the restored design.
    pub title: String,
    /// Whether it had unsaved changes when the app went away.
    pub dirty: bool,
    /// Sentences about the file (a material that could not be restored).
    pub notes: Vec<String>,
}

fn dir_of(state: &AppState) -> PathBuf {
    state.platform().app_dirs().files.join(DIR)
}

/// The autosaved design's file name (`current.isdd`).
fn design_file_name() -> String {
    format!("{DESIGN_STEM}{DESIGN_EXTENSION_DOTTED}")
}

/// Writes the open design to the autosave files when it changed since the last write.
/// Cheap (two comparisons) when it did not. A start-up design nobody touched is not
/// written.
///
/// # Errors
/// The I/O error of the write; the next call tries again.
pub fn autosave(state: &mut AppState) -> io::Result<()> {
    let generation = state.session.current_generation();
    let dirty = state.session.is_dirty();
    let mark = Mark(Some((generation, dirty)));
    if state.document.autosaved == mark {
        return Ok(());
    }
    let untouched = state.document.origin == DocumentOrigin::Startup
        && state.session.design.tiers.is_empty()
        && !dirty;
    if untouched {
        state.document.autosaved = mark;
        return Ok(());
    }
    let design = &state.session.design;
    let history = state.session.history.description_log().to_vec();
    let snapshot = super::save_snapshot(state);
    let text = design_to_string(
        design,
        state.document.printed_proportions.as_ref(),
        &DesignExtras {
            custom_material: snapshot.as_ref(),
            history_entries: &history,
            metadata: Some(&state.document.metadata),
            attachments: &state.document.attachments,
        },
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let note = Note {
        version: NOTE_VERSION,
        origin: state.document.origin.clone(),
        display_name: state.document.display_name.clone(),
        asc_filename: state.document.asc_filename.clone(),
        dirty,
        generation,
    };
    let json = serde_json::to_vec(&note)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let dir = dir_of(state);
    fs::create_dir_all(&dir)?;
    write_atomically(&dir.join(design_file_name()), text.as_bytes())?;
    write_atomically(&dir.join(NOTE_FILE), &json)?;
    state.document.autosaved = mark;
    Ok(())
}

/// Writes `bytes` to `path` through `<path>.tmp` and a rename.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

/// Restores the design of the last autosave, if there is one. Returns `None` when there
/// is none or it could not be read (a file that does not parse is renamed to
/// `<name>.corrupt-<unix seconds>`).
///
/// Call it once at start-up, before the first push. A design that had unsaved changes
/// when the app went away still reads dirty.
pub fn restore(state: &mut AppState) -> Option<RestoreReport> {
    let dir = dir_of(state);
    let design_path = dir.join(design_file_name());
    let bytes = match fs::read(&design_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(%error, path = %design_path.display(), "autosave unreadable");
            }
            return None;
        }
    };
    let note_path = dir.join(NOTE_FILE);
    let note = fs::read_to_string(&note_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Note>(&text).ok());
    match rebuild(&bytes, note.as_ref()) {
        Ok(loaded) => {
            let super::open::Loaded {
                session,
                document,
                restored,
                notes,
                ..
            } = loaded;
            install(state, session, document, restored);
            let dirty = note.as_ref().is_some_and(|note| note.dirty);
            if dirty {
                // Generations only grow, so the maximum reads as "never saved" until the
                // next save.
                state.session.saved_generation = u64::MAX;
            }
            state.document.autosaved = Mark(Some((
                state.session.current_generation(),
                state.session.is_dirty(),
            )));
            Some(RestoreReport {
                title: display_title(state),
                dirty,
                notes,
            })
        }
        Err(reason) => {
            tracing::warn!(%reason, "the autosave is damaged; moved aside");
            move_aside(&design_path);
            move_aside(&note_path);
            None
        }
    }
}

/// The design of the autosave, with the origin and names of its note.
fn rebuild(bytes: &[u8], note: Option<&Note>) -> Result<super::open::Loaded, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_owned())?;
    let parsed = design_from_str(text).map_err(|error| error.to_string())?;
    let display_name = note.and_then(|note| note.display_name.as_deref());
    let asc_filename = note.and_then(|note| note.asc_filename.clone());
    let mut loaded = from_design_file(parsed, asc_filename, display_name.unwrap_or_default());
    if let Some(note) = note {
        loaded.document.origin = note.origin.clone();
        loaded.document.display_name.clone_from(&note.display_name);
    } else {
        loaded.document.origin = DocumentOrigin::New;
        loaded.document.display_name = None;
    }
    Ok(loaded)
}

/// Renames `path` to `<path>.corrupt-<unix seconds>` (best effort).
fn move_aside(path: &Path) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".corrupt-{}", unix_now()));
    if let Err(error) = fs::rename(path, PathBuf::from(aside)) {
        tracing::debug!(%error, path = %path.display(), "could not move the autosave aside");
    }
}
