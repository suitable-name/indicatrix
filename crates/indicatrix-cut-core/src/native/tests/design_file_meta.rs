//! `[meta]` and `[[attachments]]` through the design-file conversions.

use super::fixtures::simple_design;
use crate::native::{
    AttachmentBlob, AttachmentRole, DesignExtras, DesignFileError, DesignLoadError, DesignMetadata,
    SaveExtras, design_from_file, design_from_str, design_to_file, design_to_string,
};

fn metadata() -> DesignMetadata {
    DesignMetadata {
        id: "0b5f0a8e-5a43-4d52-9c1f-7c8f3a1e2d90".to_string(),
        title: "Pavillon \u{2014} \u{1f48e}".to_string(),
        designer: "Capps, Jerry".to_string(),
        source_citation: "Lapidary Journal, May 1994, p95".to_string(),
        notes: "first line\nsecond line".to_string(),
        created_at: "2026-10-02T09:30:00Z".to_string(),
        modified_at: "2026-10-03T10:00:00Z".to_string(),
        tags: vec!["round".to_string()],
        planner_excluded: true,
        ..DesignMetadata::default()
    }
}

fn blobs() -> Vec<AttachmentBlob> {
    vec![
        AttachmentBlob::new("booklet.pdf", AttachmentRole::Pdf, (0..=255u8).collect()),
        AttachmentBlob::new("diagram.png", AttachmentRole::DiagramImage, vec![0, 1, 2]),
    ]
}

#[test]
fn metadata_and_attachments_round_trip_beside_a_bit_identical_design() {
    let design = simple_design();
    let meta = metadata();
    let attachments = blobs();
    let extras = DesignExtras {
        metadata: Some(&meta),
        attachments: &attachments,
        ..DesignExtras::default()
    };
    let text = design_to_string(&design, None, &extras).expect("serializes");
    let loaded = design_from_str(&text).expect("opens");

    assert_eq!(loaded.design, design);
    assert!(loaded.design.tier_ids_eq(&design));
    assert_eq!(loaded.metadata, meta);
    assert_eq!(loaded.attachments, attachments);

    let again = design_to_string(
        &loaded.design,
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
fn a_file_without_meta_loads_with_empty_metadata() {
    let text =
        design_to_string(&simple_design(), None, &DesignExtras::default()).expect("serializes");
    assert!(!text.contains("[meta]") && !text.contains("[[attachments]]"));
    let loaded = design_from_str(&text).expect("opens");
    assert!(loaded.metadata.is_empty() && loaded.attachments.is_empty());
}

#[test]
fn damaged_attachments_surface_as_a_load_error_from_a_hand_built_file() {
    let attachments = blobs();
    let extras = DesignExtras {
        attachments: &attachments,
        ..DesignExtras::default()
    };
    let mut file = design_to_file(&simple_design(), None, &extras);
    file.attachments[0].sha256 = "0".repeat(64);
    assert!(matches!(
        design_from_file(file),
        Err(DesignLoadError::File(DesignFileError::Attachments(_)))
    ));
}

#[test]
fn duplicate_attachment_names_fail_when_the_text_is_written() {
    let attachments = vec![
        AttachmentBlob::new("a.bin", AttachmentRole::Other, vec![1]),
        AttachmentBlob::new("a.bin", AttachmentRole::Other, vec![2]),
    ];
    let extras = DesignExtras {
        attachments: &attachments,
        ..DesignExtras::default()
    };
    assert!(matches!(
        design_to_string(&simple_design(), None, &extras),
        Err(DesignFileError::Attachments(_))
    ));
}

#[test]
fn save_extras_convert_without_meta() {
    let history = vec!["Set angle".to_string()];
    let save = SaveExtras {
        history_entries: &history,
        ..SaveExtras::default()
    };
    let extras = DesignExtras::from(&save);
    assert_eq!(extras.history_entries, history.as_slice());
    assert!(extras.metadata.is_none() && extras.attachments.is_empty());
}
