use super::*;

#[test]
fn kinds_are_read_from_the_extension_case_insensitively() {
    assert_eq!(
        InputFileKind::from_file_name("a.ASC"),
        Some(InputFileKind::Asc)
    );
    assert_eq!(
        InputFileKind::from_file_name("x.Gem"),
        Some(InputFileKind::Gem)
    );
    assert_eq!(
        InputFileKind::from_file_name("x.gcs"),
        Some(InputFileKind::Gcs)
    );
    assert_eq!(
        InputFileKind::from_file_name("sky.HDR"),
        Some(InputFileKind::Hdr)
    );
    assert_eq!(InputFileKind::from_file_name("notes.pdf"), None);
    assert_eq!(InputFileKind::from_file_name("no_extension"), None);
    assert!(!InputFileKind::Hdr.is_design());
    assert!(InputFileKind::Gcs.is_design());
    assert!(InputFileKind::Design.is_design());
}

/// Both design extensions are design files, in any case; the removed overlay sidecar
/// names (and any other `.toml`) are not.
#[test]
fn isdd_and_the_legacy_indicatrix_extension_are_design_files() {
    for name in [
        "round.isdd",
        "ROUND.ISDD",
        "round.indicatrix",
        "round.INDICATRIX",
    ] {
        assert_eq!(
            InputFileKind::from_file_name(name),
            Some(InputFileKind::Design),
            "{name}"
        );
    }
    for name in ["round.indicatrix.toml", "old.gemcut.toml", "settings.toml"] {
        assert_eq!(InputFileKind::from_file_name(name), None, "{name}");
    }
}

const DESIGN_HEADER: &str = "format = \"indicatrix-design\"\nversion = 1\n";
const SIDECAR_HEADER: &str = "format_version = 1\nasc_filename = \"round.asc\"\n";

#[test]
fn names_alone_decide_the_binary_and_text_kinds() {
    let kind = |name: &str| InputFileKind::classify(name, b"");
    assert_eq!(kind("sky.HDR"), Some(InputFileKind::Hdr));
    assert_eq!(kind("round.asc"), Some(InputFileKind::Asc));
    assert_eq!(kind("round.gem"), Some(InputFileKind::Gem));
    assert_eq!(kind("round.GCS"), Some(InputFileKind::Gcs));
    assert_eq!(kind("notes.txt"), None);
    assert_eq!(kind("noextension"), None);
}

#[test]
fn a_design_file_is_a_design_under_either_extension() {
    let bytes = DESIGN_HEADER.as_bytes();
    for name in [
        "round.isdd",
        "ROUND.ISDD",
        "round.indicatrix",
        "ROUND.INDICATRIX",
        // A design body under a `.toml` name is still read as one.
        "round.toml",
    ] {
        assert_eq!(
            InputFileKind::classify(name, bytes),
            Some(InputFileKind::Design),
            "{name}"
        );
    }
}

/// The removed overlay sidecar is not opened under any name: its `.toml` names are no
/// design file, and under a design name it stays a design for the loader to refuse.
#[test]
fn an_old_sidecar_is_not_opened_by_its_toml_names() {
    let bytes = SIDECAR_HEADER.as_bytes();
    for name in ["round.indicatrix.toml", "round.gemcut.toml", "round.toml"] {
        assert_eq!(InputFileKind::classify(name, bytes), None, "{name}");
    }
    // Saved under a design extension by mistake: the loader reports what is wrong.
    assert_eq!(
        InputFileKind::classify("round.indicatrix", bytes),
        Some(InputFileKind::Design)
    );
    assert_eq!(
        InputFileKind::classify("round.isdd", bytes),
        Some(InputFileKind::Design)
    );
}

#[test]
fn an_unrecognised_header_falls_back_to_the_extension() {
    let kind = InputFileKind::classify;
    assert_eq!(kind("a.isdd", b"x = 1\n"), Some(InputFileKind::Design));
    assert_eq!(
        kind("a.indicatrix", b"x = 1\n"),
        Some(InputFileKind::Design)
    );
    assert_eq!(kind("a.toml", b"x = 1\n"), None);
    assert_eq!(
        kind("a.isdd", &[0xff, 0xfe, 0x00]),
        Some(InputFileKind::Design)
    );
    assert_eq!(kind("a.toml", &[0xff, 0xfe, 0x00]), None);
}

#[test]
fn a_byte_order_mark_does_not_hide_the_header() {
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(DESIGN_HEADER.as_bytes());
    assert_eq!(
        InputFileKind::classify("a.toml", &bytes),
        Some(InputFileKind::Design)
    );
}

#[test]
fn converted_names_take_the_stem_like_the_vault_rule() {
    assert_eq!(
        converted_asc_file_name("Round Brilliant.gem"),
        "Round Brilliant.asc"
    );
    assert_eq!(converted_asc_file_name("a.b.gcs"), "a.b.asc");
    assert_eq!(converted_asc_file_name(".gcs"), ".gcs.asc");
    assert_eq!(converted_asc_file_name("noext"), "noext.asc");
}

#[test]
fn a_corrupt_gem_is_a_typed_error_and_asc_is_refused() {
    let err = convert_foreign_design("x.gem", InputFileKind::Gem, &[1, 2, 3])
        .expect_err("three bytes are no .gem");
    assert!(err.starts_with("not a readable .gem file"), "{err}");
    assert!(convert_foreign_design("x.asc", InputFileKind::Asc, b"GemCad 5.0\n").is_err());
}

#[test]
fn file_names_follow_the_desktop_rules() {
    assert_eq!(suggested_asc_file_name(Some("mine.asc"), &[]), "mine.asc");
    assert_eq!(
        suggested_asc_file_name(None, &["My: Oval?".to_string()]),
        "My_ Oval_.asc"
    );
    assert_eq!(
        suggested_asc_file_name(None, &["   ".to_string()]),
        "edited_design.asc"
    );
    assert_eq!(suggested_asc_file_name(None, &[]), "edited_design.asc");
    assert_eq!(
        cutting_sheet_file_name("round.asc"),
        "round_cutting_sheet.html"
    );
    assert_eq!(diagram_file_name("round.asc"), "round_diagram.png");
    assert_eq!(sanitize_file_name(" <> "), "__");
    assert_eq!(sanitize_file_name("   "), "design");
}

#[test]
fn the_degenerate_marker_is_stamped_once() {
    let header = degenerate_marker_header(&[], "Unbounded").expect("first stamp");
    assert_eq!(header, "NOT A CLOSED SOLID -- Unbounded");
    assert_eq!(degenerate_marker_header(&[header], "again"), None);
}
