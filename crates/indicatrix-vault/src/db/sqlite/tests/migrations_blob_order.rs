//! The blob-columns-last rebuild migrations (`diagram_details`, `diagram_previews`,
//! `diagram_tilt_curves`): column order after the rebuild, byte-for-byte row
//! preservation, child rows, the AUTOINCREMENT sequence and idempotence -- proven
//! against a fixture seeded in the pre-rebuild column order.

use super::{
    super::*,
    fixtures::{seed_pre_migration_db, temp_db_path},
};

/// Builds a fresh database file at a pre-fix, blob-first shape for
/// `diagram_details` (via the standard [`seed_pre_migration_db`] fixture, plus one
/// `angle_settings` pair and one `attached_files` row) AND `diagram_previews`/
/// `diagram_tilt_curves` (seeded directly here in their own historical blob-first
/// column order -- `create_tables_if_not_exist` only ever runs `CREATE TABLE IF NOT
/// EXISTS`, so a table already present when `Database::new` runs below is left exactly
/// as seeded until `Database::migrate_blob_columns_last` reshapes it). One design,
/// `entry_id`/`diagram_previews.entry_id`/`diagram_tilt_curves.entry_id` all `1`.
/// Shared by every `migrate_blob_columns_last_*` test in this file, so each can assert
/// one aspect of the rebuild without repeating this setup.
fn seed_blob_columns_last_fixture(label: &str) -> std::path::PathBuf {
    let path = temp_db_path(label);
    seed_pre_migration_db(
        &path,
        &[(
            "Solo",
            "url-solo",
            Some("1.62"),
            Some("1.1"),
            Some("0.3"),
            Some("96"),
            Some("55+6"),
        )],
    );

    let conn = Connection::open(&path).unwrap();
    let detail_id: i64 = conn
        .query_row("SELECT id FROM diagram_details", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        "INSERT INTO angle_settings (detail_id, order_idx, facet, angle, index_val, notes)
         VALUES (?1, 0, 'P1', '41', '0', ''), (?1, 1, 'P2', '41', '24', '')",
        params![detail_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attached_files (detail_id, name, url, content)
         VALUES (?1, 'solo.asc', '', X'01020304')",
        params![detail_id],
    )
    .unwrap();

    conn.execute_batch(
        "CREATE TABLE diagram_previews (
            entry_id INTEGER PRIMARY KEY,
            preview_front BLOB,
            preview_top BLOB,
            preview_material TEXT,
            preview_generated_at INTEGER,
            FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
        );
        CREATE TABLE diagram_tilt_curves (
            entry_id INTEGER PRIMARY KEY,
            curves BLOB,
            curve_image BLOB,
            generated_at INTEGER,
            perf_brilliance_global_min REAL,
            perf_brilliance_global_max REAL,
            perf_extinction_global_min REAL,
            perf_extinction_global_max REAL,
            perf_windowing_global_min REAL,
            perf_windowing_global_max REAL,
            FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO diagram_previews (entry_id, preview_front, preview_top, preview_material, preview_generated_at)
         VALUES (1, X'0102', X'0304', 'Quartz', 1700000000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO diagram_tilt_curves (entry_id, curves, curve_image, generated_at,
            perf_brilliance_global_min, perf_brilliance_global_max,
            perf_extinction_global_min, perf_extinction_global_max,
            perf_windowing_global_min, perf_windowing_global_max)
         VALUES (1, X'0506', X'0708', 1700000001, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0)",
        [],
    )
    .unwrap();
    drop(conn);
    path
}

/// The physical column order of `table` in `db`, via `PRAGMA table_info`.
fn column_order(db: &Database, table: &str) -> Vec<String> {
    let mut stmt = db
        .conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>("name"))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// `diagram_image_data` must end up as `diagram_details`' LAST column.
#[test]
fn migrate_blob_columns_last_moves_diagram_image_data_last() {
    let path = seed_blob_columns_last_fixture("blob_last_details_order");
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    let names = column_order(&db, "diagram_details");
    assert_eq!(names.last().map(String::as_str), Some("diagram_image_data"));
    let _ = std::fs::remove_file(&path);
}

/// `preview_material`/`preview_generated_at` must end up before the
/// `preview_front`/`preview_top` blobs.
#[test]
fn migrate_blob_columns_last_moves_preview_material_before_the_blobs() {
    let path = seed_blob_columns_last_fixture("blob_last_previews_order");
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    let names = column_order(&db, "diagram_previews");
    let material_pos = names.iter().position(|n| n == "preview_material").unwrap();
    let front_pos = names.iter().position(|n| n == "preview_front").unwrap();
    assert!(material_pos < front_pos);
    let _ = std::fs::remove_file(&path);
}

/// `generated_at`/the derived `perf_*` columns must end up before the
/// `curves`/`curve_image` blobs.
#[test]
fn migrate_blob_columns_last_moves_generated_at_before_the_curve_blobs() {
    let path = seed_blob_columns_last_fixture("blob_last_tilt_curves_order");
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    let names = column_order(&db, "diagram_tilt_curves");
    let generated_pos = names.iter().position(|n| n == "generated_at").unwrap();
    let curves_pos = names.iter().position(|n| n == "curves").unwrap();
    assert!(generated_pos < curves_pos);
    let _ = std::fs::remove_file(&path);
}

/// every byte of every rebuilt row must survive -- the preview images/material/
/// timestamp, the tilt-curve image, and the detail row's own data.
#[test]
fn migrate_blob_columns_last_preserves_every_rebuilt_row_byte_for_byte() {
    let path = seed_blob_columns_last_fixture("blob_last_data_survives");
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");

    let preview = db.get_preview_images(1).unwrap();
    assert_eq!(preview.front, Some(vec![1, 2]));
    assert_eq!(preview.top, Some(vec![3, 4]));
    assert_eq!(preview.material.as_deref(), Some("Quartz"));
    assert_eq!(preview.generated_at, Some(1_700_000_000));
    let sql = "SELECT curve_image FROM diagram_tilt_curves";
    let image: Option<Vec<u8>> = db.conn.query_row(sql, [], |r| r.get(0)).unwrap();
    assert_eq!(image, Some(vec![7, 8]));
    assert!(db.has_tilt_curves(1).unwrap());

    let full = db.get_diagram_full(1).unwrap().unwrap();
    assert_eq!(full.title, "Solo");
    let ri: f64 = full
        .refractive_index
        .as_deref()
        .expect("refractive_index must survive the rebuild")
        .parse()
        .unwrap();
    assert!((ri - 1.62).abs() < 1e-9);

    let _ = std::fs::remove_file(&path);
}

/// House rule: rebuilding `diagram_details` must never cascade-delete its
/// `angle_settings`/`attached_files` children -- exactly the wipe a `DROP TABLE
/// diagram_details` with `foreign_keys = ON` would otherwise cause.
#[test]
fn migrate_blob_columns_last_never_loses_child_rows() {
    let path = seed_blob_columns_last_fixture("blob_last_child_rows");
    let (angle_count_before, attached_count_before): (i64, i64) = {
        let conn = Connection::open(&path).unwrap();
        (
            conn.query_row("SELECT COUNT(*) FROM angle_settings", [], |r| r.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM attached_files", [], |r| r.get(0))
                .unwrap(),
        )
    };
    assert_eq!(
        angle_count_before, 2,
        "fixture must have seeded 2 angle rows"
    );
    assert_eq!(
        attached_count_before, 1,
        "fixture must have seeded 1 attachment row"
    );

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    let angle_count_after: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM angle_settings", [], |r| r.get(0))
        .unwrap();
    let attached_count_after: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM attached_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(angle_count_before, angle_count_after);
    assert_eq!(attached_count_before, attached_count_after);

    let _ = std::fs::remove_file(&path);
}

/// The rebuilt `diagram_details`' `AUTOINCREMENT` sequence must still hand out fresh,
/// never-reused ids afterward -- not reset to the highest surviving id (or `1`) by the
/// rebuild's own `CREATE TABLE`/`DROP TABLE`/`RENAME` sequence bookkeeping. The fixture
/// advances the sequence past the highest live id first (a row inserted at id 50 and
/// deleted), so a rebuild that merely recomputes the sequence from the surviving rows
/// yields 1, not 50.
#[test]
fn migrate_blob_columns_last_preserves_the_autoincrement_sequence() {
    const ADVANCED_SEQUENCE: i64 = 50;
    let path = seed_blob_columns_last_fixture("blob_last_autoincrement");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO diagram_entries (title, url) VALUES ('Scratch', 'url-scratch')",
            [],
        )
        .unwrap();
        let scratch_entry: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO diagram_details (id, entry_id, page_url) VALUES (?1, ?2, '')",
            params![ADVANCED_SEQUENCE, scratch_entry],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM diagram_details WHERE id = ?1",
            params![ADVANCED_SEQUENCE],
        )
        .unwrap();
        let seeded: i64 = conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'diagram_details'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            seeded, ADVANCED_SEQUENCE,
            "fixture must advance the sequence"
        );
    }

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");

    let old_max_id: i64 = db
        .conn
        .query_row("SELECT MAX(id) FROM diagram_details", [], |r| r.get(0))
        .unwrap();
    assert_eq!(old_max_id, 1, "only the original detail row survives");
    let sequence_after: i64 = db
        .conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'diagram_details'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        sequence_after, ADVANCED_SEQUENCE,
        "the rebuild must carry the AUTOINCREMENT sequence over"
    );

    db.save_diagram_detail(&crate::model::detail::FacetingDiagramDetail::default(), 1)
        .expect("save_diagram_detail must still work after the rebuild");
    let new_detail_id: i64 = db
        .conn
        .query_row(
            "SELECT id FROM diagram_details WHERE entry_id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        new_detail_id,
        ADVANCED_SEQUENCE + 1,
        "the next insert must get the carried-over sequence + 1"
    );

    let _ = std::fs::remove_file(&path);
}

