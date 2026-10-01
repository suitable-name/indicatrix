//! Tests for [`super::super::to_asc_string`]'s write-time safety handling.
//!
//! Covers [`super::super::AscWriteError`], [`super::super::is_asc_safe_tier_name`],
//! and [`super::super::asc_safe_tier_name`]: a tier name that would not survive a
//! `.asc` write/re-parse round trip unchanged must be sanitised at write time (never
//! silently corrupted at read time), while a header, footnote, or notes string with
//! an embedded line break, and any non-finite number, is still rejected outright,
//! since there is no sane single-line sanitisation for those fields.

use super::super::{
    AscSchedule, AscWriteError, asc_safe_tier_name, is_asc_safe_tier_name, parse_asc, to_asc_string,
};

const HDR: &str = "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\n";

fn one_tier_schedule() -> AscSchedule {
    parse_asc(&format!("{HDR}a 41 0.5 0 12\n")).expect("must parse")
}

#[test]
fn is_asc_safe_tier_name_accepts_empty_and_single_token_names() {
    assert!(is_asc_safe_tier_name(""));
    assert!(is_asc_safe_tier_name("P1"));
    assert!(is_asc_safe_tier_name("G")); // the real "girdle named G" corpus quirk
    assert!(is_asc_safe_tier_name("n")); // a name that happens to equal the marker word
    assert!(is_asc_safe_tier_name("c/d")); // the joined-multi-name convention
}

#[test]
fn is_asc_safe_tier_name_rejects_embedded_whitespace() {
    assert!(!is_asc_safe_tier_name("Crown Main"));
    assert!(!is_asc_safe_tier_name("Upper G girdle"));
    assert!(!is_asc_safe_tier_name("a\tb"));
    assert!(!is_asc_safe_tier_name("a\nb"));
}

#[test]
fn asc_safe_tier_name_collapses_whitespace_runs_and_trims_ends() {
    assert_eq!(asc_safe_tier_name("Crown Main"), "Crown_Main");
    assert_eq!(asc_safe_tier_name("  Star  "), "Star");
    assert_eq!(asc_safe_tier_name(""), "");
    assert_eq!(asc_safe_tier_name("Upper G girdle"), "Upper_G_girdle");
    assert_eq!(asc_safe_tier_name("a\nb"), "a_b");
    assert_eq!(asc_safe_tier_name("   "), "");
}

#[test]
fn asc_safe_tier_name_leaves_already_safe_names_untouched() {
    for name in ["", "P1", "G", "n", "c/d"] {
        assert_eq!(asc_safe_tier_name(name), name);
    }
}

/// The exact hazard: a tier name with embedded whitespace must not
/// reach [`super::super::parse_asc`] as-is (silently truncated, or worse, misread
/// as a marker) -- but per the corrected decision it is sanitised, not rejected,
/// since every built-in template names tiers with spaces (e.g. `"Crown Main"`) and
/// the native `.indicatrix.toml` sidecar is what preserves the true name.
#[test]
fn to_asc_string_sanitises_a_tier_name_with_embedded_whitespace() {
    let mut schedule = one_tier_schedule();
    schedule.tiers[0].name = "Crown Main".to_string();
    let text = to_asc_string(&schedule).expect("a spaced tier name must not be rejected");
    assert!(
        text.contains(" n Crown_Main"),
        "expected the sanitised name in the written text: {text}"
    );
    let reparsed = parse_asc(&text).expect("must still parse");
    assert_eq!(reparsed.tiers.len(), 1);
    assert_eq!(reparsed.tiers[0].name, "Crown_Main");
}

/// The specific corpus-observed corruption: a name containing an
/// embedded `G` token used to split into a truncated name plus a bogus notes tail,
/// which even changed the reparsed tier count when the split value was misread as a
/// stray marker. Writing it sanitised (not rejected) must never let that
/// corruption back in: the round trip must still preserve the tier count exactly.
#[test]
fn to_asc_string_round_trip_preserves_tier_count_for_names_with_spaces() {
    let mut schedule = one_tier_schedule();
    schedule.tiers[0].name = "Upper G girdle".to_string();
    let text = to_asc_string(&schedule).expect("a name embedding a 'G' token must not be rejected");
    let reparsed = parse_asc(&text).expect("must still parse");
    assert_eq!(
        reparsed.tiers.len(),
        schedule.tiers.len(),
        "sanitised write must not split one tier into extra tiers on re-parse: {text}"
    );
    assert_eq!(reparsed.tiers[0].name, "Upper_G_girdle");
}

