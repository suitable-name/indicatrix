//! A DEFAULT build (no `zoning` feature) must leave the rough-colour side tables alone: it
//! never creates them, and opening, saving, renaming and deleting plans, materials and jobs
//! neither reads nor deletes the rows a zoning build left in a shared file. Compiled only
//! without the feature (the zoning build's own tests are in `zoning_store`).

use super::{super::*, fixtures::temp_db_path};

/// The side tables, written by hand because the zoning module is not compiled here.
const SIDE_TABLES_SQL: &str = "
    CREATE TABLE rough_colour (
        plan_id INTEGER PRIMARY KEY, version INTEGER NOT NULL, created INTEGER NOT NULL,
        zoned_json TEXT NOT NULL, fit_json TEXT NOT NULL);
    CREATE TABLE material_zoning (
        material_name TEXT PRIMARY KEY COLLATE NOCASE, relative_to_stone INTEGER NOT NULL DEFAULT 0,
        version INTEGER NOT NULL, zoned_json TEXT NOT NULL);
    CREATE TABLE render_job_zoning (job_id INTEGER PRIMARY KEY, zoned_json TEXT NOT NULL);
";

fn count(db: &Database, table: &str) -> i64 {
    db.conn
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn a_fresh_default_database_has_no_zoning_tables() {
    let db = Database::new(Some(":memory:")).expect("create");
    let found: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN
             ('rough_colour', 'rough_colour_photo', 'stone_pose_choice',
              'material_zoning', 'render_job_zoning')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(found, 0);
}

#[test]
fn a_default_build_leaves_the_side_tables_and_their_rows_untouched() {
    let path = temp_db_path("zoning_default_untouched");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        db.conn.execute_batch(SIDE_TABLES_SQL).unwrap();
        let plan = db.save_rough_plan("Plan", 1, "{}", "s", 10).unwrap();
        db.conn
            .execute(
                "INSERT INTO rough_colour VALUES (?1, 1, 5, '{}', '{}')",
                [plan],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO material_zoning VALUES ('X colour', 0, 1, '{}')",
                [],
            )
            .unwrap();
        db.conn
            .execute("INSERT INTO render_job_zoning VALUES (9, '{}')", [])
            .unwrap();
    }
    // Reopening in the default build migrates, renames, re-saves and deletes the owners.
    let db = Database::new(Some(path_text)).expect("reopen");
    let plan = 1;
    db.rename_saved_rough_plan(plan, "Renamed", None, 20)
        .unwrap();
    db.save_custom_material(&CustomMaterialParams {
        name: "X colour",
        refractive_index: 1.7,
        dispersion: 0.02,
        birefringence: 0.0,
        absorption_rgb: [0.0; 3],
        crystal_system: None,
        optical_character: None,
        biaxial_delta_beta_alpha: None,
        per_axis_dispersion_json: None,
        specific_gravity: None,
        color_recipe_json: None,
        dispersion_model_json: None,
        absorption_bands_json: None,
    })
    .unwrap();
    db.delete_custom_material("X colour").unwrap();
    db.delete_saved_rough_plan(plan).unwrap();
    assert_eq!(count(&db, "rough_colour"), 1);
    assert_eq!(count(&db, "material_zoning"), 1);
    assert_eq!(count(&db, "render_job_zoning"), 1);
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
