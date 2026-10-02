//! This design's catalogue write-back once a save's `.indicatrix` file is already on
//! disk: builds the same [`indicatrix_vault::local::ImportedAsc`] a re-import of
//! the design's `.asc` and its `.indicatrix` file would, then updates the design's
//! own row (or inserts a new one) -- see [`write_back_to_catalogue`]'s own doc comment for exactly what gets
//! overwritten versus preserved.

use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};
use tracing::warn;

/// [`write_back_to_catalogue`]'s outcome, for [`finish_save_native_success`]'s own
/// toast wording -- the ordinary "updated existing row" case gets no extra toast at
/// all (the file-save toast already said enough), while the others are surprising
/// enough on their own to call out.
pub(super) enum CatalogueWriteBack {
    /// The design's own known `source_entry_id` still named a real row -- updated in
    /// place. `stale_cache_invalidation_failed` is true when
    /// [`write_back_locked`]'s own `delete_preview_images`/`delete_tilt_curves`/
    /// `delete_solid_extents` calls could not clear that row's now-outdated cached
    /// previews/tilt curves/solid extents,
    /// so the cutter is told on screen instead of this only being logged via
    /// `tracing::warn!`, where the cache silently showing stale geometry could go
    /// unnoticed.
    UpdatedExisting {
        stale_cache_invalidation_failed: bool,
    },
    /// This design had no known source row -- a brand-new one was inserted.
    NewRow,
    /// This design's known `source_entry_id` no longer named a real row (deleted
    /// while the design was open) -- a brand-new row was inserted instead of an
    /// update, same as [`Self::NewRow`], but worth telling the cutter about.
    SourceRowGoneNewRowCreated,
    /// The design's catalogue url (`local://<file name>`) already belongs to a
    /// DIFFERENT catalogue design, so nothing was written: `save_diagram_entry` is a
    /// url-keyed upsert and would have renamed that design and replaced its angle table
    /// and attachments. `title` is the owner's current title. The id paired with this
    /// outcome in [`write_back_to_catalogue`]'s result is the OWNER's, not this
    /// design's -- a caller must not adopt it as the design's source row.
    UrlCollision { existing_id: i64, title: String },
}

