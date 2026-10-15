//! The rough-colour side tables: creation, round trips, the read-only
//! behaviour, orphan pruning, and that opening twice keeps what was stored.

use super::{super::*, fixtures::temp_db_path};
use crate::model::zoning::{
    MaterialZoningRow, RoughColourPhotoMeta, RoughColourPhotoRow, RoughColourRow,
    StonePoseChoiceRow,
};
use std::collections::BTreeSet;

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

fn table_names(db: &Database) -> BTreeSet<String> {
    let mut stmt = db
        .conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<BTreeSet<_>>>()
        .unwrap()
}

fn colour(plan_id: i64, tag: &str) -> RoughColourRow {
    RoughColourRow {
        plan_id,
        zoned_json: format!("{{\"zoned\":\"{tag}\"}}"),
        fit_json: format!("{{\"fit\":\"{tag}\"}}"),
        version: 1,
        created: 1_700_000_000,
    }
}

fn photo(plan_id: i64, view: u32, kind: &str, bytes: Vec<u8>) -> RoughColourPhotoRow {
    RoughColourPhotoRow {
        plan_id,
        view,
        kind: kind.to_string(),
        width: 2,
        height: 3,
        encoding: "f16le".to_string(),
        data: bytes,
    }
}

fn material(name: &str, relative: bool) -> MaterialZoningRow {
    MaterialZoningRow {
        material_name: name.to_string(),
        zoned_json: "{\"z\":1}".to_string(),
        relative_to_stone: relative,
        version: 1,
    }
}

#[test]
fn a_new_database_has_all_five_side_tables_and_reopening_keeps_them() {
    let path = temp_db_path("zoning_tables");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        let tables = table_names(&db);
        for name in [
            "rough_colour",
            "rough_colour_photo",
            "stone_pose_choice",
            "material_zoning",
            "render_job_zoning",
        ] {
            assert!(tables.contains(name), "missing {name}");
        }
        db.save_rough_colour(&colour(7, "a")).unwrap();
    }
    let db = Database::new(Some(path_text)).expect("reopen");
    assert_eq!(db.load_rough_colour(7).unwrap(), Some(colour(7, "a")));
    drop(db);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn rough_colour_round_trips_replaces_lists_and_deletes_with_its_children() {
    let db = in_memory_db();
    assert_eq!(db.load_rough_colour(1).unwrap(), None);
    db.save_rough_colour(&colour(1, "first")).unwrap();
    db.save_rough_colour(&colour(2, "second")).unwrap();
    assert_eq!(db.load_rough_colour(1).unwrap(), Some(colour(1, "first")));
    db.save_rough_colour(&colour(1, "again")).unwrap();
    assert_eq!(db.load_rough_colour(1).unwrap(), Some(colour(1, "again")));
    assert_eq!(db.rough_colour_plan_ids().unwrap(), BTreeSet::from([1, 2]));

    db.save_rough_colour_photo(&photo(1, 0, "transmittance", vec![1, 2, 3]))
        .unwrap();
    db.set_stone_pose_choice(1, 0, 2, 3).unwrap();
    db.delete_rough_colour(1).unwrap();
    assert_eq!(db.load_rough_colour(1).unwrap(), None);
    assert_eq!(
        db.list_rough_colour_photos(1).unwrap(),
        [] as [RoughColourPhotoMeta; 0]
    );
    assert_eq!(
        db.stone_pose_choices(1).unwrap(),
        [] as [StonePoseChoiceRow; 0]
    );
    assert_eq!(db.load_rough_colour(2).unwrap(), Some(colour(2, "second")));
    // Deleting what is not there succeeds.
    db.delete_rough_colour(99).unwrap();
}

#[test]
fn photos_round_trip_bit_exactly_and_list_without_their_bytes() {
    let db = in_memory_db();
    let bytes: Vec<u8> = (0..=255).collect();
    db.save_rough_colour_photo(&photo(3, 1, "variance", bytes.clone()))
        .unwrap();
    db.save_rough_colour_photo(&photo(3, 0, "transmittance", vec![9; 12]))
        .unwrap();
    let loaded = db
        .load_rough_colour_photo(3, 1, "variance")
        .unwrap()
        .expect("stored");
    assert_eq!(loaded.data, bytes);
    assert_eq!((loaded.width, loaded.height), (2, 3));
    assert_eq!(loaded.encoding, "f16le");
    assert_eq!(db.load_rough_colour_photo(3, 1, "mask").unwrap(), None);

    let listing = db.list_rough_colour_photos(3).unwrap();
    assert_eq!(listing.len(), 2);
    assert_eq!(
        (listing[0].view, listing[0].kind.as_str()),
        (0, "transmittance")
    );
    assert_eq!(listing[0].byte_len, 12);
    assert_eq!(listing[1].byte_len, 256);

    // Replacing keeps one row.
    db.save_rough_colour_photo(&photo(3, 0, "transmittance", vec![1]))
        .unwrap();
    assert_eq!(db.list_rough_colour_photos(3).unwrap().len(), 2);
    assert_eq!(db.delete_rough_colour_photos(3).unwrap(), 2);
}

