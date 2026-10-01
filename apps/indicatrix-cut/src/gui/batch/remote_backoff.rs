//! The failure policy of a batch's remote lane: how long to hold off after the remote
//! fails an item, and when to give up on it for a longer stretch.
//!
//! Without this, a remote that fails every request (refused handshake, wedged
//! coordinator, corrupted TLS stream) let the lane claim the next item the instant the
//! previous one failed -- draining the whole shared pool into the local retry pile in
//! seconds and then exiting for good, with nothing logged. Pure bookkeeping (no sockets,
//! no Slint), so the schedule is unit-tested here and the lane only has to ask.

use std::time::{Duration, Instant};

/// Delay after the FIRST consecutive failure; doubles with each further one.
pub const REMOTE_BACKOFF_INITIAL: Duration = Duration::from_secs(1);

/// Ceiling of the doubling schedule. Only reachable when [`REMOTE_SIT_OUT_AFTER`] is
/// raised: with today's threshold the schedule is 1, 2, 4, 8 s and then the sit-out.
pub const REMOTE_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Consecutive failures after which the remote is treated as down rather than flaky.
pub const REMOTE_SIT_OUT_AFTER: u32 = 5;

/// How long the lane stays away from a remote that failed [`REMOTE_SIT_OUT_AFTER`]
/// times in a row, before probing it with one more item.
pub const REMOTE_SIT_OUT: Duration = Duration::from_secs(120);

/// Granularity of the cancellable wait: a Cancel click is honoured within this long.
pub const REMOTE_WAIT_SLICE: Duration = Duration::from_millis(100);

/// Consecutive-failure counter plus the earliest instant the next attempt may start.
#[derive(Debug, Default)]
pub struct RemoteBackoff {
    consecutive_failures: u32,
    not_before: Option<Instant>,
}

/// The delay after the `failures`-th consecutive failure (1-based) for a lane that
/// sits out from `sit_out_after` failures on. Split from [`RemoteBackoff::record_failure`]
/// so the cap can be tested with a threshold high enough to reach it.
fn delay_for(failures: u32, sit_out_after: u32) -> Duration {
    if failures >= sit_out_after {
        return REMOTE_SIT_OUT;
    }
    // `failures - 1` is a shift by at most a few bits; saturate anyway for a huge
    // threshold so the arithmetic can never overflow.
    let doublings = failures.saturating_sub(1).min(20);
    REMOTE_BACKOFF_INITIAL
        .saturating_mul(1_u32 << doublings)
        .min(REMOTE_BACKOFF_MAX)
}

impl RemoteBackoff {
    /// Records a failure at `now` and returns the delay before the next attempt:
    /// `INITIAL * 2^(n-1)` capped at `MAX` for `n < SIT_OUT_AFTER`, `SIT_OUT` from then on.
    pub fn record_failure(&mut self, now: Instant) -> Duration {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let delay = delay_for(self.consecutive_failures, REMOTE_SIT_OUT_AFTER);
        self.not_before = Some(now + delay);
        delay
    }

    /// Forgets every earlier failure: the remote answered, so it is healthy again.
    pub const fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.not_before = None;
    }

    /// How much longer the lane must wait at `now`, or `None` when it may attempt now.
    #[must_use]
    pub fn wait_needed(&self, now: Instant) -> Option<Duration> {
        self.not_before
            .and_then(|at| at.checked_duration_since(now))
            .filter(|remaining| !remaining.is_zero())
    }

    /// How many attempts in a row have failed since the last success.
    #[must_use]
    pub const fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Whether the remote has failed often enough that the lane is sitting it out.
    #[must_use]
    pub const fn sitting_out(&self) -> bool {
        self.consecutive_failures >= REMOTE_SIT_OUT_AFTER
    }
}

