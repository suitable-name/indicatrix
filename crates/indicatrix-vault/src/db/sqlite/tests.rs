use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// Returns a fresh, guaranteed-nonexistent temp-database path -- tests need a real
/// file (not `:memory:`) to reopen and check idempotency/persistence across two
/// `Database::new` calls.
///
/// Counter + `process::id()` dedupe within a run but not across runs (a leaked file
/// from a killed run can collide with a later pid/counter and get opened
/// pre-populated -- observed for real). Deleting any pre-existing file removes that
/// failure mode outright rather than just shrinking its odds.
fn temp_db_path(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "indicatrix_vault_test_{label}_{n}_{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// One `(title, url, ri, lw, volume, gear, facets_count)` row for
/// [`seed_pre_migration_db`].
type SeedRow<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

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

/// Creates the *pre-migration* schema directly (bypassing `Database::new`, which
/// always migrates on open) and inserts one `diagram_entries`/`diagram_details`
/// pair per `SeedRow`, exactly as the old TEXT-column schema would have held them.
/// Used to test the migration against data shaped like what's actually in
/// `facet_diagrams.sqlite` today.
fn seed_pre_migration_db(path: &std::path::Path, rows: &[SeedRow<'_>]) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
        "CREATE TABLE diagram_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                design_id TEXT
            );
            CREATE TABLE diagram_details (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_id INTEGER NOT NULL UNIQUE,
                page_url TEXT NOT NULL,
                diagram_image_name TEXT,
                diagram_image_data BLOB,
                competition_diagram TEXT,
                lw_ratio TEXT,
                refractive_index TEXT,
                index_gear TEXT,
                volume TEXT,
                facets_count TEXT,
                shape TEXT,
                designer_info TEXT,
                FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
            );
            CREATE TABLE angle_settings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                order_idx INTEGER NOT NULL,
                detail_id INTEGER NOT NULL,
                facet TEXT NOT NULL,
                angle TEXT NOT NULL,
                index_val TEXT NOT NULL,
                notes TEXT NOT NULL,
                FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
            );
            CREATE TABLE attached_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                detail_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                content BLOB NOT NULL,
                FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
            );
            CREATE TABLE custom_gem_materials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                refractive_index REAL NOT NULL,
                dispersion REAL NOT NULL,
                birefringence REAL NOT NULL,
                absorption_r REAL NOT NULL,
                absorption_g REAL NOT NULL,
                absorption_b REAL NOT NULL
            );",
    )
    .expect("create pre-migration schema");

    for (title, url, ri, lw, volume, gear, facets_count) in rows {
        conn.execute(
            "INSERT INTO diagram_entries (title, url, design_id) VALUES (?1, ?2, NULL)",
            params![title, url],
        )
        .expect("insert entry");
        let entry_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO diagram_details (
                    entry_id, page_url, competition_diagram, lw_ratio, refractive_index,
                    index_gear, volume, facets_count, shape, designer_info
                 ) VALUES (?1, '', NULL, ?2, ?3, ?4, ?5, ?6, NULL, NULL)",
            params![entry_id, lw, ri, gear, volume, facets_count],
        )
        .expect("insert detail");
    }
}

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
    // 13 original + 2 (numeric split) + 7 (proportions) + 5 (designer/attachment) =
    // 27; no duplicated `__migrated` staging columns left behind by a re-run.
    assert_eq!(column_count, 27);

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

    let mut type_stmt = db2
        .conn
        .prepare("PRAGMA table_info(diagram_entries)")
        .unwrap();
    let column_count = type_stmt
        .query_map([], |r| r.get::<_, String>("name"))
        .unwrap()
        .flatten()
        .count();
    // title, url, design_id, id, source_id, ignored -- 6, no leftover duplicate.
    assert_eq!(column_count, 6);

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
        let indexed: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_diagram_details_designer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1, "designer index must exist after migrating");
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

/// The five new fields must survive a `save_diagram_detail` round trip into their
/// typed columns -- `shape_category` is `Option<String>` on `FacetDiagramDetail`
/// bound into an INTEGER column, so this pins down it lands as a number, not text.
#[test]
fn save_diagram_detail_persists_the_designer_split_and_competition_fields() {
    let path = temp_db_path("designer_split_roundtrip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Utopia".to_string(),
        url: "https://facetdiagrams.org/diagramus/utopia/".to_string(),
        design_id: String::new(),
    };
    let entry_id = db
        .save_diagram_entry(&entry, LEGACY_SOURCE_ID)
        .expect("save entry");
    let detail = FacetDiagramDetail {
        designer_info: Some("Capps, Jerry; Lapidary Journal, May 1994, p95".to_string()),
        designer: Some("Capps, Jerry".to_string()),
        source_citation: Some("Lapidary Journal, May 1994, p95".to_string()),
        pdf_file: Some("2002SSCMasters.pdf".to_string()),
        gem_file: None,
        shape_category: Some("5".to_string()),
        ..Default::default()
    };
    db.save_diagram_detail(&detail, entry_id)
        .expect("save detail");

    let (designer, citation, pdf, gem, category) = db
        .conn
        .query_row(
            "SELECT designer, source_citation, pdf_file, gem_file, shape_category
                 FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .expect("read back detail");

    assert_eq!(designer.as_deref(), Some("Capps, Jerry"));
    assert_eq!(citation.as_deref(), Some("Lapidary Journal, May 1994, p95"));
    assert_eq!(pdf.as_deref(), Some("2002SSCMasters.pdf"));
    assert_eq!(gem, None);
    // Read back as an integer, not the "5" string that went in.
    assert_eq!(category, Some(5));

    let _ = std::fs::remove_file(&path);
}

/// Every `diagram_details` column [`MetadataUpdate`] does NOT cover -- must survive a
/// metadata edit byte-for-byte, including fields `FullDiagramRecord` can't even see.
/// See `update_diagram_metadata`'s doc comment for the trap this guards against.
type UntouchedDetailColumnsRow = (
    String,          // page_url
    Option<String>,  // diagram_image_name
    Option<Vec<u8>>, // diagram_image_data
    Option<String>,  // competition_diagram
    Option<f64>,     // tw_ratio
    Option<f64>,     // uw_ratio
    Option<String>,  // designer
    Option<String>,  // source_citation
    Option<String>,  // pdf_file
    Option<String>,  // gem_file
    Option<i64>,     // shape_category
);

/// `(hw_ratio, cw_ratio, pw_ratio, symmetry_order, mirror_symmetry)` spot-check row.
type RatioAndSymmetrySpotCheckRow = (
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
    Option<bool>,
);

fn read_untouched_detail_columns(db: &Database, entry_id: i64) -> UntouchedDetailColumnsRow {
    db.conn
        .query_row(
            "SELECT page_url, diagram_image_name, diagram_image_data, competition_diagram,
                    tw_ratio, uw_ratio, designer, source_citation, pdf_file, gem_file, shape_category
             FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            },
        )
        .expect("read untouched diagram_details columns")
}

