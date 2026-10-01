//! Shape classification from the girdle outline.

use super::super::measure::classify_shape;
use indicatrix::geometry::cuts::StandardGemCuts;

/// The strongest available anchor: the built-in standard round brilliant has 16
/// girdle facets, so the outline-based rule must call it Round.
#[test]
fn classify_shape_calls_the_standard_round_brilliant_round() {
    let planes = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        classify_shape(&planes, 1.0).as_deref(),
        Some("Round"),
        "16 girdle facets at lw == 1.0 must classify as Round"
    );
}

/// The regression this rule exists for. A fold-count rule called "Round
/// Trichecker-12" a Hexagon, because its schedule declares 6-fold symmetry while
/// the cut is round. Classification keys on the girdle OUTLINE instead, so a
/// round outline stays Round no matter what fold count the schedule declares --
/// `classify_shape` no longer receives `symmetry_order` at all, which is what
/// makes that misreading unrepresentable rather than merely unlikely.
#[test]
fn classify_shape_ignores_fold_count_entirely() {
    let planes = StandardGemCuts::standard_round_brilliant();
    // Same planes, and no symmetry_order is threaded in from anywhere: the only
    // inputs are the outline and the measured ratio.
    assert_eq!(classify_shape(&planes, 1.0).as_deref(), Some("Round"));
}

/// An elongated stone is never guessed at, however round-looking its outline:
/// Oval/Marquise/Pear cannot be told apart by side count alone, so the honest
/// answer is no shape rather than a confident wrong one.
#[test]
fn classify_shape_refuses_to_guess_for_an_elongated_outline() {
    let planes = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        classify_shape(&planes, 1.6),
        None,
        "a 1.6 length/width ratio must not be classified Round"
    );
}

/// No girdle facets at all (or too few to be confident) yields no shape rather
/// than a panic or a default.
#[test]
fn classify_shape_returns_none_without_a_usable_girdle() {
    assert_eq!(classify_shape(&[], 1.0), None);
}
