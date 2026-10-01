//! Loading a design record back out of `diagram_entries`/`diagram_details`: the full
//! local record, the metadata-only (no attachment bytes) form a remote-serving caller
//! uses, and fetching one attachment's content lazily by id.

use super::Database;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, Row, params, types::ValueRef};

/// The entry/detail row both record loaders read, one design selected by `?1`.
///
/// The `INTEGER` columns (`index_gear`, `symmetry_order`, `shape_category`) are cast to
/// TEXT here so they surface as `Option<String>`, matching `FacetingDiagramDetail`'s
/// convention; the cast is exact for an integer. The `REAL` columns (the ratios, the
/// refractive index and the volume) are NOT cast: `CAST(real AS TEXT)` keeps only 15
/// significant digits, which loses the last digit of a stored double, so they are read
/// as numbers and formatted by [`real_column_text`] instead. `mirror_symmetry` and the
/// plain-TEXT columns need no conversion. The column positions are what the two loaders'
/// row mapping indexes by.
const RECORD_ROW_SQL: &str = "SELECT de.id, de.title, de.url, de.design_id,
            dd.id, dd.page_url, dd.diagram_image_name, dd.diagram_image_data,
            dd.competition_diagram, dd.lw_ratio, dd.refractive_index,
            CAST(dd.index_gear AS TEXT), dd.volume, dd.facets_count, dd.shape, dd.designer_info,
            dd.hw_ratio, dd.tw_ratio, dd.uw_ratio,
            dd.pw_ratio, dd.cw_ratio, CAST(dd.symmetry_order AS TEXT),
            dd.mirror_symmetry, dd.designer, dd.source_citation, dd.pdf_file, dd.gem_file,
            CAST(dd.shape_category AS TEXT)
     FROM diagram_entries de
     LEFT JOIN diagram_details dd ON de.id = dd.entry_id
     WHERE de.id = ?1";

/// Formats a stored double as the shortest decimal text that parses back to exactly the
/// same double, with a `.0` appended to an integral value (as SQLite's own
/// `CAST(real AS TEXT)` writes it, so `2` and `2.0` do not change meaning on the way
/// through). Rust's `Display` for `f64` is that shortest round-trip form and never uses
/// an exponent.
fn format_real(value: f64) -> String {
    let text = value.to_string();
    if value.is_finite() && !text.contains('.') {
        format!("{text}.0")
    } else {
        text
    }
}

