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

/// While `remaining` is plentiful (far larger than the target-duration chunk), a
/// single lane's tail-aware size matches [`ChunkPolicy::samples_for_rate`] exactly --
/// the share term (`remaining` itself, since `sum_rates == rate` for one lane) never
/// binds until the run's genuine tail. Chunk sizing is therefore unaffected for a
/// single lane outside that tail.
#[test]
fn tail_aware_samples_matches_plain_sizing_for_one_lane_with_plenty_of_room() {
    let policy = ChunkPolicy::EXPORT;
    let rate = 1000.0;
    let plain = policy.samples_for_rate(rate);
    // `remaining` comfortably exceeds the plain chunk many times over.
    let remaining = plain * 50;
    assert_eq!(policy.tail_aware_samples(rate, remaining, rate), plain);
}

/// A single lane's very own tail (nothing else contributes to `sum_rates`) is still
/// capped at what is actually left, never past it -- `share == remaining` exactly.
/// `INTERACTIVE`'s `min_samples` floor is `1`, so it never masks the share term the
/// way `EXPORT`'s 32-sample floor would for a tail this small.
#[test]
fn tail_aware_samples_caps_a_single_lanes_own_tail_at_what_remains() {
    let policy = ChunkPolicy::INTERACTIVE;
    let rate = 1000.0;
    let remaining = 5; // far below the plain target-duration chunk (1500 samples).
    assert_eq!(policy.tail_aware_samples(rate, remaining, rate), remaining);
}

/// Two lanes at a 10:1 rate ratio sharing a small tail: each one's want, converted
/// back to the TIME it would take that lane to trace it (`want / rate`), lands within
/// about one fast-lane chunk duration of the other's -- the fast lane can no longer be
/// starved by a slow lane grabbing an oversized share of what is left.
#[test]
fn tail_aware_samples_shares_a_small_tail_close_to_proportionally() {
    let policy = ChunkPolicy {
        target: Duration::from_millis(300),
        min_samples: 1,
        max_samples: DEFAULT_MAX_CHUNK_SAMPLES,
        calibration_samples: 1,
    };
    let fast_rate = 1000.0;
    let slow_rate = 100.0; // 10:1, as the coordinator's A100-vs-780M report.
    let sum_rates = fast_rate + slow_rate;
    // Small relative to either lane's own target-duration chunk (300 and 30 samples).
    let remaining = 20;

    let fast_want = policy.tail_aware_samples(fast_rate, remaining, sum_rates);
    let slow_want = policy.tail_aware_samples(slow_rate, remaining, sum_rates);
    assert!(
        fast_want + slow_want <= remaining + 1,
        "shares must not overshoot what is left by more than rounding"
    );

    let fast_chunk_duration = f64::from(fast_want) / fast_rate;
    let slow_chunk_duration = f64::from(slow_want) / slow_rate;
    let fast_lane_chunk = policy.samples_for_rate(fast_rate);
    let fast_lane_chunk_duration = f64::from(fast_lane_chunk) / fast_rate;
    assert!(
        (fast_chunk_duration - slow_chunk_duration).abs() <= fast_lane_chunk_duration,
        "fast={fast_chunk_duration}s slow={slow_chunk_duration}s bound={fast_lane_chunk_duration}s"
    );
    // Without tail-awareness the slow lane's plain chunk would be
    // `policy.samples_for_rate(slow_rate)` (30, target-duration, ignoring `remaining`
    // entirely) -- several times the whole tail; the tail-aware share is much smaller.
    assert!(slow_want < policy.samples_for_rate(slow_rate));
}
