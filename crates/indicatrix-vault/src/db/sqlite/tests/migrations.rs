//! Column-retype and column-add migrations against `diagram_entries`/`diagram_details`:
//! the TEXT-to-numeric retype plus `facets_count` split, `source_id` backfill, and the
//! designer/attachment column additions -- each proven against data shaped like the
//! pre-migration schema, and idempotent across a second open.

use super::{
    super::*,
    fixtures::{seed_pre_migration_db, temp_db_path},
};

/// One `(title, refractive_index, lw_ratio, volume, index_gear, facets, girdle_facets)`
/// row from `migration_retypes_columns_and_splits_facets_count`'s post-migration
/// read-back query -- named here purely so that query's `Vec<...>` type doesn't trip
/// `clippy::type_complexity`.
type MigratedDetailRow = (
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a single migration-correctness test walking several seeded rows \
                  through the migration and asserting each one; splitting it would \
                  separate the setup from the assertions it's checking"
)]
fn migration_retypes_columns_and_splits_facets_count() {
    let path = temp_db_path("retype");
    seed_pre_migration_db(
        &path,
        &[
            (
                "A",
                "url-a",
                Some("1.540"),
                Some("1.009"),
                Some("0.171"),
                Some("96"),
                Some("55+6"),
            ),
            (
                "B",
                "url-b",
                Some("2.16"),
                Some("1.001"),
                Some("0.257"),
                Some("96"),
                Some("57"),
            ),
            ("C", "url-c", None, None, None, None, None),
            (
                "D",
                "url-d",
                Some(""),
                Some(""),
                Some(""),
                Some(""),
                Some(""),
            ),
            (
                "E",
                "url-e",
                Some("1.76"),
                Some("1.63"),
                Some("0.412"),
                Some("84"),
                Some("78-7"),
            ),
        ],
    );

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");

    assert!(Database::column_exists(&db.conn, "diagram_details", "facets").unwrap());
    assert!(Database::column_exists(&db.conn, "diagram_details", "girdle_facets").unwrap());
    assert!(Database::column_exists(&db.conn, "diagram_details", "facets_count").unwrap());

    let mut type_stmt = db
        .conn
        .prepare("PRAGMA table_info(diagram_details)")
        .unwrap();
    let types: std::collections::HashMap<String, String> = type_stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>("name")?, r.get::<_, String>("type")?))
        })
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(types["refractive_index"], "REAL");
    assert_eq!(types["lw_ratio"], "REAL");
    assert_eq!(types["volume"], "REAL");
    assert_eq!(types["index_gear"], "INTEGER");
    assert_eq!(types["facets_count"], "TEXT");
    assert_eq!(types["facets"], "INTEGER");
    assert_eq!(types["girdle_facets"], "INTEGER");

    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM diagram_details", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 5);

    // Values round-trip, including the "55+6" / "57" / "78-7" splits.
    let mut stmt = db
        .conn
        .prepare(
            "SELECT de.title, dd.refractive_index, dd.lw_ratio, dd.volume, dd.index_gear,
                        dd.facets, dd.girdle_facets
                 FROM diagram_details dd JOIN diagram_entries de ON de.id = dd.entry_id
                 ORDER BY de.title",
        )
        .unwrap();
    let got: Vec<MigratedDetailRow> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        got[0],
        (
            "A".to_string(),
            Some(1.540),
            Some(1.009),
            Some(0.171),
            Some(96),
            Some(55),
            Some(6)
        )
    );
    assert_eq!(
        got[1],
        (
            "B".to_string(),
            Some(2.16),
            Some(1.001),
            Some(0.257),
            Some(96),
            Some(57),
            Some(0)
        )
    );
    // NULL stays NULL, and facets_count = NULL splits to (None, None).
    assert_eq!(
        got[2],
        ("C".to_string(), None, None, None, None, None, None)
    );
    // Empty string also becomes NULL, not 0.0/0.
    assert_eq!(
        got[3],
        ("D".to_string(), None, None, None, None, None, None)
    );
    assert_eq!(
        got[4],
        (
            "E".to_string(),
            Some(1.76),
            Some(1.63),
            Some(0.412),
            Some(84),
            Some(78),
            Some(7)
        )
    );

    let _ = std::fs::remove_file(&path);
}

