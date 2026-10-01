//! Storage for `crate::model::mirror::MirrorState` -- see that module's doc comment for
//! why this table exists and why it's keyed by `url`.

use super::Database;
use crate::model::mirror::MirrorState;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use tracing::{debug, info};

impl Database {
    /// Adds `library_mirror_state.deleted_locally` (`INTEGER NOT NULL DEFAULT 0`) to a
    /// database created before mirror tombstones existed -- see
    /// [`Self::delete_diagram_entry`] for what sets it.
    ///
    /// Every pre-existing row whose design is already gone from `diagram_entries` was
    /// deleted locally under the old "keep the row, compare the hashes" rule, so the
    /// same transaction tombstones them: without that backfill, such a design would be
    /// re-created the first time its remote summary changed. Idempotent: gated on
    /// whether the column already exists (a fresh database declares it in
    /// `create_tables_if_not_exist`).
    ///
    /// # Errors
    ///
    /// Returns an error if probing the column, adding it or backfilling fails; nothing
    /// is committed in that case.
    pub(super) fn migrate_mirror_state_tombstone_column(&self) -> Result<()> {
        if Self::column_exists(&self.conn, "library_mirror_state", "deleted_locally")? {
            debug!("library_mirror_state.deleted_locally already present; skipping.");
            return Ok(());
        }

        info!("Adding library_mirror_state.deleted_locally column...");
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the mirror-state tombstone migration")?;
        tx.execute_batch(
            "ALTER TABLE library_mirror_state
             ADD COLUMN deleted_locally INTEGER NOT NULL DEFAULT 0;",
        )
        .context("Failed to add library_mirror_state.deleted_locally")?;
        tx.execute(
            "UPDATE library_mirror_state SET deleted_locally = 1
             WHERE NOT EXISTS (
                 SELECT 1 FROM diagram_entries de WHERE de.url = library_mirror_state.url
             )",
            [],
        )
        .context("Failed to tombstone the mirror states of already-deleted designs")?;
        tx.commit()
            .context("Failed to commit the mirror-state tombstone migration")?;
        info!("library_mirror_state.deleted_locally migration complete.");
        Ok(())
    }

    /// Looks up the last-synced remote content hashes for `url`, or `None` if this
    /// design has never been synced from a remote library into this database.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails, or if the stored row holds a
    /// hash that is not exactly 32 bytes (a hand-edited or foreign row): such a row
    /// must not read back as a plausible state that could compare equal to a real hash.
    pub fn get_mirror_state(&self, url: &str) -> Result<Option<MirrorState>> {
        self.conn
            .query_row(
                "SELECT url, source_id, summary_version, design_version, deleted_locally
                 FROM library_mirror_state WHERE url = ?1",
                params![url],
                |row| {
                    let summary: Vec<u8> = row.get(2)?;
                    let design: Vec<u8> = row.get(3)?;
                    let (Some(summary_version), Some(design_version)) =
                        (hash_from_blob(&summary), hash_from_blob(&design))
                    else {
                        return Err(rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Blob,
                            "a mirror-state hash must be exactly 32 bytes".into(),
                        ));
                    };
                    Ok(MirrorState {
                        url: row.get(0)?,
                        source_id: row.get(1)?,
                        summary_version,
                        design_version,
                        deleted_locally: row.get(4)?,
                    })
                },
            )
            .optional()
            .context(format!("Failed to load mirror state for url: {url}"))
    }

    /// Records `state` as the last-synced remote content hashes for `state.url`,
    /// overwriting whatever (if anything) was there before -- one row per design,
    /// always reflecting only the most recent sync. `state.deleted_locally` is written
    /// too, so a caller that re-mirrors a design passes `false`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying upsert fails.
    pub fn upsert_mirror_state(&self, state: &MirrorState) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO library_mirror_state
                    (url, source_id, summary_version, design_version, deleted_locally)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(url) DO UPDATE SET
                    source_id = excluded.source_id,
                    summary_version = excluded.summary_version,
                    design_version = excluded.design_version,
                    deleted_locally = excluded.deleted_locally",
                params![
                    state.url,
                    state.source_id,
                    state.summary_version.to_vec(),
                    state.design_version.to_vec(),
                    state.deleted_locally,
                ],
            )
            .context(format!(
                "Failed to upsert mirror state for url: {}",
                state.url
            ))?;
        Ok(())
    }

    /// How many `library_mirror_state` rows now have no matching `diagram_entries.url`
    /// -- designs the user deleted locally after they were pulled from a remote
    /// mirror. `library_mirror_state` deliberately has no `FOREIGN KEY`/
    /// `ON DELETE CASCADE` back to `diagram_entries` (see this module's doc comment),
    /// so `Database::delete_diagram_entry` keeps the row and tombstones it
    /// (`deleted_locally`) -- see that method's own doc comment for why that's the
    /// intended "local delete wins" semantics, not an oversight. This count is what
    /// lets a sync UI surface "N designs skipped" instead of that orphaning being
    /// entirely invisible.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn count_mirror_states_without_entry(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM library_mirror_state m
                 WHERE NOT EXISTS (
                     SELECT 1 FROM diagram_entries de WHERE de.url = m.url
                 )",
                [],
                |row| row.get(0),
            )
            .context("Failed to count mirror states without a matching diagram entry")?;
        // A SQL COUNT(*) is never negative (workspace-wide `cast_sign_loss` = "allow").
        Ok(count as u64)
    }
}

