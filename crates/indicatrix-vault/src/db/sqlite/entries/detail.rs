//! Saving a design's full detail row -- and its angle-setting/attached-file children
//! -- in one transaction, plus the cheap existence check a caller uses to decide
//! whether a detail fetch is even needed.

use super::Database;
use crate::model::{detail::FacetingDiagramDetail, facets::parse_facets_count};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use tracing::{debug, info};

impl Database {
    /// Saves the details of a faceting diagram, first deleting any existing detail,
    /// angle settings, and attached files for `entry_id` so the row set stays fresh
    /// and duplicate-free. Also bumps `entry_id`'s `diagram_entries.updated_at` (see
    /// [`Self::bump_entry_updated_at`]) for the "recently edited" sort.
    ///
    /// A caller that also saves the entry row in the same operation should call
    /// [`Self::save_design`] instead, which commits both in one transaction; this method
    /// is for a detail-only write against an entry that already exists.
    ///
    /// # Performance: one transaction per design, not one per row
    ///
    /// Lookup, delete, detail insert, and every child insert below run inside a
    /// single [`Connection::unchecked_transaction`] ("unchecked" only means the type
    /// system won't stop a nested one). Without it, each `execute` -- 50+ per
    /// competition design -- was its own autocommit transaction with its own fsync.
    /// Measured on this database's real row distribution (3027 designs, 44259 angle
    /// rows, 6428 attachments): no-transaction 625.99s vs this version's 26.17s
    /// (23.9x). Batching inserts on top of the transaction was tried and lost (see
    /// [`Self::save_angle_settings`]). On any error partway through, the transaction
    /// rolls back on drop -- never leaves old data deleted with new data half-written.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction fails to start/commit, or any lookup,
    /// delete, or insert fails -- in every case it rolls back with no partial data left.
    pub fn save_diagram_detail(&self, detail: &FacetingDiagramDetail, entry_id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction().context(format!(
            "Failed to start save transaction for entry_id: {entry_id}"
        ))?;
        Self::save_diagram_detail_tx(&tx, detail, entry_id)?;
        tx.commit().context(format!(
            "Failed to commit save transaction for entry_id: {entry_id}"
        ))?;
        info!(
            "Successfully saved diagram detail and associated data for entry_id: {}",
            entry_id
        );
        Ok(())
    }

    /// [`Self::save_diagram_detail`]'s body, minus starting/committing the
    /// transaction -- taking `conn: &Connection` (satisfied by `&Transaction<'_>` via
    /// deref, same convention as [`Self::save_angle_settings`]/
    /// [`Self::save_attached_files`] below) so [`super::Database::save_design`] can run
    /// this against an in-progress transaction it shares with
    /// `save_diagram_entry_conn`, rather than always opening/committing its own.
    ///
    /// # Errors
    ///
    /// Returns an error if any lookup, delete, or insert fails.
    pub(super) fn save_diagram_detail_tx(
        conn: &Connection,
        detail: &FacetingDiagramDetail,
        entry_id: i64,
    ) -> Result<()> {
        // ON DELETE CASCADE handles child rows in angle_settings/attached_files.
        let existing_detail_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM diagram_details WHERE entry_id = ?1",
                params![entry_id],
                |row| row.get(0),
            )
            .optional()
            .context(format!(
                "Failed to check for existing diagram detail for entry_id: {entry_id}"
            ))?;

        if let Some(old_detail_id) = existing_detail_id {
            debug!(
                "Deleting existing detail (ID: {}) and its associated data for entry_id: {}",
                old_detail_id, entry_id
            );
            conn.execute(
                "DELETE FROM diagram_details WHERE id = ?1",
                params![old_detail_id],
            )
            .context(format!(
                "Failed to delete old diagram detail (ID: {old_detail_id})"
            ))?;
        }

        // facets/girdle_facets are derived from facets_count at write time (same
        // parse_facets_count the schema migration uses) so every newly-saved design
        // is immediately range-filterable by facet count.
        let (facets, girdle_facets) = parse_facets_count(detail.facets_count.as_deref());
        let mut stmt_detail = conn.prepare_cached(
            "INSERT INTO diagram_details (
                entry_id, page_url, diagram_image_name, diagram_image_data,
                competition_diagram, lw_ratio, refractive_index, index_gear,
                volume, facets_count, facets, girdle_facets, shape, designer_info,
                hw_ratio, tw_ratio, uw_ratio, pw_ratio, cw_ratio, symmetry_order, mirror_symmetry,
                designer, source_citation, pdf_file, gem_file, shape_category,
                concave_tiers, concave_facets
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
                      ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
        )?;

