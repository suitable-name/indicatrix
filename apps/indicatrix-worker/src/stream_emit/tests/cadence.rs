//! Tests for [`effective_cadence_ms`]'s wall-clock interval averaging.

use crate::stream_emit::emitter::effective_cadence_ms;
use std::time::Duration;

#[test]
fn effective_cadence_ms_is_zero_for_fewer_than_two_emissions() {
    assert_eq!(effective_cadence_ms(Duration::from_secs(1), 0), 0);
    assert_eq!(effective_cadence_ms(Duration::from_secs(1), 1), 0);
}

#[test]
fn effective_cadence_ms_averages_the_interval() {
    // 3 emissions over 2 seconds -> 2 intervals -> 1000ms average.
    assert_eq!(effective_cadence_ms(Duration::from_secs(2), 3), 1000);
}
