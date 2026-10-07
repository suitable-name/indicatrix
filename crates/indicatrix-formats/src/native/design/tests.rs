use super::*;
use crate::native::{
    ConcaveTierTable, MaterialTable, NativeMeetConstraint, NativePreformShape, PreformTable,
    TierTable, to_toml_string,
};
use std::path::Path;

fn schedule() -> ScheduleTable {
    ScheduleTable {
        gemcad_version: "GemCad 5.0".to_string(),
        gear_teeth: 96,
        gear_reference_angle: 3.5,
        symmetry_order: 4,
        mirror: true,
        refractive_index: 1.62,
        headers: vec!["Round brilliant".to_string()],
        footnotes: vec!["Cut with care".to_string()],
        unknown: toml::Table::new(),
    }
}

fn tier(name: &str, id: u64) -> TierTable {
    TierTable::new(name, NativeMeetConstraint::MeetExisting, Vec::new())
        .with_angle_deg(Some(40.0))
        .with_indices(Some(vec![0.0, 24.0]))
        .with_tier_id(Some(id))
}

pub(super) fn sample() -> DesignFile {
    DesignFile::new(
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        MaterialTable::new(Some("Diamond".to_string()), None, Some(1.62)),
        schedule(),
        Some(6.5),
        vec![tier("A", 0), tier("B", 1)],
    )
}

#[test]
fn round_trips_through_text() {
    let file = sample();
    let text = to_string(&file).expect("serializes");
    assert_eq!(parse(&text).expect("parses own output"), file);
}

#[test]
fn output_is_deterministic_and_header_first() {
    let a = to_string(&sample()).expect("serializes");
    let b = to_string(&sample()).expect("serializes");
    assert_eq!(a, b);
    assert!(a.starts_with("format = \"indicatrix-design\"\nversion = 1\n"));
    assert!(!a.contains('\r'));
}

#[test]
fn reserializing_a_parsed_file_is_byte_identical() {
    let text = to_string(&sample()).expect("serializes");
    let again = to_string(&parse(&text).expect("parses")).expect("serializes");
    assert_eq!(text, again);
}

