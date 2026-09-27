//! Tests for `FetchDesign`/`FetchAttachment`/`FetchDesignSource`/`FilterOptions`: the
//! single-record and metadata-lookup side of the library protocol, as opposed to
//! `tests::search`'s multi-row search/paging tests.

use super::fixtures::populated_temp_db;
use crate::serve::library::handle_request;
use indicatrix_net::library::{LibraryRequest, LibraryResponse};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{detail::FacetDiagramDetail, entry::FacetDiagramEntry, file::AttachedFile},
};

#[test]
fn fetch_design_returns_metadata_only_never_attachment_content() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(&LibraryRequest::FetchDesign { entry_id: 1 }, &db);
    let LibraryResponse::Design(record) = response else {
        panic!("expected Design, got {response:?}");
    };
    assert_eq!(record.attachments.len(), 1);
    assert_eq!(record.attachments[0].name, "schedule.pdf");
    assert_eq!(record.attachments[0].size, 5);
    assert_eq!(record.angle_settings.len(), 1);
    assert_ne!(record.version, [0u8; 32]);

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// `DesignRecord::preview_material` must carry the preset name a design's cached
/// previews were generated with, so a remote client can run the same stale-material
/// check the local path does. Asserts a genuinely stored value, not `None`, since
/// `None` would pin nothing.
#[test]
fn fetch_design_carries_the_stored_preview_material() {
    let path = populated_temp_db();
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();

    // One candidate within tolerance -> chosen outright, no RNG draw, so this
    // fixture is deterministic.
    let candidates = [indicatrix_vault::model::material_match::RiPresetCandidate {
        name: "Diamond".to_string(),
        refractive_index: 2.417,
    }];
    let stored = db
        .ensure_preview_material(1, 2.417, &candidates, 0.01, &mut || 0.0)
        .unwrap();
    assert_eq!(stored.as_deref(), Some("Diamond"));

    let response = handle_request(&LibraryRequest::FetchDesign { entry_id: 1 }, &db);
    let LibraryResponse::Design(record) = response else {
        panic!("expected Design, got {response:?}");
    };
    assert_eq!(record.preview_material.as_deref(), Some("Diamond"));

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// The ratio/symmetry/designer fields on `DesignRecord` must carry the same values
/// a local lookup sees. Builds its own fixture (rather than `populated_temp_db`,
/// which leaves them `None`) with every field set to a distinct, non-default value,
/// so this pins that each survives `to_record`/the wire, not merely that it exists.
#[test]
fn fetch_design_carries_the_stored_ratio_and_symmetry_fields() {
    let path = std::env::temp_dir().join(format!(
        "indicatrix-worker-library-ratio-test-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path_str = path.to_str().unwrap();
    let db = Database::new(Some(path_str)).unwrap();

    let entry_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Round Brilliant".to_string(),
                url: "https://example.test/diagram/1".to_string(),
                design_id: "RB-1".to_string(),
            },
            "facetdiagrams.org",
        )
        .unwrap();
    let detail = FacetDiagramDetail {
        page_url: "https://example.test/diagram/1".to_string(),
        shape: Some("Round".to_string()),
        refractive_index: Some("2.417".to_string()),
        hw_ratio: Some("1.0".to_string()),
        tw_ratio: Some("0.53".to_string()),
        uw_ratio: Some("0.16".to_string()),
        pw_ratio: Some("0.43".to_string()),
        cw_ratio: Some("0.14".to_string()),
        symmetry_order: Some("8".to_string()),
        mirror_symmetry: Some(true),
        designer: Some("Capps, Jerry".to_string()),
        ..Default::default()
    };
    db.save_diagram_detail(&detail, entry_id).unwrap();
    drop(db);

    let db = Database::open_read_only(path_str).unwrap();
    let response = handle_request(&LibraryRequest::FetchDesign { entry_id }, &db);
    let LibraryResponse::Design(record) = response else {
        panic!("expected Design, got {response:?}");
    };
    assert_eq!(record.hw_ratio.as_deref(), Some("1.0"));
    assert_eq!(record.tw_ratio.as_deref(), Some("0.53"));
    assert_eq!(record.uw_ratio.as_deref(), Some("0.16"));
    assert_eq!(record.pw_ratio.as_deref(), Some("0.43"));
    assert_eq!(record.cw_ratio.as_deref(), Some("0.14"));
    assert_eq!(record.symmetry_order.as_deref(), Some("8"));
    assert_eq!(record.mirror_symmetry, Some(true));
    assert_eq!(record.designer.as_deref(), Some("Capps, Jerry"));
    assert_ne!(record.version, [0u8; 32]);

    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn fetch_design_for_an_unknown_id_is_not_found() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(&LibraryRequest::FetchDesign { entry_id: 999 }, &db);
    assert_eq!(response, LibraryResponse::NotFound);

    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn fetch_attachment_returns_exactly_that_attachments_bytes() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(&LibraryRequest::FetchAttachment { attachment_id: 1 }, &db);
    assert_eq!(
        response,
        LibraryResponse::Attachment {
            name: "schedule.pdf".to_string(),
            content: vec![1, 2, 3, 4, 5],
        }
    );

    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn fetch_attachment_for_an_unknown_id_is_not_found() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(&LibraryRequest::FetchAttachment { attachment_id: 999 }, &db);
    assert_eq!(response, LibraryResponse::NotFound);

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// `FilterOptions` serves `Database::get_unique_shapes`, the union of the seeded
/// canonical vocabulary and the shapes actually present in the library -- a remote
/// client gets the full picker list, exactly as a local one does.
#[test]
fn filter_options_reports_the_seeded_shape_alongside_the_canonical_vocabulary() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(&LibraryRequest::FilterOptions, &db);
    let LibraryResponse::FilterOptions { shapes, .. } = response else {
        panic!("expected FilterOptions, got {response:?}");
    };

    assert!(
        shapes.contains(&"Round".to_string()),
        "the served library's own shape must still be reported, got {shapes:?}"
    );
    assert!(
        shapes.contains(&"Marquise".to_string()),
        "the seeded canonical vocabulary must reach a remote client too, got {shapes:?}"
    );
    // "Round" is both seeded and present on the fixture -- must dedupe.
    assert_eq!(
        shapes.iter().filter(|s| *s == "Round").count(),
        1,
        "a shape in both the vocabulary and the data must appear once, got {shapes:?}"
    );

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// [`LibraryRequest::FetchDesignSource`] against the seeded fixture's real attached
/// `.asc` file (`populated_temp_db` names it `schedule.pdf`, NOT `.asc` -- see the
/// dedicated fixture built below) returns that attachment's exact text.
#[test]
fn fetch_design_source_returns_the_attached_asc_files_exact_text() {
    let path = std::env::temp_dir().join(format!(
        "indicatrix-worker-library-source-test-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path_str = path.to_str().unwrap();
    let db = Database::new(Some(path_str)).unwrap();

    let entry_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Round Brilliant".to_string(),
                url: "https://example.test/diagram/1".to_string(),
                design_id: "RB-1".to_string(),
            },
            "facetdiagrams.org",
        )
        .unwrap();
    let asc_text = "GemCad 5.0\nR1  c 41.0 96\n";
    let detail = FacetDiagramDetail {
        page_url: "https://example.test/diagram/1".to_string(),
        shape: Some("Round".to_string()),
        attached_files: vec![AttachedFile {
            name: "round-brilliant.asc".to_string(),
            url: "https://example.test/round-brilliant.asc".to_string(),
            content: asc_text.as_bytes().to_vec(),
        }],
        ..Default::default()
    };
    db.save_diagram_detail(&detail, entry_id).unwrap();
    drop(db);

    let ro = Database::open_read_only(path_str).unwrap();
    let response = handle_request(&LibraryRequest::FetchDesignSource { entry_id }, &ro);
    assert_eq!(
        response,
        LibraryResponse::DesignSource {
            entry_id,
            file_name: "round-brilliant.asc".to_string(),
            asc_text: asc_text.to_string(),
        }
    );

    drop(ro);
    std::fs::remove_file(&path).ok();
}

/// A design with SOME attachment but no `.asc` among them (the `populated_temp_db`
/// fixture: only `schedule.pdf`) has no genuine source text to send --
/// `DesignSourceNotAvailable`, not `NotFound` (the entry itself is real).
#[test]
fn fetch_design_source_for_a_design_with_no_asc_attachment_is_not_available() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(&LibraryRequest::FetchDesignSource { entry_id: 1 }, &db);
    assert_eq!(response, LibraryResponse::DesignSourceNotAvailable);

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// An `entry_id` with no matching row at all is `NotFound`, distinct from
/// `DesignSourceNotAvailable` (which means the entry exists but lacks a `.asc`).
#[test]
fn fetch_design_source_for_an_unknown_entry_is_not_found() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(&LibraryRequest::FetchDesignSource { entry_id: 999 }, &db);
    assert_eq!(response, LibraryResponse::NotFound);

    drop(db);
    std::fs::remove_file(&path).ok();
}
