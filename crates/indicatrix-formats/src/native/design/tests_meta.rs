//! `[meta]` and `[[attachments]]`: round trips, limits, integrity checks, determinism.

use super::{tests::sample, *};

const UUID: &str = "0b5f0a8e-5a43-4d52-9c1f-7c8f3a1e2d90";

fn full_meta() -> DesignMetadata {
    DesignMetadata {
        id: UUID.to_string(),
        title: "Rundbrillant \u{2014} \u{30c0}\u{30a4}\u{30e4} \u{1f48e}".to_string(),
        designer: "Capps, Jerry".to_string(),
        designer_info: "Capps, Jerry; Lapidary Journal, May 1994, p95".to_string(),
        source_citation: "Lapidary Journal, May 1994, p95".to_string(),
        source_url: "https://example.org/diagram/1".to_string(),
        source_design_id: "D-1".to_string(),
        shape: "Round".to_string(),
        shape_category: "1".to_string(),
        competition: "USFG 2020 Novice".to_string(),
        pdf_file: "booklet.pdf".to_string(),
        gem_file: "round.gem".to_string(),
        license: "CC BY-NC 4.0".to_string(),
        copyright: "(c) the designer".to_string(),
        notes: "line one\nline two with \"quotes\", a tab\t, a CRLF\r\nand 'single'\n".to_string(),
        created_at: "2026-10-02T09:30:00Z".to_string(),
        modified_at: "2026-10-03T10:00:00.250Z".to_string(),
        tags: vec!["round".to_string(), "caf\u{e9}".to_string()],
        planner_excluded: true,
        ignored: true,
        unknown: toml::Table::new(),
    }
}

fn all_bytes() -> Vec<u8> {
    (0..=255u8).chain((0..=255u8).rev()).collect()
}

fn blob(name: &str, data: Vec<u8>) -> AttachmentBlob {
    AttachmentBlob::new(name, AttachmentRole::Other, data)
}

#[test]
fn metadata_round_trips_with_unicode_and_control_text() {
    let file = sample().with_meta(full_meta());
    let text = to_string(&file).expect("serializes");
    let parsed = parse(&text).expect("parses");
    assert_eq!(parsed.meta, full_meta());
    assert_eq!(to_string(&parsed).expect("serializes"), text);
}

#[test]
fn long_text_round_trips_and_an_oversize_text_is_refused() {
    let mut meta = DesignMetadata {
        notes: "n\u{e4}chste Zeile\n".repeat(50_000),
        ..DesignMetadata::default()
    };
    assert!(meta.notes.len() < MAX_META_TEXT_BYTES);
    let text = to_string(&sample().with_meta(meta.clone())).expect("serializes");
    assert_eq!(parse(&text).expect("parses").meta, meta);

    meta.notes = "x".repeat(MAX_META_TEXT_BYTES + 1);
    assert!(matches!(
        to_string(&sample().with_meta(meta)),
        Err(DesignFileError::InvalidField {
            field: "meta.notes",
            ..
        })
    ));
}

#[test]
fn absent_or_empty_meta_loads_as_default_and_writes_no_table() {
    let text = to_string(&sample()).expect("serializes");
    assert!(!text.contains("[meta]") && !text.contains("[[attachments]]"));
    let parsed = parse(&text).expect("parses");
    assert!(parsed.meta.is_empty() && parsed.attachments.is_empty());

    let with_empty_table = text.replacen("[preform]", "[meta]\n\n[preform]", 1);
    assert!(parse(&with_empty_table).expect("parses").meta.is_empty());
}

#[test]
fn unknown_keys_inside_meta_and_attachments_survive() {
    let mut meta = full_meta();
    meta.unknown.insert(
        "future_meta".to_string(),
        toml::Value::String("kept".into()),
    );
    let mut file = sample()
        .with_meta(meta)
        .with_attachments(&[blob("a.bin", vec![1, 2, 3])]);
    file.attachments[0]
        .unknown
        .insert("future_att".to_string(), toml::Value::Integer(9));
    let text = to_string(&file).expect("serializes");
    let parsed = parse(&text).expect("parses");
    assert_eq!(parsed, file);
    assert_eq!(to_string(&parsed).expect("serializes"), text);
}

#[test]
fn malformed_ids_dates_and_tags_are_refused() {
    let bad = |edit: fn(&mut DesignMetadata)| {
        let mut meta = full_meta();
        edit(&mut meta);
        to_string(&sample().with_meta(meta))
    };
    assert!(bad(|m| m.id = "not-a-uuid".to_string()).is_err());
    assert!(bad(|m| m.created_at = "2026-10-02".to_string()).is_err());
    assert!(bad(|m| m.modified_at = "2026-13-02T00:00:00Z".to_string()).is_err());
    assert!(bad(|m| m.tags.push("  ".to_string())).is_err());
    assert!(is_iso8601_utc("2026-10-02T09:30:00Z"));
    assert!(is_iso8601_utc("2026-10-02T09:30:00.123456789Z"));
    assert!(!is_iso8601_utc("2026-10-02T09:30:00+02:00"));
    assert!(is_uuid(UUID) && !is_uuid("0b5f0a8e5a434d529c1f7c8f3a1e2d90"));
}