/// opening a database twice must not re-shuffle anything the second time, and
/// must not error.
#[test]
fn migrate_blob_columns_last_is_idempotent_across_a_second_open() {
    let path = seed_blob_columns_last_fixture("blob_last_idempotent");
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    drop(db);

    let db2 = Database::new(Some(path.to_str().unwrap())).expect("reopen + re-migrate (no-op)");
    let preview2 = db2.get_preview_images(1).unwrap();
    assert_eq!(preview2.material.as_deref(), Some("Quartz"));
    assert_eq!(
        column_order(&db2, "diagram_details")
            .last()
            .map(String::as_str),
        Some("diagram_image_data")
    );

    let _ = std::fs::remove_file(&path);
}

/// An older build autocommitted each `ALTER` of the proportions batch, so a crash could
/// leave only `hw_ratio`: the next open must add the other six, not skip the batch.
#[test]
fn a_partly_applied_proportions_migration_is_completed_on_open() {
    let path = temp_db_path("partial_proportions");
    seed_pre_migration_db(&path, &[("A", "url-a", None, None, None, None, None)]);
    Connection::open(&path)
        .unwrap()
        .execute_batch("ALTER TABLE diagram_details ADD COLUMN hw_ratio REAL;")
        .unwrap();

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    let columns = column_order(&db, "diagram_details");
    assert!(
        columns.iter().any(|c| c == "mirror_symmetry"),
        "{columns:?}"
    );
    assert_eq!(columns.len(), 27, "{columns:?}");
    let _ = std::fs::remove_file(&path);
}
