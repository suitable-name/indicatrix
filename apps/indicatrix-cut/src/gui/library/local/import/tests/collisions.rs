//! Filename collisions on import: the merge-and-invalidate behaviour of a re-import, the
//! pre-import collision scan and the confirm decision built on its count.

use super::super::{
    confirm::{collisions_need_confirmation, count_pending_collisions},
    pipeline::import_path,
};
use crate::gui::library::local::helpers::test_support::{
    VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
};

/// Re-importing a `.asc` over an existing row (a filename collision) must not
/// silently wipe hand-entered metadata `local::import_asc` never produces
/// (`designer_info`, here), and must invalidate the stale preview image describing
/// the old geometry. Both are asserted here.
#[test]
fn import_path_merges_hand_entered_metadata_and_invalidates_stale_preview_on_collision() {
    let dir = temp_dir_for_test("reimport_merge");
    std::fs::write(dir.join("reimport.asc"), VALID_ASC).expect("write reimport.asc");

    let db_path = temp_db_path_for_test("reimport_merge");
    let db = open_temp_db(&db_path);

    let first = import_path(&db, &dir, false, |_, _| {});
    assert_eq!(
        first.imported_ids.len(),
        1,
        "first import must create one row"
    );
    let id = first.imported_ids[0];

    {
        let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Simulate a cutter hand-typing a designer name in the metadata editor, and a
        // preview batch having already generated an image for the old geometry.
        let full = conn
            .get_diagram_full(id)
            .expect("query must succeed")
            .expect("row must exist");
        let update = indicatrix_vault::model::metadata_update::MetadataUpdate {
            designer_info: Some("Test Designer".to_string()),
            shape: full.shape,
            refractive_index: full.refractive_index,
            index_gear: full.index_gear,
            facets_count: full.facets_count,
            symmetry_order: full.symmetry_order,
            mirror_symmetry: full.mirror_symmetry,
            lw_ratio: full.lw_ratio,
            hw_ratio: full.hw_ratio,
            cw_ratio: full.cw_ratio,
            pw_ratio: full.pw_ratio,
            volume: full.volume,
        };
        conn.update_diagram_metadata(id, &update)
            .expect("metadata update must succeed");
        conn.save_preview_images(
            id,
            Some(b"stale-front-png"),
            None,
            111_111,
            "test-fingerprint",
            conn.entry_updated_at(id).unwrap(),
        )
        .expect("preview save must succeed");
    }

    let second = import_path(&db, &dir, false, |_, _| {});
    assert!(
        second.summary.contains("replaced"),
        "summary must report the collision: {}",
        second.summary
    );
    assert_eq!(
        second.imported_ids,
        vec![id],
        "re-importing the same file must update the SAME row, not create a new one"
    );

    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(id)
        .expect("query must succeed")
        .expect("row must exist");
    assert_eq!(
        full.designer_info.as_deref(),
        Some("Test Designer"),
        "hand-typed designer_info must survive a re-import collision"
    );

    let preview = conn
        .get_preview_images(id)
        .expect("preview query must succeed");
    assert!(
        preview.front.is_none() && preview.generated_at.is_none(),
        "the stale preview must be invalidated by a re-import collision"
    );
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

// --- count_pending_collisions ---

#[test]
fn count_pending_collisions_finds_no_collisions_in_an_empty_catalogue() {
    let dir = temp_dir_for_test("collisions_none");
    std::fs::write(dir.join("fresh.asc"), VALID_ASC).expect("write fresh.asc");
    let db_path = temp_db_path_for_test("collisions_none");
    let db = open_temp_db(&db_path);

    assert_eq!(count_pending_collisions(&db, &dir, false), (0, 1));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn count_pending_collisions_flags_a_filename_already_in_the_catalogue() {
    let dir = temp_dir_for_test("collisions_some");
    std::fs::write(dir.join("existing.asc"), VALID_ASC).expect("write existing.asc");
    std::fs::write(dir.join("new.asc"), VALID_ASC).expect("write new.asc");
    let db_path = temp_db_path_for_test("collisions_some");
    let db = open_temp_db(&db_path);

    // Seed the catalogue with a row already named `existing.asc`, via the exact
    // machinery a real import uses -- from a DIFFERENT folder, since the collision
    // test is filename-only, not path-based (see `save_imported_design`'s own doc
    // comment).
    let seed_dir = temp_dir_for_test("collisions_some_seed");
    std::fs::write(seed_dir.join("existing.asc"), VALID_ASC).expect("write seed existing.asc");
    let seeded = import_path(&db, &seed_dir, false, |_, _| {});
    assert_eq!(seeded.imported_ids.len(), 1, "seed import must succeed");

    // Nothing was written by the scan itself: re-running it gives the identical
    // answer, and the seeded row above is still the only row in the catalogue.
    assert_eq!(count_pending_collisions(&db, &dir, false), (1, 2));
    assert_eq!(count_pending_collisions(&db, &dir, false), (1, 2));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&seed_dir);
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn count_pending_collisions_is_zero_zero_for_an_unreadable_path() {
    let db_path = temp_db_path_for_test("collisions_unreadable");
    let db = open_temp_db(&db_path);
    let missing = std::env::temp_dir().join("indicatrix_cut_test_does_not_exist_at_all");

    assert_eq!(count_pending_collisions(&db, &missing, false), (0, 0));

    let _ = std::fs::remove_file(&db_path);
}

// --- The collision-confirm decision ---

#[test]
fn no_collisions_never_needs_confirmation() {
    assert!(!collisions_need_confirmation(0));
}

#[test]
fn a_single_collision_needs_confirmation() {
    assert!(collisions_need_confirmation(1));
}

#[test]
fn many_collisions_still_need_only_one_confirmation() {
    assert!(collisions_need_confirmation(50));
}
