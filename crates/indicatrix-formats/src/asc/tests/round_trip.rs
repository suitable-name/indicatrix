//! `to_asc_string` round-trip tests, against real `.asc` files pulled verbatim
//! from `facet_diagrams.sqlite` (the same fixtures indicatrix's geometry tests use to
//! exercise `from_asc_schedule`'s sign conventions). `to_asc_string` is not
//! byte-identical to the original file, but `parse_asc(&to_asc_string(parse_asc(x)))`
//! must equal `parse_asc(x)` for every one of them.

use super::{
    super::{mark_reconstructed, parse_asc},
    fixtures::{
        ASC_FOR_FUN, ASC_GIRDLE_NAMED_G, ASC_GIRDLE_NAMED_G_NO_NOTES, ASC_LARGE_TEXAS_STAR,
        ASC_ROUND_TRICHECKER_12, ASC_SHAH_REPLICA_NO_NAMES, REAL_SAMPLE, assert_round_trips,
    },
};

#[test]
fn round_trips_real_sample() {
    assert_round_trips(REAL_SAMPLE);
}

#[test]
fn round_trips_round_trichecker_12() {
    assert_round_trips(ASC_ROUND_TRICHECKER_12);
}

#[test]
fn round_trips_for_fun() {
    assert_round_trips(ASC_FOR_FUN);
}

#[test]
fn round_trips_large_texas_star() {
    assert_round_trips(ASC_LARGE_TEXAS_STAR);
}

#[test]
fn round_trips_shah_replica_no_names() {
    assert_round_trips(ASC_SHAH_REPLICA_NO_NAMES);
}

#[test]
fn round_trips_girdle_named_g() {
    assert_round_trips(ASC_GIRDLE_NAMED_G);
}

#[test]
fn round_trips_girdle_named_g_no_notes() {
    assert_round_trips(ASC_GIRDLE_NAMED_G_NO_NOTES);
}

#[test]
fn mark_reconstructed_prepends_marker_header_once() {
    let mut schedule = parse_asc(ASC_FOR_FUN).expect("real sample must parse");
    let original_header_count = schedule.headers.len();

    mark_reconstructed(&mut schedule, "solved via MeetPointSolver");
    assert_eq!(schedule.headers.len(), original_header_count + 1);
    assert!(schedule.headers[0].starts_with("RECONSTRUCTED"));
    assert!(schedule.headers[0].contains("solved via MeetPointSolver"));

    // Calling it again must not stack a second marker.
    mark_reconstructed(&mut schedule, "a different note");
    assert_eq!(schedule.headers.len(), original_header_count + 1);
}

#[test]
fn round_trips_fractional_indices_and_wrapped_continuation() {
    // A hand-assembled but format-faithful case combining fractional indices with
    // a wrapped continuation line, to make sure to_asc_string's single-line-per-tier
    // output still round-trips even though the source used a continuation.
    let content = "GemCad 5.0\n\
                    g 96 0.0\n\
                    y 1 n\n\
                    I 1.72\n\
                    H Test\n\
                    a 90.000000 1.08976142 96 n G1 91 85 80 75 69 64 59 53 48 43 37 32 27 21 16\n\
                     11 5 G Or continuous girdle\n\
                    a -90.00 1.02050 88.8 7.2\n";
    assert_round_trips(content);
}
