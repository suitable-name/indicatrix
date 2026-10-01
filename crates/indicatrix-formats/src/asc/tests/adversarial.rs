//! The adversarial `.asc` inputs exercised by the coordinator's own probe tool
//! (cases 1 to 17), plus later hostile-gear cases.
//!
//! Cases 1 to 17 are hostile-input cases pinned here as real unit tests, so the
//! lenient-vs-hard-reject line this module draws stays a tested contract.

use super::super::{AscLineEnding, AscParseError, parse_asc};

const HDR: &str = "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\n";

/// Case 1: a UTF-8 BOM plus CRLF line endings.
///
/// The BOM is stripped before the first line is read, so the `GemCad` keyword
/// still matches and the version survives (it used to be lost, and the writer
/// then emitted a bare `GemCad `), and the CRLF terminator is recorded.
#[test]
fn adversarial_01_bom_plus_crlf() {
    let content = "\u{feff}GemCad 5.0\r\ng 96 0.0\r\ny 8 y\r\nI 1.54\r\na 41 0.5 0 12\r\n";
    let schedule = parse_asc(content).expect("BOM-prefixed first line must not be fatal");
    assert_eq!(schedule.gemcad_version, "5.0");
    assert_eq!(schedule.gear_teeth, 96);
    assert_eq!(schedule.tiers.len(), 1);
    assert_eq!(schedule.line_ending, AscLineEnding::CrLf);
    assert_eq!(schedule.warnings, Vec::<String>::new());
}

/// Case 2: `NaN`/`inf` in an `a` record's angle/mast fields.
///
/// Rejected outright -- these feed the meet solver and stone measurement directly.
#[test]
fn adversarial_02_nan_inf_angle_and_mast() {
    let content = format!("{HDR}a NaN inf 0 12\n");
    let err = parse_asc(&content).expect_err("NaN angle must be rejected");
    assert!(
        matches!(err, AscParseError::NonFiniteValue { field: "angle", .. }),
        "{err:?}"
    );
}

/// Case 3: a garbage index token (neither numeric nor an `n`/`G` marker).
///
/// A warning, not a fatal error and not folded into the name -- the surrounding
/// real indices must still parse.
#[test]
fn adversarial_03_garbage_index_token() {
    let content = format!("{HDR}a 41 0.5 1 2 x3 4\n");
    let schedule = parse_asc(&content).expect("a stray garbage token must not be fatal");
    assert_eq!(schedule.tiers[0].indices, vec![1.0, 2.0, 4.0]);
    assert_eq!(
        schedule.tiers[0].name, "",
        "the garbage token must not become a name"
    );
    assert_eq!(schedule.warnings.len(), 1);
    assert!(schedule.warnings[0].contains("x3"));
}

/// Case 4: a comma-separated index token (`"2,5"`).
///
/// Same "neither numeric nor a marker" case as case 3 -- warned and dropped, not
/// misread as two indices or a name.
#[test]
fn adversarial_04_comma_index_token() {
    let content = format!("{HDR}a 41 0.5 1 2,5 4\n");
    let schedule = parse_asc(&content).expect("a comma index token must not be fatal");
    assert_eq!(schedule.tiers[0].indices, vec![1.0, 4.0]);
    assert_eq!(schedule.warnings.len(), 1);
    assert!(schedule.warnings[0].contains("2,5"));
}

/// Case 5: free text trailing the last facet tier.
///
/// A warning (some real files carry a stray "Designed 1999 by X" line at the very
/// end), not folded in as a bogus continuation -- the real tier's own data must
/// come through untouched.
#[test]
fn adversarial_05_trailing_free_text_annotation() {
    let content = format!("{HDR}a 41 0.5 0 12\nDesigned 1999 by X\n");
    let schedule = parse_asc(&content).expect("trailing free text must not be fatal");
    assert_eq!(schedule.tiers.len(), 1);
    assert_eq!(schedule.tiers[0].indices, vec![0.0, 12.0]);
    assert_eq!(schedule.tiers[0].name, "");
    assert_eq!(schedule.warnings.len(), 1);
    assert!(schedule.warnings[0].contains("Designed"));
}

/// Case 6: a zero symmetry order.
///
/// Rejected -- every azimuth computation in the crate divides by it.
#[test]
fn adversarial_06_symmetry_order_zero() {
    let content = "GemCad 5.0\ng 96 0.0\ny 0 n\nI 1.54\na 41 0.5 0\n";
    let err = parse_asc(content).expect_err("symmetry order 0 must be rejected");
    assert!(
        matches!(err, AscParseError::SymmetryOrderZero { line: 3 }),
        "{err:?}"
    );
}

