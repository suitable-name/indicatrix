use super::*;
use crate::native::{
    MaterialTable, NativeMeetConstraint, NativePreformShape, PreformTable, TierTable,
    to_toml_string,
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
    let text = "format = \"indicatrix-design\"\nversion = 2\n[preform]\nshape = 7\n";
    match parse(text) {
        Err(DesignFileError::UnsupportedVersion {
            found: 2,
            supported,
        }) => {
            assert_eq!(supported, DESIGN_VERSION);
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
