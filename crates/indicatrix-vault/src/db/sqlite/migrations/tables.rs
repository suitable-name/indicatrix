//! The migrations that create a whole side table (`diagram_previews`, tilt curves, solid
//! extents and hull, saved rough plans, planner exclusions, per-design variants, cutting
//! progress and lighting, render jobs, tags) for a database that
//! predates it, plus the one that prunes the first-draft tilt-curve columns. Each is idempotent and its SQL
//! text lives in [`super::schema`], shared verbatim with `create_tables_if_not_exist`.

use super::{
    super::Database,
    helpers::sql_identifier,
    schema::{
        DESIGN_CUT_PROGRESS_TABLE_SQL, DESIGN_LIGHTING_TABLE_SQL, DESIGN_VARIANTS_TABLE_SQL,
        DIAGRAM_PLANNER_EXCLUSIONS_TABLE_SQL, DIAGRAM_PREVIEWS_REORDERED_TABLE_SQL,
        DIAGRAM_PREVIEWS_TABLE_SQL, DIAGRAM_SOLID_EXTENTS_TABLE_SQL, DIAGRAM_SOLID_HULL_TABLE_SQL,
        OBSOLETE_TILT_CURVE_AGGREGATE_COLUMNS, RENDER_JOBS_TABLE_SQL,
        SAVED_ROUGH_PLANS_SUMMARY_COLUMN, SAVED_ROUGH_PLANS_TABLE_SQL, TAG_TABLES_SQL,
        diagram_tilt_curves_reordered_table_sql, diagram_tilt_curves_table_sql,
    },
};
use crate::model::performance::{
    Extreme, PerformanceMetric, all_global_extreme_columns, global_extreme_column_name,
};
use anyhow::{Context, Result};
use tracing::{debug, info};

/// The column every cached artefact table (`diagram_previews`, `diagram_tilt_curves`)
/// carries to record what its stored result was computed with.
const PARAMS_FINGERPRINT_COLUMN: &str = "params_fingerprint";

