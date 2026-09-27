//! Tests for [`RateModel`], [`ChunkPolicy`] and [`marginal_rate`].

use super::*;

#[test]
fn marginal_rate_divides_samples_by_elapsed_seconds() {
    assert_eq!(marginal_rate(100, Duration::from_secs(2)), Some(50.0));
    assert_eq!(marginal_rate(0, Duration::from_secs(1)), None);
    assert_eq!(marginal_rate(100, Duration::ZERO), None);
}

#[test]
fn export_policy_matches_the_desktop_exports_numbers() {
    assert_eq!(ChunkPolicy::EXPORT.samples_for_rate(1000.0), 22_000);
    assert_eq!(ChunkPolicy::EXPORT.samples_for_rate(0.01), 32);
    assert_eq!(ChunkPolicy::EXPORT.first_chunk_samples(), 8);
}

#[test]
fn samples_for_rate_clamps_and_survives_non_finite_rates() {
    let policy = ChunkPolicy::EXPORT;
    for rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -100.0, 0.0] {
        let samples = policy.samples_for_rate(rate);
        assert!(
            samples == 32 || samples == DEFAULT_MAX_CHUNK_SAMPLES,
            "{rate} -> {samples}"
        );
        assert!(samples > 0);
    }
    assert_eq!(policy.samples_for_rate(f64::NAN), 32);
    assert_eq!(policy.samples_for_rate(1e12), DEFAULT_MAX_CHUNK_SAMPLES);
}

#[test]
fn a_degenerate_policy_still_yields_positive_chunks() {
    let policy = ChunkPolicy {
        target: Duration::from_secs(1),
        min_samples: 0,
        max_samples: 0,
        calibration_samples: 0,
    };
    assert_eq!(policy.samples_for_rate(1000.0), 1);
    assert_eq!(policy.first_chunk_samples(), 1);
    assert_eq!(ChunkPolicy::fixed(0).samples_for_rate(5.0), 1);
}

#[test]
fn fixed_policy_ignores_the_rate() {
    let policy = ChunkPolicy::fixed(7);
    for rate in [0.0, 1.0, 1e6, f64::NAN] {
        assert_eq!(policy.samples_for_rate(rate), 7);
    }
    assert_eq!(RateModel::new(1.0).chunk_samples(&policy), 7);
}

#[test]
fn an_uncalibrated_model_reports_its_guess_and_asks_for_a_calibration_chunk() {
    let model = RateModel::new(1000.0);
    assert!(!model.is_calibrated());
    assert_eq!(model.rate(), 1000.0);
    assert_eq!(model.estimate(), None);
    assert_eq!(model.chunk_samples(&ChunkPolicy::EXPORT), 8);
}

#[test]
fn the_first_measurement_replaces_the_guess_outright() {
    let mut model = RateModel::new(1000.0);
    assert_eq!(
        model.observe(8, Duration::from_millis(80), None),
        Some(100.0)
    );
    assert!(model.is_calibrated());
    assert_eq!(model.rate(), 100.0);
    assert_eq!(model.chunk_samples(&ChunkPolicy::EXPORT), 2200);
}

#[test]
fn a_reported_rate_is_preferred_over_wall_clock() {
    let mut model = RateModel::new(1.0);
    model.observe(10, Duration::from_secs(10), Some(40.0));
    assert_eq!(model.rate(), 40.0);
    // An invalid report falls back to wall clock.
    model = RateModel::new(1.0);
    model.observe(10, Duration::from_secs(10), Some(f64::NAN));
    assert_eq!(model.rate(), 1.0);
    assert!(model.is_calibrated());
}

#[test]
fn a_measurement_of_nothing_leaves_the_model_unchanged() {
    let mut model = RateModel::calibrated(50.0);
    assert_eq!(model.observe(0, Duration::from_secs(1), None), None);
    assert_eq!(model.observe(5, Duration::ZERO, Some(-3.0)), None);
    assert_eq!(model.rate(), 50.0);
    assert_eq!(model.observations(), 0);
}

#[test]
fn calibrated_rejects_an_unusable_rate() {
    assert!(RateModel::calibrated(250.0).is_calibrated());
    assert!(!RateModel::calibrated(0.0).is_calibrated());
    assert!(!RateModel::calibrated(f64::NAN).is_calibrated());
}

#[test]
fn later_measurements_blend_with_the_default_weight() {
    let mut model = RateModel::calibrated(100.0);
    model.observe(200, Duration::from_secs(1), None);
    // 0.7 * 100 + 0.3 * 200
    assert!((model.rate() - 130.0).abs() < 1e-9);
    let mut eager = RateModel::calibrated(100.0).with_smoothing(1.0);
    eager.observe(200, Duration::from_secs(1), None);
    assert_eq!(eager.rate(), 200.0);
}

/// A lane whose true throughput jumps (thermal throttling, another job finishing)
/// converges to the new rate within a handful of noisy chunks.
#[test]
fn the_moving_average_converges_to_a_new_steady_rate() {
    let mut model = RateModel::new(10.0);
    // Calibrate at ~500/s, then the lane speeds up to ~2000/s with +-5 % noise.
    model.observe(500, Duration::from_secs(1), None);
    let noise = [1.05, 0.95, 1.02, 0.98, 1.04, 0.96, 1.01, 0.99];
    for step in 0..24 {
        let rate = 2000.0 * noise[step % noise.len()];
        model.observe(0, Duration::ZERO, Some(rate));
    }
    let error = (model.rate() - 2000.0).abs() / 2000.0;
    assert!(
        error < 0.05,
        "estimate {} not within 5 % of 2000",
        model.rate()
    );
    assert_eq!(model.observations(), 25);
}
