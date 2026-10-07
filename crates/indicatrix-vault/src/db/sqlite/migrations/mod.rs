//! Schema creation and every idempotent migration [`super::Database::new`] runs, in
//! order, against an existing database file. Split into four sibling modules by concern:
//!
//! - [`blob_order`]: the migration that rebuilds the blob-heavy tables with their BLOB
//!   columns last, and the `PRAGMA table_info` column-position probe it uses.
//! - [`helpers`]: the generic TEXT-to-numeric column retype sequence, the SQL
//!   identifier safety check every dynamic-DDL migration here relies on, and the
//!   `PRAGMA table_info` column probes.
//! - [`schema`]: the raw `CREATE TABLE`/`CREATE INDEX` SQL text shared verbatim between
//!   a migration and `create_tables_if_not_exist`.
//! - [`tables`]: the migrations that create a whole side table (previews, tilt curves,
//!   solid extents and hull, saved plans, planner exclusions, per-design variants,
//!   cutting progress and lighting, render jobs, tags) and the tilt-curve prune.
//!
//! The column-level migrations stay here, in declaration/run order.

use super::{DEFAULT_SHAPES, Database, LEGACY_SOURCE_ID};
use anyhow::{Context, Result};
use rusqlite::params;
use tracing::{debug, info};

mod blob_order;
mod helpers;
mod schema;
mod tables;

