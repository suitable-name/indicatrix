//! The `[meta]` table and attachments a Save writes: assembling them from the loaded
//! session and the library row, the size limit, determinism, the round trip through
//! the written file and back into a library row, and what the file must NOT carry.

use super::{
    design_meta::{
        RowState, assemble, assemble_metadata, check_attachment_budget, iso8601_utc,
        merge_attachments, new_design_id, row_attachments,
    },
    save_helpers::design_file_text,
};
use crate::gui::editor::state::DesignFileExtras;
use indicatrix_cut_core::{
    ConstraintTier, Design, PreformSpec, ScheduleMeta,
    native::{
        AttachmentBlob, AttachmentRole, DesignExtras, DesignMetadata, design_from_str,
        design_to_string,
    },
};
use indicatrix_formats::native::design::{MAX_ATTACHMENT_BYTES, is_iso8601_utc, is_uuid};
use indicatrix_vault::{
    local::{apply_imported_extras, import_native_design},
    model::{design_key::catalogue_design_uuid, entry::FullDiagramRecord, file::AttachedFile},
};

const NOW: &str = "2026-10-02T09:30:00Z";
const ID: &str = "0b5f0a8e-5a43-4d52-9c1f-7c8f3a1e2d90";

fn round_brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// A library row with the descriptive fields filled and nothing derived.
fn record() -> FullDiagramRecord {
    FullDiagramRecord {
        entry_id: 7,
        title: "Capps Brilliant".to_string(),
        url: "local://capps.asc".to_string(),
        design_id: Some("cb-17".to_string()),
        page_url: "https://example.org/capps".to_string(),
        diagram_image_name: Some("capps.png".to_string()),
        diagram_image_data: Some(vec![1, 2, 3]),
        competition_diagram: Some("Masters".to_string()),
        lw_ratio: None,
        refractive_index: None,
        index_gear: None,
        volume: None,
        facets_count: None,
        shape: Some("Round".to_string()),
        designer_info: Some("Capps, Jerry; Lapidary Journal, May 1994".to_string()),
        hw_ratio: None,
        tw_ratio: None,
        uw_ratio: None,
        pw_ratio: None,
        cw_ratio: None,
        symmetry_order: None,
        mirror_symmetry: None,
        designer: Some("Capps, Jerry".to_string()),
        source_citation: Some("Lapidary Journal, May 1994".to_string()),
        pdf_file: Some("capps.pdf".to_string()),
        gem_file: Some("capps.gem".to_string()),
        shape_category: Some("5".to_string()),
        angle_settings: Vec::new(),
        attached_files: vec![
            AttachedFile {
                name: "capps.asc".to_string(),
                url: String::new(),
                content: b"GemCad 5.0\n".to_vec(),
            },
            AttachedFile {
                name: "capps.indicatrix".to_string(),
                url: String::new(),
                content: b"older copy of the design file".to_vec(),
            },
            AttachedFile {
                name: "capps.pdf".to_string(),
                url: "https://example.org/capps.pdf".to_string(),
                content: b"%PDF".to_vec(),
            },
        ],
    }
}

fn row(tags: &[&str], ignored: bool, planner_excluded: bool) -> RowState {
    RowState::for_test(
        record(),
        tags.iter().map(ToString::to_string).collect(),
        ignored,
        planner_excluded,
    )
}

fn written(extras: &DesignExtras<'_>) -> String {
    design_file_text(&round_brilliant(), None, extras, false).expect("serializes")
}

#[test]
fn a_design_with_no_row_and_no_loaded_file_gets_an_id_and_the_two_dates_only() {
    let meta = assemble_metadata(&DesignMetadata::default(), None, NOW, || ID.to_string());
    let expected = DesignMetadata {
        id: ID.to_string(),
        created_at: NOW.to_string(),
        modified_at: NOW.to_string(),
        ..DesignMetadata::default()
    };
    assert_eq!(meta, expected);
}

#[test]
fn the_loaded_id_and_creation_time_are_kept_and_only_modified_at_moves() {
    let base = DesignMetadata {
        id: ID.to_string(),
        created_at: "2025-01-01T00:00:00Z".to_string(),
        modified_at: "2025-06-01T00:00:00Z".to_string(),
        ..DesignMetadata::default()
    };
    let meta = assemble_metadata(&base, None, NOW, || panic!("must not mint a second id"));
    assert_eq!(meta.id, ID);
    assert_eq!(meta.created_at, "2025-01-01T00:00:00Z");
    assert_eq!(meta.modified_at, NOW);
}