#[test]
fn pose_choices_store_only_non_canonical_poses() {
    let db = in_memory_db();
    db.set_stone_pose_choice(5, 1, 4, 2).unwrap();
    db.set_stone_pose_choice(5, 0, 7, 3).unwrap();
    db.set_stone_pose_choice(6, 0, 0, 1).unwrap();
    assert_eq!(
        db.stone_pose_choices(5).unwrap(),
        vec![
            StonePoseChoiceRow {
                layout_index: 0,
                stone_index: 7,
                pose: 3
            },
            StonePoseChoiceRow {
                layout_index: 1,
                stone_index: 4,
                pose: 2
            },
        ]
    );
    // Back to canonical removes the row.
    db.set_stone_pose_choice(5, 1, 4, 0).unwrap();
    assert_eq!(db.stone_pose_choices(5).unwrap().len(), 1);
    // Changing a choice replaces it.
    db.set_stone_pose_choice(5, 0, 7, 1).unwrap();
    assert_eq!(db.stone_pose_choices(5).unwrap()[0].pose, 1);
    assert_eq!(db.clear_stone_pose_choices(5).unwrap(), 1);
    assert_eq!(db.stone_pose_choices(6).unwrap().len(), 1);
}

#[test]
fn material_zoning_is_keyed_by_name_ignoring_ascii_case() {
    let db = in_memory_db();
    db.save_material_zoning(&material("Rough 1 colour", false))
        .unwrap();
    db.save_material_zoning(&material("Library Bicolour", true))
        .unwrap();
    let found = db
        .load_material_zoning("rough 1 COLOUR")
        .unwrap()
        .expect("case-insensitive");
    assert!(!found.relative_to_stone);
    assert!(
        db.load_material_zoning("library bicolour")
            .unwrap()
            .unwrap()
            .relative_to_stone
    );
    // The same name in another case replaces, it does not add a row.
    db.save_material_zoning(&material("ROUGH 1 COLOUR", true))
        .unwrap();
    assert_eq!(db.all_material_zonings().unwrap().len(), 2);
    assert!(
        db.load_material_zoning("Rough 1 colour")
            .unwrap()
            .unwrap()
            .relative_to_stone
    );
    assert_eq!(db.delete_material_zoning("library BICOLOUR").unwrap(), 1);
    assert_eq!(db.delete_material_zoning("library BICOLOUR").unwrap(), 0);
}

#[test]
fn render_job_zoning_round_trips() {
    let db = in_memory_db();
    assert_eq!(db.load_render_job_zoning(4).unwrap(), None);
    db.save_render_job_zoning(4, 1, "{\"a\":1}").unwrap();
    db.save_render_job_zoning(4, 2, "{\"a\":2}").unwrap();
    assert_eq!(
        db.load_render_job_zoning(4).unwrap(),
        Some((2, "{\"a\":2}".to_string()))
    );
    assert_eq!(db.delete_render_job_zoning(4).unwrap(), 1);
    assert_eq!(db.load_render_job_zoning(4).unwrap(), None);
}

