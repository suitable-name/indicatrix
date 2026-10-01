//! `diagram_entries` lifecycle mutations: rename, `updated_at` bump semantics, `url`
//! rewrite, delete cascade, and the `ignored` flag.

use super::{super::*, fixtures::temp_db_path};

// ---- Organize: rename_diagram_entry / delete_diagram_entry --------------------

#[test]
fn rename_diagram_entry_updates_the_title() {
    let path = temp_db_path("rename");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetingDiagramEntry {
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
    let entry = FacetingDiagramEntry {
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
            &FacetingDiagramEntry {
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
            &FacetingDiagramEntry {
                title: "Detail Bumps".to_string(),
                url: "local://detail-bumps.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    force_stale_updated_at(&db, id);

    db.save_diagram_detail(&FacetingDiagramDetail::default(), id)
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
            &FacetingDiagramEntry {
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
    let entry = FacetingDiagramEntry {
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
    let entry = FacetingDiagramEntry {
        title: "Doomed Design".to_string(),
        url: "local://doomed.asc".to_string(),
        design_id: String::new(),
    };
    let id = db.save_diagram_entry(&entry, "local-import").unwrap();
    db.save_diagram_detail(
        &FacetingDiagramDetail {
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

/// an import used to save the entry and the detail row in two separate
/// transactions, so a crash/error between them left an entry with no detail row
/// (observed on the real catalogue -- entry id 3200). `save_design` must commit both
/// or neither.
#[test]
fn save_design_saves_entry_and_detail_together() {
    let path = temp_db_path("save_design_atomic");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let entry_id = db
        .save_design(
            &FacetingDiagramEntry {
                title: "Atomic Design".to_string(),
                url: "local://atomic.asc".to_string(),
                design_id: String::new(),
            },
            &FacetingDiagramDetail {
                shape: Some("Round".to_string()),
                refractive_index: Some("1.76".to_string()),
                ..Default::default()
            },
            "local-import",
        )
        .expect("save_design must succeed");

    let full = db
        .get_diagram_full(entry_id)
        .unwrap()
        .expect("a detail row must exist immediately after save_design");
    assert_eq!(full.shape.as_deref(), Some("Round"));
    assert_eq!(full.refractive_index.as_deref(), Some("1.76"));

    let _ = std::fs::remove_file(&path);
}

/// When the detail insert fails, the entry insert made earlier in the same call must
/// roll back with it: no entry row is left behind and the error reaches the caller.
#[test]
fn save_design_rolls_back_the_entry_when_the_detail_insert_fails() {
    let path = temp_db_path("save_design_rollback");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    db.save_diagram_entry(
        &FacetingDiagramEntry {
            title: "Already There".to_string(),
            url: "local://already-there.asc".to_string(),
            design_id: String::new(),
        },
        "local-import",
    )
    .expect("seed one unrelated entry");
    let count_entries = || -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM diagram_entries", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count_entries(), 1);

    // Every detail insert now aborts, standing in for a constraint violation.
    db.conn
        .execute_batch(
            "CREATE TRIGGER reject_detail BEFORE INSERT ON diagram_details
             BEGIN SELECT RAISE(ABORT, 'detail rejected'); END;",
        )
        .unwrap();

    let err = db
        .save_design(
            &FacetingDiagramEntry {
                title: "Doomed".to_string(),
                url: "local://doomed.asc".to_string(),
                design_id: String::new(),
            },
            &FacetingDiagramDetail::default(),
            "local-import",
        )
        .expect_err("save_design must surface the failed detail insert");
    assert!(
        format!("{err:#}").contains("detail rejected"),
        "the underlying cause must reach the caller, got: {err:#}"
    );

    assert_eq!(count_entries(), 1, "the entry insert must have rolled back");
    assert!(
        db.diagram_entry_id_for_url("local://doomed.asc")
            .unwrap()
            .is_none()
    );

    let _ = std::fs::remove_file(&path);
}

/// A second `save_design` for the same `url` must behave like `save_diagram_entry`'s
/// own upsert (same entry id, updated title) plus a full detail re-sync -- never a
/// second, duplicate entry.
#[test]
fn save_design_upserts_the_same_entry_on_a_repeated_url() {
    let path = temp_db_path("save_design_upsert");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetingDiagramEntry {
        title: "First Title".to_string(),
        url: "local://repeated.asc".to_string(),
        design_id: String::new(),
    };
    let first_id = db
        .save_design(
            &entry,
            &FacetingDiagramDetail {
                shape: Some("Oval".to_string()),
                ..Default::default()
            },
            "local-import",
        )
        .unwrap();

    let second_id = db
        .save_design(
            &FacetingDiagramEntry {
                title: "Updated Title".to_string(),
                url: entry.url,
                design_id: String::new(),
            },
            &FacetingDiagramDetail {
                shape: Some("Pear".to_string()),
                ..Default::default()
            },
            "local-import",
        )
        .unwrap();

    assert_eq!(first_id, second_id, "same url must upsert, not duplicate");
    let full = db.get_diagram_full(first_id).unwrap().unwrap();
    assert_eq!(full.title, "Updated Title");
    assert_eq!(full.shape.as_deref(), Some("Pear"));

    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM diagram_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);

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
            &FacetingDiagramEntry {
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
fn diagram_entry_id_for_url_finds_an_existing_row_and_none_for_an_unknown_one() {
    let path = temp_db_path("diagram_entry_id_for_url");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    assert_eq!(
        db.diagram_entry_id_for_url("local://never-saved.asc")
            .unwrap(),
        None
    );

    let entry_id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Lookup Me".to_string(),
                url: "local://lookup-me.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();

    assert_eq!(
        db.diagram_entry_id_for_url("local://lookup-me.asc")
            .unwrap(),
        Some(entry_id)
    );

    let _ = std::fs::remove_file(&path);
}

/// `save_diagram_entry` is a url-keyed UPSERT: a DIFFERENT design saved under a taken url
/// lands on the first design's row and renames it, and `save_diagram_detail` then
/// replaces that row's angle table. The caller that must not do this (Save Native's
/// catalogue write-back) asks `diagram_entry_for_url` first; this pins both halves.
#[test]
fn save_diagram_entry_on_a_colliding_url_returns_the_existing_row_and_renames_it() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");
    let original_id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Original".to_string(),
                url: "local://round.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    assert_eq!(
        db.diagram_entry_for_url("local://round.asc").unwrap(),
        Some((original_id, "Original".to_string())),
        "the lookup a guarded caller makes before writing must name the current owner"
    );
    assert_eq!(
        db.diagram_entry_for_url("local://absent.asc").unwrap(),
        None
    );

    let reused_id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Impostor".to_string(),
                url: "local://round.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    assert_eq!(reused_id, original_id, "the upsert reuses the owner's row");
    assert_eq!(
        db.diagram_entry_for_url("local://round.asc").unwrap(),
        Some((original_id, "Impostor".to_string())),
        "and overwrites its title, which is why a caller has to guard"
    );
}

/// A bad numeric field must leave the title as it was: the rename and the metadata
/// update are one transaction.
#[test]
fn rename_and_update_metadata_is_all_or_nothing() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");
    let id = db
        .save_design(
            &FacetingDiagramEntry {
                title: "Before".to_string(),
                url: "local://atomic-edit.asc".to_string(),
                design_id: String::new(),
            },
            &FacetingDiagramDetail::default(),
            "local-import",
        )
        .unwrap();

    let bad = MetadataUpdate {
        refractive_index: Some("not a number".to_string()),
        ..MetadataUpdate::default()
    };
    assert!(db.rename_and_update_metadata(id, "After", &bad).is_err());
    assert_eq!(db.get_diagram_full(id).unwrap().unwrap().title, "Before");

    let good = MetadataUpdate {
        refractive_index: Some("1.76".to_string()),
        ..MetadataUpdate::default()
    };
    db.rename_and_update_metadata(id, "After", &good).unwrap();
    let full = db.get_diagram_full(id).unwrap().unwrap();
    assert_eq!(full.title, "After");
    assert_eq!(full.refractive_index.as_deref(), Some("1.76"));
}