/// Sleeps up to `total` in [`REMOTE_WAIT_SLICE`] steps, returning early -- with `true` --
/// as soon as `should_stop` says so (a Cancel click, or nothing left to claim). Returns
/// `false` when the full delay elapsed.
///
/// Sliced rather than one long sleep because a sit-out is two minutes: Cancel must not
/// have to wait it out.
pub fn wait_unless(total: Duration, slice: Duration, should_stop: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + total;
    loop {
        if should_stop() {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(slice));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_double_then_the_fifth_failure_sits_out() {
        let mut backoff = RemoteBackoff::default();
        let now = Instant::now();
        let delays: Vec<Duration> = (0..6).map(|_| backoff.record_failure(now)).collect();
        assert_eq!(
            delays,
            [1, 2, 4, 8, 120, 120].map(Duration::from_secs),
            "1, 2, 4, 8 s, then the 120 s sit-out from the fifth failure on"
        );
        assert_eq!(backoff.consecutive_failures(), 6);
    }

    #[test]
    fn sitting_out_starts_exactly_at_the_threshold() {
        let mut backoff = RemoteBackoff::default();
        let now = Instant::now();
        for failure in 1..=REMOTE_SIT_OUT_AFTER {
            assert!(
                !backoff.sitting_out(),
                "not yet after {} failure(s)",
                failure - 1
            );
            backoff.record_failure(now);
        }
        assert!(backoff.sitting_out());
    }

    #[test]
    fn doubling_is_capped_when_the_threshold_is_raised() {
        let delays: Vec<Duration> = (1..=8).map(|n| delay_for(n, 100)).collect();
        assert_eq!(
            delays,
            [1, 2, 4, 8, 16, 30, 30, 30].map(Duration::from_secs),
            "doubles up to the 30 s ceiling and stays there"
        );
        assert_eq!(delay_for(u32::MAX, 100), REMOTE_SIT_OUT);
        assert_eq!(delay_for(90, 100), REMOTE_BACKOFF_MAX, "no shift overflow");
    }

    #[test]
    fn success_resets_the_counter_and_the_wait() {
        let mut backoff = RemoteBackoff::default();
        let now = Instant::now();
        for _ in 0..REMOTE_SIT_OUT_AFTER {
            backoff.record_failure(now);
        }
        assert!(backoff.wait_needed(now).is_some());
        backoff.record_success();
        assert_eq!(backoff.consecutive_failures(), 0);
        assert!(!backoff.sitting_out());
        assert_eq!(backoff.wait_needed(now), None);
        assert_eq!(
            backoff.record_failure(now),
            REMOTE_BACKOFF_INITIAL,
            "the schedule restarts from the first step"
        );
    }

    #[test]
    fn wait_needed_counts_down_and_is_none_once_past() {
        let mut backoff = RemoteBackoff::default();
        let now = Instant::now();
        assert_eq!(backoff.wait_needed(now), None, "no failure, no wait");
        backoff.record_failure(now);
        assert_eq!(
            backoff.wait_needed(now),
            Some(Duration::from_secs(1)),
            "the full delay right after the failure"
        );
        assert_eq!(
            backoff.wait_needed(now + Duration::from_millis(400)),
            Some(Duration::from_millis(600))
        );
        assert_eq!(backoff.wait_needed(now + Duration::from_secs(1)), None);
        assert_eq!(backoff.wait_needed(now + Duration::from_secs(5)), None);
    }

    #[test]
    fn wait_returns_early_when_told_to_stop() {
        let started = Instant::now();
        let stopped = wait_unless(Duration::from_secs(60), Duration::from_millis(5), || true);
        assert!(stopped);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "did not sleep the delay out"
        );
    }

    #[test]
    fn wait_stops_mid_sleep_when_the_flag_flips() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let stopped = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                cancel.store(true, Ordering::Relaxed);
            });
            wait_unless(Duration::from_secs(60), Duration::from_millis(5), || {
                cancel.load(Ordering::Relaxed)
            })
        });
        assert!(stopped);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn wait_runs_the_full_delay_when_nothing_stops_it() {
        let started = Instant::now();
        let stopped = wait_unless(Duration::from_millis(40), Duration::from_millis(5), || {
            false
        });
        assert!(!stopped);
        assert!(started.elapsed() >= Duration::from_millis(40));
    }
}