#[test]
fn the_row_wins_for_the_fields_the_library_edits_and_the_file_keeps_the_rest() {
    let mut base = DesignMetadata {
        id: ID.to_string(),
        title: "Stale title".to_string(),
        designer: "Stale designer".to_string(),
        notes: "Cut in quartz first".to_string(),
        license: "CC BY-NC 4.0".to_string(),
        copyright: "(c) Capps".to_string(),
        ..DesignMetadata::default()
    };
    base.unknown
        .insert("future_key".to_string(), toml::Value::Integer(5));
    let meta = assemble_metadata(&base, Some(&row(&["x"], true, true)), NOW, String::new);
    assert_eq!(meta.title, "Capps Brilliant");
    assert_eq!(meta.designer, "Capps, Jerry");
    assert_eq!(meta.source_url, "https://example.org/capps");
    assert_eq!(meta.shape_category, "5");
    assert!(meta.ignored && meta.planner_excluded);
    // No column in the library: kept as loaded, unknown keys included.
    assert_eq!(meta.notes, "Cut in quartz first");
    assert_eq!(meta.license, "CC BY-NC 4.0");
    assert_eq!(meta.copyright, "(c) Capps");
    assert_eq!(
        meta.unknown.get("future_key"),
        Some(&toml::Value::Integer(5))
    );
    assert_eq!(meta.id, ID);
}

#[test]
fn tags_are_written_sorted_and_deduplicated_whatever_order_the_row_gave() {
    let a = assemble_metadata(
        &DesignMetadata::default(),
        Some(&row(
            &["zebra", "alpha", " mid ", "alpha", ""],
            false,
            false,
        )),
        NOW,
        || ID.to_string(),
    );
    let b = assemble_metadata(
        &DesignMetadata::default(),
        Some(&row(&["mid", "alpha", "zebra"], false, false)),
        NOW,
        || ID.to_string(),
    );
    assert_eq!(a.tags, ["alpha", "mid", "zebra"]);
    assert_eq!(a.tags, b.tags);
}

#[test]
fn identical_inputs_write_identical_bytes() {
    let prepared = |()| {
        assemble(
            &DesignFileExtras::default(),
            Some(&row(&["b", "a"], false, true)),
            NOW,
            || ID.to_string(),
        )
        .expect("assembles")
    };
    let render = |p: &super::design_meta::PreparedExtras| {
        written(&DesignExtras {
            metadata: Some(&p.metadata),
            attachments: &p.attachments,
            ..DesignExtras::default()
        })
    };
    assert_eq!(render(&prepared(())), render(&prepared(())));
}

#[test]
fn row_attachments_skip_design_files_keep_one_asc_and_add_the_diagram_image() {
    let rec = record();
    let blobs = row_attachments(&rec.attached_files, Some(("capps.png", &[1u8, 2, 3][..])));
    let summary: Vec<(&str, AttachmentRole)> =
        blobs.iter().map(|b| (b.name.as_str(), b.role)).collect();
    assert_eq!(
        summary,
        [
            ("capps.asc", AttachmentRole::Asc),
            ("capps.pdf", AttachmentRole::Pdf),
            ("capps.png", AttachmentRole::DiagramImage),
        ]
    );
    assert_eq!(blobs[1].source_url, "https://example.org/capps.pdf");
}

#[test]
fn a_row_attachment_replaces_the_session_one_of_the_same_name_and_the_single_roles() {
    let base = vec![
        AttachmentBlob::new("old.asc", AttachmentRole::Asc, b"old".to_vec()),
        AttachmentBlob::new("notes.txt", AttachmentRole::Other, b"keep".to_vec()),
        AttachmentBlob::new("capps.pdf", AttachmentRole::Pdf, b"stale".to_vec()),
    ];
    let row_side = vec![
        AttachmentBlob::new("capps.asc", AttachmentRole::Asc, b"new".to_vec()),
        AttachmentBlob::new("capps.pdf", AttachmentRole::Pdf, b"fresh".to_vec()),
    ];
    let merged = merge_attachments(&base, row_side);
    let names: Vec<&str> = merged.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["capps.asc", "capps.pdf", "notes.txt"]);
    assert_eq!(merged[1].data, b"fresh");
}

