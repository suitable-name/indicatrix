//! Storage for `saved_rough_plans` -- persisted user rough planner sessions and layouts.
//!
//! See `Database::migrate_saved_rough_plans_table`'s doc comment (in `super::migrations`)
//! for why this table exists without foreign keys to designs (a saved plan must survive a
//! design's deletion and report it), and `crate::model::saved_rough_plan` for the models.
//!
//! The second `impl` block holds the library lookups a saved plan's staleness check needs
//! (design titles by id, designs by title, the library's identity stamp): they are small,
//! read the catalogue without its `ignored` filter, and have no other caller.

use super::Database;
use crate::model::saved_rough_plan::{SavedRoughPlan, SavedRoughPlanMeta};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params, params_from_iter};
use std::collections::BTreeMap;

/// Most `?` placeholders bound into one `IN (...)` list.
const ID_CHUNK: usize = 500;

/// Stamps are drawn from `1..=STAMP_RANGE`, which fits SQLite's signed 32-bit
/// `user_version` and never reads as "no stamp" (0).
const STAMP_RANGE: u64 = 0x7FFF_FFFE;

/// The metadata columns of a list or get query, in the order [`meta_of`] reads them.
const META_COLUMNS: &str = "plan_id, name, created_at, updated_at, summary";

/// Reads [`META_COLUMNS`] from `row`.
fn meta_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedRoughPlanMeta> {
    Ok(SavedRoughPlanMeta {
        plan_id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        summary: row.get(4)?,
    })
}

impl Database {
    /// Lists metadata for all saved rough plans, ordered newest-first by creation:
    /// `created_at DESC, plan_id DESC` (the date a list shows, so a rename does not move
    /// a plan).
    ///
    /// Excludes the payload text for speed when populating plan pickers and lists.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing the statement or reading rows fails.
    pub fn list_saved_rough_plans(&self) -> Result<Vec<SavedRoughPlanMeta>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {META_COLUMNS}
             FROM saved_rough_plans
             ORDER BY created_at DESC, plan_id DESC"
        ))?;
        let rows = stmt.query_map([], meta_of)?;
        let mut metas = Vec::new();
        for row in rows {
            metas.push(row.context("Failed to read saved rough plan metadata row")?);
        }
        Ok(metas)
    }

    /// Fetches a full saved rough plan by its ID, including its serialized payload.
    /// Returns `None` if no plan with `id` exists.
    ///
    /// # Errors
    ///
    /// Returns an error if querying fails or if the stored `payload_version` cannot be
    /// converted to `u32`.
    pub fn get_saved_rough_plan(&self, id: i64) -> Result<Option<SavedRoughPlan>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {META_COLUMNS}, payload_version, payload
                     FROM saved_rough_plans
                     WHERE plan_id = ?1"
                ),
                params![id],
                |row| {
                    Ok(SavedRoughPlan {
                        meta: meta_of(row)?,
                        payload_version: row.get(5)?,
                        payload: row.get(6)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to get saved rough plan for plan_id: {id}"))
    }

    /// Saves a new rough plan, returning the newly allocated `plan_id`.
    ///
    /// Both `created_at` and `updated_at` are initialized to `now` (Unix seconds).
    /// `summary` is the plan's one-line list description, stored so a list never reads
    /// the payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `INSERT` fails.
    pub fn save_rough_plan(
        &self,
        name: &str,
        payload_version: u32,
        payload: &str,
        summary: &str,
        now: i64,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO saved_rough_plans (
                     name, created_at, updated_at, payload_version, summary, payload
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![name, now, now, i64::from(payload_version), summary, payload],
            )
            .with_context(|| format!("Failed to insert saved rough plan '{name}'"))?;
        let id = self.conn.last_insert_rowid();
        Ok(id)
    }

    /// Stores the list description of plan `id` without touching its other columns
    /// (`updated_at` included). Returns how many rows changed: 0 when the plan no longer
    /// exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn set_saved_rough_plan_summary(&self, id: i64, summary: &str) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE saved_rough_plans SET summary = ?1 WHERE plan_id = ?2",
                params![summary, id],
            )
            .with_context(|| format!("Failed to store the summary of plan_id: {id}"))
    }

    /// Renames an existing saved rough plan, updating its `updated_at` timestamp to `now`.
    ///
    /// `payload`, when given, replaces the stored payload in the same `UPDATE` (the
    /// plan file carries its own name, which a rename must keep in step); `None` leaves
    /// it. The stored summary is kept.
    ///
    /// Returns how many rows changed: 0 when no row with `id` exists (for example it was
    /// deleted in another window), which is not an error here -- the caller decides what
    /// to tell the user.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn rename_saved_rough_plan(
        &self,
        id: i64,
        name: &str,
        payload: Option<&str>,
        now: i64,
    ) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE saved_rough_plans
                 SET name = ?1, updated_at = ?2, payload = COALESCE(?3, payload)
                 WHERE plan_id = ?4",
                params![name, now, payload, id],
            )
            .with_context(|| format!("Failed to rename saved rough plan for plan_id: {id}"))
    }

    /// Deletes a saved rough plan by its ID. Returns how many rows were deleted: 0 when
    /// no row with `id` exists, which is not an error here.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails.
    pub fn delete_saved_rough_plan(&self, id: i64) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM saved_rough_plans WHERE plan_id = ?1",
                params![id],
            )
            .with_context(|| format!("Failed to delete saved rough plan for plan_id: {id}"))
    }
}

