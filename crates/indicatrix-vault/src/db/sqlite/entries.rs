use super::Database;
use crate::model::{
    dedup::{CrossSourceDuplicate, normalize_for_dedup},
    detail::FacetDiagramDetail,
    entry::FacetDiagramEntry,
    facets::parse_facets_count,
    metadata_update::MetadataUpdate,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use tracing::{debug, info};

/// `(id, source_id, title, designer_info)` row from
/// [`Database::find_cross_source_duplicates`]'s candidate query, pre-normalisation.
/// Named to avoid tripping `clippy::type_complexity`.
type DuplicateCandidateRow = (i64, String, String, Option<String>);

impl Database {
    /// Saves a diagram entry from `source_id` (see `crate::source::DiagramSource::id`).
    /// If `url` already exists, updates its title/`design_id`/`source_id` instead.
    /// Returns the inserted or updated row's ID. Dedupes only *within* `url` --
    /// different sources describing the same physical design under different URLs
    /// each get their own row; see [`Self::find_cross_source_duplicates`] for the
    /// cross-source check (surfaces, never merges).
    ///
    /// # Errors
    ///
    /// Returns an error if the `INSERT`, `UPDATE`, or follow-up ID `SELECT` fails.
    pub fn save_diagram_entry(&self, entry: &FacetDiagramEntry, source_id: &str) -> Result<i64> {
        let now = unix_now();
        // `INSERT OR IGNORE` won't update on conflict, so update is handled explicitly below.
        let mut stmt_insert = self.conn.prepare_cached(
            "INSERT OR IGNORE INTO diagram_entries (title, url, design_id, source_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let changes = stmt_insert
            .execute(params![
                entry.title,
                entry.url,
                entry.design_id,
                source_id,
                now,
                now
            ])
            .context(format!(
                "Failed to INSERT OR IGNORE diagram entry with URL: {}",
                entry.url
            ))?;

        if changes > 0 {
            let id = self.conn.last_insert_rowid();
            debug!(
                "Inserted new diagram entry '{}' (URL: {}, source: {}) with ID: {}",
                entry.title, entry.url, source_id, id
            );
            Ok(id)
        } else {
            debug!(
                "Diagram entry with URL '{}' already exists. Updating title, design_id, and source_id.",
                entry.url
            );
            // `created_at` is deliberately left untouched -- this branch is a re-sync
            // of an existing row, not a new design.
            let mut stmt_update = self.conn.prepare_cached(
                "UPDATE diagram_entries SET title = ?1, design_id = ?2, source_id = ?3, updated_at = ?4 WHERE url = ?5",
            )?;
            stmt_update
                .execute(params![
                    entry.title,
                    entry.design_id,
                    source_id,
                    now,
                    entry.url
                ])
                .context(format!(
                    "Failed to UPDATE existing diagram entry with URL: {}",
                    entry.url
                ))?;

            let mut stmt_select = self
                .conn
                .prepare_cached("SELECT id FROM diagram_entries WHERE url = ?1")?;
            let id: i64 = stmt_select
                .query_row(params![entry.url], |row| row.get(0))
                .context(format!(
                    "Failed to SELECT ID of existing diagram entry with URL: {}",
                    entry.url
                ))?;
            debug!(
                "Updated existing diagram entry '{}' (URL: {}), existing ID: {}",
                entry.title, entry.url, id
            );
            Ok(id)
        }
    }

    /// Looks for entries already in the catalogue, synced from a source *other than*
    /// `new_source_id`, whose normalised title (and, when both sides have one,
    /// normalised designer) matches `title`/`designer_info`, and whose facet count
    /// matches `facets` when both are known. See `crate::model::dedup`'s module doc
    /// for why this only detects and surfaces candidates -- never merges or alters.
    /// A missing designer on either side is not a mismatch (favors an extra manual
    /// review over a silently unflagged duplicate); when both sides have one, they
    /// must match. SQL narrows first by `source_id != new_source_id` (and facet
    /// count, when known) so the comparison loop below only scans a small set.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn find_cross_source_duplicates(
        &self,
        new_source_id: &str,
        title: &str,
        designer_info: Option<&str>,
        facets: Option<i64>,
    ) -> Result<Vec<CrossSourceDuplicate>> {
        let normalized_title = normalize_for_dedup(title);
        if normalized_title.is_empty() {
            return Ok(Vec::new());
        }
        let normalized_designer = designer_info.map(normalize_for_dedup);

        let mut sql = String::from(
            "SELECT de.id, de.source_id, de.title, dd.designer_info
             FROM diagram_entries de
             LEFT JOIN diagram_details dd ON de.id = dd.entry_id
             WHERE de.source_id != ?1",
        );
        let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(new_source_id.to_string())];
        if let Some(f) = facets {
            sql.push_str(" AND dd.facets = ?2");
            sql_params.push(Box::new(f));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let bound: Vec<&dyn rusqlite::ToSql> =
            sql_params.iter().map(std::convert::AsRef::as_ref).collect();
        let rows: Vec<DuplicateCandidateRow> = stmt
            .query_map(bound.as_slice(), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut matches = Vec::new();
        for (existing_entry_id, existing_source_id, existing_title, existing_designer_info) in rows
        {
            if normalize_for_dedup(&existing_title) != normalized_title {
                continue;
            }
            if let (Some(want), Some(have)) =
                (&normalized_designer, existing_designer_info.as_deref())
                && normalize_for_dedup(have) != *want
            {
                continue;
            }
            matches.push(CrossSourceDuplicate {
                existing_entry_id,
                existing_source_id,
                existing_title,
                existing_designer_info,
            });
        }
        Ok(matches)
    }

    /// Saves the details of a facet diagram, first deleting any existing detail,
    /// angle settings, and attached files for `entry_id` so the row set stays fresh
    /// and duplicate-free. Also bumps `entry_id`'s `diagram_entries.updated_at` (see
    /// [`Self::bump_entry_updated_at`]) for the "recently edited" sort.
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
    pub fn save_diagram_detail(&self, detail: &FacetDiagramDetail, entry_id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction().context(format!(
            "Failed to start save transaction for entry_id: {entry_id}"
        ))?;

        // ON DELETE CASCADE handles child rows in angle_settings/attached_files.
        let existing_detail_id: Option<i64> = tx
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
            tx.execute(
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
        let mut stmt_detail = tx.prepare_cached(
            "INSERT INTO diagram_details (
                entry_id, page_url, diagram_image_name, diagram_image_data,
                competition_diagram, lw_ratio, refractive_index, index_gear,
                volume, facets_count, facets, girdle_facets, shape, designer_info,
                hw_ratio, tw_ratio, uw_ratio, pw_ratio, cw_ratio, symmetry_order, mirror_symmetry,
                designer, source_citation, pdf_file, gem_file, shape_category
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
                      ?22, ?23, ?24, ?25, ?26)",
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
            ])
            .context(format!(
                "Failed to insert diagram detail for entry_id: {entry_id}"
            ))?;
        // prepare_cached borrows tx for the statement's lifetime; drop before reborrowing below.
        drop(stmt_detail);

        let detail_id = tx.last_insert_rowid();
        debug!(
            "Inserted diagram detail for entry_id {} with new detail_id: {}",
            entry_id, detail_id
        );

        Self::save_angle_settings(&tx, detail_id, &detail.angle_settings_table)?;
        Self::save_attached_files(&tx, detail_id, &detail.attached_files)?;
        Self::bump_entry_updated_at(&tx, entry_id)?;

        tx.commit().context(format!(
            "Failed to commit save transaction for entry_id: {entry_id}"
        ))?;

        info!(
            "Successfully saved diagram detail and associated data for entry_id: {}",
            entry_id
        );
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
            "UPDATE diagram_entries SET updated_at = ?1 WHERE id = ?2",
            params![unix_now(), entry_id],
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
            "INSERT INTO angle_settings (detail_id, order_idx, facet, angle, index_val, notes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
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

    /// Loads the full record for one diagram entry -- its entry/detail row plus all
    /// associated angle settings and attached files -- or `None` if `entry_id`
    /// doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if any underlying query fails, or a row fails to decode.
    pub fn get_diagram_full(
        &self,
        entry_id: i64,
    ) -> Result<Option<crate::model::entry::FullDiagramRecord>> {
        // Ratio/symmetry_order/shape_category columns are stored as REAL/INTEGER (see
        // create_tables_if_not_exist) but cast back to TEXT here so they stay
        // Option<String>, matching FacetDiagramDetail's convention. mirror_symmetry
        // and the plain-TEXT columns need no cast.
        let mut stmt = self.conn.prepare(
            "SELECT de.id, de.title, de.url, de.design_id,
                    dd.id, dd.page_url, dd.diagram_image_name, dd.diagram_image_data,
                    dd.competition_diagram, CAST(dd.lw_ratio AS TEXT), CAST(dd.refractive_index AS TEXT),
                    CAST(dd.index_gear AS TEXT), CAST(dd.volume AS TEXT), dd.facets_count, dd.shape, dd.designer_info,
                    CAST(dd.hw_ratio AS TEXT), CAST(dd.tw_ratio AS TEXT), CAST(dd.uw_ratio AS TEXT),
                    CAST(dd.pw_ratio AS TEXT), CAST(dd.cw_ratio AS TEXT), CAST(dd.symmetry_order AS TEXT),
                    dd.mirror_symmetry, dd.designer, dd.source_citation, dd.pdf_file, dd.gem_file,
                    CAST(dd.shape_category AS TEXT)
             FROM diagram_entries de
             LEFT JOIN diagram_details dd ON de.id = dd.entry_id
             WHERE de.id = ?1",
        )?;

        let mut rows = stmt.query(params![entry_id])?;
        if let Some(row) = rows.next()? {
            let detail_id_opt: Option<i64> = row.get(4)?;

            let (angles, files) = if let Some(detail_id) = detail_id_opt {
                let mut stmt_angles = self.conn.prepare(
                    "SELECT facet, angle, index_val, notes, order_idx
                     FROM angle_settings
                     WHERE detail_id = ?1
                     ORDER BY order_idx ASC",
                )?;
                let a_rows = stmt_angles.query_map(params![detail_id], |arow| {
                    Ok(crate::model::angle::AngleSetting {
                        facet: arow.get(0)?,
                        angle: arow.get(1)?,
                        index: arow.get(2)?,
                        notes: arow.get(3)?,
                        order_index: arow.get(4)?,
                    })
                })?;
                let angles: Vec<_> =
                    a_rows
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .context(format!(
                            "Failed to decode an angle setting row for detail_id: {detail_id}"
                        ))?;

                let mut stmt_files = self.conn.prepare(
                    "SELECT name, url, content
                     FROM attached_files
                     WHERE detail_id = ?1",
                )?;
                let f_rows = stmt_files.query_map(params![detail_id], |frow| {
                    Ok(crate::model::file::AttachedFile {
                        name: frow.get(0)?,
                        url: frow.get(1)?,
                        content: frow.get(2)?,
                    })
                })?;
                let files: Vec<_> =
                    f_rows
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .context(format!(
                            "Failed to decode an attached-file row for detail_id: {detail_id}"
                        ))?;

                (angles, files)
            } else {
                (Vec::new(), Vec::new())
            };

            return Ok(Some(crate::model::entry::FullDiagramRecord {
                entry_id: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                design_id: row.get(3)?,
                page_url: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                diagram_image_name: row.get(6)?,
                diagram_image_data: row.get(7)?,
                competition_diagram: row.get(8)?,
                lw_ratio: row.get(9)?,
                refractive_index: row.get(10)?,
                index_gear: row.get(11)?,
                volume: row.get(12)?,
                facets_count: row.get(13)?,
                shape: row.get(14)?,
                designer_info: row.get(15)?,
                hw_ratio: row.get(16)?,
                tw_ratio: row.get(17)?,
                uw_ratio: row.get(18)?,
                pw_ratio: row.get(19)?,
                cw_ratio: row.get(20)?,
                symmetry_order: row.get(21)?,
                mirror_symmetry: row.get(22)?,
                designer: row.get(23)?,
                source_citation: row.get(24)?,
                pdf_file: row.get(25)?,
                gem_file: row.get(26)?,
                shape_category: row.get(27)?,
                angle_settings: angles,
                attached_files: files,
            }));
        }

        Ok(None)
    }

    /// Loads the same record [`Self::get_diagram_full`] would, except attached files
    /// come back as [`crate::model::entry::AttachedFileMeta`] (id/name/url/size) --
    /// `content` is never selected, so no attachment bytes are loaded into memory.
    /// For a remote-serving caller (e.g. `indicatrix-worker`'s library protocol),
    /// attachments can be large and shared across designs, so loading every one's
    /// bytes on every design fetch would be wasteful when a caller only wants them
    /// lazily, one at a time, by id (see [`Self::get_attachment_content`]).
    ///
    /// # Errors
    ///
    /// Returns an error if any underlying query fails. Unlike [`Self::get_diagram_full`],
    /// a row that fails to decode in the angle-settings/attached-files loops here is
    /// silently skipped rather than propagated -- this method's callers (a remote
    /// library listing) have historically relied on that, and fixing it is out of this
    /// pass's scope (only `get_diagram_full` itself was in the finding this addresses).
    pub fn get_diagram_full_meta(
        &self,
        entry_id: i64,
    ) -> Result<Option<crate::model::entry::FullDiagramMeta>> {
        // Same TEXT-cast rationale as get_diagram_full, above.
        let mut stmt = self.conn.prepare(
            "SELECT de.id, de.title, de.url, de.design_id,
                    dd.id, dd.page_url, dd.diagram_image_name, dd.diagram_image_data,
                    dd.competition_diagram, CAST(dd.lw_ratio AS TEXT), CAST(dd.refractive_index AS TEXT),
                    CAST(dd.index_gear AS TEXT), CAST(dd.volume AS TEXT), dd.facets_count, dd.shape, dd.designer_info,
                    CAST(dd.hw_ratio AS TEXT), CAST(dd.tw_ratio AS TEXT), CAST(dd.uw_ratio AS TEXT),
                    CAST(dd.pw_ratio AS TEXT), CAST(dd.cw_ratio AS TEXT), CAST(dd.symmetry_order AS TEXT),
                    dd.mirror_symmetry, dd.designer
             FROM diagram_entries de
             LEFT JOIN diagram_details dd ON de.id = dd.entry_id
             WHERE de.id = ?1",
        )?;

        let mut rows = stmt.query(params![entry_id])?;
        if let Some(row) = rows.next()? {
            let detail_id_opt: Option<i64> = row.get(4)?;

            let mut angles = Vec::new();
            let mut files = Vec::new();

            if let Some(detail_id) = detail_id_opt {
                let mut stmt_angles = self.conn.prepare(
                    "SELECT facet, angle, index_val, notes, order_idx
                     FROM angle_settings
                     WHERE detail_id = ?1
                     ORDER BY order_idx ASC",
                )?;
                let a_rows = stmt_angles.query_map(params![detail_id], |arow| {
                    Ok(crate::model::angle::AngleSetting {
                        facet: arow.get(0)?,
                        angle: arow.get(1)?,
                        index: arow.get(2)?,
                        notes: arow.get(3)?,
                        order_index: arow.get(4)?,
                    })
                })?;
                for a in a_rows.flatten() {
                    angles.push(a);
                }

                // length(content), never content itself, keeps this metadata-only.
                let mut stmt_files = self.conn.prepare(
                    "SELECT id, name, url, length(content)
                     FROM attached_files
                     WHERE detail_id = ?1",
                )?;
                let f_rows = stmt_files.query_map(params![detail_id], |frow| {
                    Ok(crate::model::entry::AttachedFileMeta {
                        id: frow.get(0)?,
                        name: frow.get(1)?,
                        url: frow.get(2)?,
                        size: frow.get(3)?,
                    })
                })?;
                for f in f_rows.flatten() {
                    files.push(f);
                }
            }

            return Ok(Some(crate::model::entry::FullDiagramMeta {
                entry_id: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                design_id: row.get(3)?,
                page_url: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                diagram_image_name: row.get(6)?,
                diagram_image_data: row.get(7)?,
                competition_diagram: row.get(8)?,
                lw_ratio: row.get(9)?,
                refractive_index: row.get(10)?,
                index_gear: row.get(11)?,
                volume: row.get(12)?,
                facets_count: row.get(13)?,
                shape: row.get(14)?,
                designer_info: row.get(15)?,
                hw_ratio: row.get(16)?,
                tw_ratio: row.get(17)?,
                uw_ratio: row.get(18)?,
                pw_ratio: row.get(19)?,
                cw_ratio: row.get(20)?,
                symmetry_order: row.get(21)?,
                mirror_symmetry: row.get(22)?,
                designer: row.get(23)?,
                angle_settings: angles,
                attached_files: files,
            }));
        }

        Ok(None)
    }

    /// Loads exactly one attachment's name and content by id -- never a whole design's
    /// attachment set (see [`Self::get_diagram_full_meta`] for why that split exists).
    /// Bounds per-request memory use to one attachment's size.
    ///
    /// `None` if `attachment_id` doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn get_attachment_content(&self, attachment_id: i64) -> Result<Option<(String, Vec<u8>)>> {
        self.conn
            .query_row(
                "SELECT name, content FROM attached_files WHERE id = ?1",
                params![attachment_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context(format!(
                "Failed to load attachment content for attachment_id: {attachment_id}"
            ))
    }

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
                params![trimmed, unix_now(), entry_id],
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
    /// a STRICT SUBSET of [`FacetDiagramDetail`] (missing `hw_ratio`/`tw_ratio`/
    /// `uw_ratio`/`pw_ratio`/`cw_ratio`/`symmetry_order`/`mirror_symmetry`/`designer`/
    /// `source_citation`/`pdf_file`/`gem_file`/`shape_category`). Since
    /// [`Self::save_diagram_detail`] fully REPLACES the detail row, a naive
    /// read-edit-rebuild-save would silently zero every field above -- erasing a
    /// locally-imported design's just-measured proportions. This method is the fix:
    /// one `UPDATE` naming exactly `MetadataUpdate`'s fields (plus `facets`/
    /// `girdle_facets`, kept in sync below) and nothing else -- no delete, so every
    /// other column and all child rows survive unchanged.
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
            params![unix_now(), entry_id],
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
                params![url, unix_now(), entry_id],
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

/// The current wall-clock time as Unix seconds, for `diagram_entries.created_at`/
/// `updated_at`. Same `SystemTime`-based approach this crate
/// already uses for `diagram_tilt_curves.generated_at`/`diagram_previews.preview_generated_at`
/// (see those tables' save methods), just computed here instead of taken as a caller
/// parameter -- `save_diagram_entry`/`update_diagram_metadata` are existing public
/// signatures with call sites across the workspace, so stamping the time internally
/// keeps every one of them compiling unchanged.
///
/// Falls back to `0` on a system clock set before the Unix epoch, which never happens
/// on a real machine -- this only avoids a panic on `duration_since`'s `Result`.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
