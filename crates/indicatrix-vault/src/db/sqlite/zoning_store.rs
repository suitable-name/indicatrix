//! Storage for rough colour and colour zoning.
//!
//! Five side tables and the `Database` methods that read and write them.
//!
//! # Why side tables
//!
//! Same pattern as `diagram_planner_exclusions`: the saved-plan rows, the material rows and the
//! render-job rows keep their exact columns and bytes, so an older build opens the same
//! file, shows the same plans, materials and jobs, and never reads, writes or deletes anything
//! here. The tables are created by `CREATE TABLE IF NOT EXISTS` in [`Database::new`], and no
//! schema-version constant moves.
//!
//! # No foreign keys, on purpose
//!
//! A foreign key with `ON DELETE CASCADE` would let an older build (which does not know these
//! tables) delete rows here as a side effect of deleting a plan, a material or a job, with
//! `PRAGMA foreign_keys = ON`. The rows are therefore plain keyed rows, and this build removes
//! the ones whose owner is gone with [`Database::prune_zoning_orphans`]. Plan ids and job ids are
//! `AUTOINCREMENT` and never reused, so an orphan cannot be mistaken for a new owner's row.
//!
//! # Read-only databases
//!
//! A database opened with [`Database::open_read_only`] runs no migration. Every reader here
//! answers "nothing stored" when the tables do not exist (a file this build has not opened
//! yet); every writer fails with SQLite's own read-only error, like any other write. The deletes
//! and the prune answer "nothing removed" (`Ok(0)`) when their table does not exist, instead of
//! failing with "no such table".
//!
//! # Tables
//!
//! | table | key | content |
//! |---|---|---|
//! | `rough_colour` | `plan_id` | zoned absorption JSON, fit report JSON, format version, time |
//! | `rough_colour_photo` | `(plan_id, view, kind)` | working-resolution image bytes (BLOB last) |
//! | `stone_pose_choice` | `(plan_id, layout_index, stone_index)` | pose index |
//! | `material_zoning` | `material_name` | zoned absorption JSON, relative-to-stone flag, version |
//! | `render_job_zoning` | `job_id` | zoned absorption JSON of a queued render job, format version |
//!
//! `material_zoning` is keyed by the custom material's NAME, the key the material table itself is
//! unique on (`custom_gem_materials.name`), and `stone_pose_choice` carries a `layout_index`
//! because a saved plan holds up to ten result layouts.

use super::Database;
use crate::model::zoning::{
    MaterialZoningRow, RoughColourPhotoMeta, RoughColourPhotoRow, RoughColourRow,
    StonePoseChoiceRow,
};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeSet;

