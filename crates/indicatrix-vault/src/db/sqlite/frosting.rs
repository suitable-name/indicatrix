//! Storage for `diagram_frosting`: the frosted tiers and facets of a library design.
//!
//! `.asc` has no notation for frosting, so a design imported from a `.gcs` file (or saved
//! from the editor) keeps it here, keyed by `entry_id` and the tier's position in the design's
//! schedule. See [`Database::migrate_diagram_frosting_table`] for the schema, and
//! `crate::model::frosting` for the model.
//!
//! Like `diagram_planner_exclusions` this is a side table cascading off `diagram_entries`:
//! writing it never touches the entry row or its `updated_at`, and a `diagram_details`
//! re-sync leaves it alone. A caller that replaces a design's geometry replaces its frosting
//! with [`Database::set_diagram_frosting`] in the same step.

use super::{Database, migrations::DIAGRAM_FROSTING_TABLE_SQL};
use crate::model::frosting::{StoredTierFrosting, normalised};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;

/// `slots` as the comma-separated text the table stores (`""` for a whole tier).
fn slots_text(slots: &[u32]) -> String {
    slots
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The slots `text` lists; a token that is not a number is skipped.
fn slots_from_text(text: &str) -> Vec<u32> {
    text.split(',')
        .filter_map(|token| token.trim().parse().ok())
        .collect()
}

impl Database {
    /// Creates `diagram_frosting` for a database created before a library design could keep
    /// the frosting its `.gcs` file (or its design file) states.
    ///
    /// Keyed by `entry_id`, cascading off `diagram_entries`, for the reasons given on
    /// [`Self::migrate_planner_exclusion_table`]. Adds nothing to any existing table, so
    /// every design that predates it reads as unfrosted, which is the only sound reading
    /// (`.asc` never stated any). Naturally idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails.
    pub(in crate::db::sqlite) fn migrate_diagram_frosting_table(&self) -> Result<()> {
        self.conn
            .execute_batch(DIAGRAM_FROSTING_TABLE_SQL)
            .context("Failed to create diagram_frosting table")?;
        Ok(())
    }

    /// Replaces the frosting of design `entry_id` with `frosting`; an empty list makes the
    /// design unfrosted. `frosting` is normalised first (one row per tier, ascending), so
    /// the order and repeats of the list do not matter.
    ///
    /// Deliberately does NOT bump `diagram_entries.updated_at`, for the reason given on
    /// [`Self::set_planner_excluded`]: a caller that replaces the design writes this right
    /// after [`Self::save_design`], which already moved the stamp.
    ///
    /// # Errors
    ///
    /// Returns an error if a write fails, including a non-empty `frosting` for an
    /// `entry_id` that matches no `diagram_entries` row (the table's `FOREIGN KEY`); nothing
    /// is changed in that case.
    pub fn set_diagram_frosting(
        &self,
        entry_id: i64,
        frosting: &[StoredTierFrosting],
    ) -> Result<()> {
        let rows = normalised(frosting.to_vec());
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the frosting transaction")?;
        tx.execute(
            "DELETE FROM diagram_frosting WHERE entry_id = ?1",
            params![entry_id],
        )
        .with_context(|| format!("Failed to clear the frosting of diagram entry {entry_id}"))?;
        for row in &rows {
            tx.execute(
                "INSERT INTO diagram_frosting (entry_id, tier, facet_slots) VALUES (?1, ?2, ?3)",
                params![entry_id, row.tier, slots_text(&row.facets)],
            )
            .with_context(|| {
                format!(
                    "Failed to store the frosting of tier {} of diagram entry {entry_id}",
                    row.tier
                )
            })?;
        }
        tx.commit()
            .with_context(|| format!("Failed to store the frosting of diagram entry {entry_id}"))
    }

    /// The frosting of design `entry_id`, tiers ascending; empty for a design without any
    /// (and for an `entry_id` that matches no design).
    ///
    /// A read-only database of a file this build has not opened yet may lack the table
    /// ([`Self::open_read_only`] runs no migration): that reads as unfrosted too.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying query fails.
    pub fn diagram_frosting(&self, entry_id: i64) -> Result<Vec<StoredTierFrosting>> {
        if !self.frosting_table_exists()? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT tier, facet_slots FROM diagram_frosting
                 WHERE entry_id = ?1 ORDER BY tier",
            )
            .context("Failed to prepare the frosting query")?;
        let rows = stmt
            .query_map(params![entry_id], |row| {
                Ok(StoredTierFrosting {
                    tier: row.get(0)?,
                    facets: slots_from_text(&row.get::<_, String>(1)?),
                })
            })
            .with_context(|| format!("Failed to run the frosting query of entry {entry_id}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .with_context(|| format!("Failed to read the frosting of entry {entry_id}"))?;
        Ok(normalised(rows))
    }

    /// Every frosted design's rows, keyed by entry id, tiers ascending.
    ///
    /// One query instead of one [`Self::diagram_frosting`] per design. Designs without
    /// frosting are absent; the map is empty when the table does not exist (a read-only
    /// database this build has not opened yet), like the per-design read.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying query fails.
    pub fn all_diagram_frosting(&self) -> Result<BTreeMap<i64, Vec<StoredTierFrosting>>> {
        if !self.frosting_table_exists()? {
            return Ok(BTreeMap::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT entry_id, tier, facet_slots FROM diagram_frosting
                 ORDER BY entry_id, tier",
            )
            .context("Failed to prepare the all-frosting query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    StoredTierFrosting {
                        tier: row.get(1)?,
                        facets: slots_from_text(&row.get::<_, String>(2)?),
                    },
                ))
            })
            .context("Failed to run the all-frosting query")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the stored frosting")?;
        let mut by_entry: BTreeMap<i64, Vec<StoredTierFrosting>> = BTreeMap::new();
        for (entry_id, row) in rows {
            by_entry.entry(entry_id).or_default().push(row);
        }
        Ok(by_entry
            .into_iter()
            .map(|(entry_id, rows)| (entry_id, normalised(rows)))
            .collect())
    }

    /// Whether `diagram_frosting` exists in this database.
    fn frosting_table_exists(&self) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'diagram_frosting'",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to look for the diagram_frosting table")?;
        Ok(found.is_some())
    }
}
