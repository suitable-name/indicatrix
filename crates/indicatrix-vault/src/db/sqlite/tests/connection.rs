//! Connection-level behaviour: WAL/`synchronous` pragmas on a temp-file database,
//! `open_read_only` against a WAL database, and that `:memory:` still migrates
//! correctly.

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

/// The writer stays open across the read-only open, so the committed row lives in the
/// `-wal` file (a closing writer checkpoints and removes it) and the reader must go
/// through the WAL machinery to see it.
#[test]
fn open_read_only_succeeds_on_a_wal_db_after_a_write() {
    let path = temp_db_path("wal_read_only");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    // A real write beyond schema creation, so there's committed WAL content to
    // read back.
    db.conn
        .execute(
            "INSERT INTO diagram_entries (title, url) VALUES ('t', 'u')",
            [],
        )
        .expect("insert a row");

    let wal_path = path.with_extension("sqlite-wal");
    assert!(
        wal_path.exists(),
        "the live writer must have left a -wal file at {}",
        wal_path.display()
    );

    let ro = Database::open_read_only(path.to_str().unwrap())
        .expect("open_read_only should succeed against a WAL database with a live writer");
    let count: i64 = ro
        .conn
        .query_row("SELECT COUNT(*) FROM diagram_entries", [], |row| row.get(0))
        .expect("read back the row count");
    assert_eq!(count, 1);

    drop(ro);
    drop(db);
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
}

/// `open_read_only`'s `immutable=1` fallback must only ever be attempted for the
/// specific SQLite result codes its doc comment names -- any other failure means the
/// fallback would not help.
#[test]
fn is_cantopen_or_readonly_matches_only_those_two_codes() {
    let cantopen = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
        None,
    );
    let readonly = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY),
        None,
    );
    let busy =
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None);
    assert!(is_cantopen_or_readonly(&cantopen));
    assert!(is_cantopen_or_readonly(&readonly));
    assert!(
        !is_cantopen_or_readonly(&busy),
        "a busy/locked failure must not trigger the immutable=1 fallback"
    );
}

/// the `immutable=1` fallback must be gated on there being no pending `-wal`
/// writes -- a nonexistent or zero-length `-wal` file both count as "none pending".
#[test]
fn has_pending_wal_file_detects_only_a_nonempty_wal_sibling() {
    let path = temp_db_path("pending_wal_detection");
    let base = path.to_str().unwrap();

    assert!(
        !has_pending_wal_file(base),
        "no -wal file at all must count as no pending writes"
    );

    let wal_path = format!("{base}-wal");
    std::fs::write(&wal_path, b"").unwrap();
    assert!(
        !has_pending_wal_file(base),
        "an empty -wal file must count as no pending writes"
    );

    std::fs::write(&wal_path, [1u8, 2, 3, 4]).unwrap();
    assert!(
        has_pending_wal_file(base),
        "a nonempty -wal file must count as pending writes"
    );

    let _ = std::fs::remove_file(&wal_path);
}
