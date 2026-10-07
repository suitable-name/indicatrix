//! Per-design side data keyed by the design's UUID: saved variants (`design_variants`),
//! cutting progress (`design_cut_progress`) and the lighting choice (`design_lighting`).
//!
//! The point of the UUID key is that a design needs no catalogue entry: every test below
//! that does not say otherwise runs against a database with no entry at all.

use super::{super::*, fixtures::temp_db_path};
use crate::model::{
    cut_progress::CutProgressMark,
    design_key::catalogue_design_uuid,
    design_lighting::DesignLighting,
    design_variant::{DesignVariant, NewVariant, VariantSummary},
};

const DESIGN_A: &str = "0b9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10";
const DESIGN_B: &str = "7c3d2f19-8a64-4e0b-b1d5-9f2a6c4e8d73";

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

/// A variant with a name, a design text and a time, and nothing else.
const fn plain<'a>(name: &'a str, text: &'a str, created_at: i64) -> NewVariant<'a> {
    NewVariant {
        name,
        parent_variant_id: None,
        note: None,
        design_text: text,
        thumbnail_png: None,
        created_at,
    }
}

/// The names of `variants`, in order.
fn names(variants: &[VariantSummary]) -> Vec<&str> {
    variants.iter().map(|v| v.name.as_str()).collect()
}

