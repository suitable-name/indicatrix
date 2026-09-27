//! Table/index/vocabulary-level migrations: shape-vocabulary seeding, the `ignored`
//! column plus the `diagram_previews`/`diagram_tilt_curves` side tables, the
//! tilt-curve aggregate-column pruning, and the [`sql_identifier`] guard they (and the
//! column migrations in [`super::migrations`]) all rely on.

use super::{
    super::*,
    fixtures::{seed_pre_migration_db, temp_db_path},
};

#[test]
fn a_fresh_database_has_the_full_seeded_shape_vocabulary() {
    let path = temp_db_path("fresh_shape_vocab");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");

    let names: Vec<String> = {
        let mut stmt = db
            .conn
            .prepare("SELECT name FROM shape_vocabulary ORDER BY sort_order ASC")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .flatten()
            .collect()
    };
    assert_eq!(
        names,
        DEFAULT_SHAPES
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>(),
        "a fresh database must be seeded with the full canonical list, in order"
    );

    // Before any design is imported, get_unique_shapes must still return the whole
    // seeded vocabulary (the bug fixed here: a plain SELECT DISTINCT returned nothing).
    let shapes = db.get_unique_shapes().unwrap();
    let mut expected: Vec<String> = DEFAULT_SHAPES.iter().map(|s| (*s).to_string()).collect();
    expected.sort();
    assert_eq!(shapes, expected);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn shape_vocabulary_migration_is_idempotent_across_two_opens() {
    let path = temp_db_path("shape_vocab_idempotent");

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open seeds");
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM shape_vocabulary", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, DEFAULT_SHAPES.len() as i64);
    }

    // Second open must not duplicate rows: INSERT OR IGNORE skips existing names.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let count: i64 = db2
        .conn
        .query_row("SELECT COUNT(*) FROM shape_vocabulary", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, DEFAULT_SHAPES.len() as i64);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn shape_vocabulary_migration_never_overwrites_or_drops_existing_rows() {
    let path = temp_db_path("shape_vocab_preserves_data");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");

    // Simulate a user/GUI adding a custom entry and re-pointing a seeded sort_order.
    db.conn
        .execute(
            "INSERT INTO shape_vocabulary (name, sort_order) VALUES ('Portuguese', 999)",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE shape_vocabulary SET sort_order = 12345 WHERE name = 'Round'",
            [],
        )
        .unwrap();

    // Re-running the migration must not delete the custom row or reset sort_order.
    db.migrate_shape_vocabulary().expect("re-run migration");

    let custom_present: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM shape_vocabulary WHERE name = 'Portuguese'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(custom_present, 1, "custom row must survive a re-seed");

    let round_order: i64 = db
        .conn
        .query_row(
            "SELECT sort_order FROM shape_vocabulary WHERE name = 'Round'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        round_order, 12345,
        "re-seeding must not overwrite an existing row's data"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_fresh_database_already_has_the_ignored_column_and_the_two_new_side_tables() {
    let path = temp_db_path("fresh_ignored_and_side_tables");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert!(Database::column_exists(&db.conn, "diagram_entries", "ignored").unwrap());

    let table_names: Vec<String> = {
        let mut stmt = db
            .conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name IN ('diagram_previews', 'diagram_tilt_curves')
                 ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .flatten()
            .collect()
    };
    assert_eq!(
        table_names,
        vec![
            "diagram_previews".to_string(),
            "diagram_tilt_curves".to_string()
        ],
        "a fresh database's CREATE TABLE must already include both new side tables"
    );

    // All 6 generated global-extreme columns must exist, none of the 30 obsolete ones.
    for (metric, extreme) in crate::model::performance::all_global_extreme_columns() {
        let column = crate::model::performance::global_extreme_column_name(metric, extreme);
        assert!(
            Database::column_exists(&db.conn, "diagram_tilt_curves", &column).unwrap(),
            "fresh diagram_tilt_curves must already have column {column}"
        );
    }
    assert!(
        !Database::column_exists(&db.conn, "diagram_tilt_curves", "perf_brilliance_15_min")
            .unwrap(),
        "a fresh database must never have the obsolete first-draft per-radius columns"
    );

    let _ = std::fs::remove_file(&path);
}

/// Pins down `migrate_prune_tilt_curve_aggregate_columns`'s cleanup against a database
/// that ran the never-released first-draft 36-column `diagram_tilt_curves` schema:
/// seeds that old shape by hand, then asserts the 30 obsolete columns are gone, the 6
/// survivors are renamed, and their data survives the rename intact.
#[test]
fn tilt_curve_aggregate_pruning_drops_obsolete_columns_and_renames_the_survivors() {
    let path = temp_db_path("tilt_curve_pruning");
    {
        // Current migrations create the already-pruned 6-column shape, so exercising
        // the pruning migration itself requires seeding the old 36-column shape by hand.
        let conn = Connection::open(&path).expect("open raw connection for seeding");
        conn.execute_batch(
            "CREATE TABLE diagram_entries (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 title TEXT NOT NULL,
                 url TEXT NOT NULL UNIQUE,
                 design_id TEXT,
                 source_id TEXT NOT NULL DEFAULT 'facetdiagrams.org',
                 ignored BOOLEAN NOT NULL DEFAULT 0
             );
             CREATE TABLE diagram_tilt_curves (
                 entry_id INTEGER PRIMARY KEY,
                 curves BLOB,
                 curve_image BLOB,
                 generated_at INTEGER,
                 perf_brilliance_15_min REAL, perf_brilliance_15_max REAL, perf_brilliance_15_mean REAL,
                 perf_extinction_15_min REAL, perf_extinction_15_max REAL, perf_extinction_15_mean REAL,
                 perf_windowing_15_min REAL, perf_windowing_15_max REAL, perf_windowing_15_mean REAL,
                 perf_brilliance_30_min REAL, perf_brilliance_30_max REAL, perf_brilliance_30_mean REAL,
                 perf_extinction_30_min REAL, perf_extinction_30_max REAL, perf_extinction_30_mean REAL,
                 perf_windowing_30_min REAL, perf_windowing_30_max REAL, perf_windowing_30_mean REAL,
                 perf_brilliance_45_min REAL, perf_brilliance_45_max REAL, perf_brilliance_45_mean REAL,
                 perf_extinction_45_min REAL, perf_extinction_45_max REAL, perf_extinction_45_mean REAL,
                 perf_windowing_45_min REAL, perf_windowing_45_max REAL, perf_windowing_45_mean REAL,
                 perf_brilliance_90_min REAL, perf_brilliance_90_max REAL, perf_brilliance_90_mean REAL,
                 perf_extinction_90_min REAL, perf_extinction_90_max REAL, perf_extinction_90_mean REAL,
                 perf_windowing_90_min REAL, perf_windowing_90_max REAL, perf_windowing_90_mean REAL,
                 FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
             );",
        )
        .expect("create pre-pruning diagram_tilt_curves");
        conn.execute(
            "INSERT INTO diagram_entries (title, url) VALUES ('A', 'url-a')",
            [],
        )
        .expect("seed entry");
        conn.execute(
            "INSERT INTO diagram_tilt_curves (
                 entry_id, perf_brilliance_90_min, perf_brilliance_90_max, perf_brilliance_15_min
             ) VALUES (1, 12.5, 87.5, 999.0)",
            [],
        )
        .expect("seed pre-pruning row");
    }

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");

    for obsolete in [
        "perf_brilliance_15_min",
        "perf_brilliance_15_max",
        "perf_brilliance_15_mean",
        "perf_brilliance_90_mean",
        "perf_windowing_45_mean",
    ] {
        assert!(
            !Database::column_exists(&db.conn, "diagram_tilt_curves", obsolete).unwrap(),
            "{obsolete} must be dropped"
        );
    }
    for (metric, extreme) in crate::model::performance::all_global_extreme_columns() {
        let column = crate::model::performance::global_extreme_column_name(metric, extreme);
        assert!(
            Database::column_exists(&db.conn, "diagram_tilt_curves", &column).unwrap(),
            "{column} must exist after pruning"
        );
    }

    let (min, max): (f64, f64) = db
        .conn
        .query_row(
            "SELECT perf_brilliance_global_min, perf_brilliance_global_max
             FROM diagram_tilt_curves WHERE entry_id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("renamed columns must carry the pre-pruning data through");
    assert!((min - 12.5).abs() < 1e-9, "min was {min}");
    assert!((max - 87.5).abs() < 1e-9, "max was {max}");

    // Idempotent: second open must not error dropping/renaming already-gone columns.
    drop(db);
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let column_count = {
        let mut stmt = db2
            .conn
            .prepare("PRAGMA table_info(diagram_tilt_curves)")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>("name"))
            .unwrap()
            .flatten()
            .count()
    };
    // entry_id, curves, curve_image, generated_at, + 6 global columns = 10.
    assert_eq!(column_count, 10);

    let _ = std::fs::remove_file(&path);
}

/// [`sql_identifier`] must accept exactly the identifiers this crate actually formats
/// into DDL, and reject anything an injection attempt would need -- a statement
/// terminator/comment, an empty name, or one that doesn't start with a letter/underscore.
#[test]
fn sql_identifier_accepts_real_column_names_and_rejects_injection_shapes() {
    use super::super::migrations::sql_identifier;

    assert_eq!(
        sql_identifier("perf_brilliance_90_min").unwrap(),
        "perf_brilliance_90_min"
    );
    assert_eq!(
        sql_identifier("diagram_details").unwrap(),
        "diagram_details"
    );
    assert_eq!(
        sql_identifier("_leading_underscore").unwrap(),
        "_leading_underscore"
    );

    assert!(sql_identifier("x; DROP TABLE diagram_entries; --").is_err());
    assert!(sql_identifier("").is_err());
    assert!(sql_identifier("1_leading_digit").is_err());
    assert!(sql_identifier("has space").is_err());
    assert!(sql_identifier("quote'd").is_err());
}

#[test]
fn ignored_migration_backfills_not_ignored_and_is_idempotent_across_two_opens() {
    let path = temp_db_path("ignored_migration");
    // seed_pre_migration_db's schema predates `ignored` entirely.
    seed_pre_migration_db(
        &path,
        &[(
            "A",
            "url-a",
            Some("1.540"),
            Some("1.009"),
            Some("0.171"),
            Some("96"),
            Some("55+6"),
        )],
    );

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(Database::column_exists(&db.conn, "diagram_entries", "ignored").unwrap());
        let ignored: bool = db
            .conn
            .query_row(
                "SELECT ignored FROM diagram_entries WHERE title = 'A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !ignored,
            "every pre-existing row must backfill to not-ignored"
        );

        // The two new side tables must also be created by this same open, not just
        // the one ALTER TABLE.
        let entry_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM diagram_entries WHERE title = 'A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            db.get_preview_images(entry_id).unwrap(),
            crate::model::preview::PreviewImages::default()
        );
        assert!(!db.has_tilt_curves(entry_id).unwrap());
    }

    // Second open: no-op, backfilled value survives untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let ignored: bool = db2
        .conn
        .query_row(
            "SELECT ignored FROM diagram_entries WHERE title = 'A'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!ignored);

    // The exact column set, not a bare count: the point of this assertion is that
    // a re-run of the migration leaves no duplicate column behind, and comparing
    // names catches that (a duplicate shows up twice) while also saying which
    // columns the migration is expected to have produced.
    let mut type_stmt = db2
        .conn
        .prepare("PRAGMA table_info(diagram_entries)")
        .unwrap();
    let mut columns: Vec<String> = type_stmt
        .query_map([], |r| r.get::<_, String>("name"))
        .unwrap()
        .flatten()
        .collect();
    columns.sort();
    assert_eq!(
        columns,
        vec![
            "created_at",
            "derived_from_entry_id",
            "design_id",
            "id",
            "ignored",
            "source_id",
            "title",
            "updated_at",
            "url",
        ]
    );

    let _ = std::fs::remove_file(&path);
}
