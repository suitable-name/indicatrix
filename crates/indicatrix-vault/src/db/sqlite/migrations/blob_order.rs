//! The migration that rebuilds the blob-heavy tables so every BLOB column sits last,
//! and the `PRAGMA table_info` position probe it relies on. Split from the parent module
//! so the file stays readable; the functions are unchanged.

use super::{
    super::Database,
    schema::{DIAGRAM_PREVIEWS_REORDERED_TABLE_SQL, diagram_tilt_curves_reordered_table_sql},
    sql_identifier,
};
use crate::model::performance::{all_global_extreme_columns, global_extreme_column_name};
use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction};
use tracing::{debug, info};

#[cfg(doc)]
use super::schema::{DIAGRAM_PREVIEWS_TABLE_SQL, diagram_tilt_curves_table_sql};

impl Database {
    /// Rebuilds `diagram_details`, `diagram_previews`, and `diagram_tilt_curves` so
    /// every BLOB column sits after every other column (see [`DIAGRAM_PREVIEWS_TABLE_SQL`]/
    /// [`diagram_tilt_curves_table_sql`]'s own doc comments and `db::sqlite::SEARCH_INDEXES_SQL`'s
    /// sibling doc comment for the measurements that motivated this). SQLite reads a
    /// row's columns in physical storage order and must skip every earlier column's
    /// bytes -- including a multi-KB image BLOB -- to reach a later one, so a query that
    /// only touches the searched/text columns still pays for the blob it never
    /// actually reads. Measured on the real catalogue: a text search over
    /// `diagram_details` costs 40.9/39.2 ms with `diagram_image_data` at column 5, 16.5
    /// ms with it moved last; `preview_material` lookups drop from 0.9 ms to 0.4 ms.
    ///
    /// SQLite has no `ALTER TABLE ... MODIFY COLUMN`/column-reorder statement, so this
    /// uses the standard rebuild procedure from SQLite's own documentation ("Making
    /// Other Kinds Of Table Schema Changes"): `PRAGMA foreign_keys = OFF` OUTSIDE any
    /// transaction (SQLite refuses to change this pragma mid-transaction), then inside
    /// ONE transaction: for each table that needs it, `CREATE TABLE
    /// <name>__reordered` with the final column order, `INSERT INTO ... (columns...)
    /// SELECT columns... FROM <name>` naming every column explicitly (so the OLD
    /// table's physical column order never matters), `DROP TABLE <name>`, and `ALTER
    /// TABLE <name>__reordered RENAME TO <name>` -- then one `PRAGMA foreign_key_check`
    /// before committing, and `PRAGMA foreign_keys = ON` restored afterward regardless
    /// of outcome.
    ///
    /// `foreign_keys = OFF` is not optional here: `diagram_details` is the `FOREIGN
    /// KEY` parent of `angle_settings`/`attached_files` (`ON DELETE CASCADE`), and a
    /// plain `DROP TABLE diagram_details` with `foreign_keys = ON` cascades that drop
    /// into deleting every `angle_settings`/`attached_files` row before this function
    /// ever gets to reinsert `diagram_details`' own rows -- exactly the wipe this
    /// finding (and the house rule it came from) warns about.
    ///
    /// Idempotent per table, detected via [`Self::column_position`] rather than a
    /// version flag: a fresh database already gets every one of these three tables in
    /// the final shape directly from `create_tables_if_not_exist` (see
    /// [`DIAGRAM_PREVIEWS_TABLE_SQL`]/[`diagram_tilt_curves_table_sql`]'s own doc
    /// comments), so this only ever has real work to do against a database created
    /// before those files adopted the convention.
    ///
    /// # Errors
    ///
    /// Returns an error if checking any column's position, toggling `foreign_keys`,
    /// starting/committing the rebuild transaction, any `CREATE`/`INSERT`/`DROP`/
    /// `RENAME` statement, or the closing `PRAGMA foreign_key_check` fails (that last
    /// one means a child row did not survive -- this function refuses to commit rather
    /// than silently losing it).
    pub(in crate::db::sqlite) fn migrate_blob_columns_last(&self) -> Result<()> {
        let details_needs_rebuild = Self::blob_precedes_marker(
            &self.conn,
            "diagram_details",
            "diagram_image_data",
            "shape_category",
        )?;
        let previews_needs_rebuild = Self::blob_precedes_marker(
            &self.conn,
            "diagram_previews",
            "preview_front",
            "preview_material",
        )?;
        let tilt_curves_needs_rebuild = Self::blob_precedes_marker(
            &self.conn,
            "diagram_tilt_curves",
            "curves",
            "generated_at",
        )?;

        if !details_needs_rebuild && !previews_needs_rebuild && !tilt_curves_needs_rebuild {
            debug!("Blob-columns-last migration already applied to every table; skipping.");
            return Ok(());
        }

        info!("Rebuilding blob-heavy tables with blob columns last...");
        // See this function's own doc comment: PRAGMA foreign_keys can only be toggled
        // outside a transaction, and must be OFF while diagram_details (the FK parent
        // of angle_settings/attached_files) is dropped and recreated below.
        self.conn
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .context("Failed to disable foreign_keys for the blob-columns-last rebuild")?;

        let result = (|| -> Result<()> {
            let tx = self
                .conn
                .unchecked_transaction()
                .context("Failed to start the blob-columns-last rebuild transaction")?;

            if details_needs_rebuild {
                Self::rebuild_diagram_details_blob_last(&tx)?;
            }
            if previews_needs_rebuild {
                Self::rebuild_diagram_previews_blob_last(&tx)?;
            }
            if tilt_curves_needs_rebuild {
                Self::rebuild_diagram_tilt_curves_blob_last(&tx)?;
            }

            Self::check_no_foreign_key_violations(&tx)?;
            tx.commit()
                .context("Failed to commit the blob-columns-last rebuild transaction")?;
            Ok(())
        })();

        // Restored unconditionally, even on failure: leaving this connection running
        // with foreign-key enforcement off for the rest of its lifetime would be a far
        // worse outcome than this rebuild itself failing.
        self.conn
            .execute_batch("PRAGMA foreign_keys = ON;")
            .context("Failed to re-enable foreign_keys after the blob-columns-last rebuild")?;

        result?;
        info!("Blob-columns-last rebuild complete.");
        Ok(())
    }

