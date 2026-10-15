//! The producer stall watchdog the emitter loop consults every iteration.

use std::time::{Duration, Instant};

/// Watches the producer's `samples_done` against [`super::StreamSpec::stall_timeout`].
///
/// The emitter's own `PROGRESS` heartbeat says nothing about the producer, so without
/// this a wedged tracer keeps every client's silence-based liveness deadline satisfied
/// for as long as the connection lives.
pub(super) struct StallWatch {
    timeout: Option<Duration>,
    samples_done: u32,
    since: Instant,
}

impl StallWatch {
    /// A watch that starts its window now; `None` never reports a stall.
    pub(super) fn new(timeout: Option<Duration>) -> Self {
        Self {
            timeout,
            samples_done: 0,
            since: Instant::now(),
        }
    }

    /// Records the latest `samples_done` (any change restarts the window) and returns
    /// the window once the producer has gone longer than it without a new sample.
    pub(super) fn observe(&mut self, samples_done: u32) -> Option<Duration> {
        if samples_done != self.samples_done {
            self.samples_done = samples_done;
            self.since = Instant::now();
        }
        self.timeout.filter(|window| self.since.elapsed() > *window)
    }
}