#[test]
fn a_too_new_version_is_refused_before_the_body_is_read() {
    // The body is garbage a v1 reader could not parse; the version error wins.
    let text = "format = \"indicatrix-design\"\nversion = 4\n[preform]\nshape = 7\n";
    match parse(text) {
        Err(DesignFileError::UnsupportedVersion {
            found: 4,
            supported,
        }) => {
            assert_eq!(supported, DESIGN_VERSION_RELATIONS);
        }
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn a_wrong_or_missing_format_is_refused() {
    assert!(matches!(
        parse("format = \"something-else\"\nversion = 1\n"),
        Err(DesignFileError::NotADesignFile { found: Some(_) })
    ));
    assert!(matches!(
        parse("version = 1\n"),
        Err(DesignFileError::NotADesignFile { found: None })
    ));
    assert!(matches!(
        parse("format = \"indicatrix-design\"\n"),
        Err(DesignFileError::MissingVersion)
    ));
    assert!(matches!(
        parse("format = \"indicatrix-design\"\nversion = 0\n"),
        Err(DesignFileError::InvalidVersion(0))
    ));
}

#[test]
fn unknown_keys_survive_at_every_level() {
    let mut file = sample();
    file.unknown
        .insert("future_top".to_string(), toml::Value::String("kept".into()));
    file.preform
        .unknown
        .insert("future_preform".to_string(), toml::Value::Integer(42));
    file.material
        .unknown
        .insert("future_material".to_string(), toml::Value::Boolean(true));
    file.schedule
        .unknown
        .insert("future_schedule".to_string(), toml::Value::Float(0.5));
    file.tiers[1]
        .unknown
        .insert("future_tier".to_string(), toml::Value::String("too".into()));
    let text = to_string(&file).expect("serializes");
    let parsed = parse(&text).expect("parses");
    assert_eq!(parsed, file);
    assert_eq!(to_string(&parsed).expect("serializes"), text);
}

#[test]
fn a_tier_without_geometry_or_with_a_repeated_id_is_refused() {
    let mut file = sample();
    file.tiers[0].indices = None;
    let text = to_string(&file).expect("serializes");
    assert!(matches!(
        parse(&text),
        Err(DesignFileError::TierMissingGeometry { index: 0 })
    ));

    let mut file = sample();
    file.tiers[1].tier_id = Some(0);
    let text = to_string(&file).expect("serializes");
    assert!(matches!(
        parse(&text),
        Err(DesignFileError::DuplicateTierId(0))
    ));
}

#[test]
fn an_absurd_gear_is_refused() {
    let mut file = sample();
    file.schedule.gear_teeth = 100_000;
    let text = to_string(&file).expect("serializes");
    assert!(matches!(
        parse(&text),
        Err(DesignFileError::InvalidField {
            field: "schedule.gear_teeth",
            ..
        })
    ));
}

#[test]
fn detects_a_design_file_and_an_old_overlay_sidecar() {
    let design_text = to_string(&sample()).expect("serializes");
    assert_eq!(detect_kind(design_text.as_bytes()), FileKind::Design);

    let overlay = crate::native::NativeDesignFile::new(
        "design.asc",
        "00",
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        None,
        MaterialTable::new(None, None, None),
        vec![TierTable::new(
            "T",
            NativeMeetConstraint::MeetExisting,
            Vec::new(),
        )],
    );
    let overlay_text = to_toml_string(&overlay).expect("serializes");
    assert_eq!(
        detect_kind(overlay_text.as_bytes()),
        FileKind::OverlaySidecar
    );
    assert!(matches!(
        parse(&overlay_text),
        Err(DesignFileError::NotADesignFile { found: None })
    ));

    assert_eq!(detect_kind(b"hello = 1\n"), FileKind::Unknown);
    assert_eq!(detect_kind(&[0xff, 0xfe, 0x00]), FileKind::Unknown);
}

#[test]
fn path_helpers_tell_the_formats_apart() {
    assert_eq!(DESIGN_EXTENSION, "indicatrix");
    assert!(is_design_path(Path::new("a/b.indicatrix")));
    assert!(is_design_path(Path::new("B.INDICATRIX")));
    assert!(!is_design_path(Path::new("b.indicatrix.toml")));
    assert!(!is_design_path(Path::new("b.asc")));
    assert_eq!(design_path_for("stone"), Path::new("stone.indicatrix"));
    assert_eq!(
        design_path_for_sibling(Path::new("d/foo.indicatrix.toml")),
        Some(Path::new("d/foo.indicatrix").to_path_buf())
    );
    assert_eq!(
        design_path_for_sibling(Path::new("d/foo.asc")),
        Some(Path::new("d/foo.indicatrix").to_path_buf())
    );
    assert_eq!(
        detect_kind_of_path(Path::new("x.gemcut.toml")),
        FileKind::OverlaySidecar
    );
    assert_eq!(
        detect_kind_of_path(Path::new("x.indicatrix")),
        FileKind::Design
    );
    assert_eq!(detect_kind_of_path(Path::new("x.asc")), FileKind::Unknown);
}

fn concave_tier(name: &str) -> ConcaveTierTable {
    ConcaveTierTable {
        name: name.to_string(),
        angle_deg: -62.0,
        indices: vec![3.0, 11.5, 19.0],
        instructions: "cut to depth".to_string(),
        tool: "CON".to_string(),
        tool_azimuth_deg: -15.0,
        displacement: [0.0, 0.12, 0.05],
        diameter_ratio: 0.25,
        tool_angle_deg: Some(60.0),
        motion: "plunge".to_string(),
        concave_tier_id: None,
        unknown: toml::Table::new(),
    }
}

#[test]
fn a_concave_tier_id_round_trips_and_an_absent_one_never_writes_the_key() {
    let plain = sample().with_concave_tiers(vec![concave_tier("P8")]);
    let plain_text = to_string(&plain).expect("serializes");
    assert!(
        !plain_text.contains("concave_tier_id"),
        "a record without an id must not grow the key"
    );

    let mut with_ids = concave_tier("P8");
    with_ids.concave_tier_id = Some(7);
    let mut second = concave_tier("C9");
    second.concave_tier_id = Some(12);
    let file = sample().with_concave_tiers(vec![with_ids, second]);
    let text = to_string(&file).expect("serializes");
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 2\n"));
    assert_eq!(text.matches("concave_tier_id").count(), 2);
    let parsed = parse(&text).expect("parses own output");
    assert_eq!(parsed, file);
    assert_eq!(parsed.concave_tiers[0].concave_tier_id, Some(7));
    assert_eq!(parsed.concave_tiers[1].concave_tier_id, Some(12));
    assert!(
        parsed.concave_tiers[0].unknown.is_empty(),
        "the id is a typed field, not a parked unknown key"
    );
    assert_eq!(to_string(&parsed).expect("serializes"), text);
}

#[test]
fn design_file_without_concave_tiers_serialises_as_version_1_byte_identical() {
    let text = to_string(&sample()).expect("serializes");
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 1\n"));
    assert!(!text.contains("concave"));
    // An explicitly empty list is the planar file, not a version-2 one.
    let emptied = sample().with_concave_tiers(Vec::new());
    assert_eq!(to_string(&emptied).expect("serializes"), text);
}

#[test]
fn design_file_with_concave_tiers_serialises_as_version_2_and_round_trips() {
    let file = sample().with_concave_tiers(vec![concave_tier("P8"), concave_tier("C9")]);
    let text = to_string(&file).expect("serializes");
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 2\n"));
    assert!(text.contains("concave_frame = \"v0\""));
    let parsed = parse(&text).expect("parses own output");
    assert_eq!(parsed, file);
    assert_eq!(parsed.version, DESIGN_VERSION_CONCAVE);
    assert_eq!(parsed.concave_frame, CONCAVE_FRAME_V0);
    assert_eq!(to_string(&parsed).expect("serializes"), text);
}

#[test]
fn design_file_with_unknown_concave_frame_is_refused() {
    let file = sample().with_concave_tiers(vec![concave_tier("P8")]);
    let text = to_string(&file).expect("serializes");
    let other = text.replacen("concave_frame = \"v0\"", "concave_frame = \"v9\"", 1);
    assert!(matches!(
        parse(&other),
        Err(DesignFileError::UnsupportedConcaveFrame { frame }) if frame == "v9"
    ));
    let missing = text.replacen("concave_frame = \"v0\"\n", "", 1);
    assert!(matches!(
        parse(&missing),
        Err(DesignFileError::UnsupportedConcaveFrame { frame }) if frame.is_empty()
    ));
}

/// `sample()` whose second tier follows a relation.
fn related_sample() -> DesignFile {
    let mut file = sample();
    file.tiers[1] = file.tiers[1]
        .clone()
        .with_angle_relation(Some("@0 - 2".to_string()));
    file.version = file.needed_version();
    file
}

#[test]
fn a_tier_relation_makes_the_file_version_3_and_round_trips() {
    let file = related_sample();
    assert_eq!(file.version, DESIGN_VERSION_RELATIONS);
    let text = to_string(&file).expect("serializes");
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 3\n"));
    assert_eq!(text.matches("angle_relation = \"@0 - 2\"").count(), 1);
    let parsed = parse(&text).expect("parses own output");
    assert_eq!(parsed, file);
    assert_eq!(parsed.tiers[1].angle_relation.as_deref(), Some("@0 - 2"));
    assert_eq!(parsed.tiers[0].angle_relation, None);
    assert_eq!(to_string(&parsed).expect("serializes"), text);
}

#[test]
fn a_file_without_relations_never_writes_the_key() {
    let text = to_string(&sample()).expect("serializes");
    assert!(!text.contains("angle_relation"));
    assert_eq!(sample().needed_version(), DESIGN_VERSION);
}

#[test]
fn relations_and_concave_tiers_share_version_3() {
    let file = related_sample().with_concave_tiers(vec![concave_tier("P8")]);
    assert_eq!(file.version, DESIGN_VERSION_RELATIONS);
    let text = to_string(&file).expect("serializes");
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 3\n"));
    assert!(text.contains("concave_frame = \"v0\""));
    assert_eq!(parse(&text).expect("parses own output"), file);

    // Dropping the concave tiers keeps the relations' version; dropping the relations
    // as well returns to the plain one.
    let planar = file.with_concave_tiers(Vec::new());
    assert_eq!(planar.version, DESIGN_VERSION_RELATIONS);
    assert_eq!(planar.concave_frame, "");
    let mut bare = planar;
    bare.tiers[1].angle_relation = None;
    assert_eq!(bare.needed_version(), DESIGN_VERSION);
}

#[test]
fn versions_1_to_3_are_all_accepted_and_4_is_not() {
    for version in 1..=DESIGN_VERSION_RELATIONS {
        let text = to_string(&sample()).expect("serializes");
        let other = text.replacen("version = 1", &format!("version = {version}"), 1);
        assert!(parse(&other).is_ok(), "version {version}");
    }
    let text = to_string(&sample()).expect("serializes");
    let newer = text.replacen("version = 1", "version = 4", 1);
    assert!(matches!(
        parse(&newer),
        Err(DesignFileError::UnsupportedVersion { found: 4, .. })
    ));
}

#[test]
fn concave_tier_without_a_tool_code_is_refused_not_defaulted() {
    let mut tier = concave_tier("P8");
    tier.tool = String::new();
    let file = sample().with_concave_tiers(vec![tier]);
    let text = to_string(&file).expect("serializes");
    assert!(matches!(
        parse(&text),
        Err(DesignFileError::ConcaveTierMissingGeometry { index: 0 })
    ));
}
