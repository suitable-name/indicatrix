use super::*;

#[test]
fn kinds_are_read_from_the_extension_case_insensitively() {
    assert_eq!(
        InputFileKind::from_file_name("a.ASC"),
        Some(InputFileKind::Asc)
    );
    assert_eq!(
        InputFileKind::from_file_name("round.indicatrix.toml"),
        Some(InputFileKind::Sidecar)
    );
    assert_eq!(
        InputFileKind::from_file_name("old.gemcut.toml"),
        Some(InputFileKind::Sidecar)
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
    assert_eq!(
        InputFileKind::from_file_name("round.INDICATRIX"),
        Some(InputFileKind::Design)
    );
    assert_eq!(InputFileKind::from_file_name("notes.pdf"), None);
    assert_eq!(InputFileKind::from_file_name("no_extension"), None);
    assert!(!InputFileKind::Hdr.is_design());
    assert!(InputFileKind::Gcs.is_design());
    assert!(InputFileKind::Design.is_design());
    assert!(InputFileKind::Sidecar.is_design());
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
fn a_design_file_is_routed_by_its_content_whatever_it_is_called() {
    let bytes = DESIGN_HEADER.as_bytes();
    for name in [
        "round.indicatrix",
        "ROUND.INDICATRIX",
        "round.indicatrix.toml",
        "round.toml",
    ] {
        assert_eq!(
            InputFileKind::classify(name, bytes),
            Some(InputFileKind::Design),
            "{name}"
        );
    }
}

#[test]
fn an_old_sidecar_keeps_the_paired_flow_under_every_name() {
    let bytes = SIDECAR_HEADER.as_bytes();
    for name in [
        "round.indicatrix.toml",
        "round.gemcut.toml",
        "round.toml",
        // Saved under the new extension by mistake: the content still wins.
        "round.indicatrix",
    ] {
        assert_eq!(
            InputFileKind::classify(name, bytes),
            Some(InputFileKind::Sidecar),
            "{name}"
        );
    }
}

#[test]
fn an_unrecognised_header_falls_back_to_the_extension() {
    let kind = InputFileKind::classify;
    assert_eq!(
        kind("a.indicatrix", b"x = 1\n"),
        Some(InputFileKind::Design)
    );
    assert_eq!(kind("a.toml", b"x = 1\n"), Some(InputFileKind::Sidecar));
    assert_eq!(
        kind("a.indicatrix", &[0xff, 0xfe, 0x00]),
        Some(InputFileKind::Design)
    );
    assert_eq!(
        kind("a.toml", &[0xff, 0xfe, 0x00]),
        Some(InputFileKind::Sidecar)
    );
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
fn native_names_guess_their_asc() {
    assert_eq!(
        asc_name_for_native("Round.indicatrix.toml").as_deref(),
        Some("Round.asc")
    );
    assert_eq!(
        asc_name_for_native("old.GEMCUT.TOML").as_deref(),
        Some("old.asc")
    );
    assert_eq!(asc_name_for_native("settings.toml"), None);
}

#[test]
fn pairing_prefers_the_recorded_name_then_the_guess_then_a_lone_asc() {
    let names = ["other.asc", "Round.ASC", "round.asc"];
    assert_eq!(
        paired_asc_index("round.asc", "x.indicatrix.toml", &names),
        Some(2)
    );
    assert_eq!(
        paired_asc_index("ROUND.asc", "x.indicatrix.toml", &names),
        Some(1)
    );
    assert_eq!(
        paired_asc_index("missing.asc", "other.indicatrix.toml", &names),
        Some(0)
    );
    assert_eq!(
        paired_asc_index("missing.asc", "x.indicatrix.toml", &["lone.asc"]),
        Some(0)
    );
    assert_eq!(
        paired_asc_index("missing.asc", "x.indicatrix.toml", &names),
        None
    );
    assert_eq!(paired_asc_index("a.asc", "a.indicatrix.toml", &[]), None);
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
        native_file_name_for_asc("round.asc"),
        "round.indicatrix.toml"
    );
    assert_eq!(native_file_name_for_asc("round"), "round.indicatrix.toml");
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
