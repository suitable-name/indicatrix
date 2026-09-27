//! `diagram_entries` CRUD: creating/upserting an entry and cross-source duplicate
//! detection live here; the rest of the table's operations are split into sibling
//! modules by concern:
//!
//! - [`detail`]: saving a design's full detail row (and its angle-setting/attached-file
//!   children) in one transaction.
//! - [`read`]: loading a full design record back out, for local and remote-metadata
//!   callers.
//! - [`edit`]: the small, targeted mutations a cutter can make to an already-imported
//!   design (rename, hand-corrected metadata, provenance, ignore, delete).

use super::Database;
use crate::model::{
    dedup::{CrossSourceDuplicate, normalize_for_dedup},
    entry::FacetDiagramEntry,
};
use anyhow::{Context, Result};
use rusqlite::params;
use tracing::debug;

mod detail;
mod edit;
mod read;

/// `(id, source_id, title, designer_info)` row from
/// [`Database::find_cross_source_duplicates`]'s candidate query, pre-normalisation.
/// Named to avoid tripping `clippy::type_complexity`.
type DuplicateCandidateRow = (i64, String, String, Option<String>);

impl Database {
    /// Saves a diagram entry from `source_id` (see `crate::source::DiagramSource::id`).
    /// If `url` already exists, updates its title/`design_id`/`source_id` instead.
    /// Returns the inserted or updated row's ID. Dedupes only *within* `url` --
    /// different sources describing the same physical design under different URLs
    /// each get their own row; see [`Self::find_cross_source_duplicates`] for the
    /// cross-source check (surfaces, never merges).
    ///
    /// # Errors
    ///
    /// Returns an error if the `INSERT`, `UPDATE`, or follow-up ID `SELECT` fails.
    pub fn save_diagram_entry(&self, entry: &FacetDiagramEntry, source_id: &str) -> Result<i64> {
        let now = unix_now();
        // `INSERT OR IGNORE` won't update on conflict, so update is handled explicitly below.
        let mut stmt_insert = self.conn.prepare_cached(
            "INSERT OR IGNORE INTO diagram_entries (title, url, design_id, source_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let changes = stmt_insert
            .execute(params![
                entry.title,
                entry.url,
                entry.design_id,
                source_id,
                now,
                now
            ])
            .context(format!(
                "Failed to INSERT OR IGNORE diagram entry with URL: {}",
                entry.url
            ))?;

        if changes > 0 {
            let id = self.conn.last_insert_rowid();
            debug!(
                "Inserted new diagram entry '{}' (URL: {}, source: {}) with ID: {}",
                entry.title, entry.url, source_id, id
            );
            Ok(id)
        } else {
            debug!(
                "Diagram entry with URL '{}' already exists. Updating title, design_id, and source_id.",
                entry.url
            );
            // `created_at` is deliberately left untouched -- this branch is a re-sync
            // of an existing row, not a new design.
            let mut stmt_update = self.conn.prepare_cached(
                "UPDATE diagram_entries SET title = ?1, design_id = ?2, source_id = ?3, updated_at = ?4 WHERE url = ?5",
            )?;
            stmt_update
                .execute(params![
                    entry.title,
                    entry.design_id,
                    source_id,
                    now,
                    entry.url
                ])
                .context(format!(
                    "Failed to UPDATE existing diagram entry with URL: {}",
                    entry.url
                ))?;

            let mut stmt_select = self
                .conn
                .prepare_cached("SELECT id FROM diagram_entries WHERE url = ?1")?;
            let id: i64 = stmt_select
                .query_row(params![entry.url], |row| row.get(0))
                .context(format!(
                    "Failed to SELECT ID of existing diagram entry with URL: {}",
                    entry.url
                ))?;
            debug!(
                "Updated existing diagram entry '{}' (URL: {}), existing ID: {}",
                entry.title, entry.url, id
            );
            Ok(id)
        }
    }

    /// Looks for entries already in the catalogue, synced from a source *other than*
    /// `new_source_id`, whose normalised title (and, when both sides have one,
    /// normalised designer) matches `title`/`designer_info`, and whose facet count
    /// matches `facets` when both are known. See `crate::model::dedup`'s module doc
    /// for why this only detects and surfaces candidates -- never merges or alters.
    /// A missing designer on either side is not a mismatch (favors an extra manual
    /// review over a silently unflagged duplicate); when both sides have one, they
    /// must match. SQL narrows first by `source_id != new_source_id` (and facet
    /// count, when known) so the comparison loop below only scans a small set.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn find_cross_source_duplicates(
        &self,
        new_source_id: &str,
        title: &str,
        designer_info: Option<&str>,
        facets: Option<i64>,
    ) -> Result<Vec<CrossSourceDuplicate>> {
        let normalized_title = normalize_for_dedup(title);
        if normalized_title.is_empty() {
            return Ok(Vec::new());
        }
        let normalized_designer = designer_info.map(normalize_for_dedup);

        let mut sql = String::from(
            "SELECT de.id, de.source_id, de.title, dd.designer_info
             FROM diagram_entries de
             LEFT JOIN diagram_details dd ON de.id = dd.entry_id
             WHERE de.source_id != ?1",
        );
        let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(new_source_id.to_string())];
        if let Some(f) = facets {
            sql.push_str(" AND dd.facets = ?2");
            sql_params.push(Box::new(f));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let bound: Vec<&dyn rusqlite::ToSql> =
            sql_params.iter().map(std::convert::AsRef::as_ref).collect();
        let rows: Vec<DuplicateCandidateRow> = stmt
            .query_map(bound.as_slice(), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut matches = Vec::new();
        for (existing_entry_id, existing_source_id, existing_title, existing_designer_info) in rows
        {
            if normalize_for_dedup(&existing_title) != normalized_title {
                continue;
            }
            if let (Some(want), Some(have)) =
                (&normalized_designer, existing_designer_info.as_deref())
                && normalize_for_dedup(have) != *want
            {
                continue;
            }
            matches.push(CrossSourceDuplicate {
                existing_entry_id,
                existing_source_id,
                existing_title,
                existing_designer_info,
            });
        }
        Ok(matches)
    }
}

/// The current wall-clock time as Unix seconds, for `diagram_entries.created_at`/
/// `updated_at`. Same `SystemTime`-based approach this crate
/// already uses for `diagram_tilt_curves.generated_at`/`diagram_previews.preview_generated_at`
/// (see those tables' save methods), just computed here instead of taken as a caller
/// parameter -- `save_diagram_entry`/`update_diagram_metadata` are existing public
/// signatures with call sites across the workspace, so stamping the time internally
/// keeps every one of them compiling unchanged.
///
/// Falls back to `0` on a system clock set before the Unix epoch, which never happens
/// on a real machine -- this only avoids a panic on `duration_since`'s `Result`.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