#[test]
fn a_render_job_table_without_the_version_column_is_migrated_and_read_as_version_one() {
    let path = temp_db_path("zoning_job_version_column");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        db.conn
            .execute_batch(
                "DROP TABLE render_job_zoning;
                 CREATE TABLE render_job_zoning (
                    job_id INTEGER PRIMARY KEY,
                    zoned_json TEXT NOT NULL
                 );
                 INSERT INTO render_job_zoning (job_id, zoned_json) VALUES (8, '{\"old\":1}');",
            )
            .unwrap();
    }
    // A read-only open runs no migration: the old row reads as version 1.
    let ro = Database::open_read_only(path_text).expect("open read-only");
    assert_eq!(
        ro.load_render_job_zoning(8).unwrap(),
        Some((1, "{\"old\":1}".to_string()))
    );
    drop(ro);
    // A normal open adds the column, keeps the row, and stores real versions from then on.
    let db = Database::new(Some(path_text)).expect("reopen");
    assert_eq!(
        db.load_render_job_zoning(8).unwrap(),
        Some((1, "{\"old\":1}".to_string()))
    );
    db.save_render_job_zoning(9, 3, "{\"new\":1}").unwrap();
    assert_eq!(
        db.load_render_job_zoning(9).unwrap(),
        Some((3, "{\"new\":1}".to_string()))
    );
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn replacing_a_rough_colour_swaps_its_photos_and_removes_the_stale_views() {
    let db = in_memory_db();
    db.save_rough_colour(&colour(1, "old")).unwrap();
    db.save_rough_colour_photo(&photo(1, 0, "transmittance", vec![1]))
        .unwrap();
    db.save_rough_colour_photo(&photo(1, 5, "transmittance", vec![5]))
        .unwrap();
    db.save_rough_colour_photo(&photo(2, 0, "transmittance", vec![7]))
        .unwrap();
    db.set_stone_pose_choice(1, 0, 0, 2).unwrap();

    db.replace_rough_colour(
        &colour(1, "new"),
        &[
            photo(1, 0, "transmittance", vec![10, 11]),
            photo(1, 0, "variance", vec![12]),
        ],
    )
    .unwrap();

    assert_eq!(db.load_rough_colour(1).unwrap(), Some(colour(1, "new")));
    let listing = db.list_rough_colour_photos(1).unwrap();
    assert_eq!(
        listing
            .iter()
            .map(|meta| (meta.view, meta.kind.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "transmittance"), (0, "variance")],
        "view 5 was not prepared this time and must be gone"
    );
    assert_eq!(
        db.load_rough_colour_photo(1, 0, "transmittance")
            .unwrap()
            .unwrap()
            .data,
        vec![10, 11]
    );
    // Another plan's photos and this plan's pose choices are untouched.
    assert_eq!(db.list_rough_colour_photos(2).unwrap().len(), 1);
    assert_eq!(db.stone_pose_choices(1).unwrap().len(), 1);

    // An empty photo list is a valid replacement: the colour with no cached photos.
    db.replace_rough_colour(&colour(1, "bare"), &[]).unwrap();
    assert_eq!(db.list_rough_colour_photos(1).unwrap().len(), 0);
}

#[test]
fn a_failing_photo_rolls_the_whole_replacement_back() {
    let db = in_memory_db();
    db.save_rough_colour(&colour(1, "old")).unwrap();
    db.save_rough_colour_photo(&photo(1, 0, "transmittance", vec![1, 2, 3]))
        .unwrap();
    // A trigger makes the second insert fail after the colour and the first photo went in.
    db.conn
        .execute_batch(
            "CREATE TRIGGER refuse_boom BEFORE INSERT ON rough_colour_photo
             WHEN NEW.kind = 'boom'
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
    let result = db.replace_rough_colour(
        &colour(1, "new"),
        &[
            photo(1, 0, "transmittance", vec![9]),
            photo(1, 1, "boom", vec![9]),
        ],
    );
    assert!(result.is_err());
    assert_eq!(db.load_rough_colour(1).unwrap(), Some(colour(1, "old")));
    let kept = db.list_rough_colour_photos(1).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        db.load_rough_colour_photo(1, 0, "transmittance")
            .unwrap()
            .unwrap()
            .data,
        vec![1, 2, 3]
    );
}

#[test]
fn a_photo_of_another_plan_is_refused_before_anything_is_written() {
    let db = in_memory_db();
    let result = db.replace_rough_colour(&colour(1, "x"), &[photo(2, 0, "transmittance", vec![1])]);
    assert!(result.is_err());
    assert_eq!(db.load_rough_colour(1).unwrap(), None);
}