#[test]
fn to_asc_string_rejects_a_header_containing_a_newline() {
    let mut schedule = one_tier_schedule();
    schedule.headers = vec!["Title\na -10 0.3 5".to_string()];
    let err =
        to_asc_string(&schedule).expect_err("a header with an embedded newline must be rejected");
    assert!(
        matches!(
            err,
            AscWriteError::HeaderContainsNewline {
                header_index: 0,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn to_asc_string_rejects_a_footnote_containing_a_newline() {
    let mut schedule = one_tier_schedule();
    schedule.footnotes = vec!["line one\nline two".to_string()];
    let err =
        to_asc_string(&schedule).expect_err("a footnote with an embedded newline must be rejected");
    assert!(matches!(
        err,
        AscWriteError::FootnoteContainsNewline {
            footnote_index: 0,
            ..
        }
    ));
}

#[test]
fn to_asc_string_rejects_notes_containing_a_newline() {
    let mut schedule = one_tier_schedule();
    schedule.tiers[0].notes = "Meet P1\nMeet P2".to_string();
    let err =
        to_asc_string(&schedule).expect_err("notes with an embedded newline must be rejected");
    assert!(matches!(
        err,
        AscWriteError::NotesContainNewline { tier_index: 0, .. }
    ));
}

/// A schedule with no unsafe names/headers/notes must still write and round-trip
/// exactly as before -- the safety checks must not reject ordinary, real data.
#[test]
fn to_asc_string_still_succeeds_for_an_ordinary_schedule() {
    let schedule = one_tier_schedule();
    let text = to_asc_string(&schedule).expect("an ordinary schedule must still write");
    let reparsed = parse_asc(&text).expect("must still parse");
    assert_eq!(reparsed, schedule);
}

/// A lone carriage return is a line break to the reader, exactly like a newline, so
/// it is rejected in every single-line free-text field.
#[test]
fn to_asc_string_rejects_a_lone_carriage_return_in_free_text() {
    let mut schedule = one_tier_schedule();
    schedule.headers = vec!["Title\rmore".to_string()];
    assert!(matches!(
        to_asc_string(&schedule),
        Err(AscWriteError::HeaderContainsNewline {
            header_index: 0,
            ..
        })
    ));
    schedule.headers.clear();

    schedule.footnotes = vec!["note\r".to_string()];
    assert!(matches!(
        to_asc_string(&schedule),
        Err(AscWriteError::FootnoteContainsNewline {
            footnote_index: 0,
            ..
        })
    ));
    schedule.footnotes.clear();

    schedule.tiers[0].notes = "Meet P1\rMeet P2".to_string();
    assert!(matches!(
        to_asc_string(&schedule),
        Err(AscWriteError::NotesContainNewline { tier_index: 0, .. })
    ));
}

/// A name of only whitespace sanitises to nothing; written as ` n ` the next index
/// would be read back as the name. It is written as a block letter plus the tier's
/// 1-based position instead, and every index survives.
#[test]
fn to_asc_string_gives_a_whitespace_only_tier_name_an_automatic_name() {
    let mut schedule = one_tier_schedule();
    schedule.tiers[0].name = " \t ".to_string();
    let text = to_asc_string(&schedule).expect("a blank tier name must not be rejected");
    let reparsed = parse_asc(&text).expect("must still parse");
    assert_eq!(
        reparsed.tiers[0].indices, schedule.tiers[0].indices,
        "{text}"
    );
    assert_eq!(reparsed.tiers[0].name, "C1", "{text}");

    // The same holds for a blank name recorded at an explicit index position.
    let mut schedule = one_tier_schedule();
    schedule.tiers[0].index_names = vec![(1, "  ".to_string())];
    let text = to_asc_string(&schedule).expect("a blank facet name must not be rejected");
    let reparsed = parse_asc(&text).expect("must still parse");
    assert_eq!(
        reparsed.tiers[0].indices, schedule.tiers[0].indices,
        "{text}"
    );
    assert_eq!(reparsed.tiers[0].index_names, [(1, "C1".to_string())]);
}

/// `NaN` and infinities are written as `NaN`/`inf`, which the reader rejects, so the
/// write fails with a typed error naming the field instead.
#[test]
fn to_asc_string_rejects_non_finite_numbers() {
    let mut schedule = one_tier_schedule();
    schedule.tiers[0].mast = f64::NAN;
    assert!(matches!(
        to_asc_string(&schedule),
        Err(AscWriteError::NonFiniteValue {
            tier_index: Some(0),
            field: "mast",
            ..
        })
    ));

    let mut schedule = one_tier_schedule();
    schedule.tiers[0].angle_deg = f64::INFINITY;
    assert!(matches!(
        to_asc_string(&schedule),
        Err(AscWriteError::NonFiniteValue {
            tier_index: Some(0),
            field: "angle",
            ..
        })
    ));

    let mut schedule = one_tier_schedule();
    schedule.tiers[0].indices.push(f64::NEG_INFINITY);
    assert!(matches!(
        to_asc_string(&schedule),
        Err(AscWriteError::NonFiniteValue {
            tier_index: Some(0),
            field: "index",
            ..
        })
    ));

    let mut schedule = one_tier_schedule();
    schedule.refractive_index = f64::NAN;
    assert!(matches!(
        to_asc_string(&schedule),
        Err(AscWriteError::NonFiniteValue {
            tier_index: None,
            field: "refractive index",
            ..
        })
    ));

    let mut schedule = one_tier_schedule();
    schedule.gear_reference_angle = f64::INFINITY;
    let err = to_asc_string(&schedule).expect_err("an infinite reference angle is rejected");
    assert!(
        matches!(
            err,
            AscWriteError::NonFiniteValue {
                tier_index: None,
                field: "gear reference angle",
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("inf"), "{err}");
}