/// A random stamp in `1..=STAMP_RANGE`, from the standard library's per-process random
/// hasher keys mixed with the clock.
fn random_stamp() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    hasher.write_u128(nanos);
    u32::try_from(hasher.finish() % STAMP_RANGE + 1).unwrap_or(1)
}

impl Database {
    /// This library's identity stamp: a random number kept in the database file's own
    /// `user_version`, created on first use and equal for every connection to the file
    /// (and for a copy of it). A saved plan written by this library carries it, so a
    /// later open can tell a plan from this library (whose entry ids are trustworthy)
    /// from one made elsewhere (whose ids may mean other designs here).
    ///
    /// # Errors
    ///
    /// Returns an error if the stamp cannot be read or, when there is none yet, written
    /// (a read-only connection).
    pub fn library_stamp(&self) -> Result<u32> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("Failed to read the library stamp")?;
        if let Ok(stamp) = u32::try_from(current)
            && stamp != 0
        {
            return Ok(stamp);
        }
        let fresh = random_stamp();
        self.conn
            .execute_batch(&format!("PRAGMA user_version = {fresh};"))
            .context("Failed to write the library stamp")?;
        Ok(fresh)
    }

    /// The title of every design in `ids` that exists, ignored designs included, keyed
    /// by id. An id with no design is absent from the map.
    ///
    /// # Errors
    ///
    /// Returns an error if a batch's `SELECT` fails.
    pub fn entry_titles_for(&self, ids: &[i64]) -> Result<BTreeMap<i64, String>> {
        let mut found = BTreeMap::new();
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "SELECT id, title FROM diagram_entries WHERE id IN ({placeholders})"
                ))
                .context("Failed to prepare the design title lookup")?;
            let rows = stmt
                .query_map(params_from_iter(chunk), |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .context("Failed to query design titles")?;
            for row in rows {
                let (id, title) = row.context("Failed to read a design title")?;
                found.insert(id, title);
            }
        }
        Ok(found)
    }

    /// The ids of the designs whose title equals each of `titles`, compared without outer
    /// spaces and without ASCII case, ignored designs included. The map is keyed by the
    /// trimmed lowercase (ASCII) title; a title nothing carries is absent, and each id
    /// list is ascending.
    ///
    /// # Errors
    ///
    /// Returns an error if a batch's `SELECT` fails.
    pub fn entry_ids_titled(&self, titles: &[String]) -> Result<BTreeMap<String, Vec<i64>>> {
        let mut found: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for chunk in titles.chunks(ID_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "SELECT id, title FROM diagram_entries
                     WHERE (trim(title) COLLATE NOCASE) IN ({placeholders})
                     ORDER BY id"
                ))
                .context("Failed to prepare the title lookup")?;
            let wanted = chunk.iter().map(|title| title.trim());
            let rows = stmt
                .query_map(params_from_iter(wanted), |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .context("Failed to query designs by title")?;
            for row in rows {
                let (id, title) = row.context("Failed to read a design row")?;
                found
                    .entry(title.trim().to_ascii_lowercase())
                    .or_default()
                    .push(id);
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::entry::FacetingDiagramEntry;

    fn temp_db() -> (Database, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-saved-plans-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        (db, path)
    }

    fn cleanup(db: Database, path: &std::path::Path) {
        drop(db);
        std::fs::remove_file(path).ok();
    }

    fn add_entry(db: &Database, title: &str) -> i64 {
        db.save_diagram_entry(
            &FacetingDiagramEntry {
                title: title.to_string(),
                url: format!("local://{title}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap()
    }

    #[test]
    fn crud_round_trip() {
        let (db, path) = temp_db();

        let id = db
            .save_rough_plan(
                "Plan Alpha",
                1,
                "payload_alpha",
                "Block · Quartz · 2 results",
                1_000,
            )
            .unwrap();

        let loaded = db.get_saved_rough_plan(id).unwrap().expect("plan exists");
        assert_eq!(loaded.meta.plan_id, id);
        assert_eq!(loaded.meta.name, "Plan Alpha");
        assert_eq!(loaded.meta.created_at, 1_000);
        assert_eq!(loaded.meta.updated_at, 1_000);
        assert_eq!(
            loaded.meta.summary.as_deref(),
            Some("Block · Quartz · 2 results")
        );
        assert_eq!(loaded.payload_version, 1);
        assert_eq!(loaded.payload, "payload_alpha");

        assert_eq!(db.delete_saved_rough_plan(id).unwrap(), 1);
        assert_eq!(db.get_saved_rough_plan(id).unwrap(), None);

        cleanup(db, &path);
    }

    #[test]
    fn newest_first_ordering_follows_the_creation_date() {
        let (db, path) = temp_db();

        let id1 = db.save_rough_plan("Oldest", 1, "p1", "s", 100).unwrap();
        let id2 = db.save_rough_plan("Middle", 1, "p2", "s", 200).unwrap();
        let id3 = db.save_rough_plan("Newest", 1, "p3", "s", 300).unwrap();

        let list = db.list_saved_rough_plans().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].plan_id, id3);
        assert_eq!(list[1].plan_id, id2);
        assert_eq!(list[2].plan_id, id1);

        // Same timestamp orders by plan_id DESC
        let id4 = db.save_rough_plan("Same Time", 1, "p4", "s", 200).unwrap();
        let list2 = db.list_saved_rough_plans().unwrap();
        assert_eq!(list2.len(), 4);
        assert_eq!(list2[0].plan_id, id3);
        assert_eq!(list2[1].plan_id, id4);
        assert_eq!(list2[2].plan_id, id2);
        assert_eq!(list2[3].plan_id, id1);

        // A rename does not move a plan: the list shows the creation date.
        db.rename_saved_rough_plan(id1, "Oldest, renamed", None, 900)
            .unwrap();
        let list3 = db.list_saved_rough_plans().unwrap();
        assert_eq!(list3[3].plan_id, id1);

        cleanup(db, &path);
    }

    #[test]
    fn rename_updates_name_payload_and_updated_at_and_keeps_the_summary() {
        let (db, path) = temp_db();

        let id = db.save_rough_plan("Initial", 1, "p", "kept", 100).unwrap();
        assert_eq!(
            db.rename_saved_rough_plan(id, "Renamed", Some("p2"), 500)
                .unwrap(),
            1
        );

        let loaded = db.get_saved_rough_plan(id).unwrap().unwrap();
        assert_eq!(loaded.meta.name, "Renamed");
        assert_eq!(loaded.payload, "p2");
        assert_eq!(loaded.meta.summary.as_deref(), Some("kept"));
        assert_eq!(loaded.meta.created_at, 100);
        assert_eq!(loaded.meta.updated_at, 500);

        // No payload given: the stored one stays.
        db.rename_saved_rough_plan(id, "Again", None, 600).unwrap();
        let loaded = db.get_saved_rough_plan(id).unwrap().unwrap();
        assert_eq!(
            (loaded.meta.name.as_str(), loaded.payload.as_str()),
            ("Again", "p2")
        );

        let list = db.list_saved_rough_plans().unwrap();
        assert_eq!(list[0].plan_id, id);
        assert_eq!(list[0].updated_at, 600);

        cleanup(db, &path);
    }

    #[test]
    fn a_missing_row_reports_zero_rows_changed() {
        let (db, path) = temp_db();
        assert_eq!(db.delete_saved_rough_plan(99_999).unwrap(), 0);
        assert_eq!(db.rename_saved_rough_plan(99_999, "x", None, 1).unwrap(), 0);
        assert_eq!(db.set_saved_rough_plan_summary(99_999, "s").unwrap(), 0);
        cleanup(db, &path);
    }

    #[test]
    fn a_summary_can_be_filled_in_later_without_touching_the_dates() {
        let (db, path) = temp_db();
        let id = db.save_rough_plan("Legacy", 1, "p", "first", 100).unwrap();
        // A row saved before summaries existed reads NULL.
        db.conn
            .execute(
                "UPDATE saved_rough_plans SET summary = NULL WHERE plan_id = ?1",
                params![id],
            )
            .unwrap();
        assert_eq!(db.list_saved_rough_plans().unwrap()[0].summary, None);

        assert_eq!(db.set_saved_rough_plan_summary(id, "later").unwrap(), 1);
        let meta = &db.list_saved_rough_plans().unwrap()[0];
        assert_eq!(meta.summary.as_deref(), Some("later"));
        assert_eq!(meta.updated_at, 100);

        cleanup(db, &path);
    }

    #[test]
    fn the_library_stamp_is_created_once_and_survives_reopening() {
        let (db, path) = temp_db();
        let stamp = db.library_stamp().unwrap();
        assert!((1..=0x7FFF_FFFE).contains(&stamp), "stamp {stamp}");
        assert_eq!(db.library_stamp().unwrap(), stamp);
        drop(db);
        let reopened = Database::new(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(reopened.library_stamp().unwrap(), stamp);
        cleanup(reopened, &path);
    }

    #[test]
    fn titles_are_looked_up_by_id_and_by_name_including_ignored_designs() {
        let (db, path) = temp_db();
        let a = add_entry(&db, "Barion Oval");
        let b = add_entry(&db, "  barion oval ");
        let c = add_entry(&db, "Emerald");
        db.set_diagram_ignored(c, true).unwrap();

        let titles = db.entry_titles_for(&[a, c, 9_999]).unwrap();
        assert_eq!(titles.get(&a).map(String::as_str), Some("Barion Oval"));
        assert_eq!(titles.get(&c).map(String::as_str), Some("Emerald"));
        assert!(!titles.contains_key(&9_999));

        let found = db
            .entry_ids_titled(&[
                " BARION oval".to_string(),
                "emerald".to_string(),
                "none".to_string(),
            ])
            .unwrap();
        assert_eq!(found.get("barion oval"), Some(&vec![a, b]));
        assert_eq!(
            found.get("emerald"),
            Some(&vec![c]),
            "ignored designs count"
        );
        assert!(!found.contains_key("none"));

        cleanup(db, &path);
    }
}