/// Pins down the trap `update_diagram_metadata` exists to avoid: a fully-populated
/// detail row gets one narrow call that changes exactly two fields (`shape`,
/// `refractive_index`) and resubmits every other `MetadataUpdate` field unchanged,
/// as a pre-filled editor form would. Every column outside `MetadataUpdate` --
/// and `angle_settings`/`attached_files` entirely -- must come back byte-for-byte
/// identical; a regression to delete-and-reinsert would zero those columns or
/// change child rows' ids, either of which this test catches.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one round trip through a fully-populated detail row, a narrow edit, and \
                  every 'must still equal what it started as' assertion; splitting it \
                  would separate the setup from the assertions it's checking"
)]
fn update_diagram_metadata_touches_only_its_own_fields_and_nothing_else() {
    let path = temp_db_path("metadata_update_narrow");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Utopia".to_string(),
        url: "https://facetdiagrams.org/diagramus/utopia-narrow/".to_string(),
        design_id: String::new(),
    };
    let entry_id = db
        .save_diagram_entry(&entry, LEGACY_SOURCE_ID)
        .expect("save entry");

    let original = FacetDiagramDetail {
        page_url: "https://facetdiagrams.org/diagramus/utopia-narrow/".to_string(),
        diagram_image_name: Some("utopia.svg".to_string()),
        diagram_image_data: Some(vec![1, 2, 3, 4]),
        angle_settings_table: vec![crate::model::angle::AngleSetting {
            order_index: 0,
            facet: "T".to_string(),
            angle: "0".to_string(),
            index: "-".to_string(),
            notes: String::new(),
        }],
        attached_files: vec![crate::model::file::AttachedFile {
            name: "utopia.asc".to_string(),
            url: String::new(),
            content: b"original bytes, must survive untouched".to_vec(),
        }],
        competition_diagram: Some("2002SSCMasters".to_string()),
        lw_ratio: Some("1.05".to_string()),
        refractive_index: Some("2.417".to_string()),
        index_gear: Some("96".to_string()),
        volume: Some("0.42".to_string()),
        facets_count: Some("57+8".to_string()),
        shape: Some("Round".to_string()),
        designer_info: Some("Capps, Jerry; Lapidary Journal, May 1994, p95".to_string()),
        hw_ratio: Some("0.61".to_string()),
        tw_ratio: Some("0.55".to_string()),
        uw_ratio: Some("0.12".to_string()),
        pw_ratio: Some("0.44".to_string()),
        cw_ratio: Some("0.17".to_string()),
        symmetry_order: Some("8".to_string()),
        mirror_symmetry: Some(true),
        designer: Some("Capps, Jerry".to_string()),
        source_citation: Some("Lapidary Journal, May 1994, p95".to_string()),
        pdf_file: Some("2002SSCMasters.pdf".to_string()),
        gem_file: Some("utopia.gem".to_string()),
        shape_category: Some("5".to_string()),
    };
    db.save_diagram_detail(&original, entry_id)
        .expect("save original detail");

    let before = read_untouched_detail_columns(&db, entry_id);
    let detail_id: i64 = db
        .conn
        .query_row(
            "SELECT id FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| r.get(0),
        )
        .unwrap();
    let angle_count_before: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM angle_settings WHERE detail_id = ?1",
            params![detail_id],
            |r| r.get(0),
        )
        .unwrap();
    let (attachment_id_before, attachment_content_before): (i64, Vec<u8>) = db
        .conn
        .query_row(
            "SELECT id, content FROM attached_files WHERE detail_id = ?1",
            params![detail_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    let update = MetadataUpdate {
        designer_info: original.designer_info.clone(),
        shape: Some("Oval".to_string()),            // the actual edit
        refractive_index: Some("1.76".to_string()), // the actual edit
        index_gear: original.index_gear.clone(),
        facets_count: original.facets_count.clone(),
        symmetry_order: original.symmetry_order.clone(),
        mirror_symmetry: original.mirror_symmetry,
        lw_ratio: original.lw_ratio.clone(),
        hw_ratio: original.hw_ratio.clone(),
        cw_ratio: original.cw_ratio.clone(),
        pw_ratio: original.pw_ratio.clone(),
        volume: original.volume.clone(),
    };
    db.update_diagram_metadata(entry_id, &update)
        .expect("update metadata");

    let full = db.get_diagram_full(entry_id).unwrap().unwrap();
    assert_eq!(full.shape.as_deref(), Some("Oval"));
    assert_eq!(full.refractive_index.as_deref(), Some("1.76"));

    // Resubmitted MetadataUpdate fields must still read back unchanged.
    assert_eq!(full.designer_info, original.designer_info);
    assert_eq!(full.index_gear, original.index_gear);
    assert_eq!(full.facets_count, original.facets_count);
    assert_eq!(full.lw_ratio, original.lw_ratio);
    let (hw, cw, pw, sym, mirror): RatioAndSymmetrySpotCheckRow = db
        .conn
        .query_row(
            "SELECT hw_ratio, cw_ratio, pw_ratio, symmetry_order, mirror_symmetry
             FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert!((hw.unwrap() - 0.61).abs() < 1e-9);
    assert!((cw.unwrap() - 0.17).abs() < 1e-9);
    assert!((pw.unwrap() - 0.44).abs() < 1e-9);
    assert_eq!(sym, Some(8));
    assert_eq!(mirror, Some(true));

    // Every column outside MetadataUpdate must be byte-for-byte identical to before.
    assert_eq!(
        read_untouched_detail_columns(&db, entry_id),
        before,
        "update_diagram_metadata must not touch any diagram_details column outside MetadataUpdate"
    );

    // Never a delete-and-reinsert of children: same row count, same attachment id/bytes.
    let angle_count_after: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM angle_settings WHERE detail_id = ?1",
            params![detail_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(angle_count_after, angle_count_before);
    let (attachment_id_after, attachment_content_after): (i64, Vec<u8>) = db
        .conn
        .query_row(
            "SELECT id, content FROM attached_files WHERE detail_id = ?1",
            params![detail_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        attachment_id_after, attachment_id_before,
        "attached_files row must not be deleted and reinserted (its id would change)"
    );
    assert_eq!(attachment_content_after, attachment_content_before);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn update_diagram_metadata_rejects_unknown_entry_id() {
    let path = temp_db_path("metadata_update_unknown");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let result = db.update_diagram_metadata(999_999, &MetadataUpdate::default());
    assert!(result.is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_fresh_database_already_has_the_crystal_optics_columns() {
    let path = temp_db_path("fresh_crystal_optics");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    for column in [
        "crystal_system",
        "optical_character",
        "biaxial_delta_beta_alpha",
    ] {
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", column).unwrap(),
            "a fresh database's CREATE TABLE must already include {column}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

/// Seeds a `custom_gem_materials` row via the pre-crystal-optics schema directly
/// (bypassing `Database::new`): no `crystal_system`/`optical_character`/
/// `biaxial_delta_beta_alpha` columns at all.
fn seed_pre_crystal_optics_custom_material(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
            "CREATE TABLE custom_gem_materials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                refractive_index REAL NOT NULL,
                dispersion REAL NOT NULL,
                birefringence REAL NOT NULL,
                absorption_r REAL NOT NULL,
                absorption_g REAL NOT NULL,
                absorption_b REAL NOT NULL
            );
            INSERT INTO custom_gem_materials
                (name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b)
                VALUES ('Legacy Custom Sapphire', 1.768, 0.018, -0.008, 2.8, 1.2, 0.1);",
        )
        .expect("create pre-crystal-optics custom_gem_materials and seed a row");
}

#[test]
fn crystal_optics_migration_adds_the_columns_and_leaves_existing_rows_nullable() {
    let path = temp_db_path("crystal_optics_migration");
    seed_pre_crystal_optics_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        for column in [
            "crystal_system",
            "optical_character",
            "biaxial_delta_beta_alpha",
        ] {
            assert!(
                Database::column_exists(&db.conn, "custom_gem_materials", column).unwrap(),
                "migration must add {column}"
            );
        }

        // Pre-existing row survives; new fields are None (fall back to
        // GemMaterial::new_custom's inference), not defaulted to a guessed value.
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        let m = &materials[0];
        assert_eq!(m.name, "Legacy Custom Sapphire");
        assert!((m.refractive_index - 1.768).abs() < 1e-6);
        assert_eq!(m.crystal_system, None);
        assert_eq!(m.optical_character, None);
        assert_eq!(m.biaxial_delta_beta_alpha, None);
    }

    // Second open: no-op, pre-existing row survives untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2
        .get_custom_materials()
        .expect("read back materials again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].crystal_system, None);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn save_custom_material_round_trips_crystal_optics_fields() {
    let path = temp_db_path("crystal_optics_roundtrip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    // Biaxial material: all three new fields set.
    db.save_custom_material(&CustomMaterialParams {
        name: "Custom Tanzanite",
        refractive_index: 1.691,
        dispersion: 0.030,
        birefringence: 0.0130,
        absorption_rgb: [1.8, 1.6, 0.2],
        crystal_system: Some("Orthorhombic"),
        optical_character: Some("BiaxialPositive"),
        biaxial_delta_beta_alpha: Some(0.0070),
        per_axis_dispersion_json: Some(r#"{"kind":"uniaxial_extraordinary","a":1.7,"b":0.01}"#),
    })
    .expect("save biaxial custom material");

    // Uniaxial material: crystal_system/optical_character set, biaxial delta None.
    db.save_custom_material(&CustomMaterialParams {
        name: "Custom Sapphire",
        refractive_index: 1.768,
        dispersion: 0.018,
        birefringence: -0.008,
        absorption_rgb: [2.8, 1.2, 0.1],
        crystal_system: Some("Trigonal"),
        optical_character: Some("UniaxialNegative"),
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
    })
    .expect("save uniaxial custom material");

    let materials = db.get_custom_materials().expect("read back materials");
    assert_eq!(materials.len(), 2);

    let tanzanite = materials
        .iter()
        .find(|m| m.name == "Custom Tanzanite")
        .expect("Custom Tanzanite present");
    assert_eq!(tanzanite.crystal_system.as_deref(), Some("Orthorhombic"));
    assert_eq!(
        tanzanite.optical_character.as_deref(),
        Some("BiaxialPositive")
    );
    assert!((tanzanite.biaxial_delta_beta_alpha.unwrap() - 0.0070).abs() < 1e-6);
    assert_eq!(
        tanzanite.per_axis_dispersion_json.as_deref(),
        Some(r#"{"kind":"uniaxial_extraordinary","a":1.7,"b":0.01}"#)
    );

    let sapphire = materials
        .iter()
        .find(|m| m.name == "Custom Sapphire")
        .expect("Custom Sapphire present");
    assert_eq!(sapphire.crystal_system.as_deref(), Some("Trigonal"));
    assert_eq!(
        sapphire.optical_character.as_deref(),
        Some("UniaxialNegative")
    );
    assert_eq!(sapphire.biaxial_delta_beta_alpha, None);
    assert_eq!(sapphire.per_axis_dispersion_json, None);

    // Re-saving over the same name (upsert) must update crystal-optics columns too.
    db.save_custom_material(&CustomMaterialParams {
        name: "Custom Sapphire",
        refractive_index: 1.768,
        dispersion: 0.018,
        birefringence: -0.008,
        absorption_rgb: [2.8, 1.2, 0.1],
        crystal_system: None,
        optical_character: None,
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
    })
    .expect("re-save clears crystal-optics fields");
    let materials = db.get_custom_materials().expect("read back after re-save");
    let sapphire = materials
        .iter()
        .find(|m| m.name == "Custom Sapphire")
        .expect("Custom Sapphire still present");
    assert_eq!(sapphire.crystal_system, None);
    assert_eq!(sapphire.optical_character, None);

    let _ = std::fs::remove_file(&path);
}

/// Mirrors `a_fresh_database_already_has_the_crystal_optics_columns` for the new
/// `per_axis_dispersion_json` column.
#[test]
fn a_fresh_database_already_has_the_per_axis_dispersion_column() {
    let path = temp_db_path("fresh_per_axis_dispersion");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert!(
        Database::column_exists(&db.conn, "custom_gem_materials", "per_axis_dispersion_json")
            .unwrap(),
        "a fresh database's CREATE TABLE must already include per_axis_dispersion_json"
    );
    let _ = std::fs::remove_file(&path);
}

/// Seeds a `custom_gem_materials` row via the schema predating this column
/// (bypassing `Database::new`), including the crystal-optics columns
/// `migrate_crystal_optics_columns` already added -- this migration is purely
/// additive on top of that one.
fn seed_pre_per_axis_dispersion_custom_material(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
            "CREATE TABLE custom_gem_materials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                refractive_index REAL NOT NULL,
                dispersion REAL NOT NULL,
                birefringence REAL NOT NULL,
                absorption_r REAL NOT NULL,
                absorption_g REAL NOT NULL,
                absorption_b REAL NOT NULL,
                crystal_system TEXT,
                optical_character TEXT,
                biaxial_delta_beta_alpha REAL
            );
            INSERT INTO custom_gem_materials
                (name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b,
                 crystal_system, optical_character, biaxial_delta_beta_alpha)
                VALUES ('Legacy Custom Quartz', 1.544, 0.013, 0.0091, 0.8, 1.8, 0.6,
                        'Trigonal', 'UniaxialPositive', NULL);",
        )
        .expect("create pre-per-axis-dispersion custom_gem_materials and seed a row");
}

/// Old schema (crystal-optics columns, no `per_axis_dispersion_json`) -> migrated ->
/// readable, mirroring `crystal_optics_migration_adds_the_columns_and_leaves_existing_rows_nullable`.
#[test]
fn per_axis_dispersion_migration_adds_the_column_and_leaves_existing_rows_nullable() {
    let path = temp_db_path("per_axis_dispersion_migration");
    seed_pre_per_axis_dispersion_custom_material(&path);

    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
        assert!(
            Database::column_exists(&db.conn, "custom_gem_materials", "per_axis_dispersion_json")
                .unwrap(),
            "migration must add per_axis_dispersion_json"
        );

        // Pre-existing row survives, crystal-optics fields intact, new field None.
        let materials = db.get_custom_materials().expect("read back materials");
        assert_eq!(materials.len(), 1);
        let m = &materials[0];
        assert_eq!(m.name, "Legacy Custom Quartz");
        assert!((m.refractive_index - 1.544).abs() < 1e-6);
        assert_eq!(m.crystal_system.as_deref(), Some("Trigonal"));
        assert_eq!(m.optical_character.as_deref(), Some("UniaxialPositive"));
        assert_eq!(m.per_axis_dispersion_json, None);
    }

    // Second open: no-op, pre-existing row survives untouched.
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    let materials = db2
        .get_custom_materials()
        .expect("read back materials again");
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0].per_axis_dispersion_json, None);

    let _ = std::fs::remove_file(&path);
}

// LEGACY_SOURCE_ID has no guard test here: what it must stay equal to isn't visible
// from this crate. See that constant's own doc comment.

/// Builds a `Database` (fresh temp file, migrated schema) and inserts one diagram per
/// `(title, shape, ri, lw, volume, facets_count)` tuple via the public save API, for
/// exercising `search_diagrams`/`get_attribute_ranges`.
fn seeded_db(rows: &[(&str, &str, &str, &str, &str, &str)]) -> Database {
    let path = temp_db_path("search");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    for (title, shape, ri, lw, volume, facets_count) in rows {
        let entry = FacetDiagramEntry {
            title: (*title).to_string(),
            url: format!("https://example.test/{title}"),
            design_id: String::new(),
        };
        let entry_id = db
            .save_diagram_entry(&entry, LEGACY_SOURCE_ID)
            .expect("save entry");
        let detail = FacetDiagramDetail {
            shape: Some((*shape).to_string()),
            refractive_index: Some((*ri).to_string()),
            lw_ratio: Some((*lw).to_string()),
            volume: Some((*volume).to_string()),
            facets_count: Some((*facets_count).to_string()),
            index_gear: Some("96".to_string()),
            ..Default::default()
        };
        db.save_diagram_detail(&detail, entry_id)
            .expect("save detail");
    }
    db
}

#[test]
fn range_query_returns_only_rows_within_known_bounds() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    // RI in [1.6, 2.0] matches only "Mid" (1.76).
    let range = RangeFilter {
        ri_min: Some(1.6),
        ri_max: Some(2.0),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );

    // facets in [55, 65] matches only "Mid" (facets = 60).
    let range = RangeFilter {
        facets_min: Some(55),
        facets_max: Some(65),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );

    // No range filter returns everything.
    let results = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert_eq!(results.len(), 3);
}

#[test]
fn inverted_range_bounds_return_nothing_not_everything() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    // min > max must return empty, not silently act as if unfiltered.
    let range = RangeFilter {
        ri_min: Some(2.0),
        ri_max: Some(1.0),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert!(
        results.is_empty(),
        "inverted RI bounds must return no rows, got {results:?}"
    );

    let range = RangeFilter {
        facets_min: Some(90),
        facets_max: Some(10),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert!(
        results.is_empty(),
        "inverted facets bounds must return no rows, got {results:?}"
    );
}

#[test]
fn search_diagrams_page_walks_the_whole_result_set_via_keyset_cursor() {
    let db = seeded_db(&[
        ("A", "Round", "1.50", "1.00", "0.10", "50"),
        ("B", "Round", "1.55", "1.00", "0.10", "50"),
        ("C", "Round", "1.60", "1.00", "0.10", "50"),
        ("D", "Round", "1.65", "1.00", "0.10", "50"),
        ("E", "Round", "1.70", "1.00", "0.10", "50"),
    ]);

    let mut collected = Vec::new();
    let mut after_id = None;
    loop {
        let page = db
            .search_diagrams_page("", "All", "All", &RangeFilter::default(), after_id, 2)
            .unwrap();
        if page.is_empty() {
            break;
        }
        let full_page = page.len() == 2;
        after_id = page.last().map(|r| r.id);
        collected.extend(page);
        if !full_page {
            break;
        }
    }

    assert_eq!(
        collected
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B", "C", "D", "E"],
        "a multi-page walk must reach every row, in order, including ones past the \
             first page"
    );
    // Strictly increasing ids: no page boundary skipped or duplicated a row.
    for pair in collected.windows(2) {
        assert!(pair[0].id < pair[1].id);
    }

    // Pagination composes with, not changes, what search_diagrams already returns.
    let unpaged = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert_eq!(
        collected.iter().map(|r| r.id).collect::<Vec<_>>(),
        unpaged.iter().map(|r| r.id).collect::<Vec<_>>()
    );
}

#[test]
fn search_diagrams_delegates_to_the_first_unpaginated_page() {
    let db = seeded_db(&[
        ("A", "Round", "1.50", "1.00", "0.10", "50"),
        ("B", "Round", "1.55", "1.00", "0.10", "50"),
    ]);

    let via_search = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    let via_page = db
        .search_diagrams_page("", "All", "All", &RangeFilter::default(), None, 1000)
        .unwrap();
    assert_eq!(
        via_search.iter().map(|r| r.id).collect::<Vec<_>>(),
        via_page.iter().map(|r| r.id).collect::<Vec<_>>(),
        "search_diagrams must return exactly what search_diagrams_page(.., None, 1000) does"
    );
}

#[test]
fn get_attribute_ranges_uses_real_min_but_a_percentile_based_max() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    let ranges = db.get_attribute_ranges().unwrap();
    // Minimums are still the real minimums -- only the upper bound changed.
    assert!((ranges.ri.0 - 1.50).abs() < 1e-9);
    assert!((ranges.lw_ratio.0 - 1.00).abs() < 1e-9);
    // p99 (linear interpolation) of 3 sorted points sits at index 1.98: 98% from b to c.
    assert!((ranges.ri.1 - 0.98_f64.mul_add(2.40 - 1.76, 1.76)).abs() < 1e-6);
    assert!((ranges.lw_ratio.1 - 0.98_f64.mul_add(1.20 - 1.10, 1.10)).abs() < 1e-6);
    // Upper bound must not equal the raw max for a right-skewed sample -- the fix's whole point.
    assert!(
        ranges.ri.1 < 2.40,
        "p99 bound must sit below the raw max, got {}",
        ranges.ri.1
    );
    assert!(
        ranges.lw_ratio.1 < 1.20,
        "p99 bound must sit below the raw max, got {}",
        ranges.lw_ratio.1
    );
}

#[test]
fn get_attribute_ranges_does_not_let_a_single_outlier_set_the_scale() {
    // Mirrors the real catalogue's volume column: a tight cluster of normal values
    // plus one physically-impossible outlier that must not drag the slider's usable
    // bound toward it. 200 normal rows keeps the outlier under 1% of the sample.
    let mut owned: Vec<(String, String, String, String, String, String)> = (0..200)
        .map(|i| {
            let vol = f64::from(i).mul_add(0.20 / 199.0, 0.10);
            (
                format!("Normal{i}"),
                "Round".to_string(),
                "1.50".to_string(),
                "1.00".to_string(),
                format!("{vol:.4}"),
                "50".to_string(),
            )
        })
        .collect();
    owned.push((
        "Outlier".to_string(),
        "Round".to_string(),
        "1.50".to_string(),
        "1.00".to_string(),
        "195.0".to_string(),
        "50".to_string(),
    ));
    let rows: Vec<(&str, &str, &str, &str, &str, &str)> = owned
        .iter()
        .map(|(a, b, c, d, e, f)| {
            (
                a.as_str(),
                b.as_str(),
                c.as_str(),
                d.as_str(),
                e.as_str(),
                f.as_str(),
            )
        })
        .collect();
    let db = seeded_db(&rows);

    let ranges = db.get_attribute_ranges().unwrap();
    assert!(
        (ranges.volume.0 - 0.10).abs() < 1e-6,
        "min should be the real min, got {}",
        ranges.volume.0
    );
    assert!(
        ranges.volume.1 < 1.0,
        "a single outlier of 195 must not set the slider's usable max bound, got {}",
        ranges.volume.1
    );

    // Excluded from the slider's scale must not mean excluded from results.
    let results = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert!(
        results.iter().any(|r| r.title == "Outlier"),
        "outlier row must still be reachable via unfiltered search"
    );
}

#[test]
fn percentile_of_sorted_interpolates_linearly() {
    let values = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert!((percentile_of_sorted(&values, 0.0) - 1.0).abs() < 1e-9);
    assert!((percentile_of_sorted(&values, 50.0) - 3.0).abs() < 1e-9);
    assert!((percentile_of_sorted(&values, 100.0) - 5.0).abs() < 1e-9);
    // idx = 3.96 -> 96% from values[3]=4.0 to values[4]=5.0.
    assert!((percentile_of_sorted(&values, 99.0) - 4.96).abs() < 1e-9);
}

#[test]
fn percentile_of_sorted_single_value_returns_that_value() {
    assert!((percentile_of_sorted(&[42.0], 99.0) - 42.0).abs() < 1e-9);
}

#[test]
fn get_unique_gears_still_returns_display_strings_after_retype() {
    let db = seeded_db(&[("A", "Round", "1.50", "1.00", "0.10", "50")]);
    let gears = db.get_unique_gears().unwrap();
    assert_eq!(gears, vec!["96".to_string()]);
}

#[test]
fn find_cross_source_duplicates_detects_same_design_from_two_sources() {
    let path = temp_db_path("dedup_same_design");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    // Same physical design, synced from facetdiagrams.org first.
    let entry_a = FacetDiagramEntry {
        title: "Barion Heart".to_string(),
        url: "https://facetdiagrams.org/a".to_string(),
        design_id: String::new(),
    };
    let id_a = db
        .save_diagram_entry(&entry_a, "facetdiagrams.org")
        .unwrap();
    db.save_diagram_detail(
        &FacetDiagramDetail {
            designer_info: Some("Long, Bob".to_string()),
            facets_count: Some("60+9".to_string()),
            ..Default::default()
        },
        id_a,
    )
    .unwrap();

    // Encountered again under a different source, differently-cased/whitespaced title.
    let dupes = db
        .find_cross_source_duplicates(
            "gemologyproject.com",
            "  barion   heart ",
            Some("Long, Bob"),
            Some(60),
        )
        .unwrap();
    assert_eq!(dupes.len(), 1);
    assert_eq!(dupes[0].existing_entry_id, id_a);
    assert_eq!(dupes[0].existing_source_id, "facetdiagrams.org");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn find_cross_source_duplicates_does_not_flag_different_designers_sharing_a_title() {
    let path = temp_db_path("dedup_diff_designer");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let entry_a = FacetDiagramEntry {
        title: "Sunburst".to_string(),
        url: "https://facetdiagrams.org/sunburst-a".to_string(),
        design_id: String::new(),
    };
    let id_a = db
        .save_diagram_entry(&entry_a, "facetdiagrams.org")
        .unwrap();
    db.save_diagram_detail(
        &FacetDiagramDetail {
            designer_info: Some("Alice Designer".to_string()),
            facets_count: Some("50".to_string()),
            ..Default::default()
        },
        id_a,
    )
    .unwrap();

    // Same title, same facet count, different designer -- must not be flagged.
    let dupes = db
        .find_cross_source_duplicates(
            "gemologyproject.com",
            "Sunburst",
            Some("Bob Other Designer"),
            Some(50),
        )
        .unwrap();
    assert!(
        dupes.is_empty(),
        "different designers sharing a title must not be flagged as duplicates, got {dupes:?}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn find_cross_source_duplicates_ignores_matches_within_the_same_source() {
    let path = temp_db_path("dedup_same_source");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let entry_a = FacetDiagramEntry {
        title: "Sunburst".to_string(),
        url: "https://facetdiagrams.org/sunburst-a".to_string(),
        design_id: String::new(),
    };
    let id_a = db
        .save_diagram_entry(&entry_a, "facetdiagrams.org")
        .unwrap();
    db.save_diagram_detail(
        &FacetDiagramDetail {
            facets_count: Some("50".to_string()),
            ..Default::default()
        },
        id_a,
    )
    .unwrap();

    // Same source re-syncing the same title is not a cross-source collision.
    let dupes = db
        .find_cross_source_duplicates("facetdiagrams.org", "Sunburst", None, Some(50))
        .unwrap();
    assert_eq!(dupes, []);

    let _ = std::fs::remove_file(&path);
}

// ---- Organize: rename_diagram_entry / delete_diagram_entry --------------------

#[test]
fn rename_diagram_entry_updates_the_title() {
    let path = temp_db_path("rename");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Old Title".to_string(),
        url: "local://old.asc".to_string(),
        design_id: String::new(),
    };
    let id = db.save_diagram_entry(&entry, "local-import").unwrap();

    db.rename_diagram_entry(id, "  New Title  ").unwrap();

    let full = db.get_diagram_full(id).unwrap().unwrap();
    // Trimmed, per `rename_diagram_entry`'s doc comment.
    assert_eq!(full.title, "New Title");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn rename_diagram_entry_rejects_blank_titles_and_unknown_ids() {
    let path = temp_db_path("rename_errors");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Keep Me".to_string(),
        url: "local://keep.asc".to_string(),
        design_id: String::new(),
    };
    let id = db.save_diagram_entry(&entry, "local-import").unwrap();

    assert!(db.rename_diagram_entry(id, "   ").is_err());
    assert!(db.rename_diagram_entry(id + 999, "Anything").is_err());

    // The blank-title attempt must not have touched the existing row.
    let full = db.get_diagram_full(id).unwrap().unwrap();
    assert_eq!(full.title, "Keep Me");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn delete_diagram_entry_cascades_to_detail_angles_and_files() {
    let path = temp_db_path("delete");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Doomed Design".to_string(),
        url: "local://doomed.asc".to_string(),
        design_id: String::new(),
    };
    let id = db.save_diagram_entry(&entry, "local-import").unwrap();
    db.save_diagram_detail(
        &FacetDiagramDetail {
            angle_settings_table: vec![crate::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0\u{b0}".to_string(),
                index: "0".to_string(),
                notes: String::new(),
            }],
            attached_files: vec![crate::model::file::AttachedFile {
                name: "doomed.asc".to_string(),
                url: String::new(),
                content: b"GemCad 5.0\n".to_vec(),
            }],
            ..Default::default()
        },
        id,
    )
    .unwrap();

    db.delete_diagram_entry(id).unwrap();

    assert!(db.get_diagram_full(id).unwrap().is_none());
    let remaining_angles: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM angle_settings", [], |r| r.get(0))
        .unwrap();
    let remaining_files: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM attached_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining_angles, 0);
    assert_eq!(remaining_files, 0);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn delete_diagram_entry_rejects_unknown_ids() {
    let path = temp_db_path("delete_errors");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    assert!(db.delete_diagram_entry(123_456).is_err());
    let _ = std::fs::remove_file(&path);
}

// ---- shape_vocabulary --------------------------------------------------------

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
fn get_unique_shapes_unions_the_vocabulary_with_real_catalogue_data() {
    let db = seeded_db(&[
        // Already in DEFAULT_SHAPES.
        ("A", "Round", "1.50", "1.00", "0.10", "50"),
        // Real scraped shape not in the canonical list -- must not be dropped.
        ("B", "Portuguese Round", "1.55", "1.00", "0.10", "50"),
    ]);

    let shapes = db.get_unique_shapes().unwrap();

    for shape in DEFAULT_SHAPES {
        assert!(
            shapes.iter().any(|s| s == shape),
            "seeded vocabulary entry '{shape}' must appear in the union, got {shapes:?}"
        );
    }
    assert!(
        shapes.iter().any(|s| s == "Portuguese Round"),
        "a real shape string outside the canonical list must still appear, got {shapes:?}"
    );
    // "Round" must not be duplicated for being in both sources.
    assert_eq!(shapes.iter().filter(|s| s.as_str() == "Round").count(), 1);
    let mut sorted = shapes.clone();
    sorted.sort();
    assert_eq!(shapes, sorted);
}

// ---- ignored flag, diagram_previews, diagram_tilt_curves ---------------------

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
    use super::migrations::sql_identifier;

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

    let mut type_stmt = db2
        .conn
        .prepare("PRAGMA table_info(diagram_entries)")
        .unwrap();
    let column_count = type_stmt
        .query_map([], |r| r.get::<_, String>("name"))
        .unwrap()
        .flatten()
        .count();
    // title, url, design_id, id, source_id, ignored -- 6, no leftover duplicate.
    assert_eq!(column_count, 6);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn set_diagram_ignored_toggles_the_flag_and_rejects_unknown_ids() {
    let path = temp_db_path("set_ignored");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Ignore Me".to_string(),
                url: "local://ignore-me.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();

    let ignored: bool = db
        .conn
        .query_row(
            "SELECT ignored FROM diagram_entries WHERE id = ?1",
            params![entry_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!ignored, "a newly-saved entry must default to not-ignored");

    db.set_diagram_ignored(entry_id, true).unwrap();
    let ignored: bool = db
        .conn
        .query_row(
            "SELECT ignored FROM diagram_entries WHERE id = ?1",
            params![entry_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ignored);

    db.set_diagram_ignored(entry_id, false).unwrap();
    let ignored: bool = db
        .conn
        .query_row(
            "SELECT ignored FROM diagram_entries WHERE id = ?1",
            params![entry_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!ignored);

    assert!(db.set_diagram_ignored(entry_id + 999, true).is_err());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn search_excludes_ignored_designs_by_default_and_includes_them_when_opted_in() {
    let path = temp_db_path("search_ignored");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let visible_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Visible".to_string(),
                url: "local://visible.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    let hidden_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Hidden".to_string(),
                url: "local://hidden.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.set_diagram_ignored(hidden_id, true).unwrap();

    let default_results = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert!(default_results.iter().any(|r| r.id == visible_id));
    assert!(
        !default_results.iter().any(|r| r.id == hidden_id),
        "an ignored design must be excluded by default"
    );

    let opted_in = RangeFilter {
        include_ignored: true,
        ..Default::default()
    };
    let with_ignored = db.search_diagrams("", "All", "All", &opted_in).unwrap();
    assert!(
        with_ignored.iter().any(|r| r.id == hidden_id),
        "include_ignored: true must bring the ignored design back"
    );
    assert!(with_ignored.iter().any(|r| r.id == visible_id));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn ri_tolerance_composes_with_ri_min_max_by_intersection() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    // Tolerance band (centre 1.76, tolerance 0.05) matches only "Mid".
    let tolerance_only = RangeFilter {
        ri_tolerance: Some((1.76, 0.05)),
        ..Default::default()
    };
    let results = db
        .search_diagrams("", "All", "All", &tolerance_only)
        .unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );

    // Tolerance band matching "Mid" and "High" ([1.55, 2.45]), intersected with an
    // ri_max excluding "High" -- only "Mid" survives, proving AND not OR.
    let intersected = RangeFilter {
        ri_tolerance: Some((2.0, 0.45)),
        ri_max: Some(2.0),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &intersected).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );
}

/// Builds a flat (every sample identical) `TiltPerformanceCurves` for `value`, used by
/// the performance-filter tests below where only the aggregate value matters.
fn flat_tilt_curves(value: f32) -> crate::model::tilt_curves::TiltPerformanceCurves {
    use crate::model::tilt_curves::{
        AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS,
    };
    crate::model::tilt_curves::TiltPerformanceCurves {
        axes: [AxisTiltCurves {
            brilliance_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
        }; TILT_CURVE_AXIS_COUNT],
    }
}

#[test]
fn performance_filter_matches_designs_with_curves_and_excludes_those_without() {
    let path = temp_db_path("performance_filter_basic");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let good_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Good Windowing".to_string(),
                url: "local://good.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(good_id, &flat_tilt_curves(10.0), None, 1)
        .unwrap();

    let bad_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Bad Windowing".to_string(),
                url: "local://bad.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(bad_id, &flat_tilt_curves(90.0), None, 1)
        .unwrap();

    let _no_curves_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "No Curves At All".to_string(),
                url: "local://no-curves.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();

    let filter = RangeFilter {
        performance: vec![crate::model::performance::PerformanceFilter {
            metric: crate::model::performance::PerformanceMetric::Windowing,
            bound: crate::model::performance::PerformanceBound::AtMost(20.0),
            tilt_radius_deg: 45.0,
            aggregate: crate::model::performance::PerformanceAggregate::Worst,
        }],
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &filter).unwrap();
    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["Good Windowing"]);
    assert!(!titles.contains(&"Bad Windowing"));
    assert!(
        !titles.contains(&"No Curves At All"),
        "a design with no stored curves can never satisfy an active performance filter"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn performance_filter_reports_how_many_designs_were_excluded_for_missing_curves() {
    let path = temp_db_path("performance_filter_exclusions");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let with_curves_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Has Curves".to_string(),
                url: "local://has-curves.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(with_curves_id, &flat_tilt_curves(5.0), None, 1)
        .unwrap();

    for i in 0..3 {
        db.save_diagram_entry(
            &FacetDiagramEntry {
                title: format!("No Curves {i}"),
                url: format!("local://no-curves-{i}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    }

    let filter = RangeFilter {
        performance: vec![crate::model::performance::PerformanceFilter {
            metric: crate::model::performance::PerformanceMetric::Windowing,
            bound: crate::model::performance::PerformanceBound::AtMost(20.0),
            tilt_radius_deg: 45.0,
            aggregate: crate::model::performance::PerformanceAggregate::Worst,
        }],
        ..Default::default()
    };
    let result = db
        .search_diagrams_with_performance_exclusions("", "All", "All", &filter)
        .unwrap();
    assert_eq!(
        result
            .items
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Has Curves"]
    );
    assert_eq!(result.excluded_for_missing_curves, 3);

    // No performance filter active means exclusion count must be exactly 0.
    let no_filter_result = db
        .search_diagrams_with_performance_exclusions("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert_eq!(no_filter_result.excluded_for_missing_curves, 0);
    assert_eq!(no_filter_result.items.len(), 4);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn performance_filters_combine_by_and() {
    let path = temp_db_path("performance_filter_and");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    // Passes the windowing filter but not the brilliance one.
    let only_windowing_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Only Windowing Passes".to_string(),
                url: "local://only-windowing.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(
        only_windowing_id,
        &flat_tilt_curves(10.0), // windowing 10<=20 pass, brilliance 10<50 fails AtLeast(50)
        None,
        1,
    )
    .unwrap();

    // Passes both.
    let both_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Passes Both".to_string(),
                url: "local://both.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    // windowing and brilliance are separate metrics, so windowing=10 + brilliance=60 works.
    let mut curves = flat_tilt_curves(10.0);
    for axis in &mut curves.axes {
        axis.brilliance_pct = [60.0; crate::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS];
    }
    db.save_tilt_curves(both_id, &curves, None, 1).unwrap();

    let filter = RangeFilter {
        performance: vec![
            crate::model::performance::PerformanceFilter {
                metric: crate::model::performance::PerformanceMetric::Windowing,
                bound: crate::model::performance::PerformanceBound::AtMost(20.0),
                tilt_radius_deg: 45.0,
                aggregate: crate::model::performance::PerformanceAggregate::Worst,
            },
            crate::model::performance::PerformanceFilter {
                metric: crate::model::performance::PerformanceMetric::Brilliance,
                bound: crate::model::performance::PerformanceBound::AtLeast(50.0),
                tilt_radius_deg: 45.0,
                aggregate: crate::model::performance::PerformanceAggregate::Worst,
            },
        ],
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &filter).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Passes Both"],
        "combined filters must AND together, not OR"
    );

    let _ = std::fs::remove_file(&path);
}

// ---- Pruning soundness: the SQL global-min/max narrowing must never disagree with a
// brute-force per-design scan ------------------------------------------------------

/// A tiny deterministic PRNG (xorshift64): this crate stays dependency-lean (no `rand`)
/// and tests must be deterministic, so a fixed seed exercises the same curve data every run.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A pseudo-random `f32` in `[0.0, 100.0)`, matching `AxisTiltCurves`' `*_pct` scale.
    fn next_pct(&mut self) -> f32 {
        let top_24_bits = (self.next_u64() >> 40) as u32;
        (f64::from(top_24_bits) / f64::from(1u32 << 24) * 100.0) as f32
    }
}

/// Builds a fully-randomised `TiltPerformanceCurves` (every sample independently drawn)
/// -- deliberately not flat like `flat_tilt_curves`, to exercise the general case where
/// a window's value depends on which points fall inside it.
fn randomised_tilt_curves(
    rng: &mut XorShift64,
) -> crate::model::tilt_curves::TiltPerformanceCurves {
    use crate::model::tilt_curves::{
        AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS,
    };
    let mut axes = [AxisTiltCurves {
        brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
    }; TILT_CURVE_AXIS_COUNT];
    for axis in &mut axes {
        for curve in [
            &mut axis.brilliance_pct,
            &mut axis.extinction_pct,
            &mut axis.windowing_pct,
        ] {
            for sample in curve.iter_mut() {
                *sample = rng.next_pct();
            }
        }
    }
    crate::model::tilt_curves::TiltPerformanceCurves { axes }
}

/// The soundness property the two-stage (SQL-narrows, Rust-decides) design depends on:
/// searching with a `PerformanceFilter` active must return EXACTLY the same designs as
/// a brute-force scan that decodes every design's curves and evaluates the filter
/// directly, bypassing SQL narrowing. Catches wrongly-excluding pruning bugs; a
/// too-permissive pruning bug is harmless and invisible here by design (see
/// `build_search_predicate`'s doc comment -- the exact check is the source of truth).
#[test]
fn performance_filter_sql_narrowing_matches_a_brute_force_scan_exactly() {
    const DESIGN_COUNT: usize = 24;

    let path = temp_db_path("performance_pruning_soundness");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let mut rng = XorShift64(0x9E37_79B9_7F4A_7C15);

    let mut all_entry_ids = Vec::with_capacity(DESIGN_COUNT);
    for i in 0..DESIGN_COUNT {
        let entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: format!("Randomised {i}"),
                    url: format!("local://randomised-{i}.asc"),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        // Every third design gets no curves, covering exclusion consistency too.
        if i % 3 != 0 {
            let curves = randomised_tilt_curves(&mut rng);
            db.save_tilt_curves(entry_id, &curves, None, 1).unwrap();
        }
        all_entry_ids.push(entry_id);
    }

    let cases = [
        (
            crate::model::performance::PerformanceMetric::Brilliance,
            crate::model::performance::PerformanceBound::AtLeast(30.0),
            0.0_f32,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Windowing,
            crate::model::performance::PerformanceBound::AtMost(70.0),
            12.5,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Extinction,
            crate::model::performance::PerformanceBound::AtLeast(50.0),
            37.0,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Brilliance,
            crate::model::performance::PerformanceBound::AtMost(60.0),
            90.0,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Windowing,
            crate::model::performance::PerformanceBound::AtMost(55.0),
            45.0,
            crate::model::performance::PerformanceAggregate::Mean,
        ),
        (
            crate::model::performance::PerformanceMetric::Brilliance,
            crate::model::performance::PerformanceBound::AtLeast(45.0),
            22.0,
            crate::model::performance::PerformanceAggregate::Mean,
        ),
    ];

    for (metric, bound, radius, aggregate) in cases {
        let filter =
            crate::model::performance::PerformanceFilter::new(metric, bound, radius, aggregate)
                .unwrap();

        // Pruned path: search narrows in SQL via the 6 global min/max columns first.
        let pruned: std::collections::BTreeSet<i64> = db
            .search_diagrams(
                "",
                "All",
                "All",
                &RangeFilter {
                    performance: vec![filter],
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|item| item.id)
            .collect();

        // Brute-force path: decode every design's curves, evaluate the exact predicate.
        let brute: std::collections::BTreeSet<i64> = all_entry_ids
            .iter()
            .copied()
            .filter(|&id| {
                db.get_tilt_curves(id)
                    .unwrap()
                    .is_some_and(|curves| curves.matches_performance_filter(&filter))
            })
            .collect();

        assert_eq!(
            pruned, brute,
            "SQL-narrowed search and a brute-force scan disagreed for {metric:?} \
             {bound:?} radius={radius} {aggregate:?} -- the SQL narrowing predicate is \
             unsound (it excluded a design the exact check would have kept, or vice \
             versa)"
        );
    }

    let _ = std::fs::remove_file(&path);
}

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
