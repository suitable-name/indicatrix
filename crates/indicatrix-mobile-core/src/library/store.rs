//! Reading and writing single entries: the design file behind an entry, saving a design
//! into the library, and the tag, ignore and delete operations.

use std::panic::{AssertUnwindSafe, catch_unwind};

use indicatrix_cut_core::native::{DesignExtras, design_to_string};
use indicatrix_editor::files::sanitize_file_name;
use indicatrix_formats::native::design::DESIGN_EXTENSION_DOTTED;
use indicatrix_vault::{
    db::sqlite::Database,
    local::{self, ImportedAsc},
    model::entry::FullDiagramRecord,
};

use super::{
    EntryId, LibraryDesign, LibraryHandle,
    design_source::{design_of_with_frosting, primary_attachment, progress_stems},
    handle::db_error,
    import::{invalidate_caches, panic_text},
    measure::{apply_measured_metadata, fill_designer_info, merge_existing_metadata},
};
use crate::{
    app_state::AppState,
    cutting,
    platform::{OutgoingFile, mime},
};

/// The sentence for an id the library does not hold.
const GONE: &str = "That design is no longer in the library.";

impl LibraryHandle {
    /// The file entry `id` opens from: its `.isdd` (or legacy `.indicatrix`) design
    /// file, else its `.asc` (a `.gem` or `.gcs` import stores the `.asc` it converts to), else the `.gem` or
    /// `.gcs` itself. The bytes are exactly the stored ones.
    ///
    /// One exception: a frosted design stored as `.asc` (a `.gcs` import that frosts a tier
    /// or a facet records that beside the `.asc`, which cannot state it) opens as the native
    /// design file those rows complete, named after the entry's title, so the frosting is not
    /// lost on the way into the editor.
    ///
    /// # Errors
    /// A sentence when the entry is gone or holds no design file.
    pub fn load(&self, id: EntryId) -> Result<LibraryDesign, String> {
        let record = self.record(id)?;
        let file = primary_attachment(&record.attached_files)
            .ok_or_else(|| "This entry holds no design file.".to_owned())?;
        if !record.frosting.is_empty() && !local::is_native_design_name(&file.name) {
            let design = design_of_with_frosting(&record.attached_files, &record.frosting)?;
            let text =
                design_to_string(&design, None, &DesignExtras::default()).map_err(|error| {
                    format!("The design cannot be written as a design file: {error}")
                })?;
            return Ok(LibraryDesign {
                file_name: format!(
                    "{}{DESIGN_EXTENSION_DOTTED}",
                    sanitize_file_name(&record.title)
                ),
                bytes: text.into_bytes(),
            });
        }
        Ok(LibraryDesign {
            file_name: file.name.clone(),
            bytes: file.content.clone(),
        })
    }

    /// Saves the native design file `native_bytes` (named `file_name`, as
    /// `document::save_bytes` made it) into the library and returns its entry id.
    ///
    /// With `id` of an existing entry the entry is overwritten in place: the stored
    /// design file, the angle table and the measured metadata are replaced, hand-entered
    /// metadata (designer, competition, ...) and the url stay, the title follows the new
    /// file, and the cached previews are dropped. Otherwise a new entry is inserted; if
    /// another entry already has the name, the new one is named `<name> (2).<ext>` and
    /// so on, so one design never overwrites another. Designs saved here are the user's
    /// own (`local://` urls), which is what "Mine" selects.
    ///
    /// # Errors
    /// A sentence when `native_bytes` is not a design file or the database cannot be
    /// written.
    pub fn save(
        &self,
        id: Option<EntryId>,
        file_name: &str,
        native_bytes: &[u8],
    ) -> Result<EntryId, String> {
        let target = id.filter(|&id| self.entry_exists(id));
        let name = if target.is_some() {
            file_name.to_owned()
        } else {
            self.free_file_name(file_name)?
        };
        let parsed = parse_native(&name, native_bytes)?;
        let ImportedAsc {
            entry,
            mut detail,
            extras,
            ..
        } = parsed;
        let database = self.lock();
        let overwritten = match target {
            Some(id) => database
                .get_diagram_full(id)
                .map_err(db_error("The stored design could not be read"))?
                .map(|existing| (id, existing)),
            None => None,
        };
        let saved_id = if let Some((id, existing)) = overwritten {
            merge_existing_metadata(&mut detail, &existing);
            database
                .save_diagram_detail(&detail, id)
                .map_err(db_error("The design could not be stored"))?;
            let title = entry.title.trim();
            if !title.is_empty() && title != existing.title {
                database
                    .rename_diagram_entry(id, title)
                    .map_err(db_error("The design could not be renamed"))?;
            }
            invalidate_caches(&database, id);
            id
        } else {
            database
                .save_design(&entry, &detail, local::LOCAL_SOURCE_ID)
                .map_err(db_error("The design could not be stored"))?
        };
        if let Err(error) = local::apply_imported_extras(&database, saved_id, &extras) {
            tracing::warn!(
                "Library save: could not restore the tags of entry #{saved_id}: {error}"
            );
        }
        drop(database);
        self.remove_thumbs(saved_id);
        self.bump();
        Ok(saved_id)
    }