    /// Whether `table.blob_column` currently sits BEFORE `table.marker_column` in
    /// physical column order (`PRAGMA table_info`'s `cid`) -- true means this table
    /// still has its pre-fix, blob-first shape and needs
    /// [`Self::migrate_blob_columns_last`] to rebuild it. `false` covers both "already
    /// rebuilt" (the blob column now sorts after the marker) and "table doesn't exist
    /// yet" (either column missing) -- a fresh database's `create_tables_if_not_exist`
    /// already creates every one of these three tables in the final shape, so there is
    /// nothing to rebuild until a real pre-fix database is opened.
    ///
    /// # Errors
    ///
    /// Returns an error if `table`/`blob_column`/`marker_column` is not a valid SQL
    /// identifier, or the underlying `PRAGMA table_info` query fails.
    fn blob_precedes_marker(
        conn: &Connection,
        table: &str,
        blob_column: &str,
        marker_column: &str,
    ) -> Result<bool> {
        let blob_pos = Self::column_position(conn, table, blob_column)?;
        let marker_pos = Self::column_position(conn, table, marker_column)?;
        Ok(match (blob_pos, marker_pos) {
            (Some(blob), Some(marker)) => blob < marker,
            _ => false,
        })
    }

    /// Rebuilds `diagram_details` with `diagram_image_data` moved to the last column --
    /// see [`Self::migrate_blob_columns_last`]'s doc comment for the rebuild procedure
    /// and why `foreign_keys` must already be `OFF` by the time this runs.
    ///
    /// # Errors
    ///
    /// Returns an error if any `CREATE`/`INSERT`/`DROP`/`RENAME` statement fails.
    fn rebuild_diagram_details_blob_last(tx: &Transaction<'_>) -> Result<()> {
        // `DROP TABLE` also drops the table's AUTOINCREMENT sequence; it is read first and
        // put back on the rebuilt table, so an id that was handed out and deleted before
        // the rebuild is never handed out again.
        let sequence: i64 = tx
            .query_row(
                "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'diagram_details'), 0)",
                [],
                |row| row.get(0),
            )
            .context("Failed to read the diagram_details AUTOINCREMENT sequence")?;
        // Guards against a fossil staging table left behind by a previous run of this
        // migration that started but never finished (e.g. the process was killed
        // mid-rebuild) -- harmless on the far more common case where it never existed.
        tx.execute_batch("DROP TABLE IF EXISTS diagram_details__reordered;")
            .context("Failed to clear a stale diagram_details__reordered")?;
        tx.execute_batch(
            "CREATE TABLE diagram_details__reordered (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_id INTEGER NOT NULL UNIQUE,
                page_url TEXT NOT NULL,
                diagram_image_name TEXT,
                competition_diagram TEXT,
                lw_ratio REAL,
                refractive_index REAL,
                index_gear INTEGER,
                volume REAL,
                facets_count TEXT,
                facets INTEGER,
                girdle_facets INTEGER,
                shape TEXT,
                designer_info TEXT,
                hw_ratio REAL,
                tw_ratio REAL,
                uw_ratio REAL,
                pw_ratio REAL,
                cw_ratio REAL,
                symmetry_order INTEGER,
                mirror_symmetry BOOLEAN,
                designer TEXT,
                source_citation TEXT,
                pdf_file TEXT,
                gem_file TEXT,
                shape_category INTEGER,
                concave_tiers INTEGER NOT NULL DEFAULT 0,
                concave_facets INTEGER NOT NULL DEFAULT 0,
                diagram_image_data BLOB,
                FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
            );",
        )
        .context("Failed to create diagram_details__reordered")?;