/// `library_mirror_state.summary_version`/`design_version` are always written as exactly
/// 32 bytes by [`Database::upsert_mirror_state`] (a `[u8; 32]` SHA-256 digest). Any other
/// length (a hand-edited or foreign row) is `None`, never padded or truncated into a
/// hash that could accidentally compare equal to a real one.
fn hash_from_blob(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Database, std::path::PathBuf) {
        // A plain `process::id() + nanos` name (as several sibling crates' tests use)
        // can collide when two tests in this same file happen to build their names in
        // the same clock tick under `cargo test`'s default parallel-thread execution --
        // observed in practice here. An additional monotonic counter makes every call
        // within this process unique regardless of clock resolution.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-mirror-state-test-{}-{}-{n}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        (db, path)
    }

    #[test]
    fn unknown_url_has_no_mirror_state() {
        let (db, path) = temp_db();
        assert_eq!(db.get_mirror_state("https://example.test/1").unwrap(), None);
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let (db, path) = temp_db();
        let state = MirrorState {
            url: "https://example.test/1".to_string(),
            source_id: "worker.local:9443".to_string(),
            summary_version: [1u8; 32],
            design_version: [2u8; 32],
            deleted_locally: false,
        };
        db.upsert_mirror_state(&state).unwrap();
        let loaded = db.get_mirror_state(&state.url).unwrap().unwrap();
        assert_eq!(loaded, state);
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn count_mirror_states_without_entry_counts_only_orphaned_rows() {
        let (db, path) = temp_db();

        // A mirrored design still present locally: not orphaned.
        let entry_id = db
            .save_diagram_entry(
                &crate::model::entry::FacetingDiagramEntry {
                    title: "Still Here".to_string(),
                    url: "https://example.test/still-here".to_string(),
                    design_id: String::new(),
                },
                "worker.local:9443",
            )
            .unwrap();
        db.upsert_mirror_state(&MirrorState {
            url: "https://example.test/still-here".to_string(),
            source_id: "worker.local:9443".to_string(),
            summary_version: [1u8; 32],
            design_version: [1u8; 32],
            deleted_locally: false,
        })
        .unwrap();

        // A mirrored design the user already deleted locally: mirror state survives
        // the delete (see `Database::delete_diagram_entry`'s doc comment), so it's
        // orphaned from the start.
        db.upsert_mirror_state(&MirrorState {
            url: "https://example.test/deleted".to_string(),
            source_id: "worker.local:9443".to_string(),
            summary_version: [2u8; 32],
            design_version: [2u8; 32],
            deleted_locally: true,
        })
        .unwrap();

        assert_eq!(db.count_mirror_states_without_entry().unwrap(), 1);

        db.delete_diagram_entry(entry_id).unwrap();
        assert_eq!(
            db.count_mirror_states_without_entry().unwrap(),
            2,
            "deleting the still-here design must orphan its mirror state too -- local \
             delete wins, the row is never cleaned up automatically"
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn upsert_overwrites_the_previous_state_for_the_same_url() {
        let (db, path) = temp_db();
        let url = "https://example.test/1".to_string();
        db.upsert_mirror_state(&MirrorState {
            url: url.clone(),
            source_id: "worker.local:9443".to_string(),
            summary_version: [1u8; 32],
            design_version: [1u8; 32],
            deleted_locally: false,
        })
        .unwrap();
        db.upsert_mirror_state(&MirrorState {
            url: url.clone(),
            source_id: "worker.local:9443".to_string(),
            summary_version: [9u8; 32],
            design_version: [9u8; 32],
            deleted_locally: false,
        })
        .unwrap();
        let loaded = db.get_mirror_state(&url).unwrap().unwrap();
        assert_eq!(loaded.summary_version, [9u8; 32]);
        assert_eq!(loaded.design_version, [9u8; 32]);
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn deleting_a_mirrored_entry_tombstones_its_state_and_an_unmirrored_one_leaves_none() {
        let (db, path) = temp_db();
        let mirrored_url = "https://example.test/mirrored";
        let entry = |title: &str, url: &str| crate::model::entry::FacetingDiagramEntry {
            title: title.to_string(),
            url: url.to_string(),
            design_id: String::new(),
        };
        let mirrored_id = db
            .save_diagram_entry(&entry("Mirrored", mirrored_url), "worker.local:9443")
            .unwrap();
        db.upsert_mirror_state(&MirrorState {
            url: mirrored_url.to_string(),
            source_id: "worker.local:9443".to_string(),
            summary_version: [1u8; 32],
            design_version: [1u8; 32],
            deleted_locally: false,
        })
        .unwrap();
        let local_id = db
            .save_diagram_entry(&entry("Local", "local://mine.asc"), "local-import")
            .unwrap();

        db.delete_diagram_entry(mirrored_id).unwrap();
        db.delete_diagram_entry(local_id).unwrap();

        let tombstone = db.get_mirror_state(mirrored_url).unwrap().unwrap();
        assert!(tombstone.deleted_locally);
        assert_eq!(tombstone.summary_version, [1u8; 32]);
        assert_eq!(db.get_mirror_state("local://mine.asc").unwrap(), None);

        // Re-mirroring writes the flag back to false.
        db.upsert_mirror_state(&MirrorState {
            deleted_locally: false,
            ..tombstone
        })
        .unwrap();
        assert!(
            !db.get_mirror_state(mirrored_url)
                .unwrap()
                .unwrap()
                .deleted_locally
        );
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_short_stored_hash_is_an_error_not_a_zero_padded_state() {
        let (db, path) = temp_db();
        db.conn
            .execute(
                "INSERT INTO library_mirror_state (url, source_id, summary_version, design_version)
                 VALUES ('https://example.test/short', 'w', X'0102', X'0304')",
                [],
            )
            .unwrap();
        assert!(db.get_mirror_state("https://example.test/short").is_err());
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_tombstone_migration_flags_states_whose_design_is_already_gone() {
        let (db, path) = temp_db();
        let mirror = |url: &str| MirrorState {
            url: url.to_string(),
            source_id: "w".to_string(),
            summary_version: [1u8; 32],
            design_version: [1u8; 32],
            deleted_locally: false,
        };
        db.save_diagram_entry(
            &crate::model::entry::FacetingDiagramEntry {
                title: "Kept".to_string(),
                url: "https://example.test/kept".to_string(),
                design_id: String::new(),
            },
            "w",
        )
        .unwrap();
        db.upsert_mirror_state(&mirror("https://example.test/kept"))
            .unwrap();
        db.upsert_mirror_state(&mirror("https://example.test/gone"))
            .unwrap();

        // Rewind to the pre-tombstone schema, then let the migration run again.
        db.conn
            .execute_batch("ALTER TABLE library_mirror_state DROP COLUMN deleted_locally;")
            .unwrap();
        db.migrate_mirror_state_tombstone_column().unwrap();

        let kept = db
            .get_mirror_state("https://example.test/kept")
            .unwrap()
            .unwrap();
        let gone = db
            .get_mirror_state("https://example.test/gone")
            .unwrap()
            .unwrap();
        assert!(!kept.deleted_locally);
        assert!(gone.deleted_locally);
        drop(db);
        std::fs::remove_file(&path).ok();
    }
}