        stmt_detail
            .execute(params![
                entry_id,
                detail.page_url,
                detail.diagram_image_name,
                detail.diagram_image_data,
                detail.competition_diagram,
                detail.lw_ratio,
                detail.refractive_index,
                detail.index_gear,
                detail.volume,
                detail.facets_count,
                facets,
                girdle_facets,
                detail.shape,
                detail.designer_info,
                detail.hw_ratio,
                detail.tw_ratio,
                detail.uw_ratio,
                detail.pw_ratio,
                detail.cw_ratio,
                detail.symmetry_order,
                detail.mirror_symmetry,
                detail.designer,
                detail.source_citation,
                detail.pdf_file,
                detail.gem_file,
                detail.shape_category,
                detail.concave_tiers,
                detail.concave_facets,
            ])
            .context(format!(
                "Failed to insert diagram detail for entry_id: {entry_id}"
            ))?;
        // prepare_cached borrows conn for the statement's lifetime; drop before reborrowing below.
        drop(stmt_detail);

        let detail_id = conn.last_insert_rowid();
        debug!(
            "Inserted diagram detail for entry_id {} with new detail_id: {}",
            entry_id, detail_id
        );

        Self::save_angle_settings(conn, detail_id, &detail.angle_settings_table)?;
        Self::save_attached_files(conn, detail_id, &detail.attached_files)?;
        Self::bump_entry_updated_at(conn, entry_id)?;
        Ok(())
    }

    /// Bumps `entry_id`'s `diagram_entries.updated_at` to now, inside `conn` (the
    /// in-progress [`Self::save_diagram_detail`] transaction) for the "recently
    /// edited" sort: a full detail re-sync is at least as much a content
    /// change as the hand-corrections [`Self::update_diagram_metadata`] already bumps
    /// for. Split out purely to keep `save_diagram_detail` under clippy's
    /// function-length lint, not because this is reused elsewhere.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    fn bump_entry_updated_at(conn: &Connection, entry_id: i64) -> Result<()> {
        conn.execute(
            "UPDATE diagram_entries SET updated_at = MAX(?1, COALESCE(updated_at, 0) + 1) WHERE id = ?2",
            params![super::unix_now(), entry_id],
        )
        .context(format!(
            "Failed to bump updated_at for entry_id: {entry_id}"
        ))?;
        Ok(())
    }

    /// Inserts every angle-setting row for `detail_id`. Split out of
    /// `save_diagram_detail` to stay under clippy's function-length lint; takes
    /// `conn: &Connection` (not `&self`) so it can run against the in-progress
    /// `Transaction` from [`Self::save_diagram_detail`]. Deliberately one `execute`
    /// per row, not a batched multi-row `INSERT`: tried batching against this
    /// database's real distribution (3027 designs, 44259 rows) and it was slower
    /// (40-42s vs 26.17s) -- SQLite is in-process, so there's no round trip to
    /// amortize, while a variable-shape batch thrashes the `prepare_cached` cache.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing the statement or inserting any row fails.
    fn save_angle_settings(
        conn: &Connection,
        detail_id: i64,
        angle_settings: &[crate::model::angle::AngleSetting],
    ) -> Result<()> {
        let mut stmt_angle = conn.prepare_cached(
            "INSERT INTO angle_settings
                (detail_id, order_idx, facet, angle, index_val, notes, tool, tool_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for setting in angle_settings {
            stmt_angle
                .execute(params![
                    detail_id,
                    setting.order_index,
                    setting.facet,
                    setting.angle,
                    setting.index,
                    setting.notes,
                    setting.tool,
                    setting.tool_line,
                ])
                .context(format!(
                    "Failed to insert angle setting for detail_id: {detail_id}"
                ))?;
        }
        debug!(
            "Inserted {} angle settings for detail_id: {}",
            angle_settings.len(),
            detail_id
        );
        Ok(())
    }

    /// Inserts every attached-file row for `detail_id`. Same policy as
    /// [`Self::save_angle_settings`] (plain per-row loop, `&Connection`, split out
    /// for clippy's function-length lint) -- see its doc comment for why.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing the statement or inserting any row fails.
    fn save_attached_files(
        conn: &Connection,
        detail_id: i64,
        files: &[crate::model::file::AttachedFile],
    ) -> Result<()> {
        let mut stmt_file = conn.prepare_cached(
            "INSERT INTO attached_files (detail_id, name, url, content)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for file in files {
            stmt_file
                .execute(params![
                    detail_id,
                    file.name,
                    file.url,
                    file.content, // Vec<u8> stored as BLOB
                ])
                .context(format!(
                    "Failed to insert attached file '{}' for detail_id: {}",
                    file.name, detail_id
                ))?;
        }
        debug!(
            "Inserted {} attached files for detail_id: {}",
            files.len(),
            detail_id
        );
        Ok(())
    }

    /// Checks whether details for `entry_url` already exist, so a caller can skip
    /// re-fetching/processing.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `COUNT` query fails.
    pub fn has_detail_for_entry_url(&self, entry_url: &str) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(dd.id)
             FROM diagram_details dd
             JOIN diagram_entries de ON dd.entry_id = de.id
             WHERE de.url = ?1",
                params![entry_url],
                |row| row.get(0),
            )
            .context(format!(
                "Failed to check if detail exists for entry URL: {entry_url}"
            ))?;
        Ok(count > 0)
    }
}
