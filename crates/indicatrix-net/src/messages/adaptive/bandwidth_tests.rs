//! Tests for [`super::bandwidth`]: EWMA arithmetic, asymmetric smoothing, ignored writes
//! and seeding. No test reads a clock; every duration is a literal.

use super::{BandwidthConfig, BandwidthEstimator, MAX_PLAUSIBLE_MBPS, MIN_SAMPLE_BYTES};
use std::time::Duration;

/// Bytes and a duration that make exactly `mbps` Mbit/s (1 MB over `8 / mbps` seconds).
fn write_at(mbps: f64) -> (usize, Duration) {
    (1_000_000, Duration::from_secs_f64(8.0 / mbps))
}

fn feed(e: &mut BandwidthEstimator, mbps: f64) -> bool {
    let (bytes, took) = write_at(mbps);
    e.record(bytes, took)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0e-6 * b.abs().max(1.0)
}

#[test]
fn the_first_sample_is_the_estimate() {
    let mut e = BandwidthEstimator::default();
    assert_eq!(e.estimate(), None);
    assert!(feed(&mut e, 100.0));
    assert!(near(e.estimate().unwrap(), 100.0));
    assert_eq!(e.sample_count(), 1);
}

#[test]
fn small_writes_and_zero_durations_are_ignored() {
    let mut e = BandwidthEstimator::default();
    assert!(!e.record(MIN_SAMPLE_BYTES - 1, Duration::from_millis(1)));
    assert!(!e.record(0, Duration::from_millis(1)));
    assert!(!e.record(10_000_000, Duration::ZERO));
    assert_eq!(e.estimate(), None);
    assert_eq!(e.sample_count(), 0);
    assert!(e.record(MIN_SAMPLE_BYTES, Duration::from_millis(10)));
    assert_eq!(e.sample_count(), 1);
}

#[test]
fn the_ewma_moves_by_alpha_times_the_difference() {
    let config = BandwidthConfig {
        alpha_up: 0.5,
        alpha_down: 0.5,
        clearly_slower: 0.8,
    };
    let mut e = BandwidthEstimator::new(config);
    e.seed(100.0);
    feed(&mut e, 200.0);
    assert!(near(e.estimate().unwrap(), 150.0));
    feed(&mut e, 100.0);
    assert!(near(e.estimate().unwrap(), 125.0));
}

#[test]
fn a_clearly_slower_sample_moves_the_estimate_promptly() {
    let mut e = BandwidthEstimator::default();
    e.seed(1000.0);
    feed(&mut e, 100.0);
    // alpha_down 0.6: 1000 + 0.6 * (100 - 1000)
    assert!(near(e.estimate().unwrap(), 460.0));
}

#[test]
fn a_faster_sample_moves_the_estimate_slowly() {
    let mut e = BandwidthEstimator::default();
    e.seed(100.0);
    feed(&mut e, 1000.0);
    // alpha_up 0.15: 100 + 0.15 * (1000 - 100)
    assert!(near(e.estimate().unwrap(), 235.0));
}

#[test]
fn a_slightly_slower_sample_is_smoothed_slowly() {
    let mut e = BandwidthEstimator::default();
    e.seed(1000.0);
    feed(&mut e, 900.0);
    // 900 >= 0.8 * 1000, so alpha_up applies: 1000 - 0.15 * 100
    assert!(near(e.estimate().unwrap(), 985.0));
}

#[test]
fn a_slowdown_is_followed_faster_than_a_speedup() {
    let mut down = BandwidthEstimator::default();
    down.seed(1000.0);
    let mut up = BandwidthEstimator::default();
    up.seed(100.0);
    for _ in 0..4 {
        feed(&mut down, 100.0);
        feed(&mut up, 1000.0);
    }
    let fell = (1000.0 - down.estimate().unwrap()) / 900.0;
    let rose = (up.estimate().unwrap() - 100.0) / 900.0;
    assert!(fell > 0.9, "downward progress {fell}");
    assert!(rose < 0.6, "upward progress {rose}");
}

#[test]
fn seed_sets_the_start_and_rejects_nonsense() {
    let mut e = BandwidthEstimator::default();
    e.seed(300.0);
    assert_eq!(e.estimate(), Some(300.0));
    assert_eq!(e.sample_count(), 0);
    for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
        e.seed(bad);
        assert_eq!(e.estimate(), Some(300.0));
    }
    e.reset();
    assert_eq!(e.estimate(), None);
}

#[test]
fn an_instantaneous_write_is_capped() {
    let mut e = BandwidthEstimator::default();
    assert!(e.record(10_000_000, Duration::from_nanos(1)));
    assert_eq!(e.estimate(), Some(MAX_PLAUSIBLE_MBPS));
}

#[test]
fn invalid_smoothing_is_clamped_not_trusted() {
    let config = BandwidthConfig {
        alpha_up: f64::NAN,
        alpha_down: 9.0,
        clearly_slower: -1.0,
    };
    let mut e = BandwidthEstimator::new(config);
    e.seed(100.0);
    feed(&mut e, 200.0);
    let v = e.estimate().unwrap();
    assert!(v > 100.0 && v <= 200.0, "estimate {v}");
}
