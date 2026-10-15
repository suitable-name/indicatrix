//! The round trip between the open document and the library: opening a library entry,
//! saving the design back to its entry (or into a new one), and the recent-items list.
//!
//! A save has three steps so the database write can run on a worker thread while the
//! state stays on the UI thread:
//!
//! 1. [`plan_library_save`] builds the file bytes from the state (UI thread);
//! 2. [`write_library_save`] writes them through the [`LibraryHandle`] (any thread);
//! 3. [`finish_library_save`] makes the document the library entry and reads it clean when
//!    nothing was edited meanwhile (UI thread).
//!
//! [`save_to_library`] runs the three in a row for callers (and tests) that do not mind
//! waiting.

use std::sync::Arc;

use indicatrix_editor::files::converted_asc_file_name;

use super::{
    DocumentOrigin, DocumentState, OpenError, OpenReport, display_title, open::open_files,
    save_bytes, unix_now,
};
use crate::{
    app_state::AppState,
    library::{EntryId, LibraryDesign, LibraryHandle},
    platform::PickedFile,
    settings::{RecentItem, RecentKind},
};

impl DocumentState {
    /// The library entry this design is the working copy of, if it came from or was saved
    /// to the library.
    #[must_use]
    pub const fn library_entry(&self) -> Option<EntryId> {
        match self.origin {
            DocumentOrigin::Library { entry_id } => Some(entry_id),
            _ => None,
        }
    }
}

/// The library handle of `state`, opening the database on first use.
///
/// # Errors
/// The sentence from the library when its database cannot be opened.
pub fn library_handle(state: &mut AppState) -> Result<LibraryHandle, String> {
    let platform = Arc::clone(state.platform());
    state.library.handle(platform.app_dirs())
}

/// Opens a design read from the library as the current design, with the origin
/// [`DocumentOrigin::Library`] and a recent-items entry for the library entry.
///
/// The current design is replaced even when it has unsaved changes; the caller asks first.
///
/// # Errors
/// [`OpenError`] when the stored file cannot be read as a design; the current design is
/// unchanged then.
pub fn open_library(
    state: &mut AppState,
    design: LibraryDesign,
    entry_id: EntryId,
) -> Result<OpenReport, OpenError> {
    let file = PickedFile {
        display_name: design.file_name,
        bytes: design.bytes,
    };
    let report = open_files(state, vec![file], false)?;
    state.document.origin = DocumentOrigin::Library { entry_id };
    record_library_recent(state, entry_id);
    Ok(report)
}

/// Puts the library entry `entry_id` at the front of the recent items under the current
/// design's title.
pub fn record_library_recent(state: &mut AppState, entry_id: EntryId) {
    let title = display_title(state);
    state.update_settings(|settings| {
        settings.push_recent(RecentItem {
            title,
            opened_at_unix: unix_now(),
            kind: RecentKind::Library { entry_id },
        });
    });
}

/// What a save to the library writes: the file and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySavePlan {
    /// The entry to overwrite, `None` to insert a new one.
    pub entry_id: Option<EntryId>,
    /// The `.isdd` file name.
    pub file_name: String,
    /// The file.
    pub bytes: Vec<u8>,
    /// A sentence when the design is saved as a draft or does not close.
    pub warning: Option<String>,
    /// The session generation the bytes were built from.
    generation: u64,
}

/// Builds the file a save to the library writes. A Library-origin design overwrites its
/// entry, anything else inserts a new one.
///
/// # Errors
/// The sentence from [`save_bytes`] when the file cannot be built.
pub fn plan_library_save(state: &mut AppState) -> Result<LibrarySavePlan, String> {
    let generation = state.session.current_generation();
    let entry_id = state.document.library_entry();
    let saved = save_bytes(state).map_err(|error| error.message)?;
    Ok(LibrarySavePlan {
        entry_id,
        file_name: saved.file.suggested_name,
        bytes: saved.file.bytes,
        warning: saved.warning,
        generation,
    })
}

/// Writes `plan` through `handle` and returns the entry's id. An entry that no longer
/// exists (deleted from the library meanwhile) is replaced by a new one.
///
/// # Errors
/// The library's sentence when nothing could be written.
pub fn write_library_save(
    handle: &LibraryHandle,
    plan: &LibrarySavePlan,
) -> Result<EntryId, String> {
    match handle.save(plan.entry_id, &plan.file_name, &plan.bytes) {
        Ok(id) => Ok(id),
        Err(first) if plan.entry_id.is_some() => handle
            .save(None, &plan.file_name, &plan.bytes)
            .map_err(|_| first),
        Err(first) => Err(first),
    }
}

/// Records that `plan` was written as library entry `entry_id`: the document is that
/// entry now, the recent items list it, and the session reads saved unless the design
/// changed since the plan was made.
pub fn finish_library_save(state: &mut AppState, plan: &LibrarySavePlan, entry_id: EntryId) {
    if state.session.current_generation() == plan.generation {
        state.session.mark_saved();
    }
    state.document.display_name = Some(plan.file_name.clone());
    state.document.asc_filename = Some(converted_asc_file_name(&plan.file_name));
    state.document.origin = DocumentOrigin::Library { entry_id };
    record_library_recent(state, entry_id);
}

/// Saves the open design to the library through `handle`, waiting for the write. Returns
/// the entry's id.
///
/// # Errors
/// The sentence for a file that cannot be built or a database that cannot be written.
pub fn save_to_library(state: &mut AppState, handle: &LibraryHandle) -> Result<EntryId, String> {
    let plan = plan_library_save(state)?;
    let entry_id = write_library_save(handle, &plan)?;
    finish_library_save(state, &plan, entry_id);
    Ok(entry_id)
}
