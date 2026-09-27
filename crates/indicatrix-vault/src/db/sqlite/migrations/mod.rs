//! Schema creation and every idempotent migration [`super::Database::new`] runs, in
//! order, against an existing database file. Split into two sibling modules by concern:
//!
//! - [`helpers`]: the generic TEXT-to-numeric column retype sequence and the SQL
//!   identifier safety check every dynamic-DDL migration here relies on.
//! - [`schema`]: the raw `CREATE TABLE`/`CREATE INDEX` SQL text shared verbatim between
//!   a migration and `create_tables_if_not_exist`.
//!
//! Every migration method itself stays here, in declaration/run order, so the full
//! migration history can be read start to finish in one scroll.

use super::{DEFAULT_SHAPES, Database, LEGACY_SOURCE_ID};
use crate::model::performance::{Extreme, PerformanceMetric, global_extreme_column_name};
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use tracing::{debug, info};

mod helpers;
mod schema;

pub(super) use helpers::sql_identifier;
use helpers::{retype_text_column_to_numeric, split_facets_count_column};
use schema::OBSOLETE_TILT_CURVE_AGGREGATE_COLUMNS;
pub(super) use schema::{
    DIAGRAM_PREVIEWS_TABLE_SQL, SEARCH_INDEXES_SQL, TAG_TABLES_SQL, diagram_tilt_curves_table_sql,
};

