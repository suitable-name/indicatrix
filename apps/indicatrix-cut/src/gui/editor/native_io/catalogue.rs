//! This design's catalogue write-back once a native save's files are already on
//! disk: builds the same [`indicatrix_vault::local::ImportedAsc`] a re-import of
//! those same two files would, then updates the design's own row (or inserts a new
//! one) -- see [`write_back_to_catalogue`]'s own doc comment for exactly what gets
//! overwritten versus preserved.

use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};
use tracing::warn;

/// [`write_back_to_catalogue`]'s outcome, for [`finish_save_native_success`]'s own
/// toast wording -- the ordinary "updated existing row" case gets no extra toast at
/// all (the file-save toast already said enough), while the other two are surprising
/// enough on their own to call out.
pub(super) enum CatalogueWriteBack {
    /// The design's own known `source_entry_id` still named a real row -- updated in
    /// place. `stale_cache_invalidation_failed` is true when
    /// [`write_back_locked`]'s own `delete_preview_images`/`delete_tilt_curves`
    /// calls could not clear that row's now-outdated cached previews/tilt curves,
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
}

/// This design's catalogue write-back -- called by
/// [`finish_save_native_success`] once [`write_pair_atomically`] has already
/// succeeded (never before: the files on disk are the design of record, so a
/// catalogue-write failure here must never be read as "the save failed").
///
/// Builds the exact same [`indicatrix_vault::local::ImportedAsc`] an Import of these
/// same two just-written files would build
/// ([`indicatrix_vault::local::import_asc`]) plus the same measured proportions/shape
/// an import derives ([`crate::gui::library::local::apply_measured_metadata`]) --
/// this is deliberately the SAME parse-and-measure path, not a second, independently
/// maintained one, so a design's catalogue row always describes it exactly as
/// re-importing the same two files would.
///
/// # What gets overwritten versus preserved
///
/// - `source_entry_id: Some(id)`, `id` still a real row: `angle_settings_table`/
///   `attached_files` (the `.asc` + native sidecar) and every geometry-derived
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
///   inserts a brand-new row (`Database::save_diagram_entry` + `save_diagram_detail`,
///   no merge: there is nothing existing to preserve).
///
/// Returns the row's id on success, so the caller can write it back into
/// [`EditorState::source_entry_id`] -- every save after the FIRST one for a
/// previously row-less design must update that SAME new row, never insert a second.
///
/// # Errors
///
/// A ready-to-toast message. A failure here never rolls back the files
/// [`write_pair_atomically`] already wrote -- the design is safely on disk either
/// way; this only affects whether the library list reflects it yet.
pub(super) fn write_back_to_catalogue(
    db: &Arc<Mutex<Database>>,
    source_entry_id: Option<i64>,
    asc_filename: &str,
    asc_text: &str,
    native_filename: &str,
    native_toml: &str,
) -> Result<(i64, CatalogueWriteBack), String> {
    let mut parsed = indicatrix_vault::local::import_asc(
        asc_filename,
        asc_text,
        Some((native_filename, native_toml.as_bytes())),
    )
    .map_err(|e| format!("Saved to disk, but could not update the catalogue: {e}"))?;
    crate::gui::library::local::apply_measured_metadata(&mut parsed.detail);

    // The lock covers the database work and nothing else: the caller goes on to
    // toast and refresh the library list, and holding the catalogue mutex across
    // that would serialise it against every other reader.
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_back_locked(&db, source_entry_id, &mut parsed)
}

/// Deletes `existing_id`'s cached preview images and tilt curves (both now stale:
/// the design they describe was just overwritten), logging -- not propagating -- a
/// failure on either, and reporting whether either one failed so the caller can
/// surface it as `CatalogueWriteBack::UpdatedExisting::
/// stale_cache_invalidation_failed`. Two independent expressions rather than a
/// mutable accumulator reassigned by two `if let Err` statements: each deletion's
/// own outcome is total (`Result::is_err`), so there is nothing conditional left to
/// express imperatively.
fn invalidate_stale_caches(db: &Database, existing_id: i64) -> bool {
    let preview_failed = db
        .delete_preview_images(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save Native: failed to invalidate stale preview cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    let tilt_curves_failed = db
        .delete_tilt_curves(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save Native: failed to invalidate stale tilt-curve cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    preview_failed || tilt_curves_failed
}

/// [`write_back_to_catalogue`]'s database half, with the lock already taken --
/// split out so the guard's scope is exactly this call rather than the remainder of
/// its caller.
///
/// Returns the row id written and which of the three cases happened: the design's
/// own row updated in place, its row found missing and a fresh one inserted, or a
/// first-ever insert for a design that had no row.
fn write_back_locked(
    db: &Database,
    source_entry_id: Option<i64>,
    parsed: &mut indicatrix_vault::local::ImportedAsc,
) -> Result<(i64, CatalogueWriteBack), String> {
    if let Some(existing_id) = source_entry_id {
        match db.get_diagram_full(existing_id) {
            Ok(Some(existing)) => {
                crate::gui::library::local::merge_reimport_metadata(&mut parsed.detail, &existing);
                db.update_diagram_entry_url(existing_id, &parsed.entry.url)
                    .map_err(|e| e.to_string())?;
                db.save_diagram_detail(&parsed.detail, existing_id)
                    .map_err(|e| e.to_string())?;
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

    let new_id = db
        .save_diagram_entry(&parsed.entry, indicatrix_vault::local::LOCAL_SOURCE_ID)
        .map_err(|e| e.to_string())?;
    db.save_diagram_detail(&parsed.detail, new_id)
        .map_err(|e| e.to_string())?;
    let outcome = if source_entry_id.is_some() {
        CatalogueWriteBack::SourceRowGoneNewRowCreated
    } else {
        CatalogueWriteBack::NewRow
    };
    Ok((new_id, outcome))
}