#[test]
fn oversize_attachments_are_refused_with_a_message_instead_of_dropped() {
    let big = AttachmentBlob::new(
        "big.bin",
        AttachmentRole::Other,
        vec![0u8; MAX_ATTACHMENT_BYTES as usize + 1],
    );
    let message = check_attachment_budget(std::slice::from_ref(&big)).expect_err("over the limit");
    assert!(message.contains("MiB"), "message: {message}");
    let base = DesignFileExtras::new(DesignMetadata::default(), vec![big]);
    assert!(assemble(&base, None, NOW, || ID.to_string()).is_err());
    let fine = AttachmentBlob::new("ok.pdf", AttachmentRole::Pdf, vec![0u8; 1024]);
    assert!(check_attachment_budget(&[fine]).is_ok());
}

#[test]
fn the_clock_and_the_id_are_valid_for_the_file_format() {
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601_utc(1_790_933_400), "2026-10-02T09:30:00Z");
    assert!(is_iso8601_utc(&iso8601_utc(1_790_933_400)));
    let first = new_design_id();
    assert!(is_uuid(&first), "{first}");
    assert_ne!(first, new_design_id());
}

#[test]
fn metadata_survives_save_then_open_and_save_again_unchanged() {
    let mut base = DesignMetadata {
        notes: "Cut in quartz first".to_string(),
        license: "CC BY-NC 4.0".to_string(),
        ..DesignMetadata::default()
    };
    base.unknown
        .insert("future_key".to_string(), toml::Value::String("kept".into()));
    let prepared = assemble(
        &DesignFileExtras::new(base, Vec::new()),
        Some(&row(&["b", "a"], true, true)),
        NOW,
        || ID.to_string(),
    )
    .expect("assembles");
    let text = written(&DesignExtras {
        metadata: Some(&prepared.metadata),
        attachments: &prepared.attachments,
        ..DesignExtras::default()
    });

    // Open: the loaded metadata and attachments are the ones written.
    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.metadata, prepared.metadata);
    assert_eq!(loaded.attachments, prepared.attachments);

    // Save again with no row and the same clock: the same bytes (an untouched design
    // round-trips unchanged, unknown keys included).
    let again = design_to_string(
        &round_brilliant(),
        None,
        &DesignExtras {
            metadata: Some(&loaded.metadata),
            attachments: &loaded.attachments,
            ..DesignExtras::default()
        },
    )
    .expect("serializes");
    assert_eq!(again, text);
}

#[test]
fn a_saved_file_imports_back_into_a_row_with_its_marks_and_every_attachment() {
    let prepared = assemble(
        &DesignFileExtras::default(),
        Some(&row(&["b", "a"], false, true)),
        NOW,
        || ID.to_string(),
    )
    .expect("assembles");
    let text = written(&DesignExtras {
        metadata: Some(&prepared.metadata),
        attachments: &prepared.attachments,
        ..DesignExtras::default()
    });
    let imported = import_native_design("capps.indicatrix", text.as_bytes()).expect("imports");
    assert_eq!(imported.entry.title, "Capps Brilliant");
    assert_eq!(imported.detail.designer.as_deref(), Some("Capps, Jerry"));
    assert_eq!(imported.detail.shape.as_deref(), Some("Round"));
    assert_eq!(imported.detail.pdf_file.as_deref(), Some("capps.pdf"));
    assert_eq!(
        imported.detail.diagram_image_name.as_deref(),
        Some("capps.png")
    );
    assert!(
        imported
            .detail
            .attached_files
            .iter()
            .any(|f| f.name == "capps.pdf" && f.content == b"%PDF")
    );
    assert_eq!(imported.extras.tags, ["a", "b"]);
    assert!(imported.extras.planner_excluded);
    assert!(!imported.extras.ignored);

    let db = indicatrix_vault::db::sqlite::Database::new(Some(":memory:")).expect("db");
    let id = db
        .save_design(&imported.entry, &imported.detail, "local-import")
        .expect("saves");
    apply_imported_extras(&db, id, &imported.extras).expect("applies");
    assert!(db.planner_excluded_ids().expect("ids").contains(&id));
}

