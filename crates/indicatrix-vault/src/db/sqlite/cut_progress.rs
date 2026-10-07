//! Storage for `design_cut_progress`: the steps of a design a cutter marked done.
//!
//! Kept per design UUID. See `Database::migrate_design_cut_progress_table`'s doc comment
//! (in `super::migrations`) for the schema, and `crate::model::cut_progress` for the model.
//!
//! None of this touches `diagram_entries`.

use super::Database;
use crate::model::{cut_progress::CutProgressMark, design_key::require_design_uuid};
use anyhow::{Context, Result, bail};
use rusqlite::params;

/// `text` without surrounding spaces.
///
/// # Errors
///
/// Returns an error when nothing is left; `what` names the field in the message.
fn non_blank(text: &str, what: &str) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("The {what} must not be blank");
    }
    Ok(trimmed.to_string())
}

impl Database {
    /// Marks step `step_key` of the design with UUID `design_uuid` as done at `done_at`
    /// (Unix seconds), remembering `step_signature`, the fingerprint of the step's cutting
    /// values at this moment.
    ///
    /// Marking a step that is already marked replaces its signature and time, which is how
    /// a cutter confirms a step again after the design changed.
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID, the step key or signature is
    /// blank, or the write fails.
    pub fn mark_step_done(
        &self,
        design_uuid: &str,
        step_key: &str,
        step_signature: &str,
        done_at: i64,
    ) -> Result<()> {
        let key = require_design_uuid(design_uuid)?;
        let step_key = non_blank(step_key, "step key")?;
        let step_signature = non_blank(step_signature, "step signature")?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO design_cut_progress
                     (design_uuid, step_key, step_signature, done_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![key, step_key, step_signature, done_at],
            )
            .with_context(|| format!("Failed to mark step '{step_key}' of design {key} as done"))?;
        Ok(())
    }

    /// Removes the done mark of step `step_key`. Returns how many rows were deleted: 0
    /// when the step was not marked, which is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID or the `DELETE` fails.
    pub fn unmark_step(&self, design_uuid: &str, step_key: &str) -> Result<usize> {
        let key = require_design_uuid(design_uuid)?;
        self.conn
            .execute(
                "DELETE FROM design_cut_progress WHERE design_uuid = ?1 AND step_key = ?2",
                params![key, step_key.trim()],
            )
            .with_context(|| format!("Failed to unmark step '{step_key}' of design {key}"))
    }

    /// Every step of the design with UUID `design_uuid` that is marked done, oldest mark
    /// first (`done_at`, then step key).
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID or the query fails.
    pub fn cut_progress(&self, design_uuid: &str) -> Result<Vec<CutProgressMark>> {
        let key = require_design_uuid(design_uuid)?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT step_key, step_signature, done_at
                 FROM design_cut_progress
                 WHERE design_uuid = ?1
                 ORDER BY done_at, step_key",
            )
            .context("Failed to prepare the cutting progress query")?;
        stmt.query_map(params![key], |row| {
            Ok(CutProgressMark {
                step_key: row.get(0)?,
                step_signature: row.get(1)?,
                done_at: row.get(2)?,
            })
        })
        .context("Failed to run the cutting progress query")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("Failed to read the cutting progress of design {key}"))
    }

    /// Removes every done mark of the design with UUID `design_uuid`. Returns how many
    /// rows were deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID or the `DELETE` fails.
    pub fn clear_cut_progress(&self, design_uuid: &str) -> Result<usize> {
        let key = require_design_uuid(design_uuid)?;
        self.conn
            .execute(
                "DELETE FROM design_cut_progress WHERE design_uuid = ?1",
                params![key],
            )
            .with_context(|| format!("Failed to clear the cutting progress of design {key}"))
    }
}
