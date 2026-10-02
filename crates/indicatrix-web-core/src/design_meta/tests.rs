//! Time and id shapes, stamping, and the metadata/attachment pass-through of a design
//! file opened and saved again.

use super::*;
use indicatrix_cut_core::{
    FreshDesignSpec, MaterialSelection, PreformSpec,
    native::{DesignExtras, design_from_str, design_to_string},
};
use indicatrix_editor::EditorSession;
use indicatrix_formats::native::design::{AttachmentBlob, AttachmentRole, is_iso8601_utc, is_uuid};

#[test]
fn epoch_milliseconds_become_iso_utc_text() {
    assert_eq!(iso8601_utc_from_epoch_ms(0.0), "1970-01-01T00:00:00Z");
    assert_eq!(
        iso8601_utc_from_epoch_ms(1_700_000_000_999.0),
        "2023-11-14T22:13:20Z"
    );
    // A leap day.
    assert_eq!(
        iso8601_utc_from_epoch_ms(951_782_400_000.0),
        "2000-02-29T00:00:00Z"
    );
    assert_eq!(iso8601_utc_from_epoch_ms(-1000.0), "1969-12-31T23:59:59Z");
    assert_eq!(iso8601_utc_from_epoch_ms(f64::NAN), "1970-01-01T00:00:00Z");
    for ms in [0.0, 86_399_999.0, 1.7e12, 4.1e12] {
        assert!(is_iso8601_utc(&iso8601_utc_from_epoch_ms(ms)));
    }
}

#[test]
fn uuid_text_has_the_version_four_shape() {
    let id = uuid_v4_from_bytes([0xff; 16]);
    assert!(is_uuid(&id));
    assert_eq!(id, "ffffffff-ffff-4fff-bfff-ffffffffffff");
    assert_eq!(
        uuid_v4_from_bytes([0; 16]),
        "00000000-0000-4000-8000-000000000000"
    );
}

#[test]
fn stamping_keeps_an_id_and_creation_time_and_sorts_tags() {
    let opened = DesignMetadata {
        id: "0b5f0a8e-5a43-4d52-9c1f-7c8f3a1e2d90".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        modified_at: "2026-02-01T00:00:00Z".to_string(),
        tags: vec!["round".to_string(), "classic".to_string()],
        ..DesignMetadata::default()
    };
    let stamped = stamped_for_save(&opened, "2026-10-02T09:30:00Z", || {
        panic!("an existing id needs no fresh one")
    });
    assert_eq!(stamped.id, opened.id);
    assert_eq!(stamped.created_at, "2026-01-01T00:00:00Z");
    assert_eq!(stamped.modified_at, "2026-10-02T09:30:00Z");
    assert_eq!(stamped.tags, ["classic", "round"]);
}

#[test]
fn stamping_an_empty_table_creates_an_id_and_both_times() {
    let stamped = stamped_for_save(&DesignMetadata::default(), "2026-10-02T09:30:00Z", || {
        [7; 16]
    });
    assert!(is_uuid(&stamped.id));
    assert_eq!(stamped.created_at, "2026-10-02T09:30:00Z");
    assert_eq!(stamped.modified_at, "2026-10-02T09:30:00Z");
}

fn template_design() -> indicatrix_cut_core::Design {
    EditorSession::from_template(
        FreshDesignSpec {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material: MaterialSelection::none(),
            preform: PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        },
        0,
    )
    .design
}

#[test]
fn metadata_and_attachments_survive_open_and_save_with_unknown_keys() {
    let design = template_design();
    let meta = DesignMetadata {
        title: "Round brilliant".to_string(),
        designer: "Capps, Jerry".to_string(),
        notes: "Cut in quartz first".to_string(),
        license: "CC BY-NC 4.0".to_string(),
        tags: vec!["round".to_string(), "classic".to_string()],
        planner_excluded: true,
        ..DesignMetadata::default()
    };
    let blobs = [
        AttachmentBlob::new("round.pdf", AttachmentRole::Pdf, b"%PDF".to_vec()),
        AttachmentBlob::new("round.gem", AttachmentRole::Gem, vec![0, 1, 2, 255]),
    ];
    let first = design_to_string(
        &design,
        None,
        &DesignExtras {
            metadata: Some(&meta),
            attachments: &blobs,
            ..DesignExtras::default()
        },
    )
    .expect("the design file serialises");
    // A key a newer build wrote into `[meta]`.
    let first = first.replacen("[meta]\n", "[meta]\nfuture_key = 7\n", 1);

    // Open, stamp as a save does, write back.
    let opened = design_from_str(&first).expect("the design file opens");
    assert_eq!(opened.metadata.unknown.len(), 1);
    assert_eq!(opened.metadata.title, meta.title);
    assert_eq!(opened.attachments, blobs);
    let stamped = stamped_for_save(&opened.metadata, "2026-10-02T09:30:00Z", || [3; 16]);
    let second = design_to_string(
        &opened.design,
        opened.printed_proportions.as_ref(),
        &DesignExtras {
            metadata: Some(&stamped),
            attachments: &opened.attachments,
            ..DesignExtras::default()
        },
    )
    .expect("the stamped file serialises");

    let reopened = design_from_str(&second).expect("the saved file opens");
    assert_eq!(reopened.attachments, blobs);
    assert_eq!(reopened.metadata.unknown, opened.metadata.unknown);
    assert_eq!(reopened.metadata.title, meta.title);
    assert_eq!(reopened.metadata.designer, meta.designer);
    assert_eq!(reopened.metadata.notes, meta.notes);
    assert_eq!(reopened.metadata.license, meta.license);
    assert!(reopened.metadata.planner_excluded);
    assert_eq!(reopened.metadata.tags, ["classic", "round"]);
    assert_eq!(reopened.metadata.modified_at, "2026-10-02T09:30:00Z");
    assert!(is_uuid(&reopened.metadata.id));
}
