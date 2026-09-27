//! Tests for [`super::super::objective`]'s [`ObjectiveWeights::score`] and
//! [`ObjectiveWeights::score_with_yield`].

use super::super::{ObjectiveComponents, ObjectiveWeights};

// --- ObjectiveWeights::score ---

#[test]
fn score_is_the_normalized_weighted_sum_with_tilt_brilliance_flipped_to_a_loss() {
    let weights = ObjectiveWeights {
        windowing: 1.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 0.0,
    };
    let components = ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 20.0,
        tilt_brilliance_pct: 70.0, // loss = 30.0
    };
    let score = weights.score(&components);
    assert!((score - 20.0).abs() < 1e-4, "got {score}");
}

#[test]
fn score_falls_back_to_equal_weights_when_all_weights_are_non_positive() {
    let weights = ObjectiveWeights {
        windowing: 0.0,
        extinction: 0.0,
        tilt_brilliance: 0.0,
        yield_weight: 0.0,
    };
    let components = ObjectiveComponents {
        windowing_pct: 30.0,
        extinction_pct: 30.0,
        tilt_brilliance_pct: 70.0,
    };
    let score = weights.score(&components);
    assert!((score - 30.0).abs() < 1e-4, "got {score}");
}

#[test]
fn score_only_depends_on_weight_ratios_not_absolute_scale() {
    let components = ObjectiveComponents {
        windowing_pct: 12.0,
        extinction_pct: 8.0,
        tilt_brilliance_pct: 60.0,
    };
    let a = ObjectiveWeights {
        windowing: 1.0,
        extinction: 2.0,
        tilt_brilliance: 3.0,
        yield_weight: 0.0,
    };
    let b = ObjectiveWeights {
        windowing: 10.0,
        extinction: 20.0,
        tilt_brilliance: 30.0,
        yield_weight: 0.0,
    };
    assert!((a.score(&components) - b.score(&components)).abs() < 1e-4);
}

// --- ObjectiveWeights::score_with_yield ---

/// With the default `yield_weight` (`0.0`), `score_with_yield` must reproduce
/// `score`'s own output bit for bit, REGARDLESS of what `yield_loss_pct` is
/// passed -- a zero-weight term must never perturb the result.
#[test]
fn score_with_yield_at_default_weight_matches_score_bit_for_bit() {
    let weights = ObjectiveWeights::default();
    let components = ObjectiveComponents {
        windowing_pct: 12.0,
        extinction_pct: 8.0,
        tilt_brilliance_pct: 60.0,
    };
    let plain = weights.score(&components);
    for yield_loss_pct in [0.0, 25.0, 100.0] {
        let with_yield = weights.score_with_yield(&components, yield_loss_pct);
        assert_eq!(
            plain.to_bits(),
            with_yield.to_bits(),
            "yield_loss_pct={yield_loss_pct} must not move the score at yield_weight=0.0"
        );
    }
}

/// A non-zero `yield_weight` must actually blend the yield term in: two
/// candidates with identical optical components but different yield loss must
/// score differently once `yield_weight > 0.0`, and the lower-yield-loss
/// candidate must score better (lower).
#[test]
fn score_with_yield_prefers_lower_yield_loss_at_positive_weight() {
    let weights = ObjectiveWeights {
        windowing: 1.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 1.0,
    };
    let components = ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 10.0,
        tilt_brilliance_pct: 80.0,
    };
    let low_loss = weights.score_with_yield(&components, 10.0);
    let high_loss = weights.score_with_yield(&components, 90.0);
    assert!(
        low_loss < high_loss,
        "low_loss={low_loss} must score better than high_loss={high_loss}"
    );
}
