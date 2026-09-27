//! Header-field validation: rejecting a zero gear-tooth count, an invalid
//! mirror flag, missing/non-numeric required fields, and accepting a
//! whole-number field written as a decimal.

use super::super::{AscParseError, parse_asc};

#[test]
fn rejects_a_zero_gear_tooth_count_on_the_g_line() {
    let content = "GemCad 5.0\n\
                    g 0 0.0\n\
                    y 1 n\n\
                    I 1.72\n\
                    a -90.00 1.0 0\n";
    let err = parse_asc(content).unwrap_err();
    assert!(
        matches!(err, AscParseError::GearTeethZero { line: 2 }),
        "{err:?}"
    );
}

/// Zero gear teeth must also be rejected through the bare-gear-line fallback,
/// using the same `require_nonzero_gear_teeth` path as the keyworded `g` line.
#[test]
fn rejects_a_zero_gear_tooth_count_on_a_bare_gear_line() {
    let content = "GemCad 5.0\n\
                    0 0.0\n\
                    y 1 n\n\
                    I 1.72\n\
                    a -90.00 1.0 0\n";
    // The bare line fails to register as a gear line at all (leniently ignored),
    // so parsing fails later for a plain missing 'g' header, not GearTeethZero --
    // either way, gear 0 must never be accepted.
    let err = parse_asc(content).unwrap_err();
    assert!(matches!(err, AscParseError::MissingGearLine), "{err:?}");
}

#[test]
fn mirror_flag_accepts_y_and_n_case_insensitively() {
    for (flag, expected) in [("y", true), ("Y", true), ("n", false), ("N", false)] {
        let content = format!(
            "GemCad 5.0\n\
             g 96 0.0\n\
             y 1 {flag}\n\
             I 1.72\n\
             a -90.00 1.0 0\n"
        );
        let schedule = parse_asc(&content).expect("y/n must parse regardless of case");
        assert_eq!(schedule.mirror, expected, "flag {flag:?}");
    }
}

#[test]
fn mirror_flag_rejects_anything_other_than_y_or_n() {
    let content = "GemCad 5.0\n\
                    g 96 0.0\n\
                    y 1 yes\n\
                    I 1.72\n\
                    a -90.00 1.0 0\n";
    let err = parse_asc(content).unwrap_err();
    assert!(
        matches!(
            err,
            AscParseError::MirrorFlagInvalid { line: 3, ref token } if token == "yes"
        ),
        "{err:?}"
    );
}

#[test]
fn rejects_empty_input() {
    assert!(parse_asc("").is_err());
    assert!(parse_asc("   \n  \n").is_err());
}

#[test]
fn rejects_truncated_file_with_no_tiers() {
    let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Truncated design\n";
    let err = parse_asc(content)
        .expect_err("a file with headers but no 'a' records must error")
        .to_string();
    assert!(err.contains("no valid"), "unexpected error message: {err}");
}

#[test]
fn rejects_missing_mast_field() {
    let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000\n";
    let err = parse_asc(content)
        .expect_err("an 'a' record with no mast field must error")
        .to_string();
    assert!(
        err.contains("mast") || err.contains("field"),
        "unexpected error message: {err}"
    );
}

#[test]
fn rejects_garbage_angle_field() {
    let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Test\na not-a-number 0.5 92 n P1\n";
    let err = parse_asc(content)
        .expect_err("a non-numeric angle must error")
        .to_string();
    assert!(err.contains("angle"), "unexpected error message: {err}");
}

#[test]
fn rejects_missing_gear_line() {
    let content = "GemCad 5.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
    let err = parse_asc(content)
        .expect_err("a missing 'g' line must error")
        .to_string();
    assert!(err.contains("gear"), "unexpected error message: {err}");
}

#[test]
fn accepts_whole_number_gear_tooth_counts_written_as_plain_or_decimal() {
    let plain = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
    assert_eq!(
        parse_asc(plain)
            .expect("plain whole number must parse")
            .gear_teeth,
        96
    );

    let decimal = "GemCad 5.0\ng 96.0 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
    assert_eq!(
        parse_asc(decimal)
            .expect("a decimal that is exactly whole must parse")
            .gear_teeth,
        96
    );
}

#[test]
fn rejects_fractional_gear_tooth_count_with_the_line_number_and_token() {
    // Line 2 (1-indexed): the 'g' line itself.
    let content = "GemCad 5.0\ng 96.5 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
    let err = parse_asc(content)
        .expect_err("a fractional gear tooth count must error")
        .to_string();
    assert!(
        err.contains("line 2"),
        "error must name the offending line: {err}"
    );
    assert!(
        err.contains("96.5"),
        "error must name the offending token: {err}"
    );
}

#[test]
fn accepts_whole_number_symmetry_order_written_as_decimal() {
    let content = "GemCad 5.0\ng 96 0.0\ny 6.0 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
    assert_eq!(
        parse_asc(content)
            .expect("a decimal symmetry order that is exactly whole must parse")
            .symmetry_order,
        6
    );
}

#[test]
fn rejects_fractional_symmetry_order_with_the_line_number_and_token() {
    // Line 3 (1-indexed): the 'y' line itself.
    let content = "GemCad 5.0\ng 96 0.0\ny 6.5 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
    let err = parse_asc(content)
        .expect_err("a fractional symmetry order must error")
        .to_string();
    assert!(
        err.contains("line 3"),
        "error must name the offending line: {err}"
    );
    assert!(
        err.contains("6.5"),
        "error must name the offending token: {err}"
    );
}

#[test]
fn rejects_garbage_gear_reference_angle_with_the_line_number_and_token() {
    // Line 2 (1-indexed): the 'g' line itself. Previously this silently defaulted
    // to 0.0 via `unwrap_or`; it must now error like the other numeric fields.
    let content =
        "GemCad 5.0\ng 96 not-a-number\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
    let err = parse_asc(content)
        .expect_err("a non-numeric gear reference angle must error")
        .to_string();
    assert!(
        err.contains("line 2"),
        "error must name the offending line: {err}"
    );
    assert!(
        err.contains("not-a-number"),
        "error must name the offending token: {err}"
    );
}

#[test]
fn rejects_missing_refractive_index_line() {
    let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nH Test\na -41.000000 0.5 92 n P1\n";
    let err = parse_asc(content)
        .expect_err("a missing 'I' line must error")
        .to_string();
    assert!(
        err.contains("refractive"),
        "unexpected error message: {err}"
    );
}

#[test]
fn does_not_panic_on_arbitrary_garbage() {
    // Fuzz-ish smoke test: a grab-bag of binary-ish / malformed content must never
    // panic, only ever return Ok or Err.
    let samples = [
        "\u{0}\u{1}\u{2}garbage\u{ff}",
        "a a a a a a a a\n",
        "GemCad\ng\ny\nI\n",
        "g 96 0.0\ny 1 y\nI abc\na 1 2\n",
    ];
    for s in samples {
        let _ = parse_asc(s);
    }
}
