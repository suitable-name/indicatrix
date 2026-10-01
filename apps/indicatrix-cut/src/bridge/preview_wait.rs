//! The pure deadline logic behind `bridge::preview_render::render_view_remote_checked`:
//! how a catalogue-preview batch decides that a remote request has stopped being worth
//! waiting for, and the typed reason it reports when it gives up.
//!
//! The transport's own liveness rule is silence-based (`connection::LIVENESS_TIMEOUT`),
//! and a coordinator emits a `PROGRESS` heartbeat every couple of seconds whether or not
//! its tracer advances -- so a coordinator that wedges or crawls while heartbeating never
//! looks silent. The only signal that separates "slow but working" from "wedged" is the
//! reported `samples_done` count actually moving, which is what [`ProgressWatch`] tracks.
//! No sockets and no UI live here so the rule is testable with a synthetic clock.

use std::{
    fmt,
    time::{Duration, Instant},
};

/// How long the reported `samples_done` may stay put before a remote preview request is
/// abandoned. A 160x160 preview finishes in about a second, so a minute without a single
/// new sample is a wedge, not a slow render; heartbeats alone do not reset it.
pub const PREVIEW_PROGRESS_STALL: Duration = Duration::from_secs(60);

/// Hard cap on one remote preview request, however steadily it reports progress -- the
/// backstop for a remote that advances so slowly it is worse than rendering locally.
pub const PREVIEW_REMOTE_MAX_WALL: Duration = Duration::from_secs(300);

/// How long the caller waits for the remote to acknowledge a `CANCEL` (any terminal
/// update) before it stops waiting and reports the shortfall anyway. A wedged coordinator
/// never answers, and the batch must not hang behind it.
pub const PREVIEW_CANCEL_ACK_WAIT: Duration = Duration::from_secs(10);

/// Why a remote preview render did not produce an image. Carried to the batch's remote
/// lane so its failure policy (backoff, toast, local retry) and the log can say what
/// actually happened instead of collapsing everything into `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteShortfall {
    /// The attempt failed: connection, handshake, a transport error mid-stream, or a
    /// server `ERROR`. Holds the message the transport reported.
    Failed(String),
    /// The server is a plain worker that does not implement what was asked.
    Unsupported(String),
    /// The request ended cancelled -- by the batch's own Cancel, not by a deadline.
    Cancelled,
    /// The reported `samples_done` did not advance for the stall window.
    Stalled {
        /// The last `samples_done` the remote reported.
        samples_done: u32,
        /// How long that count had been unchanged.
        stalled_for: Duration,
    },
    /// The request outlived the wall-clock cap, even if it kept reporting progress.
    WallClock {
        /// The last `samples_done` the remote reported.
        samples_done: u32,
        /// Time since the request was issued.
        elapsed: Duration,
    },
    /// The request finished, but with fewer samples than the preview asked for.
    ShortSamples {
        /// Samples the accumulator holds.
        done: u32,
        /// Samples the preview asked for.
        wanted: u32,
    },
    /// The update channel closed without a terminal update -- the one-shot thread died.
    Disconnected,
}

impl fmt::Display for RemoteShortfall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(message) => write!(f, "remote render failed: {message}"),
            Self::Unsupported(message) => {
                write!(f, "remote does not support this request: {message}")
            }
            Self::Cancelled => write!(f, "remote render was cancelled"),
            Self::Stalled {
                samples_done,
                stalled_for,
            } => write!(
                f,
                "remote render stalled at {samples_done} sample(s) for {stalled_for:.0?}"
            ),
            Self::WallClock {
                samples_done,
                elapsed,
            } => write!(
                f,
                "remote render exceeded its time limit ({elapsed:.0?}, {samples_done} sample(s) done)"
            ),
            Self::ShortSamples { done, wanted } => {
                write!(f, "remote render returned {done} of {wanted} sample(s)")
            }
            Self::Disconnected => write!(f, "remote render thread ended without a result"),
        }
    }
}

/// Tracks one remote request's progress against a stall window and a wall-clock cap.
/// Times are passed in, never read, so tests drive it with a synthetic clock.
#[derive(Debug, Clone, Copy)]
pub struct ProgressWatch {
    started: Instant,
    last_advance: Instant,
    samples_done: u32,
}

impl ProgressWatch {
    /// Starts watching a request issued at `now`; both clocks begin here.
    #[must_use]
    pub const fn new(now: Instant) -> Self {
        Self {
            started: now,
            last_advance: now,
            samples_done: 0,
        }
    }

