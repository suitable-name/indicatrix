//! Tests for [`super::tier`]: the ladder, the 50% rule at every gap, hysteresis, clamping
//! and the estimator + selector pairing. No clock is read anywhere.

use super::{
    BandwidthConfig, BandwidthTier, DEFAULT_HYSTERESIS, DEFAULT_TIER_INDEX, LinkAdapter,
    TIER_COUNT, TIERS_MBPS, TierSelector,
};
use std::time::Duration;

#[test]
fn the_ladder_is_ascending_and_covers_the_measured_links() {
    assert!(TIERS_MBPS.windows(2).all(|w| w[0] < w[1]));
    for needed in [100.0, 300.0, 1000.0] {
        assert!(TIERS_MBPS.contains(&needed), "{needed} Mbit/s missing");
    }
    let ladder = TIERS_MBPS.to_vec();
    assert!(
        ladder[0] < 100.0,
        "a tier slower than the slowest measured link"
    );
    assert!(
        ladder[TIER_COUNT - 1] > 1000.0,
        "a tier faster than gigabit"
    );
    assert_eq!(TIERS_MBPS[DEFAULT_TIER_INDEX], 300.0);
    assert_eq!(BandwidthTier::DEFAULT.mbps(), 300.0);
}

#[test]
fn the_fifty_percent_rule_holds_in_every_gap() {
    for i in 0..TIER_COUNT - 1 {
        let (lo, hi) = (TIERS_MBPS[i], TIERS_MBPS[i + 1]);
        let mid = 0.5_f64.mul_add(hi - lo, lo);
        assert_eq!(BandwidthTier::for_estimate(lo).index(), i, "at tier {lo}");
        assert_eq!(
            BandwidthTier::for_estimate(mid - 0.001).index(),
            i,
            "just below {mid}"
        );
        assert_eq!(
            BandwidthTier::for_estimate(mid).index(),
            i,
            "exactly halfway is not MORE than 50%"
        );
        assert_eq!(
            BandwidthTier::for_estimate(mid + 0.001).index(),
            i + 1,
            "just above {mid}"
        );
        assert_eq!(
            BandwidthTier::for_estimate(hi).index(),
            i + 1,
            "at tier {hi}"
        );
    }
}

#[test]
fn the_rule_between_300_and_1000_flips_at_650() {
    let idx = |m: f64| BandwidthTier::for_estimate(m).mbps();
    assert_eq!(idx(649.0), 300.0);
    assert_eq!(idx(651.0), 1000.0);
}

#[test]
fn estimates_outside_the_ladder_clamp() {
    assert_eq!(BandwidthTier::for_estimate(1.0), BandwidthTier::LOWEST);
    assert_eq!(BandwidthTier::for_estimate(f64::NAN), BandwidthTier::LOWEST);
    assert_eq!(BandwidthTier::for_estimate(-3.0), BandwidthTier::LOWEST);
    assert_eq!(BandwidthTier::for_estimate(1.0e9), BandwidthTier::HIGHEST);
    assert_eq!(BandwidthTier::from_index(999), BandwidthTier::HIGHEST);
    assert_eq!(BandwidthTier::all()[0], BandwidthTier::LOWEST);
    assert_eq!(BandwidthTier::all()[TIER_COUNT - 1], BandwidthTier::HIGHEST);
}

#[test]
fn a_selector_starts_on_the_default_tier_and_ignores_missing_estimates() {
    let mut s = TierSelector::default();
    assert_eq!(s.tier(), BandwidthTier::DEFAULT);
    assert_eq!(s.update(None), BandwidthTier::DEFAULT);
    assert_eq!(s.update(Some(f64::NAN)), BandwidthTier::DEFAULT);
    assert_eq!(s.update(Some(0.0)), BandwidthTier::DEFAULT);
    assert_eq!(s.switches(), 0);
}

#[test]
fn the_first_estimate_places_the_tier_directly() {
    let mut s = TierSelector::default();
    assert_eq!(s.update(Some(10_000.0)), BandwidthTier::HIGHEST);
    assert_eq!(s.switches(), 0, "the placement is not a switch");
    let mut s = TierSelector::default();
    assert_eq!(s.update(Some(10.0)), BandwidthTier::LOWEST);
}