    /// The file to share for entry `id`: its native design file, or, for an entry
    /// that only holds `.asc` cutting instructions, a native file made from them.
    ///
    /// # Errors
    /// A sentence when the entry is gone or its design cannot be read.
    pub fn export_bytes(&self, id: EntryId) -> Result<OutgoingFile, String> {
        let record = self.record(id)?;
        let file = primary_attachment(&record.attached_files)
            .ok_or_else(|| "This entry holds no design file.".to_owned())?;
        if local::is_native_design_name(&file.name) {
            return Ok(OutgoingFile {
                suggested_name: file.name.clone(),
                mime: mime::INDICATRIX_DESIGN.to_owned(),
                bytes: file.content.clone(),
            });
        }
        let design = design_of_with_frosting(&record.attached_files, &record.frosting)?;
        let text = design_to_string(&design, None, &DesignExtras::default())
            .map_err(|error| format!("The design cannot be written as a design file: {error}"))?;
        Ok(OutgoingFile {
            suggested_name: format!(
                "{}{DESIGN_EXTENSION_DOTTED}",
                sanitize_file_name(&record.title)
            ),
            mime: mime::INDICATRIX_DESIGN.to_owned(),
            bytes: text.into_bytes(),
        })
    }

    /// Makes `tags` the exact tag set of every entry in `ids` (tags the entry has that
    /// are not listed are removed). Blank tags are dropped and names compare
    /// case-insensitively; a tag that already exists keeps its stored spelling. Use
    /// [`Self::add_tags`] to add without removing.
    ///
    /// # Errors
    /// A sentence when the database cannot be written; entries before the failing one
    /// are already changed.
    pub fn set_tags(&self, ids: &[EntryId], tags: &[String]) -> Result<(), String> {
        let wanted = clean_tags(tags);
        let database = self.lock();
        let mut result: Result<(), String> = Ok(());
        for &id in ids {
            result = replace_tags(&database, id, &wanted);
            if result.is_err() {
                break;
            }
        }
        drop(database);
        self.bump();
        result
    }