impl Database {
    /// Creates `diagram_previews` for a database created before cached preview
    /// rendering existed -- see [`Self::save_preview_images`](Database::save_preview_images)/
    /// [`Self::get_preview_images`](Database::get_preview_images)/
    /// [`Self::ensure_preview_material`](Database::ensure_preview_material) -- and adds
    /// its `params_fingerprint` column to one that predates fingerprints.
    ///
    /// Keyed by `entry_id`, cascading off `diagram_entries` (not `diagram_details`,
    /// which [`Database::save_diagram_detail`] deletes and reinserts with a new row id
    /// on every re-sync): a routine metadata re-sync can never silently discard a
    /// cached render this way -- only deleting the design itself does.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS` (also created inside
    /// `create_tables_if_not_exist` so a fresh database already has it) and a
    /// column-presence gate for `params_fingerprint`. An existing table gets that column
    /// by a rebuild, not `ADD COLUMN`: `ADD COLUMN` would append it AFTER the two PNG
    /// blobs, and the scan that reads it for every design
    /// ([`Self::entry_ids_missing_previews`](Database::entry_ids_missing_previews))
    /// would then pay to skip every image's bytes -- the cost
    /// [`Self::migrate_blob_columns_last`] exists to avoid. Every existing row is kept
    /// with a `NULL` fingerprint, which reads as "outdated" and is re-rendered once.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table, or rebuilding it with the new column,
    /// fails.
    pub(in crate::db::sqlite) fn migrate_diagram_previews_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DIAGRAM_PREVIEWS_TABLE_SQL)
            .context("Failed to create diagram_previews table")?;
        let carried: Vec<String> = [
            "entry_id",
            "preview_material",
            "preview_generated_at",
            "preview_front",
            "preview_top",
        ]
        .map(str::to_string)
        .into();
        self.add_params_fingerprint_column(
            "diagram_previews",
            DIAGRAM_PREVIEWS_REORDERED_TABLE_SQL,
            &carried,
        )
    }

    /// Creates `diagram_tilt_curves` for a database created before cached
    /// tilt-performance curves existed -- see
    /// [`Self::save_tilt_curves`](Database::save_tilt_curves)/
    /// [`Self::get_tilt_curves`](Database::get_tilt_curves).
    ///
    /// Keyed by `entry_id`, cascading off `diagram_entries`, for the same reason as
    /// `diagram_previews` -- a routine re-sync must not silently discard a curve, which
    /// is at least as expensive to regenerate as a preview render.
    ///
    /// Besides the packed curve BLOB, carries 6 derived `REAL` columns (3 metrics x
    /// {global min, global max}), generated from
    /// [`crate::model::performance::all_global_extreme_columns`] rather than
    /// hand-listed, so this table's columns and
    /// `crate::db::sqlite::search::build_search_predicate` can never drift apart via a
    /// transcription typo.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS` and a column-presence gate
    /// for `params_fingerprint`, which an existing table gets by a rebuild that keeps
    /// the BLOB columns last (see [`Self::migrate_diagram_previews_table`] for why, and
    /// for the `NULL`-means-outdated reading of an old row).
    ///
    /// A table still in the never-released first-draft 36-column shape (no
    /// `perf_*_global_*` columns yet) cannot be rebuilt by name here, so it gets a plain
    /// `ADD COLUMN`; [`Self::migrate_prune_tilt_curve_aggregate_columns`] and
    /// [`Self::migrate_blob_columns_last`] run after this and rebuild that table into its
    /// final shape, which carries the column in its place.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table, or adding `params_fingerprint` to it,
    /// fails.
    pub(in crate::db::sqlite) fn migrate_diagram_tilt_curves_table(&self) -> Result<()> {
        self.conn
            .execute_batch(&diagram_tilt_curves_table_sql())
            .context("Failed to create diagram_tilt_curves table")?;

        let first_draft_shape = !Self::column_exists(
            &self.conn,
            "diagram_tilt_curves",
            &global_extreme_column_name(PerformanceMetric::Brilliance, Extreme::Min),
        )?;
        if first_draft_shape {
            if !Self::column_exists(&self.conn, "diagram_tilt_curves", PARAMS_FINGERPRINT_COLUMN)? {
                self.conn
                    .execute_batch(&format!(
                        "ALTER TABLE diagram_tilt_curves ADD COLUMN {PARAMS_FINGERPRINT_COLUMN} TEXT;"
                    ))
                    .context("Failed to add diagram_tilt_curves.params_fingerprint column")?;
            }
            return Ok(());
        }

        let mut carried = vec!["entry_id".to_string(), "generated_at".to_string()];
        for (metric, extreme) in all_global_extreme_columns() {
            carried.push(global_extreme_column_name(metric, extreme));
        }
        carried.push("curves".to_string());
        carried.push("curve_image".to_string());
        self.add_params_fingerprint_column(
            "diagram_tilt_curves",
            &diagram_tilt_curves_reordered_table_sql(),
            &carried,
        )
    }

    /// Gives `table` the `params_fingerprint` column when it lacks it, by rebuilding it
    /// from `reordered_sql` (the table's current `CREATE TABLE` text, naming
    /// `{table}__reordered` and declaring the column before the BLOBs) and copying
    /// `carried_columns` across unchanged. A table that already has the column is left
    /// alone.
    ///
    /// The copy, the `DROP` and the `RENAME` run in one transaction, so a failure leaves
    /// the original table as it was. `diagram_previews` and `diagram_tilt_curves` are
    /// leaf tables (nothing references them; they only reference `diagram_entries`), so
    /// unlike the `diagram_details` rebuild in [`Self::migrate_blob_columns_last`] this
    /// needs no `foreign_keys = OFF`.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` or a column name is not a valid SQL identifier, or
    /// the transaction or any statement in it fails.
    fn add_params_fingerprint_column(
        &self,
        table: &str,
        reordered_sql: &str,
        carried_columns: &[String],
    ) -> Result<()> {
        let table = sql_identifier(table)?;
        if Self::column_exists(&self.conn, table, PARAMS_FINGERPRINT_COLUMN)? {
            debug!("{table}.{PARAMS_FINGERPRINT_COLUMN} already present; skipping.");
            return Ok(());
        }

        info!("Adding {table}.{PARAMS_FINGERPRINT_COLUMN}...");
        let staging = format!("{table}__reordered");
        let staging = sql_identifier(&staging)?;
        let columns = carried_columns
            .iter()
            .map(|column| sql_identifier(column))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let tx = self.conn.unchecked_transaction().with_context(|| {
            format!("Failed to start the {table}.{PARAMS_FINGERPRINT_COLUMN} transaction")
        })?;
        // A fossil staging table from a run that was killed mid-rebuild would make the
        // CREATE below fail.
        tx.execute_batch(&format!("DROP TABLE IF EXISTS {staging};"))
            .with_context(|| format!("Failed to clear a stale {staging}"))?;
        tx.execute_batch(reordered_sql)
            .with_context(|| format!("Failed to create {staging}"))?;
        tx.execute_batch(&format!(
            "INSERT INTO {staging} ({columns}) SELECT {columns} FROM {table};
             DROP TABLE {table};
             ALTER TABLE {staging} RENAME TO {table};"
        ))
        .with_context(|| format!("Failed to rebuild {table} with {PARAMS_FINGERPRINT_COLUMN}"))?;
        tx.commit().with_context(|| {
            format!("Failed to commit the {table}.{PARAMS_FINGERPRINT_COLUMN} rebuild")
        })?;
        info!("{table}.{PARAMS_FINGERPRINT_COLUMN} added.");
        Ok(())
    }

    /// Creates `diagram_solid_extents` for a database created before the Rough Planner
    /// cached each design's finished-solid extents -- see
    /// [`Self::save_solid_extents`](Database::save_solid_extents)/
    /// [`Self::solid_extents_for`](Database::solid_extents_for).
    ///
    /// Keyed by `entry_id`, cascading off `diagram_entries`, for the same reason as
    /// `diagram_previews`/`diagram_tilt_curves` -- a routine `diagram_details` re-sync
    /// must not silently discard a measurement that costs a plane solve to redo. A
    /// geometry change is invalidated explicitly by
    /// [`Self::delete_solid_extents`](Database::delete_solid_extents), never implicitly.
    ///
    /// No BLOB column, so no `__reordered` variant and no
    /// [`Self::migrate_blob_columns_last`] entry.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(in crate::db::sqlite) fn migrate_diagram_solid_extents_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DIAGRAM_SOLID_EXTENTS_TABLE_SQL)
            .context("Failed to create diagram_solid_extents table")?;
        Ok(())
    }

    /// Creates `diagram_solid_hull` for a database created before the Rough Planner
    /// cached each design's convex hull vertices -- see
    /// [`Self::save_solid_hull`](Database::save_solid_hull)/
    /// [`Self::solid_hulls_for`](Database::solid_hulls_for).
    ///
    /// Keyed by `entry_id`, cascading off `diagram_entries`, for the same reason as
    /// `diagram_solid_extents` -- a routine `diagram_details` re-sync must not
    /// silently discard a measurement. Invalidation is coupled with extents in
    /// [`Self::delete_solid_extents`](Database::delete_solid_extents).
    ///
    /// The BLOB column `vertices` sits last per house convention.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(in crate::db::sqlite) fn migrate_diagram_solid_hull_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DIAGRAM_SOLID_HULL_TABLE_SQL)
            .context("Failed to create diagram_solid_hull table")?;
        Ok(())
    }

    /// Creates `saved_rough_plans` for a database created before rough plans could be
    /// saved -- see [`Self::list_saved_rough_plans`](Database::list_saved_rough_plans)/
    /// [`Self::get_saved_rough_plan`](Database::get_saved_rough_plan)/
    /// [`Self::save_rough_plan`](Database::save_rough_plan)/
    /// [`Self::rename_saved_rough_plan`](Database::rename_saved_rough_plan)/
    /// [`Self::delete_saved_rough_plan`](Database::delete_saved_rough_plan).
    ///
    /// Also adds the `summary` column (the one-line list description stored at save time)
    /// to a table that predates it. `ADD COLUMN` appends it after `payload`; that is fine
    /// here because a saved plan's payload is small next to a design's image BLOBs and
    /// the table holds a few dozen rows. Every existing row keeps a `NULL` summary, which
    /// readers fill in once from the payload.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS` and a column-presence gate.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table or adding the column fails.
    pub(in crate::db::sqlite) fn migrate_saved_rough_plans_table(&self) -> Result<()> {
        self.conn
            .execute_batch(SAVED_ROUGH_PLANS_TABLE_SQL)
            .context("Failed to create saved_rough_plans table")?;
        if !Self::column_exists(
            &self.conn,
            "saved_rough_plans",
            SAVED_ROUGH_PLANS_SUMMARY_COLUMN,
        )? {
            self.conn
                .execute_batch(&format!(
                    "ALTER TABLE saved_rough_plans ADD COLUMN {SAVED_ROUGH_PLANS_SUMMARY_COLUMN} TEXT;"
                ))
                .context("Failed to add saved_rough_plans.summary column")?;
            info!("saved_rough_plans.{SAVED_ROUGH_PLANS_SUMMARY_COLUMN} added.");
        }
        Ok(())
    }

    /// Creates `diagram_planner_exclusions` for a database created before the Rough
    /// Planner could leave single designs out of its candidate set -- see
    /// [`Self::set_planner_excluded`](Database::set_planner_excluded)/
    /// [`Self::planner_excluded_ids`](Database::planner_excluded_ids)/
    /// [`Self::planner_excluded_among`](Database::planner_excluded_among).
    ///
    /// Keyed by `entry_id`, cascading off `diagram_entries`, for the same reason as
    /// `diagram_solid_extents` -- a routine `diagram_details` re-sync must not silently
    /// clear the mark, and deleting the design leaves no orphaned row. A side table
    /// rather than a `diagram_entries` column, so toggling the mark never rewrites the
    /// entry row or its `updated_at`.
    ///
    /// No BLOB column, so no `__reordered` variant and no
    /// [`Self::migrate_blob_columns_last`] entry. Every pre-existing design starts
    /// unexcluded, which is the only sound reading of a design that predates the table.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(in crate::db::sqlite) fn migrate_planner_exclusion_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DIAGRAM_PLANNER_EXCLUSIONS_TABLE_SQL)
            .context("Failed to create diagram_planner_exclusions table")?;
        Ok(())
    }

    /// Creates `design_variants` (and its `design_uuid` index) for a database created
    /// before a design could keep saved variants -- see
    /// [`Self::save_variant`](Database::save_variant)/
    /// [`Self::list_variants`](Database::list_variants)/
    /// [`Self::load_variant`](Database::load_variant).
    ///
    /// Keyed by the design's UUID, not by `entry_id`, and with no `FOREIGN KEY` to
    /// `diagram_entries`: a design opened from a file that was never catalogued still
    /// has a UUID and must be able to keep variants, and deleting a catalogue entry does
    /// not delete the design file those variants belong to. The only constraint is the
    /// table's own `parent_variant_id ... ON DELETE SET NULL`.
    ///
    /// Adds nothing to an existing table, so every pre-existing design starts with no
    /// variants. Naturally idempotent via `CREATE ... IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table or its index fails.
    pub(in crate::db::sqlite) fn migrate_design_variants_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DESIGN_VARIANTS_TABLE_SQL)
            .context("Failed to create design_variants table")?;
        Ok(())
    }

    /// Creates `design_cut_progress` for a database created before a design could keep
    /// cutting progress -- see [`Self::mark_step_done`](Database::mark_step_done)/
    /// [`Self::cut_progress`](Database::cut_progress).
    ///
    /// Keyed by the design's UUID and the step key, with no `FOREIGN KEY` to
    /// `diagram_entries`, for the reasons given on [`Self::migrate_design_variants_table`].
    /// Every pre-existing design starts with nothing marked done. Naturally idempotent via
    /// `CREATE TABLE IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(in crate::db::sqlite) fn migrate_design_cut_progress_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DESIGN_CUT_PROGRESS_TABLE_SQL)
            .context("Failed to create design_cut_progress table")?;
        Ok(())
    }

    /// Creates `design_lighting` for a database created before a design could keep its
    /// own lighting choice -- see
    /// [`Self::set_design_lighting`](Database::set_design_lighting)/
    /// [`Self::design_lighting`](Database::design_lighting).
    ///
    /// Keyed by the design's UUID, with no `FOREIGN KEY` to `diagram_entries`, for the
    /// reasons given on [`Self::migrate_design_variants_table`]. Every pre-existing
    /// design starts with no stored choice, which readers treat as "use the global
    /// lighting". Naturally idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(in crate::db::sqlite) fn migrate_design_lighting_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DESIGN_LIGHTING_TABLE_SQL)
            .context("Failed to create design_lighting table")?;
        Ok(())
    }

    /// Creates `render_jobs` (and its queue-order index) for a database created before
    /// the desktop app could keep a render queue -- see
    /// [`Self::add_render_job`](Database::add_render_job)/
    /// [`Self::list_render_jobs`](Database::list_render_jobs)/
    /// [`Self::get_render_job`](Database::get_render_job).
    ///
    /// A side table with no `FOREIGN KEY` to `diagram_entries`: a job is a frozen copy of
    /// what to render and must outlive the design it was made from. The snapshot is
    /// opaque JSON text (the vault never parses it, like `design_lighting.settings_json`)
    /// and is declared last, because it is the large column and a list must not walk its
    /// overflow pages.
    ///
    /// Idempotent via `CREATE ... IF NOT EXISTS`. A future column is added behind a
    /// [`Self::column_exists`] gate, the way `saved_rough_plans.summary` is in
    /// [`Self::migrate_saved_rough_plans_table`]. Never touches `PRAGMA user_version`,
    /// which is the library identity stamp, not a schema version.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table or its index fails.
    pub(in crate::db::sqlite) fn migrate_render_jobs_table(&self) -> Result<()> {
        self.conn
            .execute_batch(RENDER_JOBS_TABLE_SQL)
            .context("Failed to create render_jobs table")?;
        Ok(())
    }

    /// Creates `tags`/`diagram_tag_links` for a database created before the catalogue
    /// had a tagging system -- a flat-tag set (deliberately NOT folders/collections;
    /// see [`super::super::search::SortOrder`]'s own doc comment for the sort half this
    /// pairs with).
    ///
    /// A tag is its own row (`tags.name`, unique case-insensitively so "Competition"
    /// and "competition" can't silently become two different tags) rather than a free
    /// column on `diagram_entries`, so many-to-many attachment needs its own join
    /// table (`diagram_tag_links`) -- this mirrors `diagram_previews`/
    /// `diagram_tilt_curves`'s own "side table keyed by/cascading off `entry_id`"
    /// convention rather than growing `diagram_entries`' own column set (which two
    /// tests in this crate's `tests.rs` pin to an exact list). Both `ON DELETE
    /// CASCADE`s mean deleting a design or a tag never leaves an orphaned link row to
    /// clean up by hand.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS`, also created inside
    /// `create_tables_if_not_exist` so a fresh database already has both tables.
    ///
    /// # Errors
    ///
    /// Returns an error if creating either table fails.
    pub(in crate::db::sqlite) fn migrate_tag_tables(&self) -> Result<()> {
        self.conn
            .execute_batch(TAG_TABLES_SQL)
            .context("Failed to create tags/diagram_tag_links tables")?;
        Ok(())
    }

    /// Prunes `diagram_tilt_curves` down from this crate's first-draft 36 derived
    /// aggregate columns (3 metrics x 4 fixed tilt radii x {min, max, mean}) to the 6
    /// that survive (3 metrics x {global min, global max}) -- the tilt radius became
    /// arbitrary rather than a fixed ladder of four, making the other 30 unanswerable.
    /// Only does real work against a database from that specific prior iteration; a
    /// fresh database gets the pruned shape directly from
    /// [`Self::migrate_diagram_tilt_curves_table`]. Gated on the presence of
    /// `perf_brilliance_15_min`, one representative column from the dropped 30.
    ///
    /// Uses `DROP COLUMN` (SQLite 3.35.0+, fine here since every dropped column is a
    /// plain unconstrained nullable `REAL`) rather than leaving them as dead weight, so
    /// `PRAGMA table_info` and this crate's migration tests (which assert an exact
    /// column count) stay honest. The 6 surviving columns are also renamed
    /// (`perf_<metric>_90_min`/`_max` -> `_global_min`/`_max`), since "radius 90" no
    /// longer means anything.
    ///
    /// Runs inside one transaction: every `DROP`/`RENAME` succeeds or none are kept.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the marker column, starting/committing the
    /// transaction, any individual `DROP COLUMN`/`RENAME COLUMN` fails, or (should
    /// this list or the generated names ever stop being hard-coded literals) a
    /// column name fails [`sql_identifier`].
    pub(in crate::db::sqlite) fn migrate_prune_tilt_curve_aggregate_columns(&self) -> Result<()> {
        if !Self::column_exists(&self.conn, "diagram_tilt_curves", "perf_brilliance_15_min")? {
            debug!(
                "Tilt-curve aggregate column pruning already applied (or never needed); skipping."
            );
            return Ok(());
        }

        info!(
            "Pruning diagram_tilt_curves' first-draft per-radius aggregate columns down to \
             global min/max..."
        );
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start tilt-curve aggregate pruning transaction")?;

        for column in OBSOLETE_TILT_CURVE_AGGREGATE_COLUMNS {
            let column = sql_identifier(column)?;
            tx.execute_batch(&format!(
                "ALTER TABLE diagram_tilt_curves DROP COLUMN {column};"
            ))
            .with_context(|| format!("Failed to drop obsolete column {column}"))?;
        }
        for metric in PerformanceMetric::ALL {
            for extreme in Extreme::ALL {
                let old = format!("perf_{}_90_{}", metric.column_key(), extreme.column_key());
                let new = global_extreme_column_name(metric, extreme);
                let old = sql_identifier(&old)?;
                let new = sql_identifier(&new)?;
                tx.execute_batch(&format!(
                    "ALTER TABLE diagram_tilt_curves RENAME COLUMN {old} TO {new};"
                ))
                .with_context(|| format!("Failed to rename column {old} to {new}"))?;
            }
        }

        tx.commit()
            .context("Failed to commit tilt-curve aggregate pruning transaction")?;
        info!("Tilt-curve aggregate column pruning complete.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::entry::FacetingDiagramEntry;

    /// A database file whose `diagram_previews`/`diagram_tilt_curves` are in the shape
    /// they had before `params_fingerprint` existed (BLOBs already last), holding one
    /// row each for entry 1, then closed so the next `Database::new` runs the migration.
    fn seed_pre_fingerprint_db(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-fingerprint-migration-{}-{label}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        let entry_id = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: "Legacy".to_string(),
                    url: "local://legacy.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        assert_eq!(entry_id, 1);
        db.conn
            .execute_batch(
                "DROP TABLE diagram_previews;
                 CREATE TABLE diagram_previews (
                     entry_id INTEGER PRIMARY KEY,
                     preview_material TEXT,
                     preview_generated_at INTEGER,
                     preview_front BLOB,
                     preview_top BLOB,
                     FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
                 );
                 INSERT INTO diagram_previews
                     (entry_id, preview_material, preview_generated_at, preview_front, preview_top)
                 VALUES (1, 'Quartz', 1700000000, X'0102', X'0304');
                 DROP TABLE diagram_tilt_curves;
                 CREATE TABLE diagram_tilt_curves (
                     entry_id INTEGER PRIMARY KEY,
                     generated_at INTEGER,
                     perf_brilliance_global_min REAL,
                     perf_brilliance_global_max REAL,
                     perf_extinction_global_min REAL,
                     perf_extinction_global_max REAL,
                     perf_windowing_global_min REAL,
                     perf_windowing_global_max REAL,
                     curves BLOB,
                     curve_image BLOB,
                     FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
                 );
                 INSERT INTO diagram_tilt_curves
                     (entry_id, generated_at, perf_brilliance_global_min,
                      perf_brilliance_global_max, perf_extinction_global_min,
                      perf_extinction_global_max, perf_windowing_global_min,
                      perf_windowing_global_max, curves, curve_image)
                 VALUES (1, 1700000001, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, X'0506', X'0708');",
            )
            .unwrap();
        drop(db);
        path
    }

    fn position(db: &Database, table: &str, column: &str) -> i64 {
        Database::column_position(&db.conn, table, column)
            .unwrap()
            .unwrap_or_else(|| panic!("{table}.{column} must exist"))
    }

    #[test]
    fn a_pre_fingerprint_database_gains_the_column_ahead_of_the_blobs_and_keeps_every_row() {
        let path = seed_pre_fingerprint_db("gains_column");
        let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");

        assert!(
            position(&db, "diagram_previews", "params_fingerprint")
                < position(&db, "diagram_previews", "preview_front")
        );
        assert!(
            position(&db, "diagram_tilt_curves", "params_fingerprint")
                < position(&db, "diagram_tilt_curves", "curves")
        );

        // Every byte of the old rows survived, with no fingerprint.
        let preview = db.get_preview_images(1).unwrap();
        assert_eq!(preview.front, Some(vec![1, 2]));
        assert_eq!(preview.top, Some(vec![3, 4]));
        assert_eq!(preview.material.as_deref(), Some("Quartz"));
        assert_eq!(preview.generated_at, Some(1_700_000_000));
        let (curves, image, global_min): (Vec<u8>, Vec<u8>, f64) = db
            .conn
            .query_row(
                "SELECT curves, curve_image, perf_brilliance_global_min
                 FROM diagram_tilt_curves WHERE entry_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(curves, vec![5, 6]);
        assert_eq!(image, vec![7, 8]);
        assert!((global_min - 1.0).abs() < 1e-12);
        for table in ["diagram_previews", "diagram_tilt_curves"] {
            let fingerprint: Option<String> = db
                .conn
                .query_row(
                    &format!("SELECT params_fingerprint FROM {table} WHERE entry_id = 1"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(fingerprint, None, "{table}");
        }

        // A row with no fingerprint is outdated, so it is listed for regeneration.
        assert_eq!(
            db.entry_ids_missing_previews(|_| "current".to_string())
                .unwrap(),
            vec![1]
        );
        assert_eq!(
            db.entry_ids_missing_tilt_curves(|_| "current".to_string())
                .unwrap(),
            vec![1]
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn adding_the_fingerprint_column_is_idempotent() {
        let path = seed_pre_fingerprint_db("idempotent");
        drop(Database::new(Some(path.to_str().unwrap())).expect("first open migrates"));
        let db = Database::new(Some(path.to_str().unwrap())).expect("second open is a no-op");

        for table in ["diagram_previews", "diagram_tilt_curves"] {
            assert!(
                Database::column_exists(&db.conn, table, "params_fingerprint").unwrap(),
                "{table}"
            );
        }
        assert_eq!(db.get_preview_images(1).unwrap().front, Some(vec![1, 2]));

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_fresh_database_has_the_column_before_the_blobs() {
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-fingerprint-fresh-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();

        assert!(
            position(&db, "diagram_previews", "params_fingerprint")
                < position(&db, "diagram_previews", "preview_front")
        );
        assert!(
            position(&db, "diagram_tilt_curves", "params_fingerprint")
                < position(&db, "diagram_tilt_curves", "curves")
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }
}
