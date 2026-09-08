use super::{DEFAULT_SHAPES, Database, LEGACY_SOURCE_ID};
use crate::model::{
    facets::parse_facets_count,
    performance::{Extreme, PerformanceMetric, global_extreme_column_name},
};
use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, params};
use std::fmt::Write as _;
use tracing::{debug, info};

impl Database {
    /// Retypes `diagram_details`'s numeric-but-stored-as-TEXT columns
    /// (`refractive_index`, `lw_ratio`, `volume` -> REAL; `index_gear` -> INTEGER) and
    /// splits `facets_count` (e.g. `"55+6"`) into new `facets`/`girdle_facets` INTEGER
    /// columns, leaving `facets_count` in place for display.
    ///
    /// Idempotent: gated on whether `facets` already exists. Runs in one transaction,
    /// rolling back atomically on failure.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the `facets` column, or any step fails.
    pub(super) fn migrate_numeric_columns(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_details", "facets")? {
            debug!("Numeric column migration already applied; skipping.");
            return Ok(());
        }

        info!("Migrating diagram_details TEXT columns to numeric types...");
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start numeric-column migration transaction")?;

        retype_text_column_to_numeric(&tx, "refractive_index", "REAL")?;
        retype_text_column_to_numeric(&tx, "lw_ratio", "REAL")?;
        retype_text_column_to_numeric(&tx, "volume", "REAL")?;
        retype_text_column_to_numeric(&tx, "index_gear", "INTEGER")?;

        tx.execute_batch(
            "ALTER TABLE diagram_details ADD COLUMN facets INTEGER;
             ALTER TABLE diagram_details ADD COLUMN girdle_facets INTEGER;",
        )
        .context("Failed to add facets/girdle_facets columns")?;
        split_facets_count_column(&tx)?;