fn row_count(db: &Database, table: &str) -> i64 {
    db.conn
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

// --- variants -----------------------------------------------------------------------

#[test]
fn a_design_that_was_never_catalogued_keeps_variants() {
    let db = in_memory_db();
    assert_eq!(row_count(&db, "diagram_entries"), 0);

    let id = db
        .save_variant(
            DESIGN_A,
            &NewVariant {
                name: "Steeper crown",
                parent_variant_id: None,
                note: Some("for the quartz"),
                design_text: "format = \"indicatrix-design\"\n",
                thumbnail_png: Some(&[0x89, b'P', b'N', b'G']),
                created_at: 1_000,
            },
        )
        .unwrap();

    let listed = db.list_variants(DESIGN_A).unwrap();
    assert_eq!(
        listed,
        vec![VariantSummary {
            variant_id: id,
            design_uuid: DESIGN_A.to_string(),
            name: "Steeper crown".to_string(),
            parent_variant_id: None,
            created_at: 1_000,
            note: Some("for the quartz".to_string()),
        }]
    );
    let loaded = db.load_variant(id).unwrap().expect("variant exists");
    assert_eq!(
        loaded,
        DesignVariant {
            summary: listed[0].clone(),
            design_text: "format = \"indicatrix-design\"\n".to_string(),
        }
    );
    assert_eq!(
        db.variant_thumbnail(id).unwrap(),
        Some(vec![0x89, b'P', b'N', b'G'])
    );
}

#[test]
fn variants_list_oldest_first_and_only_for_their_own_design() {
    let db = in_memory_db();
    let late = db.save_variant(DESIGN_A, &plain("Late", "t", 300)).unwrap();
    let early = db
        .save_variant(DESIGN_A, &plain("Early", "t", 100))
        .unwrap();
    let tie = db.save_variant(DESIGN_A, &plain("Tie", "t", 100)).unwrap();
    db.save_variant(DESIGN_B, &plain("Other design", "t", 50))
        .unwrap();

    let listed = db.list_variants(DESIGN_A).unwrap();
    assert_eq!(names(&listed), ["Early", "Tie", "Late"]);
    assert_eq!(
        listed.iter().map(|v| v.variant_id).collect::<Vec<_>>(),
        [early, tie, late]
    );
    assert_eq!(
        names(&db.list_variants(DESIGN_B).unwrap()),
        ["Other design"]
    );
    assert_eq!(
        db.list_variants("11111111-2222-4333-8444-555555555555")
            .unwrap(),
        Vec::<VariantSummary>::new()
    );
}

#[test]
fn the_uuid_may_be_written_in_either_case() {
    let db = in_memory_db();
    db.save_variant(&DESIGN_A.to_ascii_uppercase(), &plain("One", "t", 1))
        .unwrap();

    let listed = db.list_variants(&format!("  {DESIGN_A}  ")).unwrap();
    assert_eq!(names(&listed), ["One"]);
    assert_eq!(
        listed[0].design_uuid, DESIGN_A,
        "the key is stored in lowercase"
    );
}

#[test]
fn a_name_is_trimmed_and_a_blank_note_or_empty_preview_is_stored_as_none() {
    let db = in_memory_db();
    let id = db
        .save_variant(
            DESIGN_A,
            &NewVariant {
                name: "  Padded  ",
                parent_variant_id: None,
                note: Some("   "),
                design_text: "t",
                thumbnail_png: Some(&[]),
                created_at: 1,
            },
        )
        .unwrap();

    let variant = db.load_variant(id).unwrap().unwrap();
    assert_eq!(variant.summary.name, "Padded");
    assert_eq!(variant.summary.note, None);
    assert_eq!(db.variant_thumbnail(id).unwrap(), None);
}

#[test]
fn nothing_is_saved_for_a_bad_key_a_blank_name_or_a_blank_design() {
    let db = in_memory_db();
    for bad_key in ["", "   ", "not-a-uuid", "0b9e6a4e5f1d4c7a9a521e3f7d8c2b10"] {
        assert!(
            db.save_variant(bad_key, &plain("Name", "t", 1)).is_err(),
            "{bad_key:?}"
        );
        assert!(db.list_variants(bad_key).is_err(), "{bad_key:?}");
    }
    assert!(db.save_variant(DESIGN_A, &plain("  ", "t", 1)).is_err());
    assert!(
        db.save_variant(DESIGN_A, &plain("Name", "  \n", 1))
            .is_err()
    );
    assert_eq!(row_count(&db, "design_variants"), 0);
}

#[test]
fn a_parent_must_exist_and_belong_to_the_same_design() {
    let db = in_memory_db();
    let parent = db.save_variant(DESIGN_A, &plain("Base", "t", 1)).unwrap();
    let foreign = db
        .save_variant(DESIGN_B, &plain("Foreign", "t", 1))
        .unwrap();

    let child = db
        .save_variant(
            DESIGN_A,
            &NewVariant {
                parent_variant_id: Some(parent),
                ..plain("Child", "t", 2)
            },
        )
        .unwrap();
    assert_eq!(
        db.load_variant(child)
            .unwrap()
            .unwrap()
            .summary
            .parent_variant_id,
        Some(parent)
    );

    for bad_parent in [foreign, parent + 999] {
        let err = db
            .save_variant(
                DESIGN_A,
                &NewVariant {
                    parent_variant_id: Some(bad_parent),
                    ..plain("Orphan", "t", 3)
                },
            )
            .expect_err("a parent that is not this design's must be refused");
        assert!(
            format!("{err:#}").contains("parent variant"),
            "the message names the parent: {err:#}"
        );
    }
    assert_eq!(row_count(&db, "design_variants"), 3, "nothing was added");
}

#[test]
fn deleting_a_parent_detaches_its_children_and_deleting_a_child_keeps_the_parent() {
    let db = in_memory_db();
    let root = db.save_variant(DESIGN_A, &plain("Root", "t", 1)).unwrap();
    let kid = db
        .save_variant(
            DESIGN_A,
            &NewVariant {
                parent_variant_id: Some(root),
                ..plain("Kid", "t", 2)
            },
        )
        .unwrap();
    let grandkid = db
        .save_variant(
            DESIGN_A,
            &NewVariant {
                parent_variant_id: Some(kid),
                ..plain("Grandkid", "t", 3)
            },
        )
        .unwrap();

    // Deleting the middle one detaches only its child; the root and the child stay.
    assert_eq!(db.delete_variant(kid).unwrap(), 1);
    let listed = db.list_variants(DESIGN_A).unwrap();
    assert_eq!(names(&listed), ["Root", "Grandkid"]);
    assert_eq!(listed[1].variant_id, grandkid);
    assert_eq!(listed[1].parent_variant_id, None);

    // Deleting a leaf touches nobody else.
    assert_eq!(db.delete_variant(grandkid).unwrap(), 1);
    assert_eq!(names(&db.list_variants(DESIGN_A).unwrap()), ["Root"]);
    assert_eq!(db.delete_variant(grandkid).unwrap(), 0, "already gone");
    assert_eq!(db.load_variant(grandkid).unwrap(), None);
}

#[test]
fn rename_and_note_change_only_their_own_column_and_report_a_missing_variant() {
    let db = in_memory_db();
    let id = db
        .save_variant(
            DESIGN_A,
            &NewVariant {
                note: Some("first"),
                thumbnail_png: Some(&[1, 2, 3]),
                ..plain("Old", "the design", 7)
            },
        )
        .unwrap();

    assert_eq!(db.rename_variant(id, "  New  ").unwrap(), 1);
    assert_eq!(db.set_variant_note(id, Some("second")).unwrap(), 1);
    let variant = db.load_variant(id).unwrap().unwrap();
    assert_eq!(variant.summary.name, "New");
    assert_eq!(variant.summary.note.as_deref(), Some("second"));
    assert_eq!(variant.summary.created_at, 7);
    assert_eq!(variant.design_text, "the design");
    assert_eq!(db.variant_thumbnail(id).unwrap(), Some(vec![1, 2, 3]));

    assert_eq!(db.set_variant_note(id, Some("  ")).unwrap(), 1);
    assert_eq!(db.load_variant(id).unwrap().unwrap().summary.note, None);
    assert_eq!(db.set_variant_note(id, None).unwrap(), 1);

    assert!(db.rename_variant(id, "   ").is_err(), "a blank name");
    assert_eq!(db.load_variant(id).unwrap().unwrap().summary.name, "New");

    let missing = id + 999;
    assert_eq!(db.rename_variant(missing, "x").unwrap(), 0);
    assert_eq!(db.set_variant_note(missing, Some("x")).unwrap(), 0);
    assert_eq!(db.variant_thumbnail(missing).unwrap(), None);
    assert_eq!(db.load_variant(missing).unwrap(), None);
}

#[test]
fn a_large_multi_line_design_text_comes_back_unchanged() {
    let db = in_memory_db();
    let text = "[[tiers]]\nname = \"P1\"\r\nnote = \"Umlaut \u{e4}\u{f6}\u{fc} \u{2014} done\"\n"
        .repeat(5_000);
    let id = db.save_variant(DESIGN_A, &plain("Big", &text, 1)).unwrap();

    assert_eq!(db.load_variant(id).unwrap().unwrap().design_text, text);
}

#[test]
fn variant_ids_are_never_handed_out_twice() {
    let db = in_memory_db();
    let first = db.save_variant(DESIGN_A, &plain("One", "t", 1)).unwrap();
    db.delete_variant(first).unwrap();
    let second = db.save_variant(DESIGN_A, &plain("Two", "t", 2)).unwrap();
    assert!(second > first, "AUTOINCREMENT: {second} after {first}");
}

// --- cutting progress ------------------------------------------------------------------

#[test]
fn done_marks_are_listed_oldest_first_and_kept_per_design() {
    let db = in_memory_db();
    db.mark_step_done(DESIGN_A, "tier-7", "sig-7", 300).unwrap();
    db.mark_step_done(DESIGN_A, "tier-2", "sig-2", 100).unwrap();
    db.mark_step_done(DESIGN_A, "tier-1", "sig-1", 100).unwrap();
    db.mark_step_done(DESIGN_B, "tier-2", "other", 50).unwrap();

    assert_eq!(
        db.cut_progress(DESIGN_A).unwrap(),
        vec![
            CutProgressMark {
                step_key: "tier-1".to_string(),
                step_signature: "sig-1".to_string(),
                done_at: 100,
            },
            CutProgressMark {
                step_key: "tier-2".to_string(),
                step_signature: "sig-2".to_string(),
                done_at: 100,
            },
            CutProgressMark {
                step_key: "tier-7".to_string(),
                step_signature: "sig-7".to_string(),
                done_at: 300,
            },
        ]
    );
    let other = db.cut_progress(DESIGN_B).unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].step_signature, "other");
    assert_eq!(
        db.cut_progress("11111111-2222-4333-8444-555555555555")
            .unwrap(),
        Vec::<CutProgressMark>::new()
    );
}