/// The full `CREATE TABLE IF NOT EXISTS` text of the five zoning side tables.
///
/// BLOB and other large columns are declared last, per the house convention (see
/// `migrate_blob_columns_last`): a new table created in this order needs no staging rebuild.
pub(in crate::db::sqlite) const ZONING_TABLES_SQL: &str = "
    CREATE TABLE IF NOT EXISTS rough_colour (
        plan_id INTEGER PRIMARY KEY,
        version INTEGER NOT NULL,
        created INTEGER NOT NULL,
        zoned_json TEXT NOT NULL,
        fit_json TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS rough_colour_photo (
        plan_id INTEGER NOT NULL,
        view INTEGER NOT NULL,
        kind TEXT NOT NULL,
        width INTEGER NOT NULL,
        height INTEGER NOT NULL,
        encoding TEXT NOT NULL,
        data_blob BLOB NOT NULL,
        PRIMARY KEY (plan_id, view, kind)
    );

    CREATE TABLE IF NOT EXISTS stone_pose_choice (
        plan_id INTEGER NOT NULL,
        layout_index INTEGER NOT NULL,
        stone_index INTEGER NOT NULL,
        pose INTEGER NOT NULL,
        PRIMARY KEY (plan_id, layout_index, stone_index)
    );

    CREATE TABLE IF NOT EXISTS material_zoning (
        material_name TEXT PRIMARY KEY COLLATE NOCASE,
        relative_to_stone INTEGER NOT NULL DEFAULT 0,
        version INTEGER NOT NULL,
        zoned_json TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS render_job_zoning (
        job_id INTEGER PRIMARY KEY,
        version INTEGER NOT NULL DEFAULT 1,
        zoned_json TEXT NOT NULL
    );
";

/// `ALTER TABLE` that adds the format `version` to a `render_job_zoning` table created before the
/// column existed (the `CREATE TABLE IF NOT EXISTS` above does not touch an existing table).
/// Rows written by those builds are version 1, the only version there was.
const RENDER_JOB_VERSION_COLUMN_SQL: &str =
    "ALTER TABLE render_job_zoning ADD COLUMN version INTEGER NOT NULL DEFAULT 1";

/// Reads the `INTEGER` column `index` of `row` as a `u32`.
fn u32_col(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u32> {
    let value: i64 = row.get(index)?;
    u32::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

impl Database {
    /// Creates the zoning side tables when they are missing.
    ///
    /// Naturally idempotent; called by [`Database::new`].
    ///
    /// # Errors
    ///
    /// Returns an error if creating a table fails.
    pub(in crate::db::sqlite) fn migrate_zoning_tables(&self) -> Result<()> {
        self.conn
            .execute_batch(ZONING_TABLES_SQL)
            .context("Failed to create the zoning side tables")?;
        if !self.zoning_column_exists("render_job_zoning", "version")? {
            self.conn
                .execute(RENDER_JOB_VERSION_COLUMN_SQL, [])
                .context("Failed to add the version column to render_job_zoning")?;
        }
        Ok(())
    }

    /// Whether table `table` has a column named `column`.
    fn zoning_column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2",
                params![table, column],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("Failed to look for column {column} of {table}"))?;
        Ok(found.is_some())
    }

    /// Whether table `name` exists in this database.
    ///
    /// A read-only database of a file this build has not opened may lack the zoning tables.
    fn zoning_table_exists(&self, name: &str) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("Failed to look for table {name}"))?;
        Ok(found.is_some())
    }

    // ---- rough_colour -------------------------------------------------------------------

    /// Stores (inserts or replaces) the rough colour of plan `row.plan_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert fails (a read-only database, or no zoning tables).
    pub fn save_rough_colour(&self, row: &RoughColourRow) -> Result<()> {
        upsert_rough_colour(&self.conn, row)
    }

    /// Replaces the rough colour of plan `row.plan_id` AND its cached photos in one transaction.
    ///
    /// The colour row is upserted, every photo row the plan had is deleted, and `photos` are
    /// inserted. Either all of it happens or none of it: a failure midway (a damaged blob, a full
    /// disk) leaves the previous colour with its previous photos, never a new colour with a mix
    /// of old and new views. Views the plan had before and `photos` omits are gone afterwards.
    /// Pose choices are not touched.
    ///
    /// # Errors
    ///
    /// Returns an error if a photo belongs to another plan than `row`, or if any statement fails
    /// (the transaction is then rolled back).
    pub fn replace_rough_colour(
        &self,
        row: &RoughColourRow,
        photos: &[RoughColourPhotoRow],
    ) -> Result<()> {
        if let Some(stray) = photos.iter().find(|photo| photo.plan_id != row.plan_id) {
            anyhow::bail!(
                "A photo of plan {} cannot be stored with the colour of plan {}",
                stray.plan_id,
                row.plan_id
            );
        }
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the rough colour replacement")?;
        upsert_rough_colour(&transaction, row)?;
        transaction
            .execute(
                "DELETE FROM rough_colour_photo WHERE plan_id = ?1",
                params![row.plan_id],
            )
            .with_context(|| format!("Failed to clear the old images of plan {}", row.plan_id))?;
        for photo in photos {
            upsert_photo(&transaction, photo)?;
        }
        transaction
            .commit()
            .context("Failed to commit the rough colour replacement")
    }

    /// The rough colour of plan `plan_id`, or `None` when it has none (or the tables do not
    /// exist).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored version is out of range.
    pub fn load_rough_colour(&self, plan_id: i64) -> Result<Option<RoughColourRow>> {
        if !self.zoning_table_exists("rough_colour")? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT version, created, zoned_json, fit_json
                 FROM rough_colour WHERE plan_id = ?1",
                params![plan_id],
                |row| {
                    Ok(RoughColourRow {
                        plan_id,
                        version: u32_col(row, 0)?,
                        created: row.get(1)?,
                        zoned_json: row.get(2)?,
                        fit_json: row.get(3)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to read the rough colour of plan {plan_id}"))
    }

    /// The ids of every plan that has a rough colour, ascending.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn rough_colour_plan_ids(&self) -> Result<BTreeSet<i64>> {
        if !self.zoning_table_exists("rough_colour")? {
            return Ok(BTreeSet::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT plan_id FROM rough_colour ORDER BY plan_id")
            .context("Failed to prepare the rough colour listing")?;
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .context("Failed to run the rough colour listing")?
            .collect::<rusqlite::Result<BTreeSet<i64>>>()
            .context("Failed to read the rough colour listing")
    }

    /// Removes everything stored for plan `plan_id`.
    ///
    /// That is its colour, its photos and its pose choices, in one transaction. Removing a plan
    /// that has none is a no-op that succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error if a `DELETE` fails.
    pub fn delete_rough_colour(&self, plan_id: i64) -> Result<()> {
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the rough colour delete")?;
        for table in ["rough_colour", "rough_colour_photo", "stone_pose_choice"] {
            if !self.zoning_table_exists(table)? {
                continue;
            }
            transaction
                .execute(
                    &format!("DELETE FROM {table} WHERE plan_id = ?1"),
                    params![plan_id],
                )
                .with_context(|| format!("Failed to delete plan {plan_id} from {table}"))?;
        }
        transaction
            .commit()
            .context("Failed to commit the rough colour delete")
    }

    // ---- rough_colour_photo -------------------------------------------------------------

    /// Stores (inserts or replaces) one working-resolution image of a plan.
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert fails.
    pub fn save_rough_colour_photo(&self, row: &RoughColourPhotoRow) -> Result<()> {
        upsert_photo(&self.conn, row)
    }

    /// One stored image, bytes included, or `None` when it is not stored.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored size is out of range.
    pub fn load_rough_colour_photo(
        &self,
        plan_id: i64,
        view: u32,
        kind: &str,
    ) -> Result<Option<RoughColourPhotoRow>> {
        if !self.zoning_table_exists("rough_colour_photo")? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT width, height, encoding, data_blob
                 FROM rough_colour_photo WHERE plan_id = ?1 AND view = ?2 AND kind = ?3",
                params![plan_id, i64::from(view), kind],
                |row| {
                    Ok(RoughColourPhotoRow {
                        plan_id,
                        view,
                        kind: kind.to_string(),
                        width: u32_col(row, 0)?,
                        height: u32_col(row, 1)?,
                        encoding: row.get(2)?,
                        data: row.get(3)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to read the {kind} image of view {view}"))
    }

    /// Every image stored for `plan_id` without its bytes, ordered by view then kind.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn list_rough_colour_photos(&self, plan_id: i64) -> Result<Vec<RoughColourPhotoMeta>> {
        if !self.zoning_table_exists("rough_colour_photo")? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT view, kind, width, height, encoding, length(data_blob)
                 FROM rough_colour_photo WHERE plan_id = ?1 ORDER BY view, kind",
            )
            .context("Failed to prepare the photo listing")?;
        let rows = stmt
            .query_map(params![plan_id], |row| {
                let byte_len: i64 = row.get(5)?;
                Ok(RoughColourPhotoMeta {
                    plan_id,
                    view: u32_col(row, 0)?,
                    kind: row.get(1)?,
                    width: u32_col(row, 2)?,
                    height: u32_col(row, 3)?,
                    encoding: row.get(4)?,
                    byte_len: u64::try_from(byte_len).unwrap_or(0),
                })
            })
            .context("Failed to run the photo listing")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the photo listing")
    }

    /// Removes every stored image of `plan_id`; the colour itself stays.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_rough_colour_photos(&self, plan_id: i64) -> Result<usize> {
        if !self.zoning_table_exists("rough_colour_photo")? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM rough_colour_photo WHERE plan_id = ?1",
                params![plan_id],
            )
            .with_context(|| format!("Failed to delete the images of plan {plan_id}"))
    }

    // ---- stone_pose_choice --------------------------------------------------------------

    /// Records the pose of one stone of a plan's layout.
    ///
    /// Stone `stone_index` of layout `layout_index` of plan `plan_id` uses pose `pose`. Pose `0`
    /// (the canonical pose) removes the row instead, so an unset stone and a canonical one are
    /// the same thing.
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert or the delete fails.
    pub fn set_stone_pose_choice(
        &self,
        plan_id: i64,
        layout_index: u32,
        stone_index: u32,
        pose: u8,
    ) -> Result<()> {
        if pose == 0 {
            self.conn
                .execute(
                    "DELETE FROM stone_pose_choice
                     WHERE plan_id = ?1 AND layout_index = ?2 AND stone_index = ?3",
                    params![plan_id, i64::from(layout_index), i64::from(stone_index)],
                )
                .context("Failed to clear a stone pose choice")?;
        } else {
            self.conn
                .execute(
                    "INSERT INTO stone_pose_choice (plan_id, layout_index, stone_index, pose)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(plan_id, layout_index, stone_index)
                     DO UPDATE SET pose = excluded.pose",
                    params![
                        plan_id,
                        i64::from(layout_index),
                        i64::from(stone_index),
                        i64::from(pose)
                    ],
                )
                .context("Failed to store a stone pose choice")?;
        }
        Ok(())
    }

    /// The non-canonical pose choices of plan `plan_id`, ordered by layout then stone.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored value is out of range.
    pub fn stone_pose_choices(&self, plan_id: i64) -> Result<Vec<StonePoseChoiceRow>> {
        if !self.zoning_table_exists("stone_pose_choice")? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT layout_index, stone_index, pose FROM stone_pose_choice
                 WHERE plan_id = ?1 ORDER BY layout_index, stone_index",
            )
            .context("Failed to prepare the pose choice listing")?;
        let rows = stmt
            .query_map(params![plan_id], |row| {
                let pose: i64 = row.get(2)?;
                Ok(StonePoseChoiceRow {
                    layout_index: u32_col(row, 0)?,
                    stone_index: u32_col(row, 1)?,
                    pose: u8::try_from(pose)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, pose))?,
                })
            })
            .context("Failed to run the pose choice listing")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the pose choice listing")
    }

    /// Removes every pose choice of `plan_id` (all stones back to the canonical pose).
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn clear_stone_pose_choices(&self, plan_id: i64) -> Result<usize> {
        if !self.zoning_table_exists("stone_pose_choice")? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM stone_pose_choice WHERE plan_id = ?1",
                params![plan_id],
            )
            .with_context(|| format!("Failed to clear the pose choices of plan {plan_id}"))
    }

    // ---- material_zoning ----------------------------------------------------------------

    /// Stores (inserts or replaces) the zones of custom material `row.material_name`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert fails.
    pub fn save_material_zoning(&self, row: &MaterialZoningRow) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO material_zoning (material_name, relative_to_stone, version, zoned_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(material_name) DO UPDATE SET
                    relative_to_stone = excluded.relative_to_stone,
                    version = excluded.version,
                    zoned_json = excluded.zoned_json",
                params![
                    row.material_name,
                    i64::from(row.relative_to_stone),
                    i64::from(row.version),
                    row.zoned_json
                ],
            )
            .with_context(|| format!("Failed to store the zones of '{}'", row.material_name))?;
        Ok(())
    }

    /// Stores custom material `material` and its zones `zoning` in ONE transaction: either both
    /// rows are written (replacing existing ones) or neither is, so a failing zoning write never
    /// leaves a plain material row behind.
    ///
    /// `zoning.material_name` must equal `material.name`.
    ///
    /// # Errors
    ///
    /// Returns an error if the names differ, or if either write or the commit fails; the
    /// transaction is then rolled back and the vault is unchanged.
    pub fn save_custom_material_with_zoning(
        &self,
        material: &super::CustomMaterialParams<'_>,
        zoning: &MaterialZoningRow,
    ) -> Result<()> {
        if material.name != zoning.material_name {
            anyhow::bail!(
                "The zones of '{}' cannot be stored with the material '{}'",
                zoning.material_name,
                material.name
            );
        }
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the material and zones write")?;
        self.save_custom_material(material)?;
        self.save_material_zoning(zoning)?;
        transaction
            .commit()
            .context("Failed to commit the material and zones write")
    }

    /// The zones of custom material `material_name` (compared ASCII case-insensitively, like
    /// the material lookup), or `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn load_material_zoning(&self, material_name: &str) -> Result<Option<MaterialZoningRow>> {
        if !self.zoning_table_exists("material_zoning")? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT material_name, relative_to_stone, version, zoned_json
                 FROM material_zoning WHERE material_name = ?1 COLLATE NOCASE",
                params![material_name],
                material_zoning_of,
            )
            .optional()
            .with_context(|| format!("Failed to read the zones of '{material_name}'"))
    }

    /// Every custom material that has zones, ordered by name.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn all_material_zonings(&self) -> Result<Vec<MaterialZoningRow>> {
        if !self.zoning_table_exists("material_zoning")? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT material_name, relative_to_stone, version, zoned_json
                 FROM material_zoning ORDER BY material_name",
            )
            .context("Failed to prepare the material zoning listing")?;
        let rows = stmt
            .query_map([], material_zoning_of)
            .context("Failed to run the material zoning listing")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the material zoning listing")
    }

    /// Removes the zones of `material_name` (ASCII case-insensitive).
    ///
    /// A material without zones is a no-op that succeeds. Returns how many rows were removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_material_zoning(&self, material_name: &str) -> Result<usize> {
        if !self.zoning_table_exists("material_zoning")? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM material_zoning WHERE material_name = ?1 COLLATE NOCASE",
                params![material_name],
            )
            .with_context(|| format!("Failed to delete the zones of '{material_name}'"))
    }

    // ---- render_job_zoning --------------------------------------------------------------

    /// Stores (inserts or replaces) the zones of queued render job `job_id`, tagged with the
    /// zoning format `version` they were written in.
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert fails.
    pub fn save_render_job_zoning(
        &self,
        job_id: i64,
        version: u32,
        zoned_json: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO render_job_zoning (job_id, version, zoned_json) VALUES (?1, ?2, ?3)
                 ON CONFLICT(job_id) DO UPDATE SET
                    version = excluded.version,
                    zoned_json = excluded.zoned_json",
                params![job_id, i64::from(version), zoned_json],
            )
            .with_context(|| format!("Failed to store the zones of render job {job_id}"))?;
        Ok(())
    }

    /// The `(format version, zones JSON)` of render job `job_id`, or `None`.
    ///
    /// A table created before the version column existed and opened read-only (no migration)
    /// answers version 1, the only version those builds wrote.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or the stored version is out of range.
    pub fn load_render_job_zoning(&self, job_id: i64) -> Result<Option<(u32, String)>> {
        if !self.zoning_table_exists("render_job_zoning")? {
            return Ok(None);
        }
        let sql = if self.zoning_column_exists("render_job_zoning", "version")? {
            "SELECT version, zoned_json FROM render_job_zoning WHERE job_id = ?1"
        } else {
            "SELECT 1, zoned_json FROM render_job_zoning WHERE job_id = ?1"
        };
        self.conn
            .query_row(sql, params![job_id], |row| {
                Ok((u32_col(row, 0)?, row.get::<_, String>(1)?))
            })
            .optional()
            .with_context(|| format!("Failed to read the zones of render job {job_id}"))
    }

    /// Removes the zones of render job `job_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_render_job_zoning(&self, job_id: i64) -> Result<usize> {
        if !self.zoning_table_exists("render_job_zoning")? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM render_job_zoning WHERE job_id = ?1",
                params![job_id],
            )
            .with_context(|| format!("Failed to delete the zones of render job {job_id}"))
    }

    // ---- housekeeping -------------------------------------------------------------------

    /// Removes the rows whose owner no longer exists.
    ///
    /// That is the colour, photos and pose choices of a deleted saved plan, the zones of a
    /// deleted custom material, and the zones of a deleted render job. Returns how many rows
    /// were removed.
    ///
    /// Needed because these tables carry no foreign keys (see the module doc).
    ///
    /// # Errors
    ///
    /// Returns an error if a `DELETE` fails (nothing is then removed: the five deletes run in
    /// one transaction). A table that does not exist (a file this build has not opened) is
    /// skipped.
    pub fn prune_zoning_orphans(&self) -> Result<usize> {
        let statements = [
            (
                "rough_colour",
                "DELETE FROM rough_colour
                 WHERE plan_id NOT IN (SELECT plan_id FROM saved_rough_plans)",
            ),
            (
                "rough_colour_photo",
                "DELETE FROM rough_colour_photo
                 WHERE plan_id NOT IN (SELECT plan_id FROM saved_rough_plans)",
            ),
            (
                "stone_pose_choice",
                "DELETE FROM stone_pose_choice
                 WHERE plan_id NOT IN (SELECT plan_id FROM saved_rough_plans)",
            ),
            (
                "material_zoning",
                "DELETE FROM material_zoning
                 WHERE material_name NOT IN (SELECT name FROM custom_gem_materials)",
            ),
            (
                "render_job_zoning",
                "DELETE FROM render_job_zoning
                 WHERE job_id NOT IN (SELECT job_id FROM render_jobs)",
            ),
        ];
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the zoning prune")?;
        let mut removed = 0;
        for (table, statement) in statements {
            if !self.zoning_table_exists(table)? {
                continue;
            }
            removed += transaction
                .execute(statement, [])
                .with_context(|| format!("Failed to prune {table}"))?;
        }
        transaction
            .commit()
            .context("Failed to commit the zoning prune")?;
        Ok(removed)
    }
}