/// Reads the `REAL` column at `index` as text without losing a digit: `NULL` is `None`, a
/// stored double goes through [`format_real`], and a value SQLite kept as TEXT (a
/// non-numeric legacy entry) is passed through untouched, exactly as the `CAST` this
/// replaces would have.
fn real_column_text(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<String>> {
    Ok(match row.get_ref(index)? {
        ValueRef::Null | ValueRef::Blob(_) => None,
        ValueRef::Real(value) => Some(format_real(value)),
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    })
}

impl Database {
    /// Loads the full record for one diagram entry -- its entry/detail row plus all
    /// associated angle settings and attached files -- or `None` if `entry_id`
    /// doesn't exist.
    ///
    /// The ratio, refractive-index and volume fields are the shortest decimal text that
    /// reads back as the stored double bit-for-bit, so a caller that round-trips them
    /// (an edit that changes one field and writes the rest back) loses nothing.
    ///
    /// # Errors
    ///
    /// Returns an error if any underlying query fails, or a row fails to decode.
    pub fn get_diagram_full(
        &self,
        entry_id: i64,
    ) -> Result<Option<crate::model::entry::FullDiagramRecord>> {
        let mut stmt = self.conn.prepare(RECORD_ROW_SQL)?;

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
                lw_ratio: real_column_text(row, 9)?,
                refractive_index: real_column_text(row, 10)?,
                index_gear: row.get(11)?,
                volume: real_column_text(row, 12)?,
                facets_count: row.get(13)?,
                shape: row.get(14)?,
                designer_info: row.get(15)?,
                hw_ratio: real_column_text(row, 16)?,
                tw_ratio: real_column_text(row, 17)?,
                uw_ratio: real_column_text(row, 18)?,
                pw_ratio: real_column_text(row, 19)?,
                cw_ratio: real_column_text(row, 20)?,
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
    /// library listing) have historically relied on that, and fixing it would change
    /// that long-standing behaviour.
    pub fn get_diagram_full_meta(
        &self,
        entry_id: i64,
    ) -> Result<Option<crate::model::entry::FullDiagramMeta>> {
        // Same row and the same exact-digits treatment of the REAL columns as
        // get_diagram_full, above.
        let mut stmt = self.conn.prepare(RECORD_ROW_SQL)?;

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
                lw_ratio: real_column_text(row, 9)?,
                refractive_index: real_column_text(row, 10)?,
                index_gear: row.get(11)?,
                volume: real_column_text(row, 12)?,
                facets_count: row.get(13)?,
                shape: row.get(14)?,
                designer_info: row.get(15)?,
                hw_ratio: real_column_text(row, 16)?,
                tw_ratio: real_column_text(row, 17)?,
                uw_ratio: real_column_text(row, 18)?,
                pw_ratio: real_column_text(row, 19)?,
                cw_ratio: real_column_text(row, 20)?,
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

    /// `entry_id`'s revision stamp (`diagram_entries.updated_at`, Unix seconds), `None`
    /// for a row that predates the column and was never re-saved.
    ///
    /// A long computation over a design (a preview render, a tilt sweep) reads this in
    /// the same locked section as the record it is about to work on, and hands it back
    /// to [`Self::save_preview_images`] / [`Self::save_tilt_curves`] as the
    /// compare-and-swap token: every write that replaces or edits a design bumps the
    /// stamp, so a result computed from a superseded record is refused rather than
    /// stored over the invalidation that should have removed it.
    ///
    /// # Errors
    ///
    /// Returns an error if `entry_id` names no row, or the query fails.
    pub fn entry_updated_at(&self, entry_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT updated_at FROM diagram_entries WHERE id = ?1",
                params![entry_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .with_context(|| format!("Failed to read updated_at for entry_id: {entry_id}"))
    }

    /// Every non-ignored `diagram_entries.id`, ordered by id: the whole local
    /// catalogue, for the "regenerate all previews / tilt curves" batches.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn all_entry_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM diagram_entries WHERE ignored = 0 ORDER BY id")?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()
            .context("Failed to read the catalogue's entry ids")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{detail::FacetingDiagramDetail, entry::FacetingDiagramEntry};

    fn temp_db() -> (Database, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-entry-read-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        (db, path)
    }

    fn add_entry_with_detail(db: &Database) -> i64 {
        let entry_id = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: "Read Test".to_string(),
                    url: "local://read-test.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        db.save_diagram_detail(&FacetingDiagramDetail::default(), entry_id)
            .unwrap();
        entry_id
    }

    /// A double that needs all 17 significant digits to be told apart from its
    /// neighbours: `0.1 + 0.2`, which is `0.30000000000000004`.
    fn seventeen_digit_value() -> f64 {
        0.1 + 0.2
    }

    #[test]
    fn a_stored_double_with_seventeen_significant_digits_reads_back_bit_for_bit() {
        let (db, path) = temp_db();
        let entry_id = add_entry_with_detail(&db);
        let value = seventeen_digit_value();
        assert_ne!(
            format!("{value:.14}").parse::<f64>().unwrap().to_bits(),
            value.to_bits(),
            "the fixture must not survive 15 significant digits"
        );
        // Bound as a double, so SQLite stores exactly these bits (a TEXT bind would go
        // through SQLite's own decimal parser instead).
        db.conn
            .execute(
                "UPDATE diagram_details SET refractive_index = ?1, lw_ratio = ?1, volume = ?1,
                     hw_ratio = ?1, tw_ratio = ?1, uw_ratio = ?1, pw_ratio = ?1, cw_ratio = ?1
                 WHERE entry_id = ?2",
                params![value, entry_id],
            )
            .unwrap();

        let full = db.get_diagram_full(entry_id).unwrap().unwrap();
        let meta = db.get_diagram_full_meta(entry_id).unwrap().unwrap();
        let read_back = |text: Option<&str>| text.unwrap().parse::<f64>().unwrap().to_bits();
        for (name, full_text, meta_text) in [
            (
                "refractive_index",
                &full.refractive_index,
                &meta.refractive_index,
            ),
            ("lw_ratio", &full.lw_ratio, &meta.lw_ratio),
            ("volume", &full.volume, &meta.volume),
            ("hw_ratio", &full.hw_ratio, &meta.hw_ratio),
            ("tw_ratio", &full.tw_ratio, &meta.tw_ratio),
            ("uw_ratio", &full.uw_ratio, &meta.uw_ratio),
            ("pw_ratio", &full.pw_ratio, &meta.pw_ratio),
            ("cw_ratio", &full.cw_ratio, &meta.cw_ratio),
        ] {
            assert_eq!(read_back(full_text.as_deref()), value.to_bits(), "{name}");
            assert_eq!(read_back(meta_text.as_deref()), value.to_bits(), "{name}");
        }

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_integral_double_keeps_its_point_zero_and_a_null_stays_none() {
        let (db, path) = temp_db();
        let entry_id = add_entry_with_detail(&db);
        db.conn
            .execute(
                "UPDATE diagram_details SET hw_ratio = 1.0, refractive_index = 2.417
                 WHERE entry_id = ?1",
                params![entry_id],
            )
            .unwrap();

        let full = db.get_diagram_full(entry_id).unwrap().unwrap();
        assert_eq!(full.hw_ratio.as_deref(), Some("1.0"));
        assert_eq!(full.refractive_index.as_deref(), Some("2.417"));
        assert_eq!(full.cw_ratio, None);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_non_numeric_text_value_in_a_real_column_passes_through_unchanged() {
        let (db, path) = temp_db();
        let entry_id = add_entry_with_detail(&db);
        db.conn
            .execute(
                "UPDATE diagram_details SET lw_ratio = 'v2-updated' WHERE entry_id = ?1",
                params![entry_id],
            )
            .unwrap();

        let full = db.get_diagram_full(entry_id).unwrap().unwrap();
        assert_eq!(full.lw_ratio.as_deref(), Some("v2-updated"));

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn format_real_writes_the_shortest_round_trip_text() {
        assert_eq!(format_real(1.762), "1.762");
        assert_eq!(format_real(2.0), "2.0");
        assert_eq!(format_real(-0.5), "-0.5");
        assert_eq!(format_real(seventeen_digit_value()), "0.30000000000000004");
        assert_eq!(format_real(1.0e-7), "0.0000001");
    }

    #[test]
    fn entry_updated_at_reads_the_stamp_and_errors_for_a_missing_entry() {
        let (db, path) = temp_db();
        let entry_id = add_entry_with_detail(&db);
        assert!(db.entry_updated_at(entry_id).unwrap().is_some());

        db.conn
            .execute(
                "UPDATE diagram_entries SET updated_at = NULL WHERE id = ?1",
                params![entry_id],
            )
            .unwrap();
        assert_eq!(db.entry_updated_at(entry_id).unwrap(), None);
        assert!(db.entry_updated_at(entry_id + 1000).is_err());

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn two_edits_in_the_same_second_strictly_advance_the_revision_stamp() {
        let (db, path) = temp_db();
        let entry_id = add_entry_with_detail(&db);
        db.rename_diagram_entry(entry_id, "First").unwrap();
        let first = db.entry_updated_at(entry_id).unwrap().unwrap();
        db.rename_diagram_entry(entry_id, "Second").unwrap();
        let second = db.entry_updated_at(entry_id).unwrap().unwrap();
        assert!(second > first);

        drop(db);
        std::fs::remove_file(&path).ok();
    }
}
