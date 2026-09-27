//! The small, targeted mutations a cutter can make to an already-imported design:
//! rename, hand-corrected metadata, its own `url`, provenance, the "ignored" flag, and
//! deletion.

use super::Database;
use crate::model::{facets::parse_facets_count, metadata_update::MetadataUpdate};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use tracing::debug;

impl Database {
    /// Renames a diagram entry -- the "Organize" library operation, works on any
    /// entry regardless of `source_id`. Bumps `updated_at` for the "recently
    /// edited" sort: a rename is a content edit to the entry, the same
    /// category of change [`Self::update_diagram_metadata`] already bumps for.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails, or if `entry_id` does not
    /// match any row (zero rows affected).
    pub fn rename_diagram_entry(&self, entry_id: i64, new_title: &str) -> Result<()> {
        let trimmed = new_title.trim();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!("Title cannot be empty."));
        }
        let changed = self
            .conn
            .execute(
                "UPDATE diagram_entries SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![trimmed, super::unix_now(), entry_id],
            )
            .context(format!("Failed to rename diagram entry {entry_id}"))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        Ok(())
    }

    /// Updates exactly the metadata fields a user might legitimately hand-correct on an
    /// already-imported design -- see [`MetadataUpdate`]'s own doc comment for which
    /// fields that is and why title isn't one of them.
    ///
    /// # The trap this exists to avoid
    ///
    /// [`Database::get_diagram_full`] returns a [`crate::model::entry::FullDiagramRecord`],
    /// a STRICT SUBSET of [`crate::model::detail::FacetDiagramDetail`] (missing
    /// `hw_ratio`/`tw_ratio`/`uw_ratio`/`pw_ratio`/`cw_ratio`/`symmetry_order`/
    /// `mirror_symmetry`/`designer`/`source_citation`/`pdf_file`/`gem_file`/
    /// `shape_category`). Since [`Self::save_diagram_detail`] fully REPLACES the
    /// detail row, a naive read-edit-rebuild-save would silently zero every field
    /// above -- erasing a locally-imported design's just-measured proportions. This
    /// method is the fix: one `UPDATE` naming exactly `MetadataUpdate`'s fields (plus
    /// `facets`/`girdle_facets`, kept in sync below) and nothing else -- no delete, so
    /// every other column and all child rows survive unchanged.
    ///
    /// `facets`/`girdle_facets` are a queryable split of `facets_count` (see
    /// [`parse_facets_count`]; the search range filter reads them directly, never the
    /// text) -- re-deriving them here keeps that filter from desyncing after an edit.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails, or if `entry_id` has no
    /// `diagram_details` row (zero rows affected -- e.g. an entry whose import never
    /// got as far as writing one).
    pub fn update_diagram_metadata(&self, entry_id: i64, update: &MetadataUpdate) -> Result<()> {
        let (facets, girdle_facets) = parse_facets_count(update.facets_count.as_deref());
        let changed = self
            .conn
            .execute(
                "UPDATE diagram_details SET
                    designer_info = ?1, shape = ?2, refractive_index = ?3, index_gear = ?4,
                    facets_count = ?5, facets = ?6, girdle_facets = ?7, symmetry_order = ?8,
                    mirror_symmetry = ?9, lw_ratio = ?10, hw_ratio = ?11, cw_ratio = ?12,
                    pw_ratio = ?13, volume = ?14
                 WHERE entry_id = ?15",
                params![
                    update.designer_info,
                    update.shape,
                    update.refractive_index,
                    update.index_gear,
                    update.facets_count,
                    facets,
                    girdle_facets,
                    update.symmetry_order,
                    update.mirror_symmetry,
                    update.lw_ratio,
                    update.hw_ratio,
                    update.cw_ratio,
                    update.pw_ratio,
                    update.volume,
                    entry_id,
                ],
            )
            .context(format!(
                "Failed to update diagram metadata for entry_id: {entry_id}"
            ))?;
        if changed == 0 {
            return Err(anyhow::anyhow!(
                "No diagram detail row for entry_id {entry_id}."
            ));
        }

        // Bumps `diagram_entries.updated_at` for the "recently edited" sort --
        // best-effort: a hand-correction to metadata having gone through
        // above is the change that matters, so a failure here is logged rather than
        // rolled back into an error the caller would otherwise treat as "nothing was
        // saved."
        if let Err(e) = self.conn.execute(
            "UPDATE diagram_entries SET updated_at = ?1 WHERE id = ?2",
            params![super::unix_now(), entry_id],
        ) {
            debug!("Failed to bump updated_at for entry_id {entry_id}: {e}");
        }
        Ok(())
    }

    /// Updates `entry_id`'s own `url` directly, by id, and bumps `updated_at` -- for a
    /// caller (Save Native's catalogue write-back) that already knows exactly which
    /// row to update and must not risk [`Self::save_diagram_entry`]'s
    /// url-keyed upsert silently creating a SECOND row when the design's file name (and
    /// so its synthetic `local://` url) changed since this row was created -- e.g. "Save
    /// Native As..." to a new file name for a design that already has a catalogue row.
    /// `title`/`design_id`/`source_id`/`created_at` are all left untouched: title in
    /// particular is a field a cutter hand-corrects (`rename_diagram_entry`), same
    /// precedent as [`Self::update_diagram_metadata`]'s own doc comment, never silently
    /// overwritten by a geometry write-back that merely changed where the file lives.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails, or if `entry_id` does not
    /// match any row (zero rows affected).
    pub fn update_diagram_entry_url(&self, entry_id: i64, url: &str) -> Result<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE diagram_entries SET url = ?1, updated_at = ?2 WHERE id = ?3",
                params![url, super::unix_now(), entry_id],
            )
            .context(format!("Failed to update url for diagram entry {entry_id}"))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        Ok(())
    }

    /// The `derived_from_entry_id` of `entry_id`'s row -- the entry it was recorded as
    /// derived from at import time, or `None` when unknown/not
    /// applicable. `None` is also returned for a nonexistent `entry_id` rather than an
    /// error, matching this column's own "unknown provenance" meaning.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn get_derived_from_entry_id(&self, entry_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT derived_from_entry_id FROM diagram_entries WHERE id = ?1",
                params![entry_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(Option::flatten)
            .context(format!(
                "Failed to read derived_from_entry_id for entry_id: {entry_id}"
            ))
    }

    /// `entry_id`'s recorded source row's own id and title -- the one query a
    /// "Derived from: <title>" badge/link needs: `set_derived_from_entry_id`
    /// already records provenance at import time, and this reads it back for
    /// display. `None`
    /// when `entry_id` has no recorded source, or when the recorded source
    /// row no longer exists (e.g. it was since deleted) -- a caller renders
    /// nothing rather than a dangling reference either way.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn get_derived_from_title(&self, entry_id: i64) -> Result<Option<(i64, String)>> {
        self.conn
            .query_row(
                "SELECT source.id, source.title \
                 FROM diagram_entries AS entry \
                 JOIN diagram_entries AS source ON source.id = entry.derived_from_entry_id \
                 WHERE entry.id = ?1",
                params![entry_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .context(format!(
                "Failed to read derived-from title for entry_id: {entry_id}"
            ))
    }

    /// Records that `entry_id` was derived from `derived_from`, e.g. an
    /// export-then-reimport of an existing catalogue design. Pass
    /// `None` to clear a previously recorded value. Deliberately takes an explicit,
    /// already-known source id rather than inferring one -- see
    /// `migrate_diagram_entries_provenance`'s doc comment for why this crate never
    /// guesses provenance from titles or other heuristics.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails, or if `entry_id` does not
    /// match any row (zero rows affected).
    pub fn set_derived_from_entry_id(
        &self,
        entry_id: i64,
        derived_from: Option<i64>,
    ) -> Result<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE diagram_entries SET derived_from_entry_id = ?1 WHERE id = ?2",
                params![derived_from, entry_id],
            )
            .context(format!(
                "Failed to set derived_from_entry_id for diagram entry {entry_id}"
            ))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        Ok(())
    }

    /// Sets or clears `entry_id`'s `ignored` flag, backing
    /// `crate::model::filter::RangeFilter::include_ignored`'s exclude-by-default
    /// search behaviour. Works on any entry regardless of `source_id`.
    ///
    /// Deliberately does NOT bump `diagram_entries.updated_at` (unlike
    /// [`Self::rename_diagram_entry`]/[`Self::save_diagram_detail`]/
    /// [`Self::update_diagram_metadata`]): "recently edited" means
    /// a change to the design's own recorded content, and hiding/restoring a design
    /// from the library view changes neither its title nor its detail data -- treating
    /// an ignore/un-ignore toggle as an "edit" would let a cutter's Show/Hide clicks
    /// reorder the catalogue's recently-edited list with no actual content change
    /// behind any of the moves.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails, or if `entry_id` does not
    /// match any row (zero rows affected).
    pub fn set_diagram_ignored(&self, entry_id: i64, ignored: bool) -> Result<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE diagram_entries SET ignored = ?1 WHERE id = ?2",
                params![ignored, entry_id],
            )
            .context(format!(
                "Failed to set ignored={ignored} for diagram entry {entry_id}"
            ))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        Ok(())
    }

    /// Permanently deletes a diagram entry and everything attached to it (detail,
    /// angle settings, attached files cascade via `ON DELETE CASCADE`, see
    /// `create_tables_if_not_exist`). Works on any entry regardless of `source_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails, or if `entry_id` does not
    /// match any row (zero rows affected).
    pub fn delete_diagram_entry(&self, entry_id: i64) -> Result<()> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM diagram_entries WHERE id = ?1",
                params![entry_id],
            )
            .context(format!("Failed to delete diagram entry {entry_id}"))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        Ok(())
    }
}
