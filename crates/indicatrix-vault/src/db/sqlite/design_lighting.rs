//! Storage for `design_lighting`: the lighting a design was last shown under.
//!
//! Kept per design UUID. See `Database::migrate_design_lighting_table`'s doc comment (in
//! `super::migrations`) for the schema, and `crate::model::design_lighting` for the model.
//!
//! None of this touches `diagram_entries`.

use super::Database;
use crate::model::{design_key::require_design_uuid, design_lighting::DesignLighting};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};

impl Database {
    /// Stores the lighting choice of the design with UUID `design_uuid`, replacing any
    /// earlier one. `updated_at` is Unix seconds.
    ///
    /// `settings_json` is kept exactly as given: this crate does not read it.
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID, the preset name is blank, or the
    /// write fails.
    pub fn set_design_lighting(
        &self,
        design_uuid: &str,
        preset_name: &str,
        settings_json: &str,
        updated_at: i64,
    ) -> Result<()> {
        let key = require_design_uuid(design_uuid)?;
        let preset_name = preset_name.trim();
        if preset_name.is_empty() {
            bail!("The lighting preset name must not be blank");
        }
        self.conn
            .execute(
                "INSERT OR REPLACE INTO design_lighting
                     (design_uuid, preset_name, settings_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![key, preset_name, settings_json, updated_at],
            )
            .with_context(|| format!("Failed to store the lighting of design {key}"))?;
        Ok(())
    }

    /// The lighting choice of the design with UUID `design_uuid`, or `None` when it has
    /// none (the design then uses the global lighting).
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID or the query fails.
    pub fn design_lighting(&self, design_uuid: &str) -> Result<Option<DesignLighting>> {
        let key = require_design_uuid(design_uuid)?;
        self.conn
            .query_row(
                "SELECT preset_name, settings_json, updated_at
                 FROM design_lighting
                 WHERE design_uuid = ?1",
                params![key],
                |row| {
                    Ok(DesignLighting {
                        preset_name: row.get(0)?,
                        settings_json: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to read the lighting of design {key}"))
    }

    /// Removes the lighting choice of the design with UUID `design_uuid`. Returns how many
    /// rows were deleted: 0 when it had none, which is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID or the `DELETE` fails.
    pub fn clear_design_lighting(&self, design_uuid: &str) -> Result<usize> {
        let key = require_design_uuid(design_uuid)?;
        self.conn
            .execute(
                "DELETE FROM design_lighting WHERE design_uuid = ?1",
                params![key],
            )
            .with_context(|| format!("Failed to clear the lighting of design {key}"))
    }
}
