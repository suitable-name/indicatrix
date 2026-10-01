//! When [`super::backend::GpuBackend`] may try to re-acquire a device it lost.
//!
//! Pure bookkeeping over caller-supplied [`Instant`]s -- no GPU, no clock reads -- so the
//! policy is unit-testable without hardware (see `super::tests`).

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// How long after a loss (or after a failed re-acquisition) the backend waits before it
/// tries to acquire a device again.
pub(super) const COOL_DOWN: Duration = Duration::from_secs(30);

/// The most re-acquisition attempts started within any one [`WINDOW`].
pub(super) const MAX_ATTEMPTS_PER_WINDOW: usize = 6;

/// The sliding window [`MAX_ATTEMPTS_PER_WINDOW`] is counted over: one hour.
pub(super) const WINDOW: Duration = Duration::from_hours(1);

/// The cool-down and rate limit on re-acquiring a lost device.
///
/// A device that keeps failing is not hammered: every attempt, successful or not,
/// restarts the [`COOL_DOWN`], and at most [`MAX_ATTEMPTS_PER_WINDOW`] attempts start in
/// any [`WINDOW`]. The oldest attempt ages out of the window one hour after it began.
#[derive(Debug, Default)]
pub(super) struct RecoveryPolicy {
    /// When the current cool-down started: the loss itself, or the latest failed
    /// attempt. `None` while no device is lost.
    cool_down_started: Option<Instant>,
    /// Start times of the attempts inside the current [`WINDOW`], oldest first.
    attempts: VecDeque<Instant>,
}

impl RecoveryPolicy {
    /// A policy with no loss recorded.
    pub(super) const fn new() -> Self {
        Self {
            cool_down_started: None,
            attempts: VecDeque::new(),
        }
    }

    /// Starts the cool-down: a device was just lost at `now`.
    pub(super) const fn record_loss(&mut self, now: Instant) {
        self.cool_down_started = Some(now);
    }

    /// Whether a re-acquisition may start at `now`: a loss is on record, its cool-down
    /// has elapsed, and fewer than [`MAX_ATTEMPTS_PER_WINDOW`] attempts began within the
    /// last [`WINDOW`]. Also drops attempts that have aged out of the window.
    pub(super) fn may_attempt(&mut self, now: Instant) -> bool {
        while self
            .attempts
            .front()
            .is_some_and(|&started| now.saturating_duration_since(started) >= WINDOW)
        {
            self.attempts.pop_front();
        }
        let cooled = self
            .cool_down_started
            .is_some_and(|started| now.saturating_duration_since(started) >= COOL_DOWN);
        cooled && self.attempts.len() < MAX_ATTEMPTS_PER_WINDOW
    }

    /// Records that an attempt began at `now`. Restarts the cool-down, so a failed
    /// attempt is followed by another full [`COOL_DOWN`].
    pub(super) fn record_attempt(&mut self, now: Instant) {
        self.attempts.push_back(now);
        self.cool_down_started = Some(now);
    }

    /// Records that the device is usable again: no cool-down is pending. The attempt
    /// history stays, so a device that keeps dying right after recovering still runs into
    /// [`MAX_ATTEMPTS_PER_WINDOW`].
    pub(super) const fn record_recovered(&mut self) {
        self.cool_down_started = None;
    }
}