pub(super) use helpers::sql_identifier;
use helpers::{retype_text_column_to_numeric, split_facets_count_column};
pub(super) use schema::{
    DESIGN_CUT_PROGRESS_TABLE_SQL, DESIGN_LIGHTING_TABLE_SQL, DESIGN_VARIANTS_TABLE_SQL,
    DIAGRAM_PLANNER_EXCLUSIONS_TABLE_SQL, DIAGRAM_PREVIEWS_TABLE_SQL,
    DIAGRAM_SOLID_EXTENTS_TABLE_SQL, DIAGRAM_SOLID_HULL_TABLE_SQL, RENDER_JOBS_TABLE_SQL,
    SAVED_ROUGH_PLANS_TABLE_SQL, SEARCH_INDEXES_SQL, TAG_TABLES_SQL, diagram_tilt_curves_table_sql,
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
    /// All nullable and brand-new. Idempotent: gated on whether the LAST column added
    /// (`mirror_symmetry`) already exists, and the seven `ALTER`s run in one
    /// transaction ([`Self::add_missing_columns`]) that skips any column an older,
    /// non-transactional build already added -- so a table left with only `hw_ratio`
    /// is completed on the next open.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column, or adding any of the seven, fails.
    pub(super) fn migrate_proportions_columns(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_details", "mirror_symmetry")? {
            debug!("Proportions/symmetry column migration already applied; skipping.");
            return Ok(());
        }

        info!("Adding diagram_details proportion-ratio and symmetry columns...");
        self.add_missing_columns(
            "diagram_details",
            &[
                ("hw_ratio", "REAL"),
                ("tw_ratio", "REAL"),
                ("uw_ratio", "REAL"),
                ("pw_ratio", "REAL"),
                ("cw_ratio", "REAL"),
                ("symmetry_order", "INTEGER"),
                ("mirror_symmetry", "BOOLEAN"),
            ],
        )
        .context("Failed to add proportion-ratio/symmetry columns")?;
        info!("Proportions/symmetry column migration complete.");
        Ok(())
    }

    /// Adds `diagram_details`' split designer/citation columns (`designer`,
    /// `source_citation`) and the competition-entry columns (`pdf_file`, `gem_file`,
    /// `shape_category`) for a database created before this crate captured them.
    ///
    /// `designer_info` is deliberately left in place (`FacetingDiagramDetail::designer`
    /// still reads it); nothing backfills the new columns -- the parser populates them
    /// on next sync.
    ///
    /// All five nullable and brand-new. Gated on the LAST column added
    /// (`shape_category`), with the five `ALTER`s in one transaction that skips any
    /// column already present -- see [`Self::migrate_proportions_columns`].
    ///
    /// An earlier version of this migration also created
    /// `idx_diagram_details_designer` here, for an "every design by X" exact lookup
    /// that turned out to have no caller -- see
    /// [`Self::migrate_drop_unused_designer_index`], which removes it.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding any of the five fails.
    pub(super) fn migrate_designer_and_attachment_columns(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_details", "shape_category")? {
            debug!("Designer/attachment columns already present; skipping the ADD COLUMN step.");
            return Ok(());
        }

        info!("Adding diagram_details designer-split and competition-entry columns...");
        self.add_missing_columns(
            "diagram_details",
            &[
                ("designer", "TEXT"),
                ("source_citation", "TEXT"),
                ("pdf_file", "TEXT"),
                ("gem_file", "TEXT"),
                ("shape_category", "INTEGER"),
            ],
        )
        .context("Failed to add designer-split/competition-entry columns")?;
        info!("Designer/attachment column migration complete.");
        Ok(())
    }

    /// Drops `idx_diagram_details_designer`, created by an earlier version of
    /// [`Self::migrate_designer_and_attachment_columns`] for an "every design by X"
    /// exact-match lookup: nothing in this crate or `apps/indicatrix-cut`
    /// ever runs that query -- every real "by designer" search goes through
    /// `crate::db::sqlite::search`'s free-text `LIKE` predicate instead, which an
    /// equality index cannot serve at all. Pure dead weight: extra bytes written on
    /// every `diagram_details` insert/update, for a lookup nothing performs.
    ///
    /// `DROP INDEX IF EXISTS` is naturally idempotent, so this always runs rather than
    /// checking first, the same way [`Self::migrate_search_indexes`] does.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DROP INDEX` fails.
    pub(super) fn migrate_drop_unused_designer_index(&self) -> Result<()> {
        self.conn
            .execute_batch("DROP INDEX IF EXISTS idx_diagram_details_designer;")
            .context("Failed to drop idx_diagram_details_designer")?;
        Ok(())
    }

    /// Adds `custom_gem_materials`' crystal-classification columns for the
    /// custom-material editor: `crystal_system`, `optical_character`, both TEXT (a
    /// `indicatrix` enum variant name, e.g. `"Trigonal"`), and
    /// `biaxial_delta_beta_alpha` REAL. See `CustomMaterialRow`'s field docs for why
    /// these stay plain text/`f32` rather than the `indicatrix` enums themselves.
    ///
    /// All three nullable and brand-new, gated on the LAST column added
    /// (`biaxial_delta_beta_alpha`), with the three `ALTER`s in one transaction that
    /// skips any column already present -- see [`Self::migrate_proportions_columns`].
    /// `NULL` on every pre-existing row already means "not stored, infer as
    /// `GemMaterial::new_custom` does", so no backfill is needed.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the `biaxial_delta_beta_alpha` column or adding
    /// any of the three fails.
    pub(super) fn migrate_crystal_optics_columns(&self) -> Result<()> {
        if Self::column_exists(
            &self.conn,
            "custom_gem_materials",
            "biaxial_delta_beta_alpha",
        )? {
            debug!("Crystal-optics columns already present; skipping the ADD COLUMN step.");
            return Ok(());
        }

        info!("Adding custom_gem_materials crystal-classification columns...");
        self.add_missing_columns(
            "custom_gem_materials",
            &[
                ("crystal_system", "TEXT"),
                ("optical_character", "TEXT"),
                ("biaxial_delta_beta_alpha", "REAL"),
            ],
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

    /// Adds the concave-tier columns: `diagram_details.concave_tiers` and
    /// `concave_facets` (`INTEGER NOT NULL DEFAULT 0`), and `angle_settings.tool` and
    /// `tool_line` (nullable `TEXT`), to a database created before concave tiers
    /// existed.
    ///
    /// The counts get their own columns because `facets_count` keeps its two-component
    /// `"55+6"` text: `parse_facets_count` splits it into `facets`/`girdle_facets`, so a
    /// third component would break every stored row. `0` is the right backfill for
    /// every existing design (none has a concave tier), and NULL tool columns mean a
    /// flat tier, so no row needs rewriting.
    ///
    /// Each column is gated on its own `column_exists`, inside one transaction, so a
    /// database an interrupted build left with half of them still ends up with all
    /// four, and an already-migrated (or fresh) database is a no-op. Must run before
    /// [`Self::migrate_blob_columns_last`], whose rebuild names `diagram_details`'
    /// columns explicitly and would otherwise drop the new ones.
    ///
    /// # Errors
    ///
    /// Returns an error if probing or adding any column fails; nothing is committed in
    /// that case.
    pub(super) fn migrate_concave_columns(&self) -> Result<()> {
        const COLUMNS: [(&str, &str, &str); 4] = [
            (
                "diagram_details",
                "concave_tiers",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "diagram_details",
                "concave_facets",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            ("angle_settings", "tool", "TEXT"),
            ("angle_settings", "tool_line", "TEXT"),
        ];
        let mut missing = Vec::new();
        for (table, column, sql_type) in COLUMNS {
            if !Self::column_exists(&self.conn, table, column)? {
                missing.push((table, column, sql_type));
            }
        }
        if missing.is_empty() {
            debug!("Concave columns already present; skipping.");
            return Ok(());
        }

        info!("Adding the concave-tier columns...");
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the concave-columns migration")?;
        for (table, column, sql_type) in missing {
            // Table/column/type come from the constant above, never from input.
            tx.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {sql_type};"
            ))
            .context(format!("Failed to add {table}.{column}"))?;
        }
        tx.commit()
            .context("Failed to commit the concave-columns migration")?;
        info!("Concave-columns migration complete.");
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

    /// Adds `custom_gem_materials.color_recipe_json`, a nullable TEXT column holding
    /// the serialized `ColorRecipe` for physically based chromophore colors.
    ///
    /// Purely additive and nullable, same idiom as [`Self::migrate_custom_material_specific_gravity`]:
    /// `NULL` means legacy fantasy mode or no recipe for every pre-existing row.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding it fails.
    pub(super) fn migrate_custom_material_color_recipe(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "custom_gem_materials", "color_recipe_json")? {
            debug!("color_recipe_json column already present; skipping the ADD COLUMN step.");
            return Ok(());
        }

        info!("Adding custom_gem_materials.color_recipe_json column...");
        self.conn
            .execute_batch("ALTER TABLE custom_gem_materials ADD COLUMN color_recipe_json TEXT;")
            .context("Failed to add custom_gem_materials.color_recipe_json column")?;
        info!("color_recipe_json column migration complete.");
        Ok(())
    }

    /// Adds `custom_gem_materials.dispersion_model_json`, a nullable TEXT column holding a
    /// custom material's dispersion curve as coefficients (Sellmeier with one or three
    /// terms, or Cauchy) in the JSON form the desktop app writes.
    ///
    /// Purely additive and nullable, same idiom as [`Self::migrate_custom_material_color_recipe`]:
    /// `NULL` for every pre-existing row, which keeps loading as the Cauchy fit of its
    /// refractive index and `n_F - n_C` figure exactly as before. The column is opaque
    /// text to this crate (it must not depend on `indicatrix`).
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding it fails.
    pub(super) fn migrate_custom_material_dispersion_model(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "custom_gem_materials", "dispersion_model_json")? {
            debug!("dispersion_model_json column already present; skipping the ADD COLUMN step.");
            return Ok(());
        }

        info!("Adding custom_gem_materials.dispersion_model_json column...");
        self.conn
            .execute_batch(
                "ALTER TABLE custom_gem_materials ADD COLUMN dispersion_model_json TEXT;",
            )
            .context("Failed to add custom_gem_materials.dispersion_model_json column")?;
        info!("dispersion_model_json column migration complete.");
        Ok(())
    }

    /// Adds `custom_gem_materials.absorption_bands_json`, a nullable TEXT column holding a
    /// custom material's seven-band body colour (the path-aware L*C*h editor's result) as JSON
    /// rows.
    ///
    /// Purely additive and nullable, same idiom as [`Self::migrate_custom_material_dispersion_model`]:
    /// `NULL` for every pre-existing row, which keeps colouring from its legacy absorption
    /// triple exactly as before. The column is opaque text to this crate.
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding it fails.
    pub(super) fn migrate_custom_material_absorption_bands(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "custom_gem_materials", "absorption_bands_json")? {
            debug!("absorption_bands_json column already present; skipping the ADD COLUMN step.");
            return Ok(());
        }

        info!("Adding custom_gem_materials.absorption_bands_json column...");
        self.conn
            .execute_batch(
                "ALTER TABLE custom_gem_materials ADD COLUMN absorption_bands_json TEXT;",
            )
            .context("Failed to add custom_gem_materials.absorption_bands_json column")?;
        info!("absorption_bands_json column migration complete.");
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
    /// Idempotent: gated on whether the LAST column added (`updated_at`) already
    /// exists, with both `ALTER`s in one transaction that skips any column already
    /// present -- see [`Self::migrate_proportions_columns`].
    ///
    /// # Errors
    ///
    /// Returns an error if checking for the column or adding either fails.
    pub(super) fn migrate_diagram_entries_timestamps(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "diagram_entries", "updated_at")? {
            debug!("diagram_entries timestamp columns already present; skipping.");
            return Ok(());
        }

        info!("Adding diagram_entries.created_at/updated_at columns...");
        self.add_missing_columns(
            "diagram_entries",
            &[("created_at", "INTEGER"), ("updated_at", "INTEGER")],
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
}