    /// Records a `samples_done` report; only a strictly larger value resets the stall
    /// clock. A repeated count is exactly what a heartbeating-but-wedged remote sends.
    pub const fn observe(&mut self, samples_done: u32, now: Instant) {
        if samples_done > self.samples_done {
            self.samples_done = samples_done;
            self.last_advance = now;
        }
    }

    /// `Some(shortfall)` once `now` is past the wall deadline or the stall deadline
    /// (the wall cap wins when both apply); `None` while both still have room. The
    /// bounds are strict: exactly at a limit is still within it.
    #[must_use]
    pub fn verdict(
        &self,
        now: Instant,
        stall: Duration,
        wall: Duration,
    ) -> Option<RemoteShortfall> {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed > wall {
            return Some(RemoteShortfall::WallClock {
                samples_done: self.samples_done,
                elapsed,
            });
        }
        let stalled_for = now.saturating_duration_since(self.last_advance);
        (stalled_for > stall).then_some(RemoteShortfall::Stalled {
            samples_done: self.samples_done,
            stalled_for,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALL: Duration = Duration::from_secs(60);
    const WALL: Duration = Duration::from_secs(300);

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_larger_samples_done_resets_the_stall_clock() {
        let t0 = Instant::now();
        let mut watch = ProgressWatch::new(t0);
        watch.observe(10, t0 + secs(50));
        // 50 s after t0 the count advanced, so 100 s in is only 50 s stalled.
        assert_eq!(watch.verdict(t0 + secs(100), STALL, WALL), None);
    }

    #[test]
    fn an_equal_report_does_not_reset_the_stall_clock() {
        let t0 = Instant::now();
        let mut watch = ProgressWatch::new(t0);
        watch.observe(10, t0 + secs(5));
        // A heartbeat repeating the same count must not buy more time.
        watch.observe(10, t0 + secs(60));
        watch.observe(3, t0 + secs(64));
        assert_eq!(
            watch.verdict(t0 + secs(66), STALL, WALL),
            Some(RemoteShortfall::Stalled {
                samples_done: 10,
                stalled_for: secs(61),
            })
        );
    }

    #[test]
    fn no_advance_past_the_stall_window_is_stalled() {
        let t0 = Instant::now();
        let watch = ProgressWatch::new(t0);
        assert_eq!(
            watch.verdict(t0 + secs(61), STALL, WALL),
            Some(RemoteShortfall::Stalled {
                samples_done: 0,
                stalled_for: secs(61),
            })
        );
    }

    #[test]
    fn total_time_past_the_wall_cap_is_wall_clock_even_with_steady_progress() {
        let t0 = Instant::now();
        let mut watch = ProgressWatch::new(t0);
        for step in 1..=31_u32 {
            watch.observe(step, t0 + secs(u64::from(step) * 10));
        }
        assert_eq!(
            watch.verdict(t0 + secs(310), STALL, WALL),
            Some(RemoteShortfall::WallClock {
                samples_done: 31,
                elapsed: secs(310),
            })
        );
    }

    #[test]
    fn verdict_is_none_just_under_either_bound() {
        let t0 = Instant::now();
        let mut watch = ProgressWatch::new(t0);
        assert_eq!(watch.verdict(t0 + secs(60), STALL, WALL), None);
        // Progress at 250 s keeps the stall clock fresh, leaving only the wall bound.
        watch.observe(1, t0 + secs(250));
        assert_eq!(watch.verdict(t0 + secs(300), STALL, WALL), None);
        assert!(watch.verdict(t0 + secs(301), STALL, WALL).is_some());
    }

    #[test]
    fn display_gives_one_human_line_per_variant() {
        let all = [
            RemoteShortfall::Failed("boom".into()),
            RemoteShortfall::Unsupported("nope".into()),
            RemoteShortfall::Cancelled,
            RemoteShortfall::Stalled {
                samples_done: 4,
                stalled_for: secs(61),
            },
            RemoteShortfall::WallClock {
                samples_done: 4,
                elapsed: secs(301),
            },
            RemoteShortfall::ShortSamples { done: 1, wanted: 2 },
            RemoteShortfall::Disconnected,
        ];
        for shortfall in &all {
            let line = shortfall.to_string();
            assert!(!line.is_empty() && !line.contains('\n'), "{line:?}");
        }
    }
}