/// Case 7: a refractive index of `1.0` or less (here, negative).
///
/// Rejected -- it has no physical meaning for a faceted gemstone.
#[test]
fn adversarial_07_refractive_index_zero_or_negative() {
    let content = "GemCad 5.0\ng 96 0.0\ny 8 n\nI -1\na 41 0.5 0\n";
    let err = parse_asc(content).expect_err("a non-positive refractive index must be rejected");
    assert!(
        matches!(
            err,
            AscParseError::RefractiveIndexOutOfRange { value, .. } if value == -1.0
        ),
        "{err:?}"
    );
}

/// Case 8: index positions beyond the gear-tooth count, or negative.
///
/// Still finite numbers -- accepted, not range-checked. Only non-finite index
/// values are hard-rejected (case 2's sibling check, applied to indices too).
#[test]
fn adversarial_08_index_beyond_gear_or_negative_is_accepted() {
    let content = format!("{HDR}a 41 0.5 96 200 -5\n");
    let schedule = parse_asc(&content).expect("out-of-range but finite indices are accepted");
    assert_eq!(schedule.tiers[0].indices, vec![96.0, 200.0, -5.0]);
    assert_eq!(schedule.warnings, Vec::<String>::new());
}

/// Case 9: a comma-decimal angle (`"41,5"`, European-locale style).
///
/// Does not parse as an `f64` -- rejected as non-numeric, not silently truncated
/// to `41`.
#[test]
fn adversarial_09_comma_decimal_angle() {
    let content = format!("{HDR}a 41,5 0,5 0\n");
    let err = parse_asc(&content).expect_err("a comma-decimal angle must not parse");
    assert!(
        matches!(err, AscParseError::AngleNotNumeric { .. }),
        "{err:?}"
    );
}

/// Case 10: lone `\r` line endings (no `\n` at all).
///
/// Not split into separate lines by [`str::lines`], so the whole file collapses
/// into one line; the leading `GemCad` keyword then swallows every subsequent
/// field as part of its version string, and no real `g` line is ever seen. Fails
/// closed (`MissingGearLine`), not silently. Files are read through
/// `decode_asc_bytes`, which turns a lone CR into LF first (see
/// `encoding::cr_only_line_endings_parse_through_parse_asc_bytes`); this pins
/// what the plain-text entry point does with undecoded input.
#[test]
fn adversarial_10_lone_cr_line_endings() {
    let content = "GemCad 5.0\rg 96 0.0\ry 8 y\rI 1.54\ra 41 0.5 0\r";
    let err = parse_asc(content).expect_err("lone-CR line endings must not silently succeed");
    assert!(matches!(err, AscParseError::MissingGearLine), "{err:?}");
}

/// Case 11: a trailing `n` marker with nothing after it.
///
/// Never resolves to a name (there is no token left to consume), but is not an
/// error either -- the tier's other data (its one real index) still comes
/// through.
#[test]
fn adversarial_11_trailing_n_marker_with_no_name() {
    let content = format!("{HDR}a 41 0.5 0 n\n");
    let schedule = parse_asc(&content).expect("a trailing 'n' with no name must not be fatal");
    assert_eq!(schedule.tiers[0].indices, vec![0.0]);
    assert_eq!(schedule.tiers[0].name, "");
}

/// Case 12: a `G` marker with nothing after it.
///
/// Produces empty notes, not an error.
#[test]
fn adversarial_12_notes_marker_with_no_text() {
    let content = format!("{HDR}a 41 0.5 G\n");
    let schedule = parse_asc(&content).expect("a 'G' marker with no notes must not be fatal");
    assert_eq!(schedule.tiers[0].indices, Vec::<f64>::new());
    assert_eq!(schedule.tiers[0].notes, "");
}

/// Case 13: a bare `a` record with no fields at all.
///
/// Rejected -- needs at least an angle and a mast.
#[test]
fn adversarial_13_tier_record_alone_with_no_fields() {
    let content = format!("{HDR}a\n");
    let err = parse_asc(&content).expect_err("an 'a' record with no fields must be rejected");
    assert!(
        matches!(
            err,
            AscParseError::TierRecordTooShort { field_count: 0, .. }
        ),
        "{err:?}"
    );
}

