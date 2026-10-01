//! Storage for `diagram_planner_exclusions`: the designs the Rough Planner must leave out
//! of its candidate set. See `Database::migrate_planner_exclusion_table`'s doc comment (in
//! `super::migrations`) for the schema and why this is a side table keyed by `entry_id`
//! rather than a column on `diagram_entries`.
//!
//! The mark is independent of `diagram_entries.ignored`: an excluded design stays in the
//! library list, in `all_entry_ids` and in every search; only the planner reads this table.

use super::Database;
use anyhow::{Context, Result};
use rusqlite::params;
use std::collections::BTreeSet;

impl Database {
    /// Marks `entry_id` as excluded from the Rough Planner (`excluded == true`) or lets
    /// the planner use it again (`excluded == false`).
    ///
    /// Excluding a design that is already excluded, and restoring one that is not, are
    /// both no-ops that succeed. Excluding an `entry_id` that names no design fails
    /// through the table's `FOREIGN KEY`; restoring one succeeds, since there is nothing
    /// to remove.
    ///
    /// Deliberately does NOT bump `diagram_entries.updated_at` (see
    /// [`Self::set_diagram_ignored`], which makes the same choice): that column is the
    /// revision stamp the mesh cache, the preview and tilt-curve compare-and-swap
    /// writes and every mirror's refetch key on, and the exclusion mark changes nothing
    /// they depend on. Bumping it would invalidate cached renders and re-trigger syncs
    /// for a design whose recorded content did not change.
    ///
    /// # Errors
    ///
    /// Returns an error if the `INSERT` or `DELETE` fails, including when excluding an
    /// `entry_id` that matches no `diagram_entries` row.
    pub fn set_planner_excluded(&self, entry_id: i64, excluded: bool) -> Result<()> {
        if excluded {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO diagram_planner_exclusions (entry_id) VALUES (?1)",
                    params![entry_id],
                )
                .with_context(|| {
                    format!("Failed to exclude diagram entry {entry_id} from the planner")
                })?;
        } else {
            self.conn
                .execute(
                    "DELETE FROM diagram_planner_exclusions WHERE entry_id = ?1",
                    params![entry_id],
                )
                .with_context(|| {
                    format!("Failed to include diagram entry {entry_id} in the planner")
                })?;
        }
        Ok(())
    }

    /// The id of every design excluded from the Rough Planner, ascending.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` fails.
    pub fn planner_excluded_ids(&self) -> Result<BTreeSet<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT entry_id FROM diagram_planner_exclusions ORDER BY entry_id")
            .context("Failed to prepare the planner exclusions query")?;
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .context("Failed to run the planner exclusions query")?
            .collect::<rusqlite::Result<BTreeSet<i64>>>()
            .context("Failed to read the planner exclusions")
    }

    /// The members of `entry_ids` that are excluded from the Rough Planner, ascending.
    /// Ids that are not excluded, and ids that match no design, are absent from the
    /// result; an empty `entry_ids` returns an empty set without querying.
    ///
    /// ONE query however many ids there are: the ids travel as a single JSON array bound
    /// to `json_each`, the way [`Self::tags_for_entries`] does it, so a whole page of
    /// library rows (or a whole catalogue) needs no chunking.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` fails.
    pub fn planner_excluded_among(&self, entry_ids: &[i64]) -> Result<BTreeSet<i64>> {
        if entry_ids.is_empty() {
            return Ok(BTreeSet::new());
        }
        let ids_json = format!(
            "[{}]",
            entry_ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut stmt = self
            .conn
            .prepare(
                "SELECT entry_id FROM diagram_planner_exclusions
                 WHERE entry_id IN (SELECT value FROM json_each(?1))
                 ORDER BY entry_id",
            )
            .context("Failed to prepare the planner exclusions lookup")?;
        stmt.query_map(params![ids_json], |row| row.get::<_, i64>(0))
            .context("Failed to run the planner exclusions lookup")?
            .collect::<rusqlite::Result<BTreeSet<i64>>>()
            .context("Failed to read the planner exclusions lookup")
    }
}
