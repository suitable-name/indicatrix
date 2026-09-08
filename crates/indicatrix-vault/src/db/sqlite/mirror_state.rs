//! Storage for `crate::model::mirror::MirrorState` -- see that module's doc comment for
//! why this table exists and why it's keyed by `url`.

use super::Database;
use crate::model::mirror::MirrorState;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

impl Database {
    /// Looks up the last-synced remote content hashes for `url`, or `None` if this
    /// design has never been synced from a remote library into this database.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn get_mirror_state(&self, url: &str) -> Result<Option<MirrorState>> {
        self.conn
            .query_row(
                "SELECT url, source_id, summary_version, design_version
                 FROM library_mirror_state WHERE url = ?1",
                params![url],
                |row| {
                    let summary: Vec<u8> = row.get(2)?;
                    let design: Vec<u8> = row.get(3)?;
                    Ok(MirrorState {
                        url: row.get(0)?,
                        source_id: row.get(1)?,
                        summary_version: hash_from_blob(&summary),
                        design_version: hash_from_blob(&design),
                    })
                },
            )
            .optional()
            .context(format!("Failed to load mirror state for url: {url}"))
    }

    /// Records `state` as the last-synced remote content hashes for `state.url`,
    /// overwriting whatever (if anything) was there before -- one row per design,
    /// always reflecting only the most recent sync.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying upsert fails.
    pub fn upsert_mirror_state(&self, state: &MirrorState) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO library_mirror_state (url, source_id, summary_version, design_version)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(url) DO UPDATE SET
                    source_id = excluded.source_id,
                    summary_version = excluded.summary_version,
                    design_version = excluded.design_version",
                params![
                    state.url,
                    state.source_id,
                    state.summary_version.to_vec(),
                    state.design_version.to_vec(),
                ],
            )
            .context(format!(
                "Failed to upsert mirror state for url: {}",
                state.url
            ))?;
        Ok(())
    }
}

/// `library_mirror_state.summary_version`/`design_version` are always written as exactly
/// 32 bytes by [`Database::upsert_mirror_state`] (a `[u8; 32]` SHA-256 digest) -- this
/// only guards against a hand-edited or foreign row, never truncating/panicking on one.
fn hash_from_blob(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    out
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
        };
        db.upsert_mirror_state(&state).unwrap();
        let loaded = db.get_mirror_state(&state.url).unwrap().unwrap();
        assert_eq!(loaded, state);
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
        })
        .unwrap();
        db.upsert_mirror_state(&MirrorState {
            url: url.clone(),
            source_id: "worker.local:9443".to_string(),
            summary_version: [9u8; 32],
            design_version: [9u8; 32],
        })
        .unwrap();
        let loaded = db.get_mirror_state(&url).unwrap().unwrap();
        assert_eq!(loaded.summary_version, [9u8; 32]);
        assert_eq!(loaded.design_version, [9u8; 32]);
        drop(db);
        std::fs::remove_file(&path).ok();
    }
}
