//! The frosting of a library design (`diagram_frosting`): the round trip, replacing and
//! clearing it, its independence from `updated_at`, the cascade on delete, the migration
//! that adds the table to an old database, and that a design without frosting leaves every
//! row it had untouched.

use super::{
    super::*,
    fixtures::{seed_pre_migration_db, temp_db_path},
};
use crate::model::frosting::StoredTierFrosting;

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

/// Saves one local entry with an (empty) detail row and returns its id.
fn save_entry(db: &Database, title: &str) -> i64 {
    db.save_design(
        &FacetingDiagramEntry {
            title: title.to_string(),
            url: format!("local://{}.asc", title.to_lowercase().replace(' ', "-")),
            design_id: String::new(),
        },
        &FacetingDiagramDetail::default(),
        "local-import",
    )
    .expect("save design")
}

fn sample() -> Vec<StoredTierFrosting> {
    vec![
        StoredTierFrosting::whole(0),
        StoredTierFrosting {
            tier: 2,
            facets: vec![1, 3],
        },
    ]
}

fn frosting_row_count(db: &Database) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM diagram_frosting", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_design_without_frosting_reads_as_unfrosted() {
    let db = in_memory_db();
    let id = save_entry(&db, "Plain");
    assert_eq!(db.diagram_frosting(id).unwrap().len(), 0);
    assert_eq!(db.get_diagram_full(id).unwrap().unwrap().frosting.len(), 0);
    assert_eq!(
        db.get_diagram_full_meta(id)
            .unwrap()
            .unwrap()
            .frosting
            .len(),
        0
    );
    assert_eq!(frosting_row_count(&db), 0);
}

#[test]
fn the_frosting_round_trips_through_the_table_and_both_record_readers() {
    let db = in_memory_db();
    let id = save_entry(&db, "Frosted");
    db.set_diagram_frosting(id, &sample()).unwrap();

    assert_eq!(db.diagram_frosting(id).unwrap(), sample());
    assert_eq!(db.get_diagram_full(id).unwrap().unwrap().frosting, sample());
    assert_eq!(
        db.get_diagram_full_meta(id).unwrap().unwrap().frosting,
        sample()
    );
    assert_eq!(frosting_row_count(&db), 2, "one row per frosted tier");
}

#[test]
fn setting_the_frosting_replaces_it_and_an_empty_list_clears_it() {
    let db = in_memory_db();
    let a = save_entry(&db, "Alpha");
    let b = save_entry(&db, "Beta");
    db.set_diagram_frosting(a, &sample()).unwrap();
    db.set_diagram_frosting(b, &[StoredTierFrosting::whole(4)])
        .unwrap();

    db.set_diagram_frosting(a, &[StoredTierFrosting::whole(1)])
        .unwrap();
    assert_eq!(
        db.diagram_frosting(a).unwrap(),
        vec![StoredTierFrosting::whole(1)],
        "the earlier tiers are gone, not merged"
    );
    assert_eq!(
        db.diagram_frosting(b).unwrap(),
        vec![StoredTierFrosting::whole(4)],
        "another design's frosting is untouched"
    );

    db.set_diagram_frosting(a, &[]).unwrap();
    assert_eq!(db.diagram_frosting(a).unwrap().len(), 0);
    assert_eq!(frosting_row_count(&db), 1);
}

#[test]
fn the_list_is_normalised_on_the_way_in() {
    let db = in_memory_db();
    let id = save_entry(&db, "Unsorted");
    db.set_diagram_frosting(
        id,
        &[
            StoredTierFrosting {
                tier: 5,
                facets: vec![3, 1, 3],
            },
            StoredTierFrosting::whole(2),
        ],
    )
    .unwrap();
    assert_eq!(
        db.diagram_frosting(id).unwrap(),
        vec![
            StoredTierFrosting::whole(2),
            StoredTierFrosting {
                tier: 5,
                facets: vec![1, 3]
            },
        ]
    );
}

#[test]
fn frosting_an_unknown_entry_fails_through_the_foreign_key_and_writes_nothing() {
    let db = in_memory_db();
    let id = save_entry(&db, "Real");
    db.set_diagram_frosting(id, &sample()).unwrap();
    let err = db
        .set_diagram_frosting(id + 1000, &sample())
        .expect_err("no such design");
    assert!(format!("{err:#}").contains("frosting"), "{err:#}");
    assert_eq!(
        frosting_row_count(&db),
        2,
        "the real design's rows are unchanged"
    );
    // Clearing a design that does not exist is a harmless no-op.
    db.set_diagram_frosting(id + 1000, &[]).unwrap();
}

#[test]
fn setting_the_frosting_does_not_touch_the_revision_stamp() {
    let db = in_memory_db();
    let id = save_entry(&db, "Stamped");
    let before = db.entry_updated_at(id).unwrap();
    db.set_diagram_frosting(id, &sample()).unwrap();
    db.set_diagram_frosting(id, &[]).unwrap();
    assert_eq!(db.entry_updated_at(id).unwrap(), before);
}