        tx.execute_batch(
            "INSERT INTO diagram_details__reordered (
                id, entry_id, page_url, diagram_image_name, competition_diagram,
                lw_ratio, refractive_index, index_gear, volume, facets_count, facets,
                girdle_facets, shape, designer_info, hw_ratio, tw_ratio, uw_ratio,
                pw_ratio, cw_ratio, symmetry_order, mirror_symmetry, designer,
                source_citation, pdf_file, gem_file, shape_category, concave_tiers,
                concave_facets, diagram_image_data
            )
            SELECT
                id, entry_id, page_url, diagram_image_name, competition_diagram,
                lw_ratio, refractive_index, index_gear, volume, facets_count, facets,
                girdle_facets, shape, designer_info, hw_ratio, tw_ratio, uw_ratio,
                pw_ratio, cw_ratio, symmetry_order, mirror_symmetry, designer,
                source_citation, pdf_file, gem_file, shape_category, concave_tiers,
                concave_facets, diagram_image_data
            FROM diagram_details;
            DROP TABLE diagram_details;
            ALTER TABLE diagram_details__reordered RENAME TO diagram_details;",
        )
        .context("Failed to rebuild diagram_details with diagram_image_data last")?;
        // The copy of the rows seeds the new sequence with their largest id (or leaves no
        // row at all when the table was empty); the old sequence wins when it was higher.
        tx.execute(
            "INSERT INTO sqlite_sequence (name, seq)
             SELECT 'diagram_details', ?1
             WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'diagram_details')",
            [sequence],
        )
        .context("Failed to seed the diagram_details AUTOINCREMENT sequence")?;
        tx.execute(
            "UPDATE sqlite_sequence SET seq = MAX(seq, ?1) WHERE name = 'diagram_details'",
            [sequence],
        )
        .context("Failed to restore the diagram_details AUTOINCREMENT sequence")?;
        Ok(())
    }

    /// Rebuilds `diagram_previews` with `preview_material`/`preview_generated_at`
    /// moved before the `preview_front`/`preview_top` blobs -- see
    /// [`Self::migrate_blob_columns_last`]'s doc comment for the rebuild procedure.
    ///
    /// # Errors
    ///
    /// Returns an error if any `CREATE`/`INSERT`/`DROP`/`RENAME` statement fails.
    fn rebuild_diagram_previews_blob_last(tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch("DROP TABLE IF EXISTS diagram_previews__reordered;")
            .context("Failed to clear a stale diagram_previews__reordered")?;
        tx.execute_batch(DIAGRAM_PREVIEWS_REORDERED_TABLE_SQL)
            .context("Failed to create diagram_previews__reordered")?;
        tx.execute_batch(
            "INSERT INTO diagram_previews__reordered (
                entry_id, preview_material, preview_generated_at, params_fingerprint,
                preview_front, preview_top
            )
            SELECT entry_id, preview_material, preview_generated_at, params_fingerprint,
                   preview_front, preview_top
            FROM diagram_previews;
            DROP TABLE diagram_previews;
            ALTER TABLE diagram_previews__reordered RENAME TO diagram_previews;",
        )
        .context("Failed to rebuild diagram_previews with blob columns last")?;
        Ok(())
    }

    /// Rebuilds `diagram_tilt_curves` with `generated_at`/the 6 derived `perf_*`
    /// columns moved before the `curves`/`curve_image` blobs -- see
    /// [`Self::migrate_blob_columns_last`]'s doc comment for the rebuild procedure. The
    /// column list is built from [`all_global_extreme_columns`]/
    /// [`global_extreme_column_name`], the same single source of truth
    /// [`diagram_tilt_curves_table_sql`] uses, so it can never drift from the table
    /// this rebuilds.
    ///
    /// # Errors
    ///
    /// Returns an error if any `CREATE`/`INSERT`/`DROP`/`RENAME` statement fails.
    fn rebuild_diagram_tilt_curves_blob_last(tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch("DROP TABLE IF EXISTS diagram_tilt_curves__reordered;")
            .context("Failed to clear a stale diagram_tilt_curves__reordered")?;
        tx.execute_batch(&diagram_tilt_curves_reordered_table_sql())
            .context("Failed to create diagram_tilt_curves__reordered")?;

        let mut columns = vec![
            "entry_id".to_string(),
            "generated_at".to_string(),
            "params_fingerprint".to_string(),
        ];
        for (metric, extreme) in all_global_extreme_columns() {
            columns.push(global_extreme_column_name(metric, extreme));
        }
        columns.push("curves".to_string());
        columns.push("curve_image".to_string());
        let column_list = columns.join(", ");

        tx.execute_batch(&format!(
            "INSERT INTO diagram_tilt_curves__reordered ({column_list})
             SELECT {column_list} FROM diagram_tilt_curves;
             DROP TABLE diagram_tilt_curves;
             ALTER TABLE diagram_tilt_curves__reordered RENAME TO diagram_tilt_curves;"
        ))
        .context("Failed to rebuild diagram_tilt_curves with blob columns last")?;
        Ok(())
    }

    /// Runs `PRAGMA foreign_key_check` and turns the first reported violation (if any)
    /// into an error -- the guard that makes [`Self::migrate_blob_columns_last`]'s
    /// rebuild refuse to commit if a child row somehow didn't survive, instead of
    /// silently persisting a broken database.
    ///
    /// # Errors
    ///
    /// Returns an error if the `PRAGMA` itself fails to run, or if it reports any
    /// violation.
    fn check_no_foreign_key_violations(tx: &Transaction<'_>) -> Result<()> {
        let mut stmt = tx
            .prepare("PRAGMA foreign_key_check;")
            .context("Failed to prepare PRAGMA foreign_key_check")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let table: String = row.get(0)?;
            let rowid: Option<i64> = row.get(1)?;
            let parent: String = row.get(2)?;
            anyhow::bail!(
                "PRAGMA foreign_key_check found a violation after the blob-columns-last \
                 rebuild: table {table} row {rowid:?} references missing {parent} row -- \
                 refusing to commit"
            );
        }
        Ok(())
    }

    /// `table.column`'s 0-based physical column position (`PRAGMA table_info`'s
    /// `cid`), or `None` if `table` has no such column -- the ordering counterpart of
    /// [`Self::column_sql_type`], used by [`Self::migrate_blob_columns_last`] (via
    /// [`Self::blob_precedes_marker`]) to detect whether a table's blob column already
    /// sits after its marker column, without a third hand-parsing of `PRAGMA
    /// table_info`.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` is not a valid SQL identifier (see
    /// [`sql_identifier`]) or the underlying `PRAGMA table_info` query fails.
    pub(super) fn column_position(
        conn: &Connection,
        table: &str,
        column: &str,
    ) -> Result<Option<i64>> {
        let table_ident = sql_identifier(table)?;
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table_ident})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get("name")?;
            if name == column {
                return Ok(Some(row.get("cid")?));
            }
        }
        Ok(None)
    }
}