/// This design's catalogue write-back -- called by
/// [`finish_save_native_success`] once [`write_file_atomically`] has already
/// succeeded (never before: the file on disk is the design of record, so a
/// catalogue-write failure here must never be read as "the save failed").
///
/// Builds the exact same [`indicatrix_vault::local::ImportedAsc`] an Import of the
/// design's `.asc` with its `.indicatrix` file beside it would build
/// ([`indicatrix_vault::local::import_asc`]) plus the same measured proportions/shape
/// an import derives ([`crate::gui::library::local::apply_measured_metadata`]) --
/// this is deliberately the SAME parse-and-measure path, not a second, independently
/// maintained one, so a design's catalogue row always describes it exactly as
/// re-importing those same files would.
///
/// # What gets overwritten versus preserved
///
/// - `source_entry_id: Some(id)`, `id` still a real row: `angle_settings_table`/
///   `attached_files` (the `.asc`, the `.indicatrix` design file and the attachments and
///   `[meta]` fields that file carries) and every geometry-derived
///   column (`refractive_index`/`index_gear`/`facets_count`/`symmetry_order`/
///   `mirror_symmetry`/the measured `lw`/`hw`/`cw`/`pw`/`volume` ratios/`shape`) are
///   always replaced with this save's own fresh values. Everything else --
///   `designer_info`, a hand-corrected `shape` override, the competition/citation/
///   scrape-only columns -- is merged forward from the existing row first
///   ([`crate::gui::library::local::merge_reimport_metadata`], the SAME rule
///   already applied to a `.asc` re-import), so a cutter's hand-typed
///   metadata survives a Save exactly as it survives an Import. `title` is left
///   untouched entirely (`Database::update_diagram_entry_url` never touches it --
///   same precedent as `Database::update_diagram_metadata`): a title is something a
///   cutter hand-corrects, never something a geometry write-back should silently
///   rename. Previews and tilt curves are invalidated (deleted, to be regenerated on
///   demand) -- they describe the geometry as it was before this save, same as a
///   `.asc` re-import collision already does.
/// - `source_entry_id: Some(id)`, but `id` no longer names a real row (deleted while
///   this design was open): falls through to the next case, exactly as if
///   `source_entry_id` had been `None`, so this save still lands somewhere instead
///   of silently failing or resurrecting a deleted row.
/// - `source_entry_id: None`: this design has never been saved to the catalogue --
///   inserts a brand-new row, entry and detail in one transaction
///   (`Database::save_design`, no merge: there is nothing existing to preserve).
/// - Either way, when the design's url (`local://<file name>`) already belongs to a
///   DIFFERENT catalogue row, nothing is written and the outcome is
///   [`CatalogueWriteBack::UrlCollision`]: the url-keyed upsert would otherwise rename
///   that other design and replace its angle table and attachments, and leave its
///   previews, tilt curves and extents describing geometry that no longer exists. An
///   explicit Import (which confirms the replacement) is the way to overwrite it.
///
/// Returns the row's id on success, so the caller can write it back into
/// [`EditorState::source_entry_id`] -- every save after the FIRST one for a
/// previously row-less design must update that SAME new row, never insert a second.
/// For [`CatalogueWriteBack::UrlCollision`] the id is the OTHER design's and must not
/// be written back.
///
/// # Errors
///
/// A ready-to-toast message. A failure here never rolls back the files
/// [`write_file_atomically`] already wrote -- the design is safely on disk either
/// way; this only affects whether the library list reflects it yet.
pub(super) fn write_back_to_catalogue(
    db: &Arc<Mutex<Database>>,
    source_entry_id: Option<i64>,
    asc_filename: &str,
    asc_text: &str,
    design_filename: &str,
    design_text: &str,
) -> Result<(i64, CatalogueWriteBack), String> {
    // Parse + measure runs inside `catch_file_panic`, exactly like a `.asc` import's
    // own per-file loop (`gui::library::local::import::pipeline::parse_one_import`):
    // `apply_measured_metadata` reaches into `indicatrix`'s geometry code, which this
    // module cannot guarantee is panic-free on every design. This call runs on Save's
    // own background thread (`native_io::save::spawn_native_save_write`),
    // which has no `catch_unwind` of its own and no `BusyGuard`-style Drop-based
    // recovery -- an uncaught panic here would silently kill that thread mid-save,
    // the UI's completion closure would never run, and the cutter would be left
    // looking at a save that never finishes and never errors. Turning it into an
    // ordinary `Err` instead means the existing `show_toast(&ui, &message, "error")`
    // path in `native_io::save::spawn_native_save_write` reports it like any other
    // catalogue write-back failure -- see this function's own doc comment: the files
    // on disk are safe either way, since this only runs after
    // `write_file_atomically` already succeeded.
    let mut parsed = match crate::gui::library::local::catch_file_panic(
        std::panic::AssertUnwindSafe(|| {
            indicatrix_vault::local::import_asc(
                asc_filename,
                asc_text,
                Some((design_filename, design_text.as_bytes())),
            )
            .and_then(|mut parsed| {
                // The design file just written carries the descriptive metadata and
                // the attachments (PDF, `.gem`, diagram image, ...): the row gets them
                // the way a re-import of the file would give them.
                let file = indicatrix_formats::native::design::parse(design_text)
                    .map_err(|e| e.to_string())?;
                indicatrix_vault::local::apply_design_file_meta(&mut parsed, &file)?;
                crate::gui::library::local::apply_measured_metadata(&mut parsed.detail);
                Ok(parsed)
            })
        }),
    ) {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(e)) => {
            return Err(format!(
                "Saved to disk, but could not update the catalogue: {e}"
            ));
        }
        Err(panic_msg) => {
            warn!(
                "Save: catalogue write-back panicked while measuring '{asc_filename}': \
                 {panic_msg}"
            );
            return Err(format!(
                "Saved to disk, but updating the catalogue failed unexpectedly ({panic_msg}) -- \
                 the file itself is safe; try Save again to retry the catalogue update."
            ));
        }
    };

    // The lock covers the database work and nothing else: the caller goes on to
    // toast and refresh the library list, and holding the catalogue mutex across
    // that would serialise it against every other reader.
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_back_locked(&db, source_entry_id, &mut parsed)
}

