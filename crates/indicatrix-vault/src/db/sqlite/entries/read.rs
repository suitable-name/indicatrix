//! Loading a design record back out of `diagram_entries`/`diagram_details`: the full
//! local record, the metadata-only (no attachment bytes) form a remote-serving caller
//! uses, and fetching one attachment's content lazily by id.

use super::Database;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

impl Database {
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
}