#[test]
fn binary_attachments_round_trip_with_every_byte_value() {
    let blobs = vec![
        AttachmentBlob {
            source_url: "https://example.org/booklet.pdf".to_string(),
            role: AttachmentRole::Pdf,
            ..blob("booklet.pdf", all_bytes())
        },
        blob("empty.bin", Vec::new()),
        blob("one.bin", vec![0xff]),
        blob("two.bin", vec![0xff, 0x00]),
    ];
    let text = to_string(&sample().with_attachments(&blobs)).expect("serializes");
    assert!(text.contains("mime_type = \"application/pdf\""));
    let parsed = parse(&text).expect("parses");
    assert_eq!(
        attachment_blobs(&parsed.attachments).expect("decodes"),
        blobs
    );
}

#[test]
fn a_sha256_mismatch_is_refused_on_read() {
    let text = to_string(&sample().with_attachments(&[blob("a.bin", vec![1, 2, 3, 4])]))
        .expect("serializes");
    // Same size, different bytes: AQIDBA== is 1,2,3,4; AQIDBQ== is 1,2,3,5.
    let damaged = text.replace("AQIDBA==", "AQIDBQ==");
    assert_ne!(damaged, text);
    match parse(&damaged) {
        Err(DesignFileError::Attachments(AttachmentsError::Entry {
            problem: AttachmentProblem::HashMismatch,
            ..
        })) => {}
        other => panic!("expected a hash mismatch, got {other:?}"),
    }
}

#[test]
fn a_size_mismatch_and_bad_base64_are_refused() {
    let text = to_string(&sample().with_attachments(&[blob("a.bin", vec![1, 2, 3, 4])]))
        .expect("serializes");
    let wrong_size = text.replace("size = 4", "size = 5");
    assert!(matches!(
        parse(&wrong_size),
        Err(DesignFileError::Attachments(AttachmentsError::Entry {
            problem: AttachmentProblem::SizeMismatch { .. },
            ..
        }))
    ));
    let bad_data = text.replace("AQIDBA==", "AQID*A==");
    assert!(matches!(
        parse(&bad_data),
        Err(DesignFileError::Attachments(AttachmentsError::Entry {
            problem: AttachmentProblem::BadEncoding,
            ..
        }))
    ));
}

#[test]
fn the_total_size_cap_is_enforced_when_writing_and_reading() {
    // Declared sizes are summed before anything is decoded, so the cap is exercised
    // without allocating 64 MiB.
    let half = MAX_ATTACHMENT_BYTES / 2;
    let sized = |sizes: [u64; 3]| {
        let mut file = sample().with_attachments(&[
            blob("a.bin", vec![1]),
            blob("b.bin", vec![2]),
            blob("c.bin", vec![3]),
        ]);
        for (table, size) in file.attachments.iter_mut().zip(sizes) {
            table.size = size;
        }
        file
    };
    let over = sized([half, half, 1]);
    assert!(matches!(
        to_string(&over),
        Err(DesignFileError::Attachments(
            AttachmentsError::TotalTooLarge { total }
        )) if total == MAX_ATTACHMENT_BYTES + 1
    ));
    let text = toml::to_string_pretty(&over).expect("raw serialization");
    assert!(matches!(
        parse(&text),
        Err(DesignFileError::Attachments(
            AttachmentsError::TotalTooLarge { .. }
        ))
    ));

    // Exactly at the cap passes the size check; only the (fake) size then disagrees
    // with the bytes.
    let at_cap = sized([half, half, 0]);
    assert!(matches!(
        validate_attachments(&at_cap.attachments),
        Err(AttachmentsError::Entry {
            problem: AttachmentProblem::SizeMismatch { .. },
            ..
        })
    ));

    // One attachment alone over the cap.
    let mut single = sample().with_attachments(&[blob("a.bin", vec![1])]);
    single.attachments[0].size = MAX_ATTACHMENT_BYTES + 1;
    assert!(validate_attachments(&single.attachments).is_err());
}

#[test]
fn duplicate_and_invalid_names_are_refused() {
    let dup = [blob("a.bin", vec![1]), blob("a.bin", vec![2])];
    assert!(matches!(
        to_string(&sample().with_attachments(&dup)),
        Err(DesignFileError::Attachments(AttachmentsError::Entry {
            problem: AttachmentProblem::DuplicateName,
            ..
        }))
    ));
    for name in ["", "dir/a.bin", "dir\\a.bin", "tab\there"] {
        assert!(
            to_string(&sample().with_attachments(&[blob(name, vec![1])])).is_err(),
            "{name:?}"
        );
    }
    let long = "n".repeat(MAX_ATTACHMENT_NAME_BYTES + 1);
    assert!(to_string(&sample().with_attachments(&[blob(&long, vec![1])])).is_err());
}

#[test]
fn output_with_meta_and_attachments_is_byte_identical_twice() {
    let make = || {
        sample()
            .with_meta(full_meta())
            .with_attachments(&[blob("a.bin", all_bytes()), blob("b.pdf", vec![5; 1000])])
    };
    let a = to_string(&make()).expect("serializes");
    let b = to_string(&make()).expect("serializes");
    assert_eq!(a, b);
    assert!(a.starts_with("format = \"indicatrix-design\"\nversion = 1\n"));
    assert!(!a.contains('\r'));
}

#[test]
fn base64_matches_the_rfc_vectors_and_refuses_loose_input() {
    use super::base64::{decode, encode};
    for (plain, coded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(encode(plain.as_bytes()), coded);
        assert_eq!(decode(coded).as_deref(), Some(plain.as_bytes()));
    }
    for loose in [
        "Zg",
        "Zg=",
        "Zm9v\n",
        "Zm9v YmE=",
        "Zh==",
        "Zg==Zg==",
        "Zm-v",
        "=m9v",
    ] {
        assert_eq!(decode(loose), None, "{loose:?}");
    }
}
