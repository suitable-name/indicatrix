//! The culet/table side convention, per-index facet names, glued keywords and the
//! other lenient-read fixes to the `.asc` reader.

use super::super::{AscSchedule, AscTier, parse_asc, to_asc_string};

const HDR: &str = "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.54\n";

fn parse_with(tiers: &str) -> AscSchedule {
    parse_asc(&format!("{HDR}{tiers}")).expect("fixture must parse")
}

/// The `a ...` lines of `to_asc_string(schedule)`.
fn written_tier_lines(schedule: &AscSchedule) -> Vec<String> {
    to_asc_string(schedule)
        .expect("must write")
        .lines()
        .filter(|line| line.starts_with("a "))
        .map(str::to_string)
        .collect()
}

/// The three real corpus culet spellings (pc45015 tier 4, pc43001a tier 3,
/// pc45116): a zero angle with a negative distance becomes a sign-negative zero
/// angle with a positive mast, whatever tier came before it.
#[test]
fn corpus_culet_lines_become_sign_negative_zero_with_positive_mast() {
    let schedule = parse_with(
        "a 41 0.5 0 48\n\
         a 0.00 -0.28924 0\n\
         a 0.00 -0.368 0\n\
         a -0.000000 -0.76837424 96 n F G SMALL CULET FACET\n",
    );
    let expected_masts = [0.28924, 0.368, 0.768_374_24];
    for (tier, mast) in schedule.tiers[1..].iter().zip(expected_masts) {
        assert_eq!(tier.angle_deg, 0.0);
        assert!(tier.angle_deg.is_sign_negative(), "{tier:?}");
        assert!(tier.is_culet());
        assert_eq!(tier.mast, mast);
    }
    assert_eq!(schedule.tiers[3].name, "F");
    assert_eq!(schedule.tiers[3].notes, "SMALL CULET FACET");
    assert_eq!(schedule.warnings, Vec::<String>::new());
}

/// pc28212's `a 0.000000 0.57655827 96 n T` straight after a pavilion tier is the
/// table: a positive zero, not a culet.
#[test]
fn positive_zero_after_a_pavilion_tier_is_the_table() {
    let schedule = parse_with("a -41 0.6 0 48\na 0.000000 0.57655827 96 n T\n");
    let table = &schedule.tiers[1];
    assert_eq!(table.angle_deg, 0.0);
    assert!(!table.angle_deg.is_sign_negative());
    assert!(!table.is_culet());
    assert_eq!(table.mast, 0.576_558_27);
}

/// A negative distance on a nonzero angle is undocumented: kept as written, with
/// a warning.
#[test]
fn negative_mast_on_a_nonzero_angle_warns() {
    let schedule = parse_with("a -41 -0.6 0\n");
    assert_eq!(schedule.tiers[0].angle_deg, -41.0);
    assert_eq!(schedule.tiers[0].mast, -0.6);
    assert_eq!(schedule.warnings.len(), 1);
    assert!(schedule.warnings[0].contains("negative mast"));
}

/// Real corpus lines survive parse -> write unchanged apart from the writer's
/// numeric formatting: every name stays at the index it followed, and the culet
/// keeps `GemCAD`'s `-0 -<mast>` form.
#[test]
fn corpus_tier_lines_round_trip_byte_for_byte() {
    let cases = [
        (
            "a -90.00 0.7169 18 n G10 78 n G10",
            "a -90 0.7169 18 n G10 78 n G10",
        ),
        (
            "a -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.",
            "a -90 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.",
        ),
        (
            "a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4",
            "a -41 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4",
        ),
        (
            "a -0.000000 -0.76837424 96 n F G SMALL CULET FACET",
            "a -0 -0.76837424 96 n F G SMALL CULET FACET",
        ),
        ("a 0.00 -0.368 0", "a -0 -0.368 0"),
        ("a 0.000000 0.57655827 96 n T", "a 0 0.57655827 96 n T"),
    ];
    for (input, expected) in cases {
        let schedule = parse_with(&format!("{input}\n"));
        let lines = written_tier_lines(&schedule);
        assert_eq!(lines, vec![expected.to_string()], "input {input:?}");
        // And the written line is a fixed point.
        let reparsed = parse_with(&format!("{expected}\n"));
        assert_eq!(reparsed, schedule, "input {input:?}");
        assert_eq!(written_tier_lines(&reparsed), vec![expected.to_string()]);
    }
}

/// `index_names` records each name at the index it follows; `name` folds them.
#[test]
fn two_names_on_one_tier_keep_their_positions_and_fold_into_name() {
    let schedule = parse_with("a -40.00000 0.49897 92 n c 84 76 94 n d 90 86 G Meet girdle\n");
    let tier = &schedule.tiers[0];
    assert_eq!(tier.name, "c/d");
    assert_eq!(
        tier.index_names,
        vec![(0, "c".to_string()), (3, "d".to_string())]
    );
    assert_eq!(
        written_tier_lines(&schedule),
        vec!["a -40 0.49897 92 n c 84 76 94 n d 90 86 G Meet girdle".to_string()]
    );
}