/// Case 14: `NaN` in the `g` line's gear-tooth-count field.
///
/// Rejected -- shares the ordinary numeric-range validation's finiteness check.
#[test]
fn adversarial_14_gear_teeth_nan() {
    let content = "GemCad 5.0\ng NaN 0\ny 8 y\nI 1.5\na 1 1\n";
    let err = parse_asc(content).expect_err("NaN gear tooth count must be rejected");
    assert!(
        matches!(
            err,
            AscParseError::NonFiniteValue {
                field: "gear tooth count",
                ..
            }
        ),
        "{err:?}"
    );
}

/// Case 15: `NaN` in the `g` line's reference-angle field.
///
/// Rejected.
#[test]
fn adversarial_15_gear_reference_angle_nan() {
    let content = "GemCad 5.0\ng 96 NaN\ny 8 y\nI 1.5\na 1 1\n";
    let err = parse_asc(content).expect_err("NaN gear reference angle must be rejected");
    assert!(
        matches!(
            err,
            AscParseError::NonFiniteValue {
                field: "gear reference angle",
                ..
            }
        ),
        "{err:?}"
    );
}

/// Case 16: `NaN` in the `I` (refractive index) line.
///
/// Rejected.
#[test]
fn adversarial_16_refractive_index_nan() {
    let content = "GemCad 5.0\ng 96 0\ny 8 y\nI NaN\na 1 1\n";
    let err = parse_asc(content).expect_err("NaN refractive index must be rejected");
    assert!(
        matches!(
            err,
            AscParseError::NonFiniteValue {
                field: "refractive index",
                ..
            }
        ),
        "{err:?}"
    );
}

/// Case 17: duplicate `g` lines.
///
/// Not an error, but the second value must win, and the override must be
/// recorded as a warning (not silently swapped).
#[test]
fn adversarial_17_duplicate_gear_lines_last_wins_with_a_warning() {
    let content = "GemCad 5.0\ng 96 0\ng 64 0\ny 8 y\nI 1.5\na 1 1 0\n";
    let schedule = parse_asc(content).expect("duplicate 'g' lines must not be fatal");
    assert_eq!(schedule.gear_teeth, 64, "the later 'g' line must win");
    assert_eq!(schedule.warnings.len(), 1);
    assert!(schedule.warnings[0].contains("duplicate"));
}

/// Case 18: a gear of two billion teeth.
///
/// Fits an `i32`, so the whole-number check passes, but every consumer that draws
/// the index wheel does work per tooth: rejected with a typed error naming the
/// limit, on the `g` line and on the bare-gear-line fallback alike.
#[test]
fn adversarial_18_gear_of_two_billion_teeth() {
    let content = "GemCad 5.0\ng 2000000000 0\ny 8 y\nI 1.5\na 1 1 0\n";
    let err = parse_asc(content).expect_err("a two-billion-tooth gear must be rejected");
    assert!(
        matches!(
            err,
            AscParseError::GearTeethTooLarge {
                line: 2,
                teeth: 2_000_000_000
            }
        ),
        "{err:?}"
    );

    let bare = "GemCad 5.0\n2000000000 0\ny 8 y\nI 1.5\na 1 1 0\n";
    let err = parse_asc(bare).expect_err("the bare-gear fallback must not accept it either");
    assert!(matches!(err, AscParseError::MissingGearLine), "{err:?}");
}

/// Case 19: the gear limit itself.
///
/// [`AscParseError::MAX_GEAR_TEETH`] teeth parse in either handedness; one more
/// tooth does not.
#[test]
fn adversarial_19_gear_limit_is_inclusive() {
    let max = i32::try_from(AscParseError::MAX_GEAR_TEETH).expect("the limit fits an i32");
    for teeth in [max, -max] {
        let content = format!("GemCad 5.0\ng {teeth} 0\ny 8 y\nI 1.5\na 1 1 0\n");
        let schedule = parse_asc(&content).expect("the limit itself is accepted");
        assert_eq!(schedule.gear_teeth, teeth);
    }
    for teeth in [max + 1, -(max + 1)] {
        let content = format!("GemCad 5.0\ng {teeth} 0\ny 8 y\nI 1.5\na 1 1 0\n");
        let err = parse_asc(&content).expect_err("one tooth past the limit is rejected");
        assert!(
            matches!(err, AscParseError::GearTeethTooLarge { line: 2, teeth: t } if t == teeth),
            "{err:?}"
        );
    }
}