    /// Adds `tags` to every entry in `ids`, keeping the tags they have. Blank tags are
    /// dropped; adding a tag an entry already has changes nothing.
    ///
    /// # Errors
    /// A sentence when the database cannot be written.
    pub fn add_tags(&self, ids: &[EntryId], tags: &[String]) -> Result<(), String> {
        let wanted = clean_tags(tags);
        let database = self.lock();
        let mut result = Ok(());
        'entries: for &id in ids {
            for name in &wanted {
                if let Err(error) = database.add_tag_to_entry(id, name) {
                    result = Err(format!("A tag could not be added: {error:#}."));
                    break 'entries;
                }
            }
        }
        drop(database);
        self.bump();
        result
    }

    /// Marks every entry in `ids` ignored (hidden from the list unless the query shows
    /// ignored entries) or restores it.
    ///
    /// # Errors
    /// A sentence when the database cannot be written.
    pub fn set_ignored(&self, ids: &[EntryId], ignored: bool) -> Result<(), String> {
        let database = self.lock();
        let mut result = Ok(());
        for &id in ids {
            if let Err(error) = database.set_diagram_ignored(id, ignored) {
                result = Err(format!("The mark could not be changed: {error:#}."));
                break;
            }
        }
        drop(database);
        self.bump();
        result
    }

    /// Deletes every entry in `ids` with its design file, tags and cached thumbnails, and
    /// the cutting progress the app kept for each design outside the database: the done
    /// marks of `<files>/cut_progress/`, both the file named after the design's id and the
    /// one named after its text (`cutting::progress_file_stems`). The names come from the
    /// stored design, so they are read before the entry goes.
    ///
    /// Marks kept under the text of an earlier edit of a design without an id are not
    /// found; `cutting::delete_all_progress` removes those. What the app state holds of the
    /// entries (recent items, an autosave, the open design) is not touched here: see
    /// [`AppState::forget_deleted_entries`].
    ///
    /// # Errors
    /// A sentence when the database cannot be written. The entries before the failing one
    /// are deleted; [`Self::missing`] says which.
    pub fn delete(&self, ids: &[EntryId]) -> Result<(), String> {
        let database = self.lock();
        let mut result = Ok(());
        let mut stems: Vec<String> = Vec::new();
        for &id in ids {
            let entry_stems = progress_stems_of(&database, id);
            if let Err(error) = database.delete_diagram_entry(id) {
                result = Err(format!("The design could not be deleted: {error:#}."));
                break;
            }
            stems.extend(entry_stems);
        }
        drop(database);
        cutting::remove_progress_files(&self.files_dir, &stems);
        for &id in ids {
            self.remove_thumbs(id);
        }
        self.bump();
        result
    }

    /// The entries among `ids` that the library positively does not hold (any more): their
    /// row is not there. An entry whose row cannot be read (a database that is failing) is
    /// not listed, because the callers act on "gone" by deleting what the app kept for the
    /// entry (recent items, an autosave, progress): the clean-up after a delete and the
    /// check of a design restored at start-up must never do that for an entry that still
    /// exists.
    #[must_use]
    pub fn missing(&self, ids: &[EntryId]) -> Vec<EntryId> {
        let database = self.lock();
        ids.iter()
            .copied()
            .filter(|&id| matches!(database.get_diagram_full_meta(id), Ok(None)))
            .collect()
    }

    /// The full record of entry `id`.
    fn record(&self, id: EntryId) -> Result<FullDiagramRecord, String> {
        self.lock()
            .get_diagram_full(id)
            .map_err(db_error("The design could not be read"))?
            .ok_or_else(|| GONE.to_owned())
    }

    /// Whether the library has an entry `id`.
    fn entry_exists(&self, id: EntryId) -> bool {
        self.lock().entry_updated_at(id).is_ok()
    }

    /// `file_name`, or `<stem> (2).<ext>` and so on, whichever no entry's url uses yet.
    fn free_file_name(&self, file_name: &str) -> Result<String, String> {
        let untitled = format!("Untitled{DESIGN_EXTENSION_DOTTED}");
        let base = if file_name.trim().is_empty() {
            untitled.as_str()
        } else {
            file_name.trim()
        };
        let (stem, extension) = base.rfind('.').map_or((base, ""), |dot| base.split_at(dot));
        let database = self.lock();
        for number in 1..=999_u32 {
            let candidate = if number == 1 {
                base.to_owned()
            } else {
                format!("{stem} ({number}){extension}")
            };
            let taken = database
                .diagram_entry_id_for_url(&format!("local://{candidate}"))
                .map_err(db_error("The library could not be checked for the name"))?;
            if taken.is_none() {
                return Ok(candidate);
            }
        }
        drop(database);
        Err("Too many designs in the library share this name.".to_owned())
    }
}

impl AppState {
    /// Forgets what the app state still holds of the library entries `ids` after they were
    /// deleted ([`LibraryHandle::delete`], then [`LibraryHandle::missing`] names the ones
    /// that are really gone):
    ///
    /// - their lines of the recent items are removed;
    /// - an autosave of one of them is deleted, so a restart does not bring the design back;
    /// - the open design, when it is the working copy of one of them, stays open but belongs
    ///   to no entry and reads as unsaved (see
    ///   [`DocumentState::forget_library_entries`](crate::document::DocumentState::forget_library_entries)),
    ///   and its cutting progress in memory is cleared with the files that were deleted.
    ///
    /// Returns whether the open design was the working copy of a deleted entry; the front
    /// end says so to the user.
    pub fn forget_deleted_entries(&mut self, ids: &[EntryId]) -> bool {
        if ids.is_empty() {
            return false;
        }
        self.update_settings(|settings| {
            settings.forget_library_entries(ids);
        });
        let files = self.platform().app_dirs().files.clone();
        let detached = self
            .document
            .forget_library_entries(&mut self.session, &files, ids);
        if detached {
            // The store binds lazily (only the Cut page and Cutting Mode bind it, and only
            // for a solved design), so it may still describe a design opened before this
            // one; reset it only after it is bound to the design that was deleted, or the
            // marks of that other design would go.
            cutting::sync_progress(self);
            self.cutting.progress.reset();
        }
        detached
    }
}

