//! The three per-design side tables (`design_variants`, `design_cut_progress`,
//! `design_lighting`): created on a fresh database, added to an existing one that lacks
//! them, and idempotent however often the migrations run.

use super::{
    super::*,
    fixtures::{seed_pre_migration_db, temp_db_path},
};
use crate::model::{
    cut_progress::CutProgressMark,
    design_variant::{NewVariant, VariantSummary},
};

const DESIGN: &str = "0b9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10";

const VARIANT_COLUMNS: [&str; 8] = [
    "variant_id",
    "design_uuid",
    "name",
    "parent_variant_id",
    "created_at",
    "note",
    "design_text",
    "thumbnail_png",
];
const PROGRESS_COLUMNS: [&str; 4] = ["design_uuid", "step_key", "step_signature", "done_at"];
const LIGHTING_COLUMNS: [&str; 4] = ["design_uuid", "preset_name", "settings_json", "updated_at"];

fn assert_side_tables_exist(db: &Database) {
    for (table, columns) in [
        ("design_variants", &VARIANT_COLUMNS[..]),
        ("design_cut_progress", &PROGRESS_COLUMNS[..]),
        ("design_lighting", &LIGHTING_COLUMNS[..]),
    ] {
        for column in columns {
            assert!(
                Database::column_exists(&db.conn, table, column).unwrap(),
                "{table} must have column {column}"
            );
        }
    }
    let index_count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'index' AND name = 'idx_design_variants_design_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(index_count, 1, "the design_uuid index");
}

/// One row in each table, so a later open can prove it kept them.
fn write_one_of_each(db: &Database) -> i64 {
    let variant = db
        .save_variant(
            DESIGN,
            &NewVariant {
                name: "Kept",
                parent_variant_id: None,
                note: None,
                design_text: "text",
                thumbnail_png: None,
                created_at: 1,
            },
        )
        .unwrap();
    db.mark_step_done(DESIGN, "tier-1", "sig", 2).unwrap();
    db.set_design_lighting(DESIGN, "Studio", "{}", 3).unwrap();
    variant
}

fn assert_one_of_each_survived(db: &Database, variant: i64) {
    let kept = db.load_variant(variant).unwrap().expect("variant kept");
    assert_eq!(kept.summary.name, "Kept");
    assert_eq!(kept.design_text, "text");
    assert_eq!(db.cut_progress(DESIGN).unwrap().len(), 1);
    assert_eq!(
        db.design_lighting(DESIGN).unwrap().unwrap().preset_name,
        "Studio"
    );
}

#[test]
fn a_fresh_database_already_has_the_per_design_tables() {
    let path = temp_db_path("fresh_design_side_tables");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    assert_side_tables_exist(&db);
    assert_eq!(
        db.list_variants(DESIGN).unwrap(),
        Vec::<VariantSummary>::new()
    );
    assert_eq!(
        db.cut_progress(DESIGN).unwrap(),
        Vec::<CutProgressMark>::new()
    );
    assert_eq!(db.design_lighting(DESIGN).unwrap(), None);
    drop(db);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn migrating_an_old_database_adds_the_per_design_tables_and_keeps_its_rows() {
    let path = temp_db_path("migrate_design_side_tables");
    seed_pre_migration_db(&path, &[("A", "url-a", None, None, None, None, None)]);

    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    assert_side_tables_exist(&db);
    let entry_count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM diagram_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(entry_count, 1, "the old catalogue rows are untouched");
    assert_eq!(
        db.list_variants(DESIGN).unwrap(),
        Vec::<VariantSummary>::new()
    );
    drop(db);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_database_that_lacks_the_tables_gets_them_on_the_next_open_and_a_second_open_changes_nothing() {
    let path = temp_db_path("design_side_tables_dropped");
    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
        db.conn
            .execute_batch(
                "DROP TABLE design_variants;
                 DROP TABLE design_cut_progress;
                 DROP TABLE design_lighting;",
            )
            .unwrap();
        assert!(!Database::column_exists(&db.conn, "design_variants", "name").unwrap());
    }

    let first = Database::new(Some(path.to_str().unwrap())).expect("first open migrates");
    assert_side_tables_exist(&first);
    let variant = write_one_of_each(&first);
    drop(first);

    let second = Database::new(Some(path.to_str().unwrap())).expect("second open is a no-op");
    assert_side_tables_exist(&second);
    assert_one_of_each_survived(&second, variant);
    drop(second);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn re_running_each_migration_keeps_the_rows() {
    let path = temp_db_path("design_side_tables_rerun");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    let variant = write_one_of_each(&db);

    for _ in 0..2 {
        db.migrate_design_variants_table().expect("variants");
        db.migrate_design_cut_progress_table().expect("progress");
        db.migrate_design_lighting_table().expect("lighting");
    }

    assert_side_tables_exist(&db);
    assert_one_of_each_survived(&db, variant);
    drop(db);
    let _ = std::fs::remove_file(&path);
}

/// The tables exist alongside everything else on an in-memory database too, the way the
/// other crates' tests open it.
#[test]
fn an_in_memory_database_has_the_per_design_tables() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");
    assert_side_tables_exist(&db);
}