/// Deletes `existing_id`'s cached preview images, tilt curves and solid extents (all
/// now stale: the design they describe was just overwritten), logging -- not
/// propagating -- a failure on any, and reporting whether any one failed so the caller
/// can surface it as `CatalogueWriteBack::UpdatedExisting::
/// stale_cache_invalidation_failed`. Three independent expressions rather than a
/// mutable accumulator reassigned by three `if let Err` statements: each deletion's
/// own outcome is total (`Result::is_err`), so there is nothing conditional left to
/// express imperatively.
fn invalidate_stale_caches(db: &Database, existing_id: i64) -> bool {
    let preview_failed = db
        .delete_preview_images(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save: failed to invalidate stale preview cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    let tilt_curves_failed = db
        .delete_tilt_curves(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save: failed to invalidate stale tilt-curve cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    let solid_extents_failed = db
        .delete_solid_extents(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save: failed to invalidate stale solid-extents cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    preview_failed || tilt_curves_failed || solid_extents_failed
}

/// Writes the tags, ignored mark and planner exclusion the saved design file carries
/// onto `entry_id`'s row (they only ever set, never clear). Logged, not propagated: the
/// design file and the row itself are already written.
fn restore_extras(db: &Database, entry_id: i64, extras: &indicatrix_vault::local::ImportedExtras) {
    if let Err(e) = indicatrix_vault::local::apply_imported_extras(db, entry_id, extras) {
        warn!("Save: could not restore the tags/marks of entry #{entry_id}: {e}");
    }
}

/// The catalogue row that owns `url` when that row is not `own_id`, already shaped as
/// the write-back outcome that declines to touch it. `None` when `url` is free or is
/// `own_id`'s own url.
fn url_owned_by_another(
    db: &Database,
    url: &str,
    own_id: Option<i64>,
) -> Result<Option<(i64, CatalogueWriteBack)>, String> {
    let owner = db.diagram_entry_for_url(url).map_err(|e| {
        format!(
            "Saved to disk, but could not check the catalogue for a design with the same \
             file name: {e}"
        )
    })?;
    Ok(owner
        .filter(|(id, _)| Some(*id) != own_id)
        .map(|(existing_id, title)| {
            (
                existing_id,
                CatalogueWriteBack::UrlCollision { existing_id, title },
            )
        }))
}

/// [`write_back_to_catalogue`]'s database half, with the lock already taken --
/// split out so the guard's scope is exactly this call rather than the remainder of
/// its caller.
///
/// Returns the row id written and which of the cases happened: the design's own row
/// updated in place, its row found missing and a fresh one inserted, a first-ever
/// insert for a design that had no row, or a declined write because another design
/// owns the url (see [`write_back_to_catalogue`]'s doc comment).
fn write_back_locked(
    db: &Database,
    source_entry_id: Option<i64>,
    parsed: &mut indicatrix_vault::local::ImportedAsc,
) -> Result<(i64, CatalogueWriteBack), String> {
    if let Some(existing_id) = source_entry_id {
        match db.get_diagram_full(existing_id) {
            Ok(Some(existing)) => {
                // "Save As" to a file name another design already owns would
                // move this row's url onto it; decline before changing anything.
                if let Some(collision) =
                    url_owned_by_another(db, &parsed.entry.url, Some(existing_id))?
                {
                    return Ok(collision);
                }
                crate::gui::library::local::merge_reimport_metadata(&mut parsed.detail, &existing);
                db.update_diagram_entry_url(existing_id, &parsed.entry.url)
                    .map_err(|e| e.to_string())?;
                db.save_diagram_detail(&parsed.detail, existing_id)
                    .map_err(|e| e.to_string())?;
                restore_extras(db, existing_id, &parsed.extras);
                let stale_cache_invalidation_failed = invalidate_stale_caches(db, existing_id);
                return Ok((
                    existing_id,
                    CatalogueWriteBack::UpdatedExisting {
                        stale_cache_invalidation_failed,
                    },
                ));
            }
            Ok(None) => {
                // Source row deleted while this design was open -- fall through to
                // insert a fresh row below, rather than updating nothing or erroring.
            }
            Err(e) => {
                return Err(format!(
                    "Saved to disk, but could not read this design's catalogue row to \
                     update it: {e}"
                ));
            }
        }
    }

    // This design owns no row (none known, or the known one is gone), so any row that
    // holds its url belongs to another design: `save_design` upserts by url and would
    // overwrite that design's title, angle table and attachments.
    if let Some(collision) = url_owned_by_another(db, &parsed.entry.url, None)? {
        return Ok(collision);
    }
    let new_id = db
        .save_design(
            &parsed.entry,
            &parsed.detail,
            indicatrix_vault::local::LOCAL_SOURCE_ID,
        )
        .map_err(|e| e.to_string())?;
    restore_extras(db, new_id, &parsed.extras);
    let outcome = if source_entry_id.is_some() {
        CatalogueWriteBack::SourceRowGoneNewRowCreated
    } else {
        CatalogueWriteBack::NewRow
    };
    Ok((new_id, outcome))
}