#[test]
fn hysteresis_delays_the_flip_in_both_directions() {
    // Between 300 and 1000 the 50% line is 650; with 15% the up line is 755 and the
    // down line 545.
    let mut s = TierSelector::new(0.15);
    assert_eq!(s.update(Some(300.0)).mbps(), 300.0);
    assert_eq!(
        s.update(Some(700.0)).mbps(),
        300.0,
        "past 50% but inside the band"
    );
    assert_eq!(s.update(Some(754.0)).mbps(), 300.0);
    assert_eq!(s.update(Some(756.0)).mbps(), 1000.0);
    assert_eq!(
        s.update(Some(700.0)).mbps(),
        1000.0,
        "inside the band, stays up"
    );
    assert_eq!(s.update(Some(546.0)).mbps(), 1000.0);
    assert_eq!(s.update(Some(544.0)).mbps(), 300.0);
    assert_eq!(s.switches(), 2);
}

#[test]
fn a_big_jump_crosses_several_tiers_at_once() {
    let mut s = TierSelector::default();
    s.update(Some(TIERS_MBPS[0]));
    assert_eq!(s.update(Some(1.0e6)), BandwidthTier::HIGHEST);
    assert_eq!(s.update(Some(1.0)), BandwidthTier::LOWEST);
    assert_eq!(
        s.switches(),
        2,
        "one switch per update however many rungs it crossed"
    );
}

/// A deterministic signal of `centre +- spread` that alternates and never rests.
fn noisy(centre: f64, spread: f64, n: usize) -> Vec<f64> {
    let mut state = 0x2545_f491_u32;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let unit = f64::from(state % 2001) / 1000.0 - 1.0;
            spread.mul_add(unit, centre)
        })
        .collect()
}

#[test]
fn noise_at_the_boundary_does_not_flap() {
    let mut s = TierSelector::default();
    for e in noisy(650.0, 80.0, 500) {
        s.update(Some(e));
    }
    assert_eq!(s.switches(), 0, "stayed on {}", s.tier().mbps());
}

#[test]
fn without_hysteresis_the_same_noise_does_flap() {
    let mut s = TierSelector::new(0.0);
    for e in noisy(650.0, 80.0, 500) {
        s.update(Some(e));
    }
    assert!(s.switches() > 10, "only {} switches", s.switches());
}

#[test]
fn the_hysteresis_is_clamped_to_a_usable_range() {
    let mut s = TierSelector::new(7.0);
    s.update(Some(100.0));
    assert_eq!(s.update(Some(1.0e6)), BandwidthTier::HIGHEST);
    let mut s = TierSelector::new(f64::NAN);
    s.update(Some(300.0));
    assert_eq!(
        s.update(Some(700.0)).mbps(),
        300.0,
        "NaN falls back to the default band"
    );
    assert_eq!(
        TierSelector::default(),
        TierSelector::new(DEFAULT_HYSTERESIS)
    );
}

/// One megabyte over `8 / mbps` seconds.
fn write_at(mbps: f64) -> (usize, Duration) {
    (1_000_000, Duration::from_secs_f64(8.0 / mbps))
}

#[test]
fn a_link_adapter_follows_a_slowdown_and_a_recovery() {
    let mut link = LinkAdapter::default();
    assert_eq!(link.estimate(), None);
    let (bytes, took) = write_at(1000.0);
    assert_eq!(link.observe(bytes, took).mbps(), 1000.0);
    let (bytes, took) = write_at(100.0);
    let mut tier = link.tier();
    for _ in 0..4 {
        tier = link.observe(bytes, took);
    }
    assert_eq!(tier.mbps(), 100.0, "estimate {:?}", link.estimate());
    let (bytes, took) = write_at(1000.0);
    for _ in 0..40 {
        tier = link.observe(bytes, took);
    }
    assert_eq!(tier.mbps(), 1000.0);
    assert!(link.tier_switches() >= 2);
}

#[test]
fn small_writes_leave_the_adapter_where_it_is() {
    let mut link = LinkAdapter::new(BandwidthConfig::DEFAULT, DEFAULT_HYSTERESIS);
    let before = link.tier();
    assert_eq!(link.observe(1000, Duration::from_secs(5)), before);
    assert_eq!(link.estimate(), None);
}

#[test]
fn seeding_places_a_new_connection_at_its_last_known_speed() {
    let mut link = LinkAdapter::default();
    assert_eq!(link.seed(2600.0).mbps(), 2500.0);
    assert_eq!(link.estimate(), Some(2600.0));
    assert_eq!(
        link.seed(f64::NAN).mbps(),
        2500.0,
        "nonsense changes nothing"
    );
    assert_eq!(
        link.seed(80.0).mbps(),
        100.0,
        "80 is past the 75 midpoint of 50..100"
    );
}
