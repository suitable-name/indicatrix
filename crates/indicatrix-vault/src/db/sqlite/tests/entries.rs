//! `diagram_entries` lifecycle mutations: cross-source dedup detection, rename,
//! `updated_at` bump semantics, `url` rewrite, delete cascade, and the `ignored` flag.

use super::{super::*, fixtures::temp_db_path};

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

/// Reads `diagram_entries.updated_at` directly -- shared by the
/// tests below, which check whether a given write bumps it.
fn read_updated_at(db: &Database, entry_id: i64) -> Option<i64> {
    db.conn
        .query_row(
            "SELECT updated_at FROM diagram_entries WHERE id = ?1",
            params![entry_id],
            |r| r.get(0),
        )
        .unwrap()
}

/// Forces `entry_id`'s `updated_at` to a known-stale value (`0`), bypassing whatever
/// `save_diagram_entry` itself just stamped -- lets a test prove a later write moved it
/// forward without depending on wall-clock resolution (`unix_now()` only has 1-second
/// granularity, too coarse to reliably observe within a single fast test).
fn force_stale_updated_at(db: &Database, entry_id: i64) {
    db.conn
        .execute(
            "UPDATE diagram_entries SET updated_at = 0 WHERE id = ?1",
            params![entry_id],
        )
        .unwrap();
}

/// `rename_diagram_entry` must bump `updated_at` for the "recently edited" sort,
/// same as `update_diagram_metadata`/
/// `update_diagram_entry_url` already do.
#[test]
fn rename_diagram_entry_bumps_updated_at() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");
    let id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Before".to_string(),
                url: "local://rename-bumps.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    force_stale_updated_at(&db, id);

    db.rename_diagram_entry(id, "After").unwrap();

    assert!(
        read_updated_at(&db, id).is_some_and(|t| t > 0),
        "rename_diagram_entry must bump updated_at forward from its stale value"
    );
}

/// `save_diagram_detail` must bump its `entry_id`'s
/// `diagram_entries.updated_at` -- a full detail re-sync is at least as much a content
/// change as the hand-corrections `update_diagram_metadata` already bumps for.
#[test]
fn save_diagram_detail_bumps_the_entrys_updated_at() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");
    let id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Detail Bumps".to_string(),
                url: "local://detail-bumps.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    force_stale_updated_at(&db, id);

    db.save_diagram_detail(&FacetDiagramDetail::default(), id)
        .unwrap();

    assert!(
        read_updated_at(&db, id).is_some_and(|t| t > 0),
        "save_diagram_detail must bump the entry's updated_at forward from its stale value"
    );
}

/// `set_diagram_ignored` deliberately does NOT bump `updated_at` --
/// see that method's own doc comment for why (hiding/restoring a design isn't a
/// content edit).
#[test]
fn set_diagram_ignored_does_not_bump_updated_at() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");
    let id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Ignore Me".to_string(),
                url: "local://ignore-me.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    force_stale_updated_at(&db, id);

    db.set_diagram_ignored(id, true).unwrap();
    assert_eq!(
        read_updated_at(&db, id),
        Some(0),
        "set_diagram_ignored(true) must leave updated_at untouched"
    );

    db.set_diagram_ignored(id, false).unwrap();
    assert_eq!(
        read_updated_at(&db, id),
        Some(0),
        "set_diagram_ignored(false) must leave updated_at untouched"
    );
}

#[test]
fn update_diagram_entry_url_changes_only_the_url_and_bumps_updated_at() {
    let path = temp_db_path("update_entry_url");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Save Native round trip".to_string(),
        url: "local://old_name.asc".to_string(),
        design_id: "keep-me".to_string(),
    };
    let id = db.save_diagram_entry(&entry, "local-import").unwrap();

    db.update_diagram_entry_url(id, "local://new_name.asc")
        .unwrap();

    let full = db.get_diagram_full(id).unwrap().unwrap();
    assert_eq!(full.url, "local://new_name.asc");
    // Title/design_id are a cutter's own hand-corrections (or, for design_id, synced
    // from elsewhere) -- a "Save Native As..." file-name change must not touch either.
    assert_eq!(full.title, "Save Native round trip");
    assert_eq!(full.design_id.as_deref(), Some("keep-me"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn update_diagram_entry_url_rejects_an_unknown_entry_id() {
    let path = temp_db_path("update_entry_url_errors");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    assert!(
        db.update_diagram_entry_url(999_999, "local://nope.asc")
            .is_err()
    );

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