/// The upsert behind [`Database::save_rough_colour`] and [`Database::replace_rough_colour`].
fn upsert_rough_colour(conn: &rusqlite::Connection, row: &RoughColourRow) -> Result<()> {
    conn.execute(
        "INSERT INTO rough_colour (plan_id, version, created, zoned_json, fit_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(plan_id) DO UPDATE SET
            version = excluded.version,
            created = excluded.created,
            zoned_json = excluded.zoned_json,
            fit_json = excluded.fit_json",
        params![
            row.plan_id,
            i64::from(row.version),
            row.created,
            row.zoned_json,
            row.fit_json
        ],
    )
    .with_context(|| format!("Failed to store the rough colour of plan {}", row.plan_id))?;
    Ok(())
}

/// The upsert behind [`Database::save_rough_colour_photo`] and [`Database::replace_rough_colour`].
fn upsert_photo(conn: &rusqlite::Connection, row: &RoughColourPhotoRow) -> Result<()> {
    conn.execute(
        "INSERT INTO rough_colour_photo
            (plan_id, view, kind, width, height, encoding, data_blob)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(plan_id, view, kind) DO UPDATE SET
            width = excluded.width,
            height = excluded.height,
            encoding = excluded.encoding,
            data_blob = excluded.data_blob",
        params![
            row.plan_id,
            i64::from(row.view),
            row.kind,
            i64::from(row.width),
            i64::from(row.height),
            row.encoding,
            row.data
        ],
    )
    .with_context(|| {
        format!(
            "Failed to store the {} image of view {} of plan {}",
            row.kind, row.view, row.plan_id
        )
    })?;
    Ok(())
}

/// Reads `material_name, relative_to_stone, version, zoned_json`.
fn material_zoning_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<MaterialZoningRow> {
    let relative: i64 = row.get(1)?;
    Ok(MaterialZoningRow {
        material_name: row.get(0)?,
        relative_to_stone: relative != 0,
        version: u32_col(row, 2)?,
        zoned_json: row.get(3)?,
    })
}