#[test]
fn a_resave_of_the_design_keeps_the_frosting_and_a_delete_removes_it() {
    let db = in_memory_db();
    let id = save_entry(&db, "Kept");
    db.set_diagram_frosting(id, &sample()).unwrap();

    // `save_design` replaces the detail rows under the same entry (a re-import by url).
    let again = save_entry(&db, "Kept");
    assert_eq!(again, id, "the same url reuses the row");
    assert_eq!(db.diagram_frosting(id).unwrap(), sample());

    db.delete_diagram_entry(id).unwrap();
    assert_eq!(
        frosting_row_count(&db),
        0,
        "the cascade leaves no orphan row"
    );
}

#[test]
fn storing_no_frosting_changes_none_of_the_design_rows() {
    let db = in_memory_db();
    let id = save_entry(&db, "Untouched");
    let snapshot = |db: &Database| {
        let full = db.get_diagram_full(id).unwrap().unwrap();
        (
            full.title,
            full.url,
            full.angle_settings.len(),
            full.attached_files.len(),
            db.entry_updated_at(id).unwrap(),
        )
    };
    let before = snapshot(&db);
    db.set_diagram_frosting(id, &[]).unwrap();
    assert_eq!(snapshot(&db), before);
    assert_eq!(
        frosting_row_count(&db),
        0,
        "an unfrosted design has no row at all"
    );
}

/// The one-query read of the staleness scan equals the per-design reads: it lists exactly the
/// frosted designs, each with its normalised rows, and nothing for an unfrosted one.
#[test]
fn all_diagram_frosting_equals_the_per_design_reads() {
    let db = in_memory_db();
    let a = save_entry(&db, "Alpha");
    let b = save_entry(&db, "Beta");
    let plain = save_entry(&db, "Plain");
    assert!(db.all_diagram_frosting().unwrap().is_empty());

    db.set_diagram_frosting(a, &sample()).unwrap();
    db.set_diagram_frosting(
        b,
        &[
            StoredTierFrosting {
                tier: 5,
                facets: vec![3, 1, 3],
            },
            StoredTierFrosting::whole(2),
        ],
    )
    .unwrap();

    let all = db.all_diagram_frosting().unwrap();
    assert_eq!(all.len(), 2, "the unfrosted design is absent");
    assert!(!all.contains_key(&plain));
    for id in [a, b] {
        assert_eq!(all[&id], db.diagram_frosting(id).unwrap(), "entry {id}");
    }
    assert_eq!(db.diagram_frosting(plain).unwrap().len(), 0);
}

#[test]
fn all_diagram_frosting_is_empty_without_the_table() {
    let path = temp_db_path("all_frosting_no_table");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create");
    save_entry(&db, "Served");
    db.conn
        .execute_batch("DROP TABLE diagram_frosting;")
        .expect("drop the table");
    let ro = Database::open_read_only(path.to_str().unwrap()).expect("open read-only");
    assert!(ro.all_diagram_frosting().unwrap().is_empty());

    drop(ro);
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn migrating_an_old_database_adds_the_table_and_every_design_reads_as_unfrosted() {
    let path = temp_db_path("migrate_frosting");
    seed_pre_migration_db(&path, &[("A", "url-a", None, None, None, None, None)]);

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    for column in ["entry_id", "tier", "facet_slots"] {
        assert!(
            Database::column_exists(&db.conn, "diagram_frosting", column).unwrap(),
            "migration must create diagram_frosting.{column}"
        );
    }
    assert!(
        db.diagram_frosting(1).unwrap().is_empty(),
        "a design that predates the table is unfrosted"
    );
    assert_eq!(db.get_diagram_full(1).unwrap().unwrap().frosting.len(), 0);

    // The migrated table is live, and re-running the migration keeps what it holds.
    db.set_diagram_frosting(1, &sample()).unwrap();
    db.migrate_diagram_frosting_table()
        .expect("re-run migration");
    assert_eq!(db.diagram_frosting(1).unwrap(), sample());
    drop(db);

    // A second open (the migration runs again) keeps it too.
    let db = Database::new(Some(path.to_str().unwrap())).expect("second open");
    assert_eq!(db.diagram_frosting(1).unwrap(), sample());
    drop(db);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_current_database_that_lacks_only_the_table_gets_it_back_on_open() {
    let path = temp_db_path("frosting_table_dropped");
    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("create");
        save_entry(&db, "Old");
        db.conn
            .execute_batch("DROP TABLE diagram_frosting;")
            .expect("drop the table");
    }
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    assert_eq!(db.diagram_frosting(1).unwrap().len(), 0);
    db.set_diagram_frosting(1, &sample()).unwrap();
    assert_eq!(db.diagram_frosting(1).unwrap(), sample());
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn a_read_only_connection_to_a_database_without_the_table_reads_as_unfrosted() {
    let path = temp_db_path("frosting_read_only_old");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create");
    let id = save_entry(&db, "Served");
    db.conn
        .execute_batch("DROP TABLE diagram_frosting;")
        .expect("drop the table");

    // `open_read_only` runs no migration, like a worker serving a file this build never opened.
    let ro = Database::open_read_only(path.to_str().unwrap()).expect("open read-only");
    assert_eq!(ro.diagram_frosting(id).unwrap().len(), 0);
    let full = ro.get_diagram_full(id).unwrap().expect("a record");
    assert_eq!(full.frosting.len(), 0);
    assert_eq!(
        ro.get_diagram_full_meta(id)
            .unwrap()
            .unwrap()
            .frosting
            .len(),
        0
    );

    drop(ro);
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
