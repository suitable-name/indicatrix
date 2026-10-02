//! `diagram_entries` CRUD: creating/upserting an entry lives here; the rest of the
//! table's operations are split into sibling modules by concern:
//!
//! - [`detail`]: saving a design's full detail row (and its angle-setting/attached-file
//!   children) in one transaction.
//! - [`read`]: loading a full design record back out, for local and remote-metadata
//!   callers.
//! - [`edit`]: the small, targeted mutations a cutter can make to an already-imported
//!   design (rename, hand-corrected metadata, provenance, ignore, delete).

use super::Database;
use crate::model::{detail::FacetingDiagramDetail, entry::FacetingDiagramEntry};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use tracing::debug;

mod detail;
mod edit;
mod read;

impl Database {
    /// Saves a diagram entry from `source_id` (see `crate::source::DiagramSource::id`).
    /// If `url` already exists, updates its title/`design_id`/`source_id` instead.
    /// Returns the inserted or updated row's ID. Dedupes only *within* `url` --
    /// different sources describing the same physical design under different URLs
    /// each get their own row.
    ///
    /// The upsert does not distinguish "the same design again" from "a different design
    /// that happens to share the url": a second design saved under a taken url lands on
    /// the first one's row, and a following [`Self::save_diagram_detail`] replaces that
    /// row's detail, angle table and attachments. A caller that must not do that asks
    /// [`Self::diagram_entry_for_url`] first and declines to write on a hit it does not
    /// own. Prefer [`Self::save_design`] when a detail is saved alongside.
    ///
    /// # Errors
    ///
    /// Returns an error if the `INSERT`, `UPDATE`, or follow-up ID `SELECT` fails.
    pub fn save_diagram_entry(&self, entry: &FacetingDiagramEntry, source_id: &str) -> Result<i64> {
        Self::save_diagram_entry_conn(&self.conn, entry, source_id)
    }

    /// [`Self::save_diagram_entry`]'s body, taking `conn: &Connection` rather than
    /// `&self` so [`Self::save_design`] can run it against an in-progress
    /// [`rusqlite::Transaction`] (which derefs to `&Connection`) instead of always
    /// opening/committing its own.
    fn save_diagram_entry_conn(
        conn: &Connection,
        entry: &FacetingDiagramEntry,
        source_id: &str,
    ) -> Result<i64> {
        let now = unix_now();
        // `INSERT OR IGNORE` won't update on conflict, so update is handled explicitly below.
        let mut stmt_insert = conn.prepare_cached(
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
            let id = conn.last_insert_rowid();
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
            let mut stmt_update = conn.prepare_cached(
                "UPDATE diagram_entries SET title = ?1, design_id = ?2, source_id = ?3, updated_at = MAX(?4, COALESCE(updated_at, 0) + 1) WHERE url = ?5",
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

            let mut stmt_select =
                conn.prepare_cached("SELECT id FROM diagram_entries WHERE url = ?1")?;
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

    /// Saves `entry` and `detail` together in ONE transaction: either both land, or
    /// neither does. Exactly [`Self::save_diagram_entry`] followed by
    /// [`Self::save_diagram_detail`], except that a failure partway through cannot
    /// leave an entry row with no matching detail row behind -- a gap seen on the real
    /// catalogue itself (an import that saves the entry, then fails or is killed before
    /// saving detail, leaves an entry with no detail row to this day). An entry without
    /// a detail row reads as "not yet imported" to the import collision check, and as a
    /// half-saved design the mirror's local-row guard then skips forever.
    /// `save_diagram_entry`/`save_diagram_detail` themselves are left as their own
    /// public, separately-committing methods -- existing callers that intentionally
    /// save an entry without a detail yet (or vice versa) keep working unchanged; this
    /// is for a caller that has both in hand at once and wants the atomicity. The
    /// catalogue's three writers (Save's write-back, the mirror sync and the
    /// `.asc` import) all go through it.
    ///
    /// Returns the entry's id (new or existing, same meaning as
    /// [`Self::save_diagram_entry`]'s own return value).
    ///
    /// # Errors
    ///
    /// Returns an error, with nothing committed, if starting/committing the
    /// transaction or either underlying save fails.
    pub fn save_design(
        &self,
        entry: &FacetingDiagramEntry,
        detail: &FacetingDiagramDetail,
        source_id: &str,
    ) -> Result<i64> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start save_design transaction")?;
        let entry_id = Self::save_diagram_entry_conn(&tx, entry, source_id)?;
        Self::save_diagram_detail_tx(&tx, detail, entry_id)?;
        tx.commit()
            .context("Failed to commit save_design transaction")?;
        Ok(entry_id)
    }

    /// Looks up `diagram_entries.id` for `url`, or `None` if no row has it yet.
    ///
    /// A plain existence check with no side effect -- unlike [`Self::save_diagram_entry`],
    /// this never inserts or updates a row. Exists so a caller that must decide WHETHER
    /// to save (e.g. `apps/indicatrix-cut`'s mirror sync, guarding against overwriting a
    /// hand-imported local row that happens to share a remote design's `url` -- see
    /// `crate::model::mirror`'s module doc comment) can ask "does this row already
    /// exist" without the upsert-and-report-id shape `save_diagram_entry` always commits
    /// to.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn diagram_entry_id_for_url(&self, url: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM diagram_entries WHERE url = ?1",
                params![url],
                |row| row.get(0),
            )
            .optional()
            .context(format!("Failed to look up diagram entry id for url: {url}"))
    }

    /// [`Self::diagram_entry_id_for_url`] plus the row's title: `(id, title)` of the
    /// entry that owns `url`, or `None` if no row has it. For a caller that declines to
    /// write over a row it does not own and wants to name that row to the user.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn diagram_entry_for_url(&self, url: &str) -> Result<Option<(i64, String)>> {
        self.conn
            .query_row(
                "SELECT id, title FROM diagram_entries WHERE url = ?1",
                params![url],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context(format!("Failed to look up diagram entry for url: {url}"))
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