#[test]
fn marking_a_step_again_replaces_its_signature_and_time() {
    let db = in_memory_db();
    db.mark_step_done(DESIGN_A, "tier-1", "old", 100).unwrap();
    db.mark_step_done(DESIGN_A, "tier-1", "new", 500).unwrap();

    let marks = db.cut_progress(DESIGN_A).unwrap();
    assert_eq!(marks.len(), 1, "one row per step");
    assert_eq!(
        (marks[0].step_signature.as_str(), marks[0].done_at),
        ("new", 500)
    );
}

#[test]
fn unmarking_and_clearing_remove_only_what_they_name() {
    let db = in_memory_db();
    for key in ["tier-1", "tier-2", "tier-3"] {
        db.mark_step_done(DESIGN_A, key, "s", 1).unwrap();
    }
    db.mark_step_done(DESIGN_B, "tier-1", "s", 1).unwrap();

    assert_eq!(db.unmark_step(DESIGN_A, "tier-2").unwrap(), 1);
    assert_eq!(
        db.unmark_step(DESIGN_A, "tier-2").unwrap(),
        0,
        "already off"
    );
    assert_eq!(db.unmark_step(DESIGN_A, "never-marked").unwrap(), 0);
    let keys: Vec<String> = db
        .cut_progress(DESIGN_A)
        .unwrap()
        .into_iter()
        .map(|m| m.step_key)
        .collect();
    assert_eq!(keys, ["tier-1", "tier-3"]);

    assert_eq!(db.clear_cut_progress(DESIGN_A).unwrap(), 2);
    assert_eq!(
        db.cut_progress(DESIGN_A).unwrap(),
        Vec::<CutProgressMark>::new()
    );
    assert_eq!(db.cut_progress(DESIGN_B).unwrap().len(), 1, "other design");
    assert_eq!(db.clear_cut_progress(DESIGN_A).unwrap(), 0);
}