#[test]
fn the_written_file_holds_no_derived_field() {
    let prepared = assemble(
        &DesignFileExtras::default(),
        Some(&row(&["a"], false, false)),
        NOW,
        || ID.to_string(),
    )
    .expect("assembles");
    let text = written(&DesignExtras {
        metadata: Some(&prepared.metadata),
        attachments: &prepared.attachments,
        ..DesignExtras::default()
    });
    for key in [
        "volume",
        "facets",
        "facets_count",
        "lw_ratio",
        "hw_ratio",
        "tw_ratio",
        "uw_ratio",
        "pw_ratio",
        "cw_ratio",
        "index_gear",
        "angle_settings",
        "preview",
        "tilt",
    ] {
        assert!(
            !text
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{key} ="))
                    || line.trim() == format!("[{key}]")),
            "the file must not store derived `{key}`"
        );
    }
    assert!(!text.contains("local://"), "no machine-local url");
    assert!(
        !text.contains("entry_id") && !text.contains("diagram_id"),
        "no row ids"
    );
}

/// The UUID a design gets when it opens is the one every later write carries: the first
/// Save, a Save As (the state keeps the saved metadata), an autosave and a reopen.
#[test]
fn the_uuid_assigned_when_a_design_opens_is_what_every_write_carries() {
    let opened = DesignFileExtras::default().with_design_uuid_assigned(None);
    let uuid = opened.metadata.id.clone();
    assert!(is_uuid(&uuid), "{uuid}");

    // First Save: no new id is made, and the file holds the one assigned at open.
    let first = assemble(&opened, None, NOW, || {
        panic!("the design already has its UUID")
    })
    .expect("assembles");
    assert_eq!(first.metadata.id, uuid);
    let text = written(&DesignExtras {
        metadata: Some(&first.metadata),
        attachments: &first.attachments,
        ..DesignExtras::default()
    });
    assert_eq!(design_from_str(&text).expect("opens").metadata.id, uuid);

    // Save As elsewhere, later, from the metadata the first Save left in the state: a copy
    // of the same design, so the same UUID (and so the same variants and progress).
    let after_first_save = DesignFileExtras::new(first.metadata, first.attachments);
    let second = assemble(
        &after_first_save,
        Some(&row(&["a"], false, false)),
        "2026-10-03T00:00:00Z",
        || panic!("Save As keeps the UUID"),
    )
    .expect("assembles");
    assert_eq!(second.metadata.id, uuid);

    // Autosave writes the state's metadata as it is, with no assembly step.
    let autosave = written(&DesignExtras {
        metadata: Some(&after_first_save.metadata),
        ..DesignExtras::default()
    });
    assert_eq!(design_from_str(&autosave).expect("opens").metadata.id, uuid);

    // Reopening the saved file keeps the id it carries.
    let reopened =
        DesignFileExtras::new(design_from_str(&text).expect("opens").metadata, Vec::new())
            .with_design_uuid_assigned(None);
    assert_eq!(reopened.metadata.id, uuid);
}

/// A catalogue design with no id of its own is named after its entry, the same way every
/// time, and saving it to a file keeps that name.
#[test]
fn a_catalogue_design_without_an_id_is_named_after_its_entry_and_a_save_keeps_the_name() {
    let url = "local://capps.asc";
    let first = DesignFileExtras::default().with_design_uuid_assigned(Some(url));
    let again = DesignFileExtras::default().with_design_uuid_assigned(Some(url));
    assert_eq!(first.metadata.id, again.metadata.id);
    assert_eq!(first.metadata.id, catalogue_design_uuid(url));

    let saved = assemble(&first, Some(&row(&[], false, false)), NOW, || {
        panic!("the design already has its UUID")
    })
    .expect("assembles");
    assert_eq!(saved.metadata.id, catalogue_design_uuid(url));

    // A catalogue design whose attached file carries an id keeps the file's id.
    let with_file_id = DesignFileExtras::new(
        DesignMetadata {
            id: ID.to_string(),
            ..DesignMetadata::default()
        },
        Vec::new(),
    )
    .with_design_uuid_assigned(Some(url));
    assert_eq!(with_file_id.metadata.id, ID);
}
