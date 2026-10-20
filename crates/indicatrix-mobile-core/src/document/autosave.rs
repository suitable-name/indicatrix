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
//!
//! A design deleted from the library must not come back from the autosave:
//! [`DocumentState::forget_library_entries`] removes an autosave that describes a deleted
//! entry and says what becomes of the open design (see there).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use indicatrix_cut_core::native::{DesignExtras, design_from_str, design_to_string};
use indicatrix_editor::EditorSession;
use indicatrix_formats::native::design::DESIGN_EXTENSION_DOTTED;
use serde::{Deserialize, Serialize};

use super::{
    DocumentOrigin, DocumentState, display_title, install, open::from_design_file, unix_now,
};
use crate::{app_state::AppState, library::EntryId};

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
    let temporary = temporary_of(path);
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

/// The temporary name [`write_atomically`] writes `path`'s bytes to first.
fn temporary_of(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    PathBuf::from(temporary)
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

impl DocumentState {
    /// Settles the autosave and the open design after the library entries `entries` were
    /// deleted. `files` is the app's files directory and `session` the open design's.
    ///
    /// - The autosave is deleted when it describes one of the entries: its note names the
    ///   entry as where the design came from, or this document wrote it (see
    ///   [`autosave`]) and is itself the working copy of one of them. Otherwise a restart
    ///   would bring the deleted design back.
    /// - When the open design is the working copy of one of the entries, it stays open (the
    ///   user may be looking at it) but belongs to no entry any more: its origin becomes
    ///   [`DocumentOrigin::New`], so "Save" no longer overwrites or re-creates an entry by
    ///   itself, and it reads as unsaved, so replacing it asks first. The autosave does not
    ///   write it again until the design changes; an edit after the delete is the user's
    ///   own act and is autosaved like any unsaved design.
    ///
    /// Returns whether the open design was one of the deleted entries' working copy.
    pub fn forget_library_entries(
        &mut self,
        session: &mut EditorSession,
        files: &Path,
        entries: &[EntryId],
    ) -> bool {
        let open_deleted = self
            .library_entry()
            .is_some_and(|entry| entries.contains(&entry));
        let dir = files.join(DIR);
        let wrote_it = self.autosaved != Mark::default();
        if (open_deleted && wrote_it) || note_names_one_of(&dir, entries) {
            remove_autosave_files(&dir);
        }
        if !open_deleted {
            return false;
        }
        self.origin = DocumentOrigin::New;
        // Generations only grow, so the maximum reads as "never saved" (what `restore`
        // does for a design that had unsaved changes).
        session.saved_generation = u64::MAX;
        self.autosaved = Mark(Some((session.current_generation(), session.is_dirty())));
        true
    }
}

/// Whether the autosave note in `dir` names the library entry of one of `entries` as where
/// the autosaved design came from. A missing or unreadable note names nothing.
fn note_names_one_of(dir: &Path, entries: &[EntryId]) -> bool {
    let note = fs::read_to_string(dir.join(NOTE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Note>(&text).ok());
    matches!(
        note,
        Some(Note {
            origin: DocumentOrigin::Library { entry_id },
            ..
        }) if entries.contains(&entry_id)
    )
}

/// Deletes the autosave of `state`'s app, so a closed design does not come back at the next
/// start.
pub(super) fn remove_autosave(state: &AppState) {
    remove_autosave_files(&dir_of(state));
}

/// Deletes the autosave's two files and any temporary file beside them. A file that is not
/// there is fine; any other failure is logged.
fn remove_autosave_files(dir: &Path) {
    for name in [design_file_name(), NOTE_FILE.to_owned()] {
        let path = dir.join(name);
        for target in [temporary_of(&path), path] {
            if let Err(error) = fs::remove_file(&target)
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(%error, path = %target.display(), "the autosave could not be removed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use indicatrix_cut_core::Edit;

    use super::*;
    use crate::{document::new_from_template, platform::Platform, platform_mock::MockPlatform};

    /// A saved round brilliant of the library entry `entry_id` (a template, so it has tiers),
    /// with the autosave written for it.
    fn autosaved_entry(entry_id: EntryId) -> (AppState, Arc<MockPlatform>) {
        let mock = Arc::new(MockPlatform::default());
        let platform: Arc<dyn Platform> = mock.clone();
        let mut state = AppState::new(platform);
        new_from_template(&mut state, 1).expect("template 1 exists");
        state.session.mark_saved();
        state.document.origin = DocumentOrigin::Library { entry_id };
        autosave(&mut state).expect("writes");
        (state, mock)
    }

    /// The autosave's design file and note.
    fn autosave_files(mock: &MockPlatform) -> (PathBuf, PathBuf) {
        let dir = mock.app_dirs().files.join(DIR);
        (dir.join(design_file_name()), dir.join(NOTE_FILE))
    }

    #[test]
    fn deleting_the_entry_of_the_open_design_removes_its_autosave() {
        let (mut state, mock) = autosaved_entry(42);
        let (design, note) = autosave_files(&mock);
        assert!(design.is_file() && note.is_file());
        let files = mock.app_dirs().files.clone();

        let detached = state
            .document
            .forget_library_entries(&mut state.session, &files, &[41, 42]);

        assert!(detached);
        assert!(!design.exists() && !note.exists());
        // The design stays open, belongs to no entry and reads as unsaved.
        assert_eq!(state.document.origin, DocumentOrigin::New);
        assert_eq!(state.document.library_entry(), None);
        assert!(state.session.is_dirty());
        assert_ne!(
            state.session.design.tiers,
            [] as [indicatrix_cut_core::ConstraintTier; 0]
        );

        // Nothing writes the deleted design back while it is unchanged ...
        autosave(&mut state).expect("nothing to write");
        assert!(!design.exists(), "the deleted design came back");
        // ... an edit is the user's own doing and is autosaved like any unsaved design.
        state
            .session
            .apply(Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(6.0),
            })
            .expect("the edit applies");
        autosave(&mut state).expect("writes");
        assert!(design.is_file());
        let text = fs::read_to_string(&note).expect("the note is written");
        assert!(text.contains("\"type\":\"new\""), "{text}");
    }

    #[test]
    fn an_autosave_of_another_entry_stays() {
        let (mut state, mock) = autosaved_entry(7);
        let (design, note) = autosave_files(&mock);
        let files = mock.app_dirs().files.clone();

        let detached = state
            .document
            .forget_library_entries(&mut state.session, &files, &[8, 9]);

        assert!(!detached);
        assert!(design.is_file() && note.is_file());
        assert_eq!(state.document.library_entry(), Some(7));
        assert!(!state.session.is_dirty());
    }

    #[test]
    fn a_stale_autosave_of_a_deleted_entry_goes_although_another_design_is_open() {
        // The autosave on disk still describes entry 5; a design of entry 6 was opened
        // since and has not been written yet.
        let (mut state, mock) = autosaved_entry(5);
        state.document.origin = DocumentOrigin::Library { entry_id: 6 };
        let (design, note) = autosave_files(&mock);
        let files = mock.app_dirs().files.clone();

        let detached = state
            .document
            .forget_library_entries(&mut state.session, &files, &[5]);

        assert!(!detached);
        assert!(!design.exists() && !note.exists());
        assert_eq!(state.document.library_entry(), Some(6));
        assert!(!state.session.is_dirty());
    }

    #[test]
    fn the_autosave_the_open_design_wrote_goes_even_when_its_note_cannot_be_read() {
        let (mut state, mock) = autosaved_entry(3);
        let (design, note) = autosave_files(&mock);
        fs::write(&note, b"{").expect("the note can be damaged");
        let files = mock.app_dirs().files.clone();

        let detached = state
            .document
            .forget_library_entries(&mut state.session, &files, &[3]);

        assert!(detached);
        assert!(!design.exists() && !note.exists());
    }

    #[test]
    fn an_unreadable_note_names_no_entry() {
        let (mut state, mock) = autosaved_entry(3);
        let (design, note) = autosave_files(&mock);
        fs::write(&note, b"{").expect("the note can be damaged");
        state.document.origin = DocumentOrigin::Library { entry_id: 4 };
        let files = mock.app_dirs().files.clone();

        let detached = state
            .document
            .forget_library_entries(&mut state.session, &files, &[3]);

        assert!(!detached);
        assert!(design.is_file() && note.is_file());
    }

    #[test]
    fn deleting_with_no_autosave_is_fine() {
        let mock = Arc::new(MockPlatform::default());
        let platform: Arc<dyn Platform> = mock.clone();
        let mut state = AppState::new(platform);
        let files = mock.app_dirs().files.clone();

        let detached = state
            .document
            .forget_library_entries(&mut state.session, &files, &[1]);

        assert!(!detached);
        assert_eq!(state.document.origin, DocumentOrigin::Startup);
    }
}