/// A name written before any index binds to position 0.
#[test]
fn a_name_before_any_index_binds_to_position_zero() {
    let schedule = parse_with("a 0.000000 0.44755829 n U\n");
    assert_eq!(schedule.tiers[0].index_names, vec![(0, "U".to_string())]);
    assert_eq!(
        written_tier_lines(&schedule),
        vec!["a 0 0.44755829 n U".to_string()]
    );
}

/// A tier authored in the editor carries a `name` but no recorded positions:
/// the name goes after the FIRST index, `GemCAD`'s default label placement.
#[test]
fn an_editor_authored_tier_writes_its_name_after_the_first_index() {
    let schedule = AscSchedule {
        gemcad_version: "5.0".to_string(),
        gear_teeth: 96,
        symmetry_order: 1,
        refractive_index: 1.54,
        tiers: vec![AscTier {
            angle_deg: -41.0,
            mast: 0.6,
            name: "P1".to_string(),
            indices: vec![3.0, 9.0, 15.0],
            ..AscTier::default()
        }],
        ..AscSchedule::default()
    };
    assert_eq!(
        written_tier_lines(&schedule),
        vec!["a -41 0.6 3 n P1 9 15".to_string()]
    );
}

/// The editor's templates author the culet as `-0.0` with no index; the writer
/// must emit `GemCAD`'s own form (negative distance, one index = the gear tooth
/// count), and the parser must read that back as the same culet.
#[test]
fn an_editor_culet_without_indices_writes_gemcads_culet_form() {
    let schedule = AscSchedule {
        gemcad_version: "5.0".to_string(),
        gear_teeth: -96,
        symmetry_order: 8,
        mirror: true,
        refractive_index: 2.417,
        tiers: vec![AscTier {
            angle_deg: -0.0,
            mast: 0.88,
            name: "Culet".to_string(),
            ..AscTier::default()
        }],
        ..AscSchedule::default()
    };
    assert_eq!(
        written_tier_lines(&schedule),
        vec!["a -0 -0.88 96 n Culet".to_string()]
    );

    let written = to_asc_string(&schedule).expect("must write");
    let reparsed = parse_asc(&written).expect("must reparse");
    let culet = &reparsed.tiers[0];
    assert!(culet.is_culet());
    assert_eq!(culet.mast, 0.88);
    assert_eq!(culet.indices, vec![96.0]);
    assert_eq!(culet.name, "Culet");
}

/// `g96 0.0`, `y8y` and `I1.54`: each glued keyword is split with a warning
/// instead of failing the whole file.
#[test]
fn glued_keywords_are_split_with_a_warning() {
    let cases = [
        ("g96 0.0\ny 1 n\nI 1.54\n", 96, 1, false, 1.54),
        ("g 96 0.0\ny8y\nI 1.54\n", 96, 8, true, 1.54),
        ("g 64 0\ny 2 n\nI1.76\n", 64, 2, false, 1.76),
    ];
    for (header, gear, order, mirror, ri) in cases {
        let content = format!("GemCad 5.0\n{header}a 41 0.5 0\n");
        let schedule = parse_asc(&content).unwrap_or_else(|e| panic!("{header:?}: {e}"));
        assert_eq!(schedule.gear_teeth, gear, "{header:?}");
        assert_eq!(schedule.symmetry_order, order, "{header:?}");
        assert_eq!(schedule.mirror, mirror, "{header:?}");
        assert_eq!(schedule.refractive_index, ri, "{header:?}");
        assert_eq!(schedule.warnings.len(), 1, "{header:?}");
        assert!(schedule.warnings[0].contains("glued"), "{header:?}");
    }
}

/// A word that merely starts with a keyword letter is not split.
#[test]
fn a_word_starting_with_a_keyword_letter_is_not_split() {
    let schedule =
        parse_asc("GemCad 5.0\ngear 96\ng 96 0\ny 1 n\nI 1.54\na 41 0.5 0\n").expect("must parse");
    assert_eq!(schedule.gear_teeth, 96);
    assert_eq!(schedule.warnings.len(), 1);
    assert!(schedule.warnings[0].contains("unrecognized line"));
}

/// An unknown line (a possible preform record) is skipped with a warning, not
/// silently.
#[test]
fn an_unrecognized_line_outside_any_tier_warns() {
    let schedule =
        parse_asc("GemCad 5.0\ng 96 0\ny 1 n\nI 1.54\nP 1 2 3\na 41 0.5 0\n").expect("must parse");
    assert_eq!(schedule.tiers.len(), 1);
    assert_eq!(schedule.warnings.len(), 1);
    assert!(schedule.warnings[0].contains("P 1 2 3"));
}

/// The Unicode minus the manuals' typeset examples use reads as `-`.
#[test]
fn unicode_minus_reads_as_minus() {
    let schedule = parse_with("a \u{2212}41.5 0.6 3 \u{2212}2\n");
    assert_eq!(schedule.tiers[0].angle_deg, -41.5);
    assert_eq!(schedule.tiers[0].indices, vec![3.0, -2.0]);
}