/// A FRESH database's `diagram_details` (via
/// `create_tables_if_not_exist`) already declares `refractive_index`/`lw_ratio`/
/// `volume`/`index_gear` as REAL/INTEGER and already has `facets`/`girdle_facets` --
/// `migrate_numeric_columns` must detect that from the column's actual type and skip
/// its DROP-COLUMN/RENAME-COLUMN retype cycle entirely, never running it against
/// columns that were never TEXT. Also proves the migration logic is safe to invoke
/// twice in a row on the same already-typed database (idempotency), since production
/// code runs it on every `Database::new` regardless of how many times the process has
/// opened this file before.
#[test]
fn fresh_database_columns_are_already_typed_and_the_numeric_migration_is_a_no_op() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");

    let types = |db: &Database| -> std::collections::HashMap<String, String> {
        let mut stmt = db
            .conn
            .prepare("PRAGMA table_info(diagram_details)")
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get::<_, String>("name")?, r.get::<_, String>("type")?))
        })
        .unwrap()
        .flatten()
        .collect()
    };

    let before = types(&db);
    assert_eq!(before["refractive_index"], "REAL");
    assert_eq!(before["lw_ratio"], "REAL");
    assert_eq!(before["volume"], "REAL");
    assert_eq!(before["index_gear"], "INTEGER");
    assert_eq!(before["facets"], "INTEGER");
    assert_eq!(before["girdle_facets"], "INTEGER");
    assert!(Database::column_exists(&db.conn, "diagram_details", "facets").unwrap());
    assert!(Database::column_exists(&db.conn, "diagram_details", "girdle_facets").unwrap());

    // `Database::new` (above) already ran `migrate_numeric_columns` once as part of
    // opening. Running the exact same migration logic a second time, directly, proves
    // it is a genuine no-op against an already-typed table rather than merely "ran
    // without erroring the first time because the table happened to be fresh."
    db.migrate_numeric_columns()
        .expect("re-running the migration against an already-typed table must not error");

    let after = types(&db);
    assert_eq!(
        before, after,
        "a second run must leave every diagram_details column exactly as it was, with \
         no duplicated staging column and no retyping"
    );
}