#[test]
fn a_bad_key_a_blank_step_or_a_blank_signature_is_refused() {
    let db = in_memory_db();
    assert!(db.mark_step_done("", "tier-1", "s", 1).is_err());
    assert!(db.mark_step_done("nope", "tier-1", "s", 1).is_err());
    assert!(db.mark_step_done(DESIGN_A, "  ", "s", 1).is_err());
    assert!(db.mark_step_done(DESIGN_A, "tier-1", "", 1).is_err());
    assert!(db.unmark_step("nope", "tier-1").is_err());
    assert!(db.cut_progress("nope").is_err());
    assert!(db.clear_cut_progress("nope").is_err());
    assert_eq!(row_count(&db, "design_cut_progress"), 0);
}

// --- lighting ----------------------------------------------------------------------------

#[test]
fn a_lighting_choice_is_stored_replaced_and_cleared_per_design() {
    let db = in_memory_db();
    assert_eq!(db.design_lighting(DESIGN_A).unwrap(), None);

    let json = "{\n  \"tilt\": 12.5,\n  \"label\": \"Caf\u{e9} \u{2014} warm\"\n}";
    db.set_design_lighting(DESIGN_A, "Light tent", json, 100)
        .unwrap();
    db.set_design_lighting(DESIGN_B, "Studio", "{}", 200)
        .unwrap();
    assert_eq!(
        db.design_lighting(DESIGN_A).unwrap(),
        Some(DesignLighting {
            preset_name: "Light tent".to_string(),
            settings_json: json.to_string(),
            updated_at: 100,
        }),
        "the settings text comes back exactly as given"
    );

    db.set_design_lighting(DESIGN_A, "  Studio  ", "{\"a\":1}", 300)
        .unwrap();
    let replaced = db.design_lighting(DESIGN_A).unwrap().unwrap();
    assert_eq!(
        (
            replaced.preset_name.as_str(),
            replaced.settings_json.as_str(),
            replaced.updated_at
        ),
        ("Studio", "{\"a\":1}", 300)
    );
    assert_eq!(row_count(&db, "design_lighting"), 2, "one row per design");

    assert_eq!(db.clear_design_lighting(DESIGN_A).unwrap(), 1);
    assert_eq!(db.clear_design_lighting(DESIGN_A).unwrap(), 0);
    assert_eq!(db.design_lighting(DESIGN_A).unwrap(), None);
    assert!(db.design_lighting(DESIGN_B).unwrap().is_some());
}

