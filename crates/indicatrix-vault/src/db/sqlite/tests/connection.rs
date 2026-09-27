//! Connection-level behaviour: WAL/`synchronous` pragmas on a temp-file database,
//! `open_read_only` against a WAL database, `checkpoint`'s truncation (and its
//! no-op on a non-WAL database), and that `:memory:` still migrates correctly.

use super::{super::*, fixtures::temp_db_path};

/// Removes `path` plus its `-wal`/`-shm` siblings, if any -- the cleanup a WAL-mode
/// temp-db test needs beyond the plain `std::fs::remove_file(&path)` every other test
/// here uses (a non-WAL database never has those siblings, so this is safe to call
/// unconditionally).
fn remove_db_and_wal_files(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn wal_is_enabled_on_a_temp_file_db() {
    let path = temp_db_path("wal_enabled");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");

    let journal_mode: String = db
        .conn
        .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
        .expect("read journal_mode");
    assert_eq!(
        journal_mode.to_ascii_lowercase(),
        "wal",
        "Database::new should enable WAL on a temp-file database"
    );

    let synchronous: i64 = db
        .conn
        .query_row("PRAGMA synchronous;", [], |row| row.get(0))
        .expect("read synchronous");
    // SQLite reports `synchronous` back as an integer: 0=OFF, 1=NORMAL, 2=FULL.
    assert_eq!(
        synchronous, 1,
        "Database::new should set synchronous=NORMAL"
    );

    remove_db_and_wal_files(&path);
}

#[test]
fn open_read_only_succeeds_on_a_wal_db_after_a_write() {
    let path = temp_db_path("wal_read_only");
    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
        // A real write beyond schema creation, so there's committed WAL content to
        // read back.
        db.conn
            .execute(
                "INSERT INTO diagram_entries (title, url) VALUES ('t', 'u')",
                [],
            )
            .expect("insert a row");
    }

    let ro = Database::open_read_only(path.to_str().unwrap())
        .expect("open_read_only should succeed against a WAL database");
    let count: i64 = ro
        .conn
        .query_row("SELECT COUNT(*) FROM diagram_entries", [], |row| row.get(0))
        .expect("read back the row count");
    assert_eq!(count, 1);

    remove_db_and_wal_files(&path);
}

#[test]
fn checkpoint_truncates_the_wal_file() {
    let path = temp_db_path("wal_checkpoint");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    db.conn
        .execute(
            "INSERT INTO diagram_entries (title, url) VALUES ('t', 'u')",
            [],
        )
        .expect("insert a row");

    db.checkpoint().expect("checkpoint should succeed");

    let wal_path = path.with_extension("sqlite-wal");
    let wal_len = std::fs::metadata(&wal_path).map_or(0, |m| m.len());
    assert_eq!(
        wal_len, 0,
        "PRAGMA wal_checkpoint(TRUNCATE) should truncate the -wal file back to empty"
    );

    remove_db_and_wal_files(&path);
}

#[test]
fn memory_db_still_works() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");

    // `:memory:` always reports `journal_mode` as `memory` and cannot be changed --
    // `Database::new` must not have failed trying.
    let journal_mode: String = db
        .conn
        .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
        .expect("read journal_mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "memory");

    // Schema creation/migration still ran against it.
    assert!(Database::column_exists(&db.conn, "diagram_entries", "source_id").unwrap());

    // `checkpoint` on a non-WAL (here, in-memory) database is documented as a no-op,
    // not an error.
    db.checkpoint()
        .expect("checkpoint should be a no-op, not an error, on a non-WAL database");
}