#[test]
fn migration_is_idempotent_across_two_opens() {
    let path = temp_db_path("idempotent");
    seed_pre_migration_db(
        &path,
        &[
            (
                "A",
                "url-a",
                Some("1.540"),
                Some("1.009"),
                Some("0.171"),
                Some("96"),
                Some("55+6"),
            ),
            (
                "B",
                "url-b",
                Some("2.16"),
                Some("1.001"),
                Some("0.257"),
                Some("96"),
                Some("57"),
            ),
        ],
    );

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM diagram_details", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    // Second open must be a no-op: same schema/row count/values, no re-add error.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let count: i64 = db2
        .conn
        .query_row("SELECT COUNT(*) FROM diagram_details", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);

    let ri: f64 = db2
            .conn
            .query_row(
                "SELECT refractive_index FROM diagram_details dd JOIN diagram_entries de ON de.id = dd.entry_id WHERE de.title = 'A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert!((ri - 1.540).abs() < 1e-9);

    let mut type_stmt = db2
        .conn
        .prepare("PRAGMA table_info(diagram_details)")
        .unwrap();
    let column_count = type_stmt
        .query_map([], |r| r.get::<_, String>("name"))
        .unwrap()
        .flatten()
        .count();
    // 13 original + 2 (numeric split) + 7 (proportions) + 5 (designer/attachment) +
    // 2 (concave counts) = 29; no duplicated `__migrated` staging columns left behind
    // by a re-run.
    assert_eq!(column_count, 29);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn source_id_migration_backfills_legacy_rows_and_is_idempotent_across_two_opens() {
    let path = temp_db_path("source_id_migration");
    // seed_pre_migration_db's schema predates source_id entirely.
    seed_pre_migration_db(
        &path,
        &[
            (
                "A",
                "url-a",
                Some("1.540"),
                Some("1.009"),
                Some("0.171"),
                Some("96"),
                Some("55+6"),
            ),
            (
                "B",
                "url-b",
                Some("2.16"),
                Some("1.001"),
                Some("0.257"),
                Some("96"),
                Some("57"),
            ),
        ],
    );

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(Database::column_exists(&db.conn, "diagram_entries", "source_id").unwrap());
        let ids: Vec<String> = {
            let mut stmt = db
                .conn
                .prepare("SELECT source_id FROM diagram_entries ORDER BY title")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .flatten()
                .collect()
        };
        // Backfilled with the legacy source -- could only have come from facetdiagrams.org.
        assert_eq!(
            ids,
            vec![LEGACY_SOURCE_ID.to_string(), LEGACY_SOURCE_ID.to_string()]
        );
    }

    // Second open: no-op, backfilled values survive untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let count: i64 = db2
        .conn
        .query_row(
            "SELECT COUNT(*) FROM diagram_entries WHERE source_id = ?1",
            params![LEGACY_SOURCE_ID],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

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

#[test]
fn a_fresh_database_already_has_the_source_id_column() {
    let path = temp_db_path("fresh_source_id");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert!(Database::column_exists(&db.conn, "diagram_entries", "source_id").unwrap());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn designer_and_attachment_migration_adds_the_columns_and_is_idempotent() {
    let path = temp_db_path("designer_split_migration");
    // seed_pre_migration_db's schema predates all five of these columns.
    seed_pre_migration_db(
        &path,
        &[
            (
                "A",
                "url-a",
                Some("1.540"),
                Some("1.009"),
                Some("0.171"),
                Some("96"),
                Some("55+6"),
            ),
            (
                "B",
                "url-b",
                Some("2.16"),
                Some("1.001"),
                Some("0.257"),
                Some("96"),
                Some("57"),
            ),
        ],
    );

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        for column in [
            "designer",
            "source_citation",
            "pdf_file",
            "gem_file",
            "shape_category",
        ] {
            assert!(
                Database::column_exists(&db.conn, "diagram_details", column).unwrap(),
                "migration must add {column}"
            );
        }
        // designer_info is kept, not dropped -- other callers still read it.
        assert!(Database::column_exists(&db.conn, "diagram_details", "designer_info").unwrap());
        // idx_diagram_details_designer used to be created here too, but
        // nothing ever queries it -- see `Database::migrate_drop_unused_designer_index`.
        // It must be gone, not merely absent from this migration's own CREATE list.
        let indexed: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_diagram_details_designer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            indexed, 0,
            "the unused designer index must be dropped, never created"
        );
    }

    // Second open: no-op, pre-existing rows survive untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let count: i64 = db2
        .conn
        .query_row("SELECT COUNT(*) FROM diagram_details", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn a_fresh_database_already_has_the_designer_and_attachment_columns() {
    let path = temp_db_path("fresh_designer_split");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    for column in [
        "designer",
        "source_citation",
        "pdf_file",
        "gem_file",
        "shape_category",
    ] {
        assert!(
            Database::column_exists(&db.conn, "diagram_details", column).unwrap(),
            "a fresh database's CREATE TABLE must already include {column}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn migrate_drop_unused_designer_index_removes_a_pre_existing_index() {
    let path = temp_db_path("drop_designer_index");
    seed_pre_migration_db(&path, &[("A", "url-a", None, None, None, None, None)]);
    {
        // Simulate a database from a build old enough to still have the now-removed
        // index (indexed column doesn't matter -- this only proves ANY pre-existing
        // index of this name gets dropped).
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE INDEX idx_diagram_details_designer ON diagram_details (designer_info);",
        )
        .unwrap();
    }

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    let indexed: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'idx_diagram_details_designer'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0);
    let _ = std::fs::remove_file(&path);
}