        tx.commit()
            .context("Failed to commit numeric-column migration")?;
        info!("Numeric column migration complete.");
        Ok(())
    }

    /// Adds `diagram_entries.source_id` (`TEXT NOT NULL DEFAULT` [`LEGACY_SOURCE_ID`])
    /// for a database created before multi-source support existed, so pre-existing rows
    /// don't read back as unattributed.
    ///
    /// Idempotent: gated on whether the column already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column, or adding it, fails.
    pub(super) fn migrate_source_id_column(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_entries", "source_id")? {
            debug!("source_id column migration already applied; skipping.");
            return Ok(());
        }

        info!("Adding diagram_entries.source_id column...");
        self.conn
            .execute_batch(&format!(
                "ALTER TABLE diagram_entries ADD COLUMN source_id TEXT NOT NULL DEFAULT '{LEGACY_SOURCE_ID}';"
            ))
            .context("Failed to add diagram_entries.source_id column")?;
        info!("source_id column migration complete.");
        Ok(())
    }

    /// Adds `diagram_details`' proportion-ratio and symmetry columns (`hw_ratio`,
    /// `tw_ratio`, `uw_ratio`, `pw_ratio`, `cw_ratio`, `symmetry_order`,
    /// `mirror_symmetry`) for a database created before this crate captured them.
    ///
    /// All nullable and brand-new. Idempotent: gated on whether `hw_ratio` (chosen
    /// arbitrarily) already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column, or adding any of the seven, fails.
    pub(super) fn migrate_proportions_columns(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_details", "hw_ratio")? {
            debug!("Proportions/symmetry column migration already applied; skipping.");
            return Ok(());
        }

        info!("Adding diagram_details proportion-ratio and symmetry columns...");
        self.conn
            .execute_batch(
                "ALTER TABLE diagram_details ADD COLUMN hw_ratio REAL;
                 ALTER TABLE diagram_details ADD COLUMN tw_ratio REAL;
                 ALTER TABLE diagram_details ADD COLUMN uw_ratio REAL;
                 ALTER TABLE diagram_details ADD COLUMN pw_ratio REAL;
                 ALTER TABLE diagram_details ADD COLUMN cw_ratio REAL;
                 ALTER TABLE diagram_details ADD COLUMN symmetry_order INTEGER;
                 ALTER TABLE diagram_details ADD COLUMN mirror_symmetry BOOLEAN;",
            )
            .context("Failed to add proportion-ratio/symmetry columns")?;
        info!("Proportions/symmetry column migration complete.");
        Ok(())
    }

    /// Adds `diagram_details`' split designer/citation columns (`designer`,
    /// `source_citation`) and the competition-entry columns (`pdf_file`, `gem_file`,
    /// `shape_category`) for a database created before this crate captured them, plus
    /// an index for "every design by X" instead of a `LIKE '%X%'` scan.
    ///
    /// `designer_info` is deliberately left in place (`FacetDiagramDetail::designer`
    /// still reads it); nothing backfills the new columns -- the parser populates them
    /// on next sync.
    ///
    /// All five nullable and brand-new, gated on whether `designer` already exists. The
    /// index sits outside that gate; see the comment on it for why.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column, adding any of the five, or
    /// creating the index fails.
    pub(super) fn migrate_designer_and_attachment_columns(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_details", "designer")? {
            debug!("Designer/attachment columns already present; skipping the ADD COLUMN step.");
        } else {
            info!("Adding diagram_details designer-split and competition-entry columns...");
            self.conn
                .execute_batch(
                    "ALTER TABLE diagram_details ADD COLUMN designer TEXT;
                     ALTER TABLE diagram_details ADD COLUMN source_citation TEXT;
                     ALTER TABLE diagram_details ADD COLUMN pdf_file TEXT;
                     ALTER TABLE diagram_details ADD COLUMN gem_file TEXT;
                     ALTER TABLE diagram_details ADD COLUMN shape_category INTEGER;",
                )
                .context("Failed to add designer-split/competition-entry columns")?;
            info!("Designer/attachment column migration complete.");
        }

        // Outside the gate above: a fresh database skips the ADD COLUMN branch, and a
        // pre-migration database doesn't have `designer` yet when
        // `create_tables_if_not_exist` runs. Here is the one place both cases have the
        // column. `IF NOT EXISTS` makes repeat opens a no-op.
        self.conn
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_diagram_details_designer
                 ON diagram_details (designer);",
            )
            .context("Failed to create the diagram_details.designer index")?;
        Ok(())
    }

    /// Adds `custom_gem_materials`' crystal-classification columns for the
    /// custom-material editor: `crystal_system`, `optical_character`, both TEXT (a
    /// `indicatrix` enum variant name, e.g. `"Trigonal"`), and
    /// `biaxial_delta_beta_alpha` REAL. See `CustomMaterialRow`'s field docs for why
    /// these stay plain text/`f32` rather than the `indicatrix` enums themselves.
    ///
    /// All three nullable and brand-new, gated on whether `crystal_system` already
    /// exists. `NULL` on every pre-existing row already means "not stored, infer as
    /// `GemMaterial::new_custom` does", so no backfill is needed.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the `crystal_system` column or adding any of
    /// the three fails.
    pub(super) fn migrate_crystal_optics_columns(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "custom_gem_materials", "crystal_system")? {
            debug!("Crystal-optics columns already present; skipping the ADD COLUMN step.");
            return Ok(());
        }

        info!("Adding custom_gem_materials crystal-classification columns...");
        self.conn
            .execute_batch(
                "ALTER TABLE custom_gem_materials ADD COLUMN crystal_system TEXT;
                 ALTER TABLE custom_gem_materials ADD COLUMN optical_character TEXT;
                 ALTER TABLE custom_gem_materials ADD COLUMN biaxial_delta_beta_alpha REAL;",
            )
            .context("Failed to add custom_gem_materials crystal-classification columns")?;
        info!("Crystal-optics column migration complete.");
        Ok(())
    }

    /// Creates `shape_vocabulary` for a database created before it existed, and seeds
    /// (or re-seeds) it from [`DEFAULT_SHAPES`] -- fixes `get_unique_shapes` returning
    /// nothing on a fresh database (no imported design has a `shape` value yet).
    ///
    /// `CREATE TABLE IF NOT EXISTS` and `INSERT OR IGNORE` (keyed on `name`) are each
    /// naturally idempotent, so this always runs both rather than checking first: a
    /// second run always re-seeds harmlessly, since `OR IGNORE` never overwrites an
    /// existing row (a hand-edit survives).
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table or inserting any of
    /// [`DEFAULT_SHAPES`]'s entries fails.
    pub(super) fn migrate_shape_vocabulary(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS shape_vocabulary (
                     name TEXT PRIMARY KEY,
                     sort_order INTEGER NOT NULL
                 );",
            )
            .context("Failed to create shape_vocabulary table")?;

        let mut stmt = self
            .conn
            .prepare("INSERT OR IGNORE INTO shape_vocabulary (name, sort_order) VALUES (?1, ?2)")
            .context("Failed to prepare shape_vocabulary seed insert")?;
        for (order, shape) in DEFAULT_SHAPES.iter().enumerate() {
            stmt.execute(params![shape, order as i64])
                .with_context(|| format!("Failed to seed shape_vocabulary entry '{shape}'"))?;
        }
        Ok(())
    }

    /// Adds `diagram_entries.ignored` (`BOOLEAN NOT NULL DEFAULT 0`) for a database
    /// created before the "ignored" library feature existed -- see
    /// [`Self::set_diagram_ignored`](Database::set_diagram_ignored) and
    /// `crate::model::filter::RangeFilter::include_ignored`.
    ///
    /// `NOT NULL DEFAULT 0` is safe here (unlike this file's nullable migrations):
    /// SQLite backfills every pre-existing row, and "not ignored" is the only sound
    /// interpretation for a design that predates this column.
    ///
    /// Idempotent: gated on whether `ignored` already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding it fails.
    pub(super) fn migrate_ignored_column(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_entries", "ignored")? {
            debug!("ignored column migration already applied; skipping.");
            return Ok(());
        }

        info!("Adding diagram_entries.ignored column...");
        self.conn
            .execute_batch(
                "ALTER TABLE diagram_entries ADD COLUMN ignored BOOLEAN NOT NULL DEFAULT 0;",
            )
            .context("Failed to add diagram_entries.ignored column")?;
        info!("ignored column migration complete.");
        Ok(())
    }

    /// Creates `diagram_previews` for a database created before cached preview
    /// rendering existed -- see [`Self::save_preview_images`](Database::save_preview_images)/
    /// [`Self::get_preview_images`](Database::get_preview_images)/
    /// [`Self::ensure_preview_material`](Database::ensure_preview_material).
    ///
    /// Keyed by `entry_id`, cascading off `diagram_entries` (not `diagram_details`,
    /// which [`Database::save_diagram_detail`] deletes and reinserts with a new row id
    /// on every re-sync): a routine metadata re-sync can never silently discard a
    /// cached render this way -- only deleting the design itself does.
    ///
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS`, also created inside
    /// `create_tables_if_not_exist` so a fresh database already has it.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(super) fn migrate_diagram_previews_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DIAGRAM_PREVIEWS_TABLE_SQL)
            .context("Failed to create diagram_previews table")?;
        Ok(())
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
    /// Naturally idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(super) fn migrate_diagram_tilt_curves_table(&self) -> Result<()> {
        self.conn
            .execute_batch(&diagram_tilt_curves_table_sql())
            .context("Failed to create diagram_tilt_curves table")?;
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
    pub(super) fn migrate_prune_tilt_curve_aggregate_columns(&self) -> Result<()> {
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

    /// Adds `custom_gem_materials.per_axis_dispersion_json`, a nullable TEXT column
    /// holding per-axis dispersion coefficients as JSON, for a custom material
    /// authored with `indicatrix`'s optional per-axis dispersion rather than the
    /// single scalar `refractive_index`/`birefringence` REAL columns this table has
    /// always had. JSON, not several new REAL columns, since the per-axis shape varies
    /// (uniaxial vs. biaxial) and is a `indicatrix`-defined enum this crate must not
    /// depend on.
    ///
    /// Purely additive and nullable, same idiom as
    /// [`Self::migrate_crystal_optics_columns`]: `NULL` already means "no per-axis data
    /// stored" for every pre-existing row.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding it fails.
    pub(super) fn migrate_per_axis_dispersion_column(&self) -> Result<()> {
        if Self::column_exists(
            &self.conn,
            "custom_gem_materials",
            "per_axis_dispersion_json",
        )? {
            debug!(
                "per_axis_dispersion_json column already present; skipping the ADD COLUMN step."
            );
            return Ok(());
        }

        info!("Adding custom_gem_materials.per_axis_dispersion_json column...");
        self.conn
            .execute_batch(
                "ALTER TABLE custom_gem_materials ADD COLUMN per_axis_dispersion_json TEXT;",
            )
            .context("Failed to add custom_gem_materials.per_axis_dispersion_json column")?;
        info!("per_axis_dispersion_json column migration complete.");
        Ok(())
    }

    /// Whether `table` currently has a column named `column`, via `PRAGMA table_info`.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` is not a valid SQL identifier (see
    /// [`sql_identifier`]) or the `PRAGMA table_info` query fails.
    pub(super) fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        let table = sql_identifier(table)?;
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get("name")?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Validates `name` as safe to interpolate directly into DDL/`PRAGMA` text.
///
/// SQLite's prepared-statement placeholders (`?1`, `params![...]`) can only
/// bind *values*, never identifiers, so every table/column name this module
/// formats into `ALTER TABLE`/`CREATE INDEX`/`PRAGMA table_info` text has to
/// be interpolated as a string -- the same mechanism a real SQL-injection
/// bug would use. Every identifier this module currently formats is a
/// hard-coded literal (or built from one, like
/// [`retype_text_column_to_numeric`]'s `{column}__migrated`), so nothing can
/// actually be injected today; this guard exists so a future refactor that
/// makes any of them dynamic (a caller-supplied column name, say) cannot
/// silently reopen that door.
///
/// Accepts only `^[A-Za-z_][A-Za-z0-9_]*$`: an ASCII letter or underscore,
/// then any run of ASCII letters/digits/underscores. Checked with a plain
/// char loop rather than a `regex` crate dependency -- the pattern is simple
/// enough not to need one.
///
/// # Errors
///
/// Returns an error naming `name` if it is empty or contains any character
/// outside that pattern.
pub(super) fn sql_identifier(name: &str) -> Result<&str> {
    let mut chars = name.chars();
    let starts_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if starts_ok && rest_ok {
        Ok(name)
    } else {
        anyhow::bail!("'{name}' is not a valid SQL identifier")
    }
}

/// Retypes `diagram_details.{column}` from TEXT to `sql_type` (`"REAL"` or
/// `"INTEGER"`) in place, via SQLite's standard "add the replacement, populate it, drop
/// the original, rename the replacement" sequence (SQLite has no `ALTER COLUMN ...
/// TYPE`). Non-numeric-looking values become `NULL` rather than `CAST`'s silent `0.0`,
/// which would otherwise fabricate data for empty rows.
///
/// # Errors
///
/// Returns an error if `column` or `sql_type` is not a valid SQL identifier
/// (see [`sql_identifier`]), or if adding the replacement column, populating
/// it, or dropping/renaming the original fails.
fn retype_text_column_to_numeric(tx: &Transaction<'_>, column: &str, sql_type: &str) -> Result<()> {
    let column = sql_identifier(column)?;
    let sql_type = sql_identifier(sql_type)?;
    let staging = format!("{column}__migrated");
    let staging = sql_identifier(&staging)?;
    tx.execute_batch(&format!(
        "ALTER TABLE diagram_details ADD COLUMN {staging} {sql_type};"
    ))
    .with_context(|| format!("Failed to add staging column for '{column}'"))?;

    tx.execute(
        &format!(
            "UPDATE diagram_details
             SET {staging} = CASE
                 WHEN {column} IS NULL OR TRIM({column}) = '' THEN NULL
                 ELSE CAST({column} AS {sql_type})
             END"
        ),
        [],
    )
    .with_context(|| format!("Failed to populate staging column for '{column}'"))?;

    tx.execute_batch(&format!(
        "ALTER TABLE diagram_details DROP COLUMN {column};
         ALTER TABLE diagram_details RENAME COLUMN {staging} TO {column};"
    ))
    .with_context(|| format!("Failed to swap staging column into place for '{column}'"))?;

    Ok(())
}

/// Populates the new `facets`/`girdle_facets` INTEGER columns from the existing
/// `facets_count` TEXT column (e.g. `"55+6"` -> `facets = 55, girdle_facets = 6`), via
/// [`parse_facets_count`]; `facets_count` itself is left untouched as the display
/// value. Done row-by-row in Rust, not SQL string functions, since the real data has
/// more shapes than `"N+M"` and `parse_facets_count` already handles all of them.
///
/// # Errors
///
/// Returns an error if reading `(id, facets_count)` rows or writing any
/// `facets`/`girdle_facets` update fails.
fn split_facets_count_column(tx: &Transaction<'_>) -> Result<()> {
    let rows: Vec<(i64, Option<String>)> = {
        let mut select_stmt = tx
            .prepare("SELECT id, facets_count FROM diagram_details")
            .context("Failed to prepare facets_count read for splitting")?;
        select_stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .context("Failed to run facets_count read for splitting")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to decode a row while reading facets_count for splitting")?
    };

    let mut update_stmt = tx
        .prepare("UPDATE diagram_details SET facets = ?1, girdle_facets = ?2 WHERE id = ?3")
        .context("Failed to prepare facets/girdle_facets update")?;
    for (id, raw) in rows {
        let (facets, girdle_facets) = parse_facets_count(raw.as_deref());
        update_stmt
            .execute(params![facets, girdle_facets, id])
            .with_context(|| format!("Failed to write facets/girdle_facets for id {id}"))?;
    }
    Ok(())
}

/// `diagram_previews`' full `CREATE TABLE IF NOT EXISTS` text, shared verbatim between
/// [`Database::migrate_diagram_previews_table`] and `create_tables_if_not_exist` so the
/// two can never define this table differently.
pub(super) const DIAGRAM_PREVIEWS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_previews (
        entry_id INTEGER PRIMARY KEY,
        preview_front BLOB,
        preview_top BLOB,
        preview_material TEXT,
        preview_generated_at INTEGER,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// Builds `diagram_tilt_curves`' full `CREATE TABLE IF NOT EXISTS` text, including its
/// 6 derived global-extreme columns generated from
/// [`crate::model::performance::all_global_extreme_columns`] (not hand-listed), shared
/// verbatim between the migration path and `create_tables_if_not_exist`. A function,
/// not a `const` like [`DIAGRAM_PREVIEWS_TABLE_SQL`], since the derived-column tail
/// needs iterating `all_global_extreme_columns` at runtime.
pub(super) fn diagram_tilt_curves_table_sql() -> String {
    let mut sql = String::from(
        "CREATE TABLE IF NOT EXISTS diagram_tilt_curves (
    entry_id INTEGER PRIMARY KEY,
    curves BLOB,
    curve_image BLOB,
    generated_at INTEGER,\n",
    );
    for (metric, extreme) in crate::model::performance::all_global_extreme_columns() {
        let _ = writeln!(
            sql,
            "    {} REAL,",
            global_extreme_column_name(metric, extreme)
        );
    }
    sql.push_str(
        "    FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE\n);",
    );
    sql
}

/// The 30 `diagram_tilt_curves` columns this crate's first-draft 36-column schema
/// created that do not survive into the current 6-column shape -- see
/// [`Database::migrate_prune_tilt_curve_aggregate_columns`]. A frozen literal list: the
/// enum/functions that would generate these names no longer exist in this crate.
const OBSOLETE_TILT_CURVE_AGGREGATE_COLUMNS: &[&str] = &[
    "perf_brilliance_15_min",
    "perf_brilliance_15_max",
    "perf_brilliance_15_mean",
    "perf_extinction_15_min",
    "perf_extinction_15_max",
    "perf_extinction_15_mean",
    "perf_windowing_15_min",
    "perf_windowing_15_max",
    "perf_windowing_15_mean",
    "perf_brilliance_30_min",
    "perf_brilliance_30_max",
    "perf_brilliance_30_mean",
    "perf_extinction_30_min",
    "perf_extinction_30_max",
    "perf_extinction_30_mean",
    "perf_windowing_30_min",
    "perf_windowing_30_max",
    "perf_windowing_30_mean",
    "perf_brilliance_45_min",
    "perf_brilliance_45_max",
    "perf_brilliance_45_mean",
    "perf_extinction_45_min",
    "perf_extinction_45_max",
    "perf_extinction_45_mean",
    "perf_windowing_45_min",
    "perf_windowing_45_max",
    "perf_windowing_45_mean",
    "perf_brilliance_90_mean",
    "perf_extinction_90_mean",
    "perf_windowing_90_mean",
];
