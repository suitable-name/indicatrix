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
    design_source::{design_of, primary_attachment},
    handle::db_error,
    import::{invalidate_caches, panic_text},
    measure::{apply_measured_metadata, fill_designer_info, merge_existing_metadata},
};
use crate::platform::{OutgoingFile, mime};

/// The sentence for an id the library does not hold.
const GONE: &str = "That design is no longer in the library.";

impl LibraryHandle {
    /// The file entry `id` opens from: its `.isdd` (or legacy `.indicatrix`) design
    /// file, else its `.asc` (a `.gem` or `.gcs` import stores the `.asc` it converts to), else the `.gem` or
    /// `.gcs` itself. The bytes are exactly the stored ones.
    ///
    /// # Errors
    /// A sentence when the entry is gone or holds no design file.
    pub fn load(&self, id: EntryId) -> Result<LibraryDesign, String> {
        let record = self.record(id)?;
        let file = primary_attachment(&record.attached_files)
            .ok_or_else(|| "This entry holds no design file.".to_owned())?;
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
        let design = design_of(&record.attached_files)?;
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

    /// Deletes every entry in `ids` with its design file, tags and cached thumbnails.
    /// Cutting progress kept outside the database is not touched.
    ///
    /// # Errors
    /// A sentence when the database cannot be written.
    pub fn delete(&self, ids: &[EntryId]) -> Result<(), String> {
        let database = self.lock();
        let mut result = Ok(());
        for &id in ids {
            if let Err(error) = database.delete_diagram_entry(id) {
                result = Err(format!("The design could not be deleted: {error:#}."));
                break;
            }
        }
        drop(database);
        for &id in ids {
            self.remove_thumbs(id);
        }
        self.bump();
        result
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