/// The done-mark file names of entry `id`'s design, from the stored entry; empty when the
/// entry cannot be read (it is then deleted without them).
fn progress_stems_of(database: &Database, id: EntryId) -> Vec<String> {
    let Ok(Some(record)) = database.get_diagram_full(id) else {
        return Vec::new();
    };
    progress_stems(&record.attached_files, &record.frosting)
}

/// Parses `bytes` as a native design file for the library: the vault's entry point, the
/// measured metadata, and the designer line. A panic becomes an error.
fn parse_native(name: &str, bytes: &[u8]) -> Result<ImportedAsc, String> {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<ImportedAsc, String> {
        let mut parsed = local::import_native_design(name, bytes)
            .map_err(|error| format!("This is not a readable design file: {error}"))?;
        apply_measured_metadata(&mut parsed.detail);
        fill_designer_info(&mut parsed.detail);
        Ok(parsed)
    }));
    match outcome {
        Ok(result) => result,
        Err(payload) => Err(format!(
            "The design could not be processed ({}).",
            panic_text(payload.as_ref())
        )),
    }
}

/// The tags trimmed, without blanks and without case-insensitive repeats.
fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut cleaned: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        let lower = tag.to_lowercase();
        if cleaned.iter().all(|kept| kept.to_lowercase() != lower) {
            cleaned.push(tag.to_owned());
        }
    }
    cleaned
}

/// Makes `wanted` the tag set of entry `id`.
fn replace_tags(database: &Database, id: EntryId, wanted: &[String]) -> Result<(), String> {
    let current = database
        .tags_for_entry(id)
        .map_err(db_error("The tags could not be read"))?;
    for tag in current {
        let keep = wanted
            .iter()
            .any(|name| name.to_lowercase() == tag.name.to_lowercase());
        if !keep {
            database
                .remove_tag_from_entry(id, tag.id)
                .map_err(db_error("A tag could not be removed"))?;
        }
    }
    for name in wanted {
        database
            .add_tag_to_entry(id, name)
            .map_err(db_error("A tag could not be added"))?;
    }
    Ok(())
}