#[test]
fn a_bad_key_or_a_blank_preset_name_is_refused_for_lighting() {
    let db = in_memory_db();
    assert!(db.set_design_lighting("", "Studio", "{}", 1).is_err());
    assert!(db.set_design_lighting("nope", "Studio", "{}", 1).is_err());
    assert!(db.set_design_lighting(DESIGN_A, "  ", "{}", 1).is_err());
    assert!(db.design_lighting("nope").is_err());
    assert!(db.clear_design_lighting("nope").is_err());
    assert_eq!(row_count(&db, "design_lighting"), 0);
}

// --- independence from the catalogue ------------------------------------------------------

/// The side data is keyed by UUID, not by entry: deleting the catalogue entry that a
/// catalogue-derived UUID came from leaves it alone (the design may live on as a file).
#[test]
fn deleting_a_catalogue_entry_leaves_its_side_data_alone() {
    let db = in_memory_db();
    let url = "local://capps-brilliant.asc";
    let entry = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Capps Brilliant".to_string(),
                url: url.to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    let uuid = catalogue_design_uuid(url);
    db.save_variant(&uuid, &plain("Variant", "t", 1)).unwrap();
    db.mark_step_done(&uuid, "tier-1", "s", 1).unwrap();
    db.set_design_lighting(&uuid, "Studio", "{}", 1).unwrap();

    db.delete_diagram_entry(entry).unwrap();

    assert_eq!(names(&db.list_variants(&uuid).unwrap()), ["Variant"]);
    assert_eq!(db.cut_progress(&uuid).unwrap().len(), 1);
    assert!(db.design_lighting(&uuid).unwrap().is_some());
}

/// Like the planner mark, none of this may bump `diagram_entries.updated_at`: it is the
/// revision stamp every cache and compare-and-swap write keys on.
#[test]
fn writing_side_data_does_not_bump_updated_at() {
    let db = in_memory_db();
    let url = "local://stamp.asc";
    let entry = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Stamp".to_string(),
                url: url.to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE diagram_entries SET updated_at = 0 WHERE id = ?1",
            params![entry],
        )
        .unwrap();
    let uuid = catalogue_design_uuid(url);

    let variant = db.save_variant(&uuid, &plain("V", "t", 1)).unwrap();
    db.rename_variant(variant, "W").unwrap();
    db.set_variant_note(variant, Some("n")).unwrap();
    db.delete_variant(variant).unwrap();
    db.mark_step_done(&uuid, "tier-1", "s", 1).unwrap();
    db.unmark_step(&uuid, "tier-1").unwrap();
    db.set_design_lighting(&uuid, "Studio", "{}", 1).unwrap();
    db.clear_design_lighting(&uuid).unwrap();

    assert_eq!(db.entry_updated_at(entry).unwrap(), Some(0));
}

/// The readers work on a read-only connection (a second window or the worker), and the
/// writers refuse there without changing anything.
#[test]
fn the_readers_work_on_a_read_only_connection_and_the_writers_refuse() {
    let path = temp_db_path("design_side_data_read_only");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    let variant = db.save_variant(DESIGN_A, &plain("Kept", "t", 1)).unwrap();
    db.mark_step_done(DESIGN_A, "tier-1", "s", 1).unwrap();
    db.set_design_lighting(DESIGN_A, "Studio", "{}", 1).unwrap();

    let ro = Database::open_read_only(path.to_str().unwrap()).expect("open read-only");
    assert_eq!(names(&ro.list_variants(DESIGN_A).unwrap()), ["Kept"]);
    assert!(ro.load_variant(variant).unwrap().is_some());
    assert_eq!(ro.cut_progress(DESIGN_A).unwrap().len(), 1);
    assert!(ro.design_lighting(DESIGN_A).unwrap().is_some());
    assert!(
        ro.save_variant(DESIGN_A, &plain("Refused", "t", 2))
            .is_err()
    );
    assert!(ro.delete_variant(variant).is_err());
    assert!(ro.mark_step_done(DESIGN_A, "tier-2", "s", 2).is_err());
    assert!(ro.unmark_step(DESIGN_A, "tier-1").is_err());
    assert!(ro.set_design_lighting(DESIGN_A, "Other", "{}", 2).is_err());
    assert!(ro.clear_design_lighting(DESIGN_A).is_err());
    assert_eq!(
        names(&db.list_variants(DESIGN_A).unwrap()),
        ["Kept"],
        "the refused writes changed nothing"
    );

    drop(ro);
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
