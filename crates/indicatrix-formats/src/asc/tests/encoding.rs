//! Byte decoding ([`super::super::decode_asc_bytes`]/[`super::super::parse_asc_bytes`])
//! and line endings: Windows-1252 files, a UTF-8 byte-order mark, CR-only files,
//! and LF/CRLF surviving a parse -> write round trip byte for byte.

use super::super::{
    AscLineEnding, AscSchedule, AscTier, decode_asc_bytes, parse_asc, parse_asc_bytes,
    to_asc_string,
};
use std::borrow::Cow;

/// A header with a Windows-1252 degree sign (`0xB0`), the case 591 real catalogue
/// files hit: `read_to_string` rejects the whole file, the decoder must not.
#[test]
fn windows_1252_degree_sign_in_header_parses() {
    let bytes: &[u8] = b"GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.54\nH Cut at 43\xB0.\n\
a 41 0.5 0\nF \x80 5 \x93quoted\x94\n";
    // `Owned` means the UTF-8 read failed and the Windows-1252 path ran.
    let decoded = decode_asc_bytes(bytes);
    assert!(
        matches!(decoded, Cow::Owned(_)),
        "fixture must not be UTF-8"
    );

    let schedule = parse_asc_bytes(bytes).expect("a Windows-1252 file must parse");
    assert_eq!(schedule.headers, vec!["Cut at 43\u{b0}.".to_string()]);
    assert!(schedule.headers[0].contains('\u{b0}'));
    // 0x80..=0x9F is where Windows-1252 differs from Latin-1.
    assert_eq!(
        schedule.footnotes,
        vec!["\u{20ac} 5 \u{201c}quoted\u{201d}".to_string()]
    );
    assert_eq!(schedule.tiers.len(), 1);
}

/// Valid UTF-8 comes back borrowed and unchanged.
#[test]
fn utf8_input_is_borrowed_unchanged() {
    let text = "GemCad 5.0\nH Cut at 43\u{b0}.\n";
    let decoded = decode_asc_bytes(text.as_bytes());
    assert!(matches!(decoded, Cow::Borrowed(_)));
    assert_eq!(decoded, text);
}

/// A BOM-prefixed file parses, and keeps its `GemCad` version line.
#[test]
fn bom_prefixed_file_parses() {
    let bytes: &[u8] = b"\xEF\xBB\xBFGemCad 5.0\ng 96 0.0\ny 1 n\nI 1.54\na 41 0.5 0\n";
    assert!(!decode_asc_bytes(bytes).starts_with('\u{feff}'));
    let schedule = parse_asc_bytes(bytes).expect("a BOM-prefixed file must parse");
    assert_eq!(schedule.gemcad_version, "5.0");
    assert_eq!(schedule.gear_teeth, 96);
    assert_eq!(schedule.warnings, Vec::<String>::new());
}

/// CR-only (classic Mac) line endings parse once decoded, and count as LF.
#[test]
fn cr_only_line_endings_parse_through_parse_asc_bytes() {
    let bytes: &[u8] = b"GemCad 5.0\rg 96 0.0\ry 8 y\rI 1.54\ra 41 0.5 0\r";
    assert_eq!(
        decode_asc_bytes(bytes),
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41 0.5 0\n"
    );
    let schedule = parse_asc_bytes(bytes).expect("CR-only line endings must parse");
    assert_eq!(schedule.gear_teeth, 96);
    assert_eq!(schedule.symmetry_order, 8);
    assert_eq!(schedule.tiers.len(), 1);
    assert_eq!(schedule.line_ending, AscLineEnding::Lf);
}

/// CRLF input keeps parsing (its `\r` never reaches a token) and is recorded.
#[test]
fn crlf_input_parses_and_is_recorded() {
    let text = "GemCad 5.0\r\ng 96 0.0\r\ny 1 n\r\nI 1.54\r\na 41 0.5 0 n C1\r\n";
    let schedule = parse_asc(text).expect("CRLF must parse");
    assert_eq!(schedule.line_ending, AscLineEnding::CrLf);
    assert_eq!(schedule.tiers[0].name, "C1");
    assert_eq!(schedule.tiers[0].indices, vec![0.0]);
    let decoded = decode_asc_bytes(text.as_bytes());
    assert!(matches!(decoded, Cow::Borrowed(_)), "CRLF is not a lone CR");
}

/// A file already in the writer's canonical form, LF-terminated.
const CANONICAL_LF: &str = "GemCad 5.0\ng 96 0\ny 1 y\nI 1.54\nH Test\n\
a -41 0.65 92 n 1 84 76\n\
a -0 -0.368 0\n\
a 0 0.5 96 n T G Set stone size.\n\
F Foot\n";

#[test]
fn lf_in_lf_out_byte_identical() {
    let schedule = parse_asc(CANONICAL_LF).expect("must parse");
    assert_eq!(schedule.line_ending, AscLineEnding::Lf);
    let written = to_asc_string(&schedule).expect("must write");
    assert_eq!(written, CANONICAL_LF);
    assert_eq!(parse_asc(&written).expect("must reparse"), schedule);
}

#[test]
fn crlf_in_crlf_out_byte_identical() {
    let crlf = CANONICAL_LF.replace('\n', "\r\n");
    let schedule = parse_asc(&crlf).expect("must parse");
    assert_eq!(schedule.line_ending, AscLineEnding::CrLf);
    let written = to_asc_string(&schedule).expect("must write");
    assert_eq!(written, crlf);
    assert_eq!(parse_asc(&written).expect("must reparse"), schedule);
}

fn hand_built_schedule(line_ending: AscLineEnding) -> AscSchedule {
    AscSchedule {
        gemcad_version: "5.0".to_string(),
        gear_teeth: 96,
        symmetry_order: 1,
        refractive_index: 1.54,
        tiers: vec![AscTier {
            angle_deg: 41.0,
            mast: 0.5,
            indices: vec![0.0, 48.0],
            ..AscTier::default()
        }],
        line_ending,
        ..AscSchedule::default()
    }
}

/// A schedule built by hand (the editor's export path) defaults to CRLF, the
/// ending `GemCAD` itself writes.
#[test]
fn hand_built_schedule_writes_crlf_by_default() {
    let schedule = hand_built_schedule(AscLineEnding::default());
    assert_eq!(schedule.line_ending, AscLineEnding::CrLf);
    let written = to_asc_string(&schedule).expect("must write");
    assert_eq!(
        written.matches('\n').count(),
        written.matches("\r\n").count()
    );
    assert_eq!(written.matches("\r\n").count(), 5);
}

/// `parse(write(s)) == s` for either ending.
#[test]
fn hand_built_schedule_round_trips_with_either_ending() {
    for ending in [AscLineEnding::Lf, AscLineEnding::CrLf] {
        let schedule = hand_built_schedule(ending);
        let written = to_asc_string(&schedule).expect("must write");
        assert_eq!(AscLineEnding::detect(&written), ending);
        assert_eq!(parse_asc(&written).expect("must reparse"), schedule);
    }
}