impl Database {
    /// Retypes `diagram_details`'s numeric-but-stored-as-TEXT columns
    /// (`refractive_index`, `lw_ratio`, `volume` -> REAL; `index_gear` -> INTEGER) and
    /// splits `facets_count` (e.g. `"55+6"`) into new `facets`/`girdle_facets` INTEGER
    /// columns, leaving `facets_count` in place for display.
    ///
    /// Idempotent: gated on `refractive_index`'s ACTUAL column type, read via `PRAGMA
    /// table_info` ([`Self::column_sql_type`]), not on whether `facets` exists. A fresh
    /// database's `diagram_details` (see `create_tables_if_not_exist`) already declares
    /// `refractive_index`/`lw_ratio`/`volume`/`index_gear` as REAL/INTEGER and already
    /// has `facets`/`girdle_facets` -- gating on column presence alone would still run
    /// the full DROP-COLUMN/RENAME-COLUMN retype cycle (`retype_text_column_to_numeric`)
    /// against columns that were never TEXT to begin with, on every single fresh
    /// install. Gating on the type itself makes that a no-op read instead, while an old
    /// database whose columns are genuinely still TEXT is unaffected and still migrates.
    /// Runs in one transaction, rolling back atomically on failure.
    ///
    /// # Errors
    ///
    /// Returns an error if checking `refractive_index`'s column type, or any migration
    /// step, fails.
    pub(super) fn migrate_numeric_columns(&self) -> Result<()> {
        if Self::column_sql_type(&self.conn, "diagram_details", "refractive_index")?
            .is_some_and(|sql_type| !sql_type.eq_ignore_ascii_case("text"))
        {
            debug!(
                "Numeric column migration already applied (refractive_index is already typed); \
                 skipping."
            );
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

    /// Creates the library search predicate's supporting indexes on an existing
    /// database -- see [`SEARCH_INDEXES_SQL`] for which, and for the measurements that
    /// motivated them.
    ///
    /// `CREATE INDEX IF NOT EXISTS` is naturally idempotent, so this always runs rather
    /// than checking first, the same way [`Database::migrate_shape_vocabulary`] does.
    /// Must run AFTER [`Database::migrate_tag_tables`], since `diagram_tag_links` may
    /// not exist yet on a database old enough to predate it.
    ///
    /// # Errors
    ///
    /// Returns an error if creating either index fails.
    pub(super) fn migrate_search_indexes(&self) -> Result<()> {
        self.conn
            .execute_batch(SEARCH_INDEXES_SQL)
            .context("Failed to create the library search indexes")?;
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

    /// Creates `tags`/`diagram_tag_links` for a database created before the catalogue
    /// had a tagging system -- a flat-tag set (deliberately NOT folders/collections;
    /// see [`super::search::SortOrder`]'s own doc comment for the sort half this
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
    pub(super) fn migrate_tag_tables(&self) -> Result<()> {
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

    /// Adds `custom_gem_materials.specific_gravity`, a nullable REAL column holding
    /// the material's density relative to water, so a custom material can carry its
    /// own SG the way the thirteen built-in species already do: without it, Est.
    /// Carat Weight stays empty for a custom material unless the cutter separately
    /// types an SG override for every design.
    ///
    /// Purely additive and nullable, same idiom as
    /// [`Self::migrate_per_axis_dispersion_column`]: `NULL` means "no SG recorded"
    /// for every pre-existing row, not a guessed value.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding it fails.
    pub(super) fn migrate_custom_material_specific_gravity(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "custom_gem_materials", "specific_gravity")? {
            debug!("specific_gravity column already present; skipping the ADD COLUMN step.");
            return Ok(());
        }

        info!("Adding custom_gem_materials.specific_gravity column...");
        self.conn
            .execute_batch("ALTER TABLE custom_gem_materials ADD COLUMN specific_gravity REAL;")
            .context("Failed to add custom_gem_materials.specific_gravity column")?;
        info!("specific_gravity column migration complete.");
        Ok(())
    }

    /// Adds `diagram_entries.created_at`/`updated_at` (both nullable `INTEGER` Unix
    /// seconds) for a database created before this crate recorded when a design was
    /// added or last changed -- see
    /// [`Self::migrate_diagram_entries_provenance`] for the column it's paired with.
    ///
    /// Both `NULL`, not backfilled: a pre-existing row's real creation/edit time is
    /// simply unknown, and a fabricated "now" would be a lie a "recently edited" sort
    /// could act on. [`crate::db::sqlite::SortOrder::Newest`]/
    /// [`crate::db::sqlite::SortOrder::RecentlyEdited`] already sort `NULL` last for
    /// exactly this reason (SQLite's own `DESC` ordering, no extra `CASE` needed).
    ///
    /// Idempotent: gated on whether `created_at` already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding either fails.
    pub(super) fn migrate_diagram_entries_timestamps(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_entries", "created_at")? {
            debug!("diagram_entries timestamp columns already present; skipping.");
            return Ok(());
        }

        info!("Adding diagram_entries.created_at/updated_at columns...");
        self.conn
            .execute_batch(
                "ALTER TABLE diagram_entries ADD COLUMN created_at INTEGER;
                 ALTER TABLE diagram_entries ADD COLUMN updated_at INTEGER;",
            )
            .context("Failed to add diagram_entries timestamp columns")?;
        info!("diagram_entries timestamp column migration complete.");
        Ok(())
    }

    /// Adds `diagram_entries.derived_from_entry_id` (nullable `INTEGER`, no
    /// `FOREIGN KEY`) for a database created before this crate recorded provenance
    /// between rows: without it, an export-then-reimport of an existing catalogue
    /// design lands as an indistinguishable second row, since a different `url`
    /// means `INSERT`, not `UPDATE` (`Database::save_diagram_entry`).
    ///
    /// This migration only adds the column and leaves every row's value `NULL`; it
    /// does not attempt to backfill provenance for existing rows by guessing from
    /// titles or any other heuristic (deliberately -- see this column's callers).
    /// No `FOREIGN KEY`: the row this points at can be renamed, re-synced, or deleted
    /// independently without this column blocking or cascading that operation.
    ///
    /// Idempotent: gated on whether `derived_from_entry_id` already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding it fails.
    pub(super) fn migrate_diagram_entries_provenance(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_entries", "derived_from_entry_id")? {
            debug!("diagram_entries.derived_from_entry_id already present; skipping.");
            return Ok(());
        }

        info!("Adding diagram_entries.derived_from_entry_id column...");
        self.conn
            .execute_batch("ALTER TABLE diagram_entries ADD COLUMN derived_from_entry_id INTEGER;")
            .context("Failed to add diagram_entries.derived_from_entry_id column")?;
        info!("diagram_entries.derived_from_entry_id column migration complete.");
        Ok(())
    }

    /// Whether `table` currently has a column named `column`, via `PRAGMA table_info`.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` is not a valid SQL identifier (see
    /// [`sql_identifier`]) or the `PRAGMA table_info` query fails.
    pub(super) fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        Ok(Self::column_sql_type(conn, table, column)?.is_some())
    }

    /// `table.column`'s declared SQL type (e.g. `"TEXT"`, `"REAL"`, `"INTEGER"`) via
    /// `PRAGMA table_info`, or `None` if `table` has no such column. Lets a migration
    /// gate itself on what a column actually IS rather than merely whether it exists
    /// -- see [`Self::migrate_numeric_columns`] for why that distinction matters: a
    /// fresh database can already have every column a migration would otherwise add or
    /// retype, already in its final shape, and presence alone can't tell those two
    /// cases apart.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` is not a valid SQL identifier (see
    /// [`sql_identifier`]) or the `PRAGMA table_info` query fails.
    pub(super) fn column_sql_type(
        conn: &Connection,
        table: &str,
        column: &str,
    ) -> Result<Option<String>> {
        let table = sql_identifier(table)?;
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get("name")?;
            if name == column {
                return Ok(Some(row.get("type")?));
            }
        }
        Ok(None)
    }
}
