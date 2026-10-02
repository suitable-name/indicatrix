//! The small, targeted mutations a cutter can make to an already-imported design:
//! rename, hand-corrected metadata, its own `url`, provenance, the "ignored" flag, and
//! deletion.

use super::Database;
use crate::model::{
    facets::parse_facets_count,
    metadata_update::{MetadataUpdate, parse_optional_numeric},
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
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
        Self::rename_diagram_entry_conn(&self.conn, entry_id, new_title)
    }

    /// [`Self::rename_diagram_entry`]'s body, taking `conn: &Connection` so
    /// [`Self::rename_and_update_metadata`] can run it inside its own transaction.
    fn rename_diagram_entry_conn(conn: &Connection, entry_id: i64, new_title: &str) -> Result<()> {
        let trimmed = new_title.trim();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!("Title cannot be empty."));
        }
        let changed = conn
            .execute(
                "UPDATE diagram_entries SET title = ?1, updated_at = MAX(?2, COALESCE(updated_at, 0) + 1) WHERE id = ?3",
                params![trimmed, super::unix_now(), entry_id],
            )
            .context(format!("Failed to rename diagram entry {entry_id}"))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        Ok(())
    }

    /// Renames `entry_id` and applies `update` to its detail row in ONE transaction:
    /// either both land or neither does. The metadata editor saves the title and the
    /// other fields together; issuing [`Self::rename_diagram_entry`] and
    /// [`Self::update_diagram_metadata`] separately left the new title committed when a
    /// hand-typed numeric field was then rejected, while the dialog reported the whole
    /// save as failed.
    ///
    /// # Errors
    ///
    /// Returns an error, with nothing committed, if the title is blank, `entry_id`
    /// names no entry or no detail row, a numeric field of `update` does not parse (see
    /// [`Self::update_diagram_metadata`]), or an `UPDATE` or the transaction itself
    /// fails.
    pub fn rename_and_update_metadata(
        &self,
        entry_id: i64,
        new_title: &str,
        update: &MetadataUpdate,
    ) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the rename-and-metadata transaction")?;
        Self::rename_diagram_entry_conn(&tx, entry_id, new_title)?;
        Self::update_diagram_metadata_conn(&tx, entry_id, update)?;
        tx.commit()
            .context("Failed to commit the rename-and-metadata transaction")
    }

    /// Updates exactly the metadata fields a user might legitimately hand-correct on an
    /// already-imported design -- see [`MetadataUpdate`]'s own doc comment for which
    /// fields that is and why title isn't one of them.
    ///
    /// # The trap this exists to avoid
    ///
    /// [`Database::get_diagram_full`] returns a [`crate::model::entry::FullDiagramRecord`],
    /// a STRICT SUBSET of [`crate::model::detail::FacetingDiagramDetail`] (missing
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
    /// # Hand-typed numeric fields are parsed, never bound as raw text
    ///
    /// `refractive_index`/`index_gear`/`symmetry_order`/`lw_ratio`/`hw_ratio`/
    /// `cw_ratio`/`pw_ratio`/`volume` are `Option<String>` on [`MetadataUpdate`] (a
    /// text field's natural type), but their columns are REAL/INTEGER. Binding that
    /// text directly would rely on SQLite's column-affinity conversion, which only
    /// converts a value that already looks like a plain number and otherwise silently
    /// stores it as TEXT in the REAL/INTEGER column -- e.g. a hand-typed European
    /// `"1,76"` persisted as the literal string `"1,76"`, which then sorts/filters
    /// wrong against every other row's real `REAL` value. Each of those eight fields is
    /// therefore parsed through [`parse_optional_numeric`] first: a blank/`None` field still
    /// means "clear this value" (`Ok(None)`), but any other unparsable text is a hard
    /// error naming the field, and NOTHING is written -- see that function's doc
    /// comment. `facets_count` is not in this list: its column is TEXT, so it never had
    /// this problem, and it keeps `parse_facets_count`'s own, deliberately tolerant
    /// splitting (shared with the scrape-import path, where a partial parse like
    /// `"45+R"` is real, legitimate data, not hand-typed garbage to reject).
    ///
    /// # Errors
    ///
    /// Returns an error if any of the eight numeric fields above is non-blank and
    /// fails to parse (see [`parse_optional_numeric`]; nothing is written in that
    /// case), if the underlying `UPDATE` fails, or if `entry_id` has no
    /// `diagram_details` row (zero rows affected -- e.g. an entry whose import never
    /// got as far as writing one).
    pub fn update_diagram_metadata(&self, entry_id: i64, update: &MetadataUpdate) -> Result<()> {
        Self::update_diagram_metadata_conn(&self.conn, entry_id, update)
    }

    /// [`Self::update_diagram_metadata`]'s body, taking `conn: &Connection` so
    /// [`Self::rename_and_update_metadata`] can run it inside its own transaction.
    /// Every numeric field is parsed before the first write.
    fn update_diagram_metadata_conn(
        conn: &Connection,
        entry_id: i64,
        update: &MetadataUpdate,
    ) -> Result<()> {
        let (facets, girdle_facets) = parse_facets_count(update.facets_count.as_deref());
        let refractive_index =
            parse_optional_numeric::<f64>("refractive_index", update.refractive_index.as_deref())?;
        let index_gear = parse_optional_numeric::<i64>("index_gear", update.index_gear.as_deref())?;
        let symmetry_order =
            parse_optional_numeric::<i64>("symmetry_order", update.symmetry_order.as_deref())?;
        let lw_ratio = parse_optional_numeric::<f64>("lw_ratio", update.lw_ratio.as_deref())?;
        let hw_ratio = parse_optional_numeric::<f64>("hw_ratio", update.hw_ratio.as_deref())?;
        let cw_ratio = parse_optional_numeric::<f64>("cw_ratio", update.cw_ratio.as_deref())?;
        let pw_ratio = parse_optional_numeric::<f64>("pw_ratio", update.pw_ratio.as_deref())?;
        let volume = parse_optional_numeric::<f64>("volume", update.volume.as_deref())?;

        let changed = conn
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
                    refractive_index,
                    index_gear,
                    update.facets_count,
                    facets,
                    girdle_facets,
                    symmetry_order,
                    update.mirror_symmetry,
                    lw_ratio,
                    hw_ratio,
                    cw_ratio,
                    pw_ratio,
                    volume,
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
        if let Err(e) = conn.execute(
            "UPDATE diagram_entries SET updated_at = MAX(?1, COALESCE(updated_at, 0) + 1) WHERE id = ?2",
            params![super::unix_now(), entry_id],
        ) {
            debug!("Failed to bump updated_at for entry_id {entry_id}: {e}");
        }
        Ok(())
    }

    /// Updates `entry_id`'s own `url` directly, by id, and bumps `updated_at` -- for a
    /// caller (Save's catalogue write-back) that already knows exactly which
    /// row to update and must not risk [`Self::save_diagram_entry`]'s
    /// url-keyed upsert silently creating a SECOND row when the design's file name (and
    /// so its synthetic `local://` url) changed since this row was created -- e.g. "Save
    /// As..." to a new file name for a design that already has a catalogue row.
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
                "UPDATE diagram_entries SET url = ?1, updated_at = MAX(?2, COALESCE(updated_at, 0) + 1) WHERE id = ?3",
                params![url, super::unix_now(), entry_id],
            )
            .context(format!("Failed to update url for diagram entry {entry_id}"))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        Ok(())
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

    /// Whether `entry_id` is marked ignored (see [`Self::set_diagram_ignored`]); `false`
    /// for an `entry_id` that names no row.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn is_diagram_ignored(&self, entry_id: i64) -> Result<bool> {
        let ignored: Option<bool> = self
            .conn
            .query_row(
                "SELECT ignored FROM diagram_entries WHERE id = ?1",
                params![entry_id],
                |row| row.get(0),
            )
            .optional()
            .context(format!(
                "Failed to read the ignored flag of diagram entry {entry_id}"
            ))?;
        Ok(ignored.unwrap_or(false))
    }

    /// Permanently deletes a diagram entry and everything attached to it (detail,
    /// angle settings, attached files cascade via `ON DELETE CASCADE`, see
    /// `create_tables_if_not_exist`). Works on any entry regardless of `source_id`.
    ///
    /// # Mirror-state semantics: "local delete wins"
    ///
    /// `crate::model::mirror::MirrorState`/`library_mirror_state` is keyed by `url`,
    /// not `entry_id`, and has no `FOREIGN KEY` back to `diagram_entries` -- deleting a
    /// design synced from a remote mirror keeps its `library_mirror_state` row and
    /// marks it `deleted_locally` (a tombstone), in the same transaction as the
    /// delete. This is deliberate: if a later sync saw no mirror-state row at all for
    /// that url, it would treat the design as never seen before and re-download it,
    /// silently undoing the user's deletion.
    ///
    /// The tombstone, not the stored hashes, is what keeps the deletion in force: a
    /// mirror pass skips every tombstoned url whatever the remote hashes are, so a
    /// later change to the remote design cannot resurrect it. A url without a
    /// mirror-state row (a hand-imported design) has nothing to tombstone. The cost:
    /// the row is now permanently orphaned (no `diagram_entries` row will match its
    /// `url` again unless the exact same design is deliberately re-imported), which is
    /// invisible to a sync UI unless it asks -- see
    /// [`Self::count_mirror_states_without_entry`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails, or if `entry_id` does not
    /// match any row (zero rows affected). Nothing is changed in either case.
    pub fn delete_diagram_entry(&self, entry_id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction().context(format!(
            "Failed to start the delete of diagram entry {entry_id}"
        ))?;
        // Before the DELETE: the url is only known while the row still exists.
        tx.execute(
            "UPDATE library_mirror_state SET deleted_locally = 1
             WHERE url = (SELECT url FROM diagram_entries WHERE id = ?1)",
            params![entry_id],
        )
        .context(format!(
            "Failed to tombstone the mirror state of diagram entry {entry_id}"
        ))?;
        let changed = tx
            .execute(
                "DELETE FROM diagram_entries WHERE id = ?1",
                params![entry_id],
            )
            .context(format!("Failed to delete diagram entry {entry_id}"))?;
        if changed == 0 {
            return Err(anyhow::anyhow!("No diagram entry with id {entry_id}."));
        }
        tx.commit().context(format!(
            "Failed to commit the delete of diagram entry {entry_id}"
        ))
    }
}
