//! Parsing a real sample file's headers/tiers, folded repeated `n <name>`
//! groups, a bare culet-like tier, wrapped continuation lines, and the
//! missing-`g`-keyword tolerance.

use super::{super::parse_asc, fixtures::REAL_SAMPLE};

#[test]
fn parses_real_sample_header_fields() {
    let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
    assert_eq!(schedule.gemcad_version, "5.0");
    assert_eq!(schedule.gear_teeth, 96);
    assert_eq!(schedule.symmetry_order, 1);
    assert!(schedule.mirror);
    assert!((schedule.refractive_index - 1.54).abs() < 1e-9);
    assert_eq!(schedule.headers.len(), 4);
    assert_eq!(schedule.headers[0], "PC 46.019  For Fun");
    assert_eq!(schedule.footnotes.len(), 3);
}

#[test]
fn parses_real_sample_tier_count_and_fields() {
    let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
    assert_eq!(schedule.tiers.len(), 4);

    let p1 = &schedule.tiers[0];
    assert!((p1.angle_deg - (-44.864_054)).abs() < 1e-6);
    assert!((p1.mast - 0.537_910_82).abs() < 1e-9);
    assert_eq!(p1.name, "P1");
    assert_eq!(p1.indices, vec![84.0, 12.0]);
    assert_eq!(p1.notes, "Cut to mast depth X.");
}

#[test]
fn folds_repeated_name_groups_into_one_tier() {
    // The "-90 ... 36 12 84 n G1 60 n G1 ..." row: G1's indices are split across
    // two "n G1" groups on the same physical line. All four index positions must
    // land on the single resulting tier, not be split or deduplicated away.
    let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
    let g1 = &schedule.tiers[2];
    assert_eq!(g1.name, "G1");
    assert_eq!(g1.indices, vec![36.0, 12.0, 84.0, 60.0]);
    assert_eq!(g1.notes, "Set stone size.");
}

#[test]
fn single_index_culet_like_tier_parses() {
    let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
    let u = &schedule.tiers[3];
    assert!((u.angle_deg - 0.0).abs() < 1e-9);
    assert_eq!(u.name, "U");
    assert_eq!(u.indices, vec![96.0]);
    assert_eq!(u.notes, "");
}

#[test]
fn handles_continuation_lines_for_wrapped_index_lists() {
    // Verified real wrap pattern: a bare-number continuation line, then the next
    // record starts fresh on its own "a" line.
    let content = "GemCad 5.0\n\
                    g 96 0.0\n\
                    y 1 n\n\
                    I 1.72\n\
                    H Test\n\
                    a 90.000000 1.08976142 96 n G1 91 85 80 75 69 64 59 53 48 43 37 32 27 21 16\n\
                     11 5 G Or continuous girdle\n\
                    a -44.001549 0.51438487 96 n P1 85 75 64 53 43 32 21 11 G Cut to TCP\n";
    let schedule = parse_asc(content).expect("must parse continuation lines");
    assert_eq!(schedule.tiers.len(), 2);
    let g1 = &schedule.tiers[0];
    // 96 (gear ref) + 91..16 (15 more on the first line) + 11, 5 (continuation) = 18.
    assert_eq!(
        g1.indices.len(),
        18,
        "wrapped continuation indices must be folded into the same tier"
    );
    assert_eq!(g1.notes, "Or continuous girdle");
}

#[test]
fn handles_n_name_continuation_line() {
    let content = "GemCad 5.0\n\
                    g 96 0.0\n\
                    y 1 n\n\
                    I 1.72\n\
                    H Test\n\
                    a -53.00 0.83825 93 87 81 75 69 63 57 51 45 39 33 27 21 15 9 3\n\
                     n 4\n\
                    a -48.00 0.82316 0 n 3 90 84 78 72 66 60 54 48 42 36 30 24 18 12 6\n";
    let schedule = parse_asc(content).expect("must parse 'n <name>' continuation");
    assert_eq!(schedule.tiers.len(), 2);
    assert_eq!(schedule.tiers[0].name, "4");
    assert_eq!(schedule.tiers[0].indices.len(), 16);
}

#[test]
fn tolerates_missing_g_keyword_prefix() {
    // Real quirk seen once in the corpus (attached_files "Astryx Star" file): the
    // gear line lost its leading "g" and reads as a bare "96 0.0".
    let content = "GemCad 5.0\n\
                    96 0.0\n\
                    y 8 n\n\
                    I 1.54\n\
                    H Astryx Star\n\
                    a -42.800507 0.53960274 92 n P1 84 76 68 60 52 44 36 28 20 12 4 G Cut to centerpoint.\n";
    let schedule = parse_asc(content).expect("must tolerate a bare gear line");
    assert_eq!(schedule.gear_teeth, 96);
}