#[cfg(test)]
mod delete_tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
    };

    use super::*;
    use crate::{
        document::{
            DocumentOrigin, asc_bytes, autosave, library_handle, new_from_template, open_library,
            restore, save_to_library,
        },
        library::LibraryQuery,
        platform::{PickedFile, Platform},
        platform_mock::MockPlatform,
        settings::{RecentItem, RecentKind},
    };

    /// An app state on a fresh mock platform showing the round brilliant, with its library.
    fn app() -> (AppState, LibraryHandle, Arc<MockPlatform>) {
        let mock = Arc::new(MockPlatform::default());
        let platform: Arc<dyn Platform> = mock.clone();
        let mut state = AppState::new(platform);
        new_from_template(&mut state, 1).expect("template 1 exists");
        let handle = library_handle(&mut state).expect("the library opens");
        (state, handle, mock)
    }

    /// The file of the done marks named `stem` in `dir`.
    fn marks_file(dir: &Path, stem: &str) -> PathBuf {
        dir.join(format!("{stem}.json"))
    }

    /// Writes a done-marks file named `stem` into `dir`.
    fn write_marks(dir: &Path, stem: &str) {
        fs::create_dir_all(dir).expect("the folder is created");
        fs::write(
            marks_file(dir, stem),
            r#"{"version":1,"marks":{"step":"signature"}}"#,
        )
        .expect("the file is written");
    }

    /// The round brilliant saved to the library and opened from it again, the way the app
    /// opens a library design, with a done-marks file for every key the open design uses.
    /// Returns the entry, those files' names and the folder they are in.
    fn stored_design_with_marks(
        state: &mut AppState,
        handle: &LibraryHandle,
        mock: &MockPlatform,
    ) -> (EntryId, Vec<String>, PathBuf) {
        let id = save_to_library(state, handle).expect("saves");
        let stored = handle.load(id).expect("loads");
        open_library(state, stored, id).expect("opens");
        let stems =
            cutting::progress_file_stems(&state.document.metadata.id, &state.session.design);
        assert_eq!(stems.len(), 2, "an id file and a text file: {stems:?}");
        let dir = cutting::progress_dir(&mock.app_dirs().files);
        for stem in &stems {
            write_marks(&dir, stem);
        }
        (id, stems, dir)
    }

    #[test]
    fn deleting_a_design_removes_both_of_its_progress_files_and_no_others() {
        let (mut state, handle, mock) = app();
        let (id, stems, dir) = stored_design_with_marks(&mut state, &handle, &mock);
        write_marks(&dir, "id-another-design");
        assert_eq!(handle.missing(&[id]), [] as [i64; 0]);

        handle.delete(&[id]).expect("deletes");

        for stem in &stems {
            assert!(!marks_file(&dir, stem).exists(), "{stem} was left behind");
        }
        assert!(marks_file(&dir, "id-another-design").is_file());
        assert_eq!(handle.missing(&[id]), [id]);
    }

    #[test]
    fn a_delete_that_stops_early_still_removes_the_progress_of_the_entries_it_deleted() {
        let (mut state, handle, mock) = app();
        let (id, stems, dir) = stored_design_with_marks(&mut state, &handle, &mock);

        let result = handle.delete(&[id, 987_654]);

        assert!(result.is_err(), "an id the library never held is an error");
        for stem in &stems {
            assert!(!marks_file(&dir, stem).exists(), "{stem} was left behind");
        }
        assert_eq!(handle.missing(&[id, 987_654]), [id, 987_654]);
    }

    #[test]
    fn deleting_an_imported_asc_removes_its_text_keyed_progress_file() {
        let (mut state, handle, mock) = app();
        let asc = asc_bytes(&state).expect("exports").file;
        let report = handle.import(vec![PickedFile {
            display_name: asc.suggested_name,
            bytes: asc.bytes,
        }]);
        assert_eq!(report.added, 1, "{report:?}");
        let (cards, _) = handle
            .search(&LibraryQuery::default(), 0, 10)
            .expect("searches");
        let id = cards[0].id;
        let stored = handle.load(id).expect("loads");
        open_library(&mut state, stored, id).expect("opens");
        let stems =
            cutting::progress_file_stems(&state.document.metadata.id, &state.session.design);
        assert_eq!(stems.len(), 1, "an .asc has no id: {stems:?}");
        let dir = cutting::progress_dir(&mock.app_dirs().files);
        write_marks(&dir, &stems[0]);

        handle.delete(&[id]).expect("deletes");

        assert!(!marks_file(&dir, &stems[0]).exists());
    }

    #[test]
    fn the_app_state_forgets_a_deleted_entry() {
        let (mut state, handle, mock) = app();
        let id = save_to_library(&mut state, &handle).expect("saves");
        let other = id + 100;
        state.update_settings(|settings| {
            settings.push_recent(RecentItem {
                title: "Elsewhere".to_owned(),
                opened_at_unix: 1,
                kind: RecentKind::Library { entry_id: other },
            });
            settings.push_recent(RecentItem {
                title: "A file".to_owned(),
                opened_at_unix: 2,
                kind: RecentKind::Document {
                    display_name: "a.asc".to_owned(),
                },
            });
        });
        autosave(&mut state).expect("writes");
        let autosaved = mock.app_dirs().files.join("autosave").join("current.isdd");
        assert!(autosaved.is_file());
        assert_eq!(
            state.settings.recent.len(),
            3,
            "{:?}",
            state.settings.recent
        );
        assert!(!state.session.is_dirty());

        handle.delete(&[id]).expect("deletes");
        let detached = state.forget_deleted_entries(&[id]);

        assert!(detached, "the open design was the deleted entry");
        let kinds: Vec<RecentKind> = state
            .settings
            .recent
            .iter()
            .map(|item| item.kind.clone())
            .collect();
        assert_eq!(
            kinds,
            [
                RecentKind::Document {
                    display_name: "a.asc".to_owned()
                },
                RecentKind::Library { entry_id: other },
            ]
        );
        assert!(!autosaved.exists(), "the deleted design is still autosaved");
        // The design stays open, as an unsaved design of no entry.
        assert_ne!(
            state.session.design.tiers,
            [] as [indicatrix_cut_core::ConstraintTier; 0]
        );
        assert_eq!(state.document.origin, DocumentOrigin::New);
        assert!(state.session.is_dirty());
        // And the next autosave does not write it back.
        autosave(&mut state).expect("nothing to write");
        assert!(!autosaved.exists());
    }

    #[test]
    fn deleting_an_unsolved_open_design_leaves_the_marks_of_the_design_the_store_describes() {
        let (mut state, handle, mock) = app();
        let (first, first_stems, dir) = stored_design_with_marks(&mut state, &handle, &mock);
        // The store is bound to the first design and holds its marks.
        cutting::sync_progress(&state);
        assert!(state.cutting.progress.has_marks());

        // A second design is opened from the library. It is not solved, so nothing binds
        // the store to it: it still describes the first design.
        new_from_template(&mut state, 2).expect("template 2 exists");
        let second = save_to_library(&mut state, &handle).expect("saves the second");
        assert_ne!(first, second);
        let stored = handle.load(second).expect("loads");
        open_library(&mut state, stored, second).expect("opens");
        assert!(
            state.cutting.progress.has_marks(),
            "still the first's marks"
        );

        handle.delete(&[second]).expect("deletes");
        let detached = state.forget_deleted_entries(&[second]);

        assert!(detached, "the open design was the deleted entry");
        for stem in &first_stems {
            assert!(
                marks_file(&dir, stem).is_file(),
                "{stem} of the other design was deleted"
            );
        }
        assert!(!state.cutting.progress.has_marks(), "the deleted design's");
        // The first design opens again with its marks.
        let stored = handle.load(first).expect("loads");
        open_library(&mut state, stored, first).expect("opens");
        cutting::sync_progress(&state);
        assert!(state.cutting.progress.has_marks());
    }

    #[test]
    fn a_restored_design_of_a_deleted_entry_is_detached_by_the_start_up_check() {
        let (mut state, handle, mock) = app();
        let id = save_to_library(&mut state, &handle).expect("saves");
        autosave(&mut state).expect("writes");
        let autosaved = mock.app_dirs().files.join("autosave").join("current.isdd");
        assert!(autosaved.is_file());
        // The app dies after the delete, before the UI thread tidied up (or the autosave
        // comes from a build that never tidied up).
        handle.delete(&[id]).expect("deletes");
        let platform: Arc<dyn Platform> = mock;
        let mut restarted = AppState::new(platform);
        restore(&mut restarted).expect("the autosave restores");
        assert_eq!(restarted.document.library_entry(), Some(id));

        // What the start-up check does: ask the library, then let the state forget.
        let gone = handle.missing(&[id]);
        assert_eq!(gone, [id]);
        assert!(restarted.forget_deleted_entries(&gone));

        // The design stays open as an unsaved design of no entry: a Save cannot re-create
        // the deleted entry on its own, and the autosave is gone.
        assert_eq!(restarted.document.origin, DocumentOrigin::New);
        assert!(restarted.session.is_dirty());
        assert_ne!(
            restarted.session.design.tiers,
            [] as [indicatrix_cut_core::ConstraintTier; 0]
        );
        assert!(!autosaved.exists(), "the deleted design is still autosaved");
        autosave(&mut restarted).expect("nothing to write");
        assert!(!autosaved.exists());
    }

    #[test]
    fn an_entry_that_still_exists_is_not_missing() {
        let (mut state, handle, _mock) = app();
        let id = save_to_library(&mut state, &handle).expect("saves");
        assert_eq!(handle.missing(&[id]), [] as [i64; 0]);
        assert_eq!(handle.missing(&[id, id + 1000]), [id + 1000]);
    }

    #[test]
    fn deleting_another_design_leaves_the_open_design_and_its_autosave_alone() {
        let (mut state, handle, mock) = app();
        let first = save_to_library(&mut state, &handle).expect("saves the first");
        new_from_template(&mut state, 2).expect("template 2 exists");
        let second = save_to_library(&mut state, &handle).expect("saves the second");
        assert_ne!(first, second);
        autosave(&mut state).expect("writes");
        let autosaved = mock.app_dirs().files.join("autosave").join("current.isdd");
        assert!(autosaved.is_file());

        handle.delete(&[first]).expect("deletes");
        let detached = state.forget_deleted_entries(&[first]);

        assert!(!detached);
        assert_eq!(state.document.library_entry(), Some(second));
        assert!(!state.session.is_dirty());
        assert!(autosaved.is_file());
        let kinds: Vec<RecentKind> = state
            .settings
            .recent
            .iter()
            .map(|item| item.kind.clone())
            .collect();
        assert_eq!(kinds, [RecentKind::Library { entry_id: second }]);
    }
}