#[test]
fn pruning_removes_only_rows_whose_owner_is_gone() {
    let db = in_memory_db();
    let kept_plan = db.save_rough_plan("Kept", 1, "{}", "s", 10).unwrap();
    let gone_plan = db.save_rough_plan("Gone", 1, "{}", "s", 10).unwrap();
    for plan in [kept_plan, gone_plan] {
        db.save_rough_colour(&colour(plan, "x")).unwrap();
        db.save_rough_colour_photo(&photo(plan, 0, "transmittance", vec![1]))
            .unwrap();
        db.set_stone_pose_choice(plan, 0, 1, 2).unwrap();
    }
    db.save_custom_material(&CustomMaterialParams {
        name: "Kept colour",
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
    db.save_material_zoning(&material("Kept colour", false))
        .unwrap();
    db.save_material_zoning(&material("Deleted colour", false))
        .unwrap();
    db.save_render_job_zoning(404, 1, "{}").unwrap();

    assert_eq!(db.delete_saved_rough_plan(gone_plan).unwrap(), 1);
    let removed = db.prune_zoning_orphans().unwrap();
    // gone plan: colour + photo + pose = 3; deleted material = 1; missing job = 1.
    assert_eq!(removed, 5);
    assert_eq!(
        db.rough_colour_plan_ids().unwrap(),
        BTreeSet::from([kept_plan])
    );
    assert_eq!(db.list_rough_colour_photos(kept_plan).unwrap().len(), 1);
    assert_eq!(db.stone_pose_choices(kept_plan).unwrap().len(), 1);
    assert!(db.load_material_zoning("Kept colour").unwrap().is_some());
    assert!(db.load_material_zoning("Deleted colour").unwrap().is_none());
    assert_eq!(db.load_render_job_zoning(404).unwrap(), None);
}

#[test]
fn a_read_only_connection_reads_what_is_there_and_refuses_writes() {
    let path = temp_db_path("zoning_read_only");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        db.save_rough_colour(&colour(1, "ro")).unwrap();
        db.save_material_zoning(&material("Ro colour", false))
            .unwrap();
    }
    let ro = Database::open_read_only(path_text).expect("open read-only");
    assert_eq!(ro.load_rough_colour(1).unwrap(), Some(colour(1, "ro")));
    assert!(ro.load_material_zoning("ro colour").unwrap().is_some());
    assert!(ro.save_rough_colour(&colour(2, "no")).is_err());
    drop(ro);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

fn plain_material(name: &str) -> CustomMaterialParams<'_> {
    CustomMaterialParams {
        name,
        refractive_index: 1.55,
        dispersion: 0.01,
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
    }
}

fn zoning_of(name: &str) -> MaterialZoningRow {
    MaterialZoningRow {
        material_name: name.to_string(),
        zoned_json: "{\"zones\":1}".to_string(),
        relative_to_stone: false,
        version: 1,
    }
}

#[test]
fn a_material_and_its_zones_are_written_together() {
    let db = in_memory_db();
    db.save_custom_material_with_zoning(
        &plain_material("Melon colour"),
        &zoning_of("Melon colour"),
    )
    .unwrap();
    assert_eq!(db.get_custom_materials().unwrap().len(), 1);
    assert_eq!(
        db.load_material_zoning("Melon colour").unwrap(),
        Some(zoning_of("Melon colour"))
    );
    // Replacing keeps one row of each.
    db.save_custom_material_with_zoning(
        &plain_material("Melon colour"),
        &zoning_of("Melon colour"),
    )
    .unwrap();
    assert_eq!(db.get_custom_materials().unwrap().len(), 1);
    assert_eq!(db.all_material_zonings().unwrap().len(), 1);
}

#[test]
fn a_refused_zoning_write_leaves_no_material_row_behind() {
    let db = in_memory_db();
    db.conn
        .execute_batch("DROP TABLE material_zoning;")
        .unwrap();
    let error = db
        .save_custom_material_with_zoning(
            &plain_material("Melon colour"),
            &zoning_of("Melon colour"),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("Melon colour"), "{error:#}");
    assert_eq!(
        db.get_custom_materials().unwrap(),
        [] as [crate::model::material::CustomMaterialRow; 0]
    );

    // A mismatching name is refused before anything is written.
    let db = in_memory_db();
    assert!(
        db.save_custom_material_with_zoning(&plain_material("A"), &zoning_of("B"))
            .is_err()
    );
    assert_eq!(db.get_custom_materials().unwrap().len(), 0);
}

#[test]
fn a_read_only_connection_to_a_file_without_the_tables_reads_nothing() {
    let path = temp_db_path("zoning_ro_without_tables");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        db.conn
            .execute_batch(
                "DROP TABLE rough_colour; DROP TABLE rough_colour_photo;
                 DROP TABLE stone_pose_choice; DROP TABLE material_zoning;
                 DROP TABLE render_job_zoning;",
            )
            .unwrap();
    }
    let ro = Database::open_read_only(path_text).expect("open read-only");
    assert_eq!(ro.load_rough_colour(1).unwrap(), None);
    assert_eq!(ro.rough_colour_plan_ids().unwrap(), BTreeSet::new());
    assert_eq!(
        ro.list_rough_colour_photos(1).unwrap(),
        [] as [RoughColourPhotoMeta; 0]
    );
    assert_eq!(
        ro.stone_pose_choices(1).unwrap(),
        [] as [StonePoseChoiceRow; 0]
    );
    assert_eq!(ro.load_material_zoning("x").unwrap(), None);
    assert_eq!(
        ro.all_material_zonings().unwrap(),
        [] as [MaterialZoningRow; 0]
    );
    assert_eq!(ro.load_render_job_zoning(1).unwrap(), None);
    // The deletes and the prune have nothing to remove from tables that are not there; they do
    // not fail with "no such table".
    assert_eq!(ro.delete_rough_colour_photos(1).unwrap(), 0);
    assert_eq!(ro.clear_stone_pose_choices(1).unwrap(), 0);
    assert_eq!(ro.delete_material_zoning("x").unwrap(), 0);
    assert_eq!(ro.delete_render_job_zoning(1).unwrap(), 0);
    ro.delete_rough_colour(1).unwrap();
    assert_eq!(ro.prune_zoning_orphans().unwrap(), 0);
    drop(ro);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
