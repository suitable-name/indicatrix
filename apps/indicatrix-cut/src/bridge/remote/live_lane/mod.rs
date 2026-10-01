//! [`LiveLane`]: the live viewport's remote side for one [`LiveEpoch`] -- which chunk
//! to request next, how big, and what to do when one finishes, fails or is abandoned.
//!
//! Pure bookkeeping: no socket, no thread, no Slint. `gui::remote::orchestrator` owns
//! one `LiveLane` per settle and turns each [`ChunkRequest`] it hands out into one
//! `RenderRequest{first_sample, samples}` on the persistent worker connection, feeding
//! the connection's `Done`/`Failed` updates back in. That split is what lets the
//! drag-cancel, failure-reclaim and exact-count behaviour be unit-tested here without a
//! window or a worker (see `tests`).
//!
//! # Chunking
//!
//! Remote no longer reserves a fixed `[0, N)` up front. It claims chunks from the
//! epoch's shared cursor, each sized to about [`LIVE_CHUNK_TARGET_SECS`] of the
//! worker's MEASURED rate ([`live_chunk_samples`]) -- far smaller than the export's
//! ~22 s chunks, so cancelling on a drag wastes at most a second or two of worker time.
//! The next chunk is requested when the previous one's `Done` arrives; a slow worker
//! simply claims less, a fast one more, and nothing is claimed past the epoch's target.
//!
//! # Failure
//!
//! A failed chunk (worker error, dropped connection, liveness timeout) keeps its valid
//! prefix (the worker's `samples_done` is always a prefix of the requested range) and
//! returns the untraced remainder: to the LOCAL tracer through the cursor's retry pile
//! when a local lane exists (`LiveComputeTarget::Both`), or to this lane's own retry
//! queue otherwise (`RemoteOnly`, where local is suspended). After
//! [`MAX_CONSECUTIVE_CHUNK_FAILURES`] failures in a row the lane gives up for this
//! epoch; the next settle starts a fresh epoch and tries again.

#[cfg(test)]
mod tests;

use crate::bridge::sample_cursor::{ChunkEnd, LiveEpoch};
use indicatrix_net::client::Accumulator;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Target wall-clock length of one live remote chunk at the worker's measured rate.
pub const LIVE_CHUNK_TARGET_SECS: f64 = 1.5;
/// The first chunk of an epoch, before any rate has been measured on this lane: small,
/// so a slow worker cannot hold a big slice hostage before its rate is known.
pub const LIVE_FIRST_CHUNK_SAMPLES: u32 = 8;
/// Floor for a rate-sized chunk -- per-request overhead dominates below this.
pub const LIVE_CHUNK_MIN_SAMPLES: u32 = 2;
/// Ceiling for one chunk: the worker's own per-request limit
/// (`indicatrix-worker`'s `MAX_SAMPLES_PER_REQUEST`, not advertised in `Welcome`), the
/// one definition `indicatrix_dispatch` owns.
pub use indicatrix_dispatch::DEFAULT_MAX_CHUNK_SAMPLES as LIVE_CHUNK_MAX_SAMPLES;
/// Consecutive chunk failures after which the lane stops claiming for this epoch.
pub const MAX_CONSECUTIVE_CHUNK_FAILURES: u32 = 2;

/// How many samples the next chunk should cover given the lane's current rate estimate
/// (samples/second), `None` before the first measurement.
#[must_use]
pub fn live_chunk_samples(rate_samples_per_sec: Option<f64>) -> u32 {
    let Some(rate) = rate_samples_per_sec else {
        return LIVE_FIRST_CHUNK_SAMPLES;
    };
    let target = rate * LIVE_CHUNK_TARGET_SECS;
    if !target.is_finite() || target < f64::from(LIVE_CHUNK_MIN_SAMPLES) {
        return LIVE_CHUNK_MIN_SAMPLES;
    }
    if target > f64::from(LIVE_CHUNK_MAX_SAMPLES) {
        return LIVE_CHUNK_MAX_SAMPLES;
    }
    target.round() as u32
}

/// One chunk to send: a `RenderRequest` covering `[first_sample, first_sample +
/// samples)` whose replies must be applied into `accumulator` (already started for
/// `request_id` with that expected range).
pub struct ChunkRequest {
    /// The request id the chunk was claimed under.
    pub request_id: u32,
    /// First absolute sample index.
    pub first_sample: u32,
    /// Number of samples.
    pub samples: u32,
    /// The chunk's own fresh accumulator, also registered as the epoch's in-flight
    /// chunk so the display can fold in its streamed progress.
    pub accumulator: Arc<Mutex<Accumulator>>,
}

/// What the caller should do after [`LiveLane::chunk_failed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkVerdict {
    /// Keep going: request the next chunk.
    Continue,
    /// Too many consecutive failures -- stop remote work for this epoch (and show one
    /// status note).
    GaveUp,
}

/// The lane's life cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneState {
    /// Claiming and dispatching chunks.
    Active,
    /// Nothing left to claim; the last chunk (if any) finished.
    Exhausted,
    /// Stopped after [`MAX_CONSECUTIVE_CHUNK_FAILURES`] failures in a row.
    GaveUp,
    /// Dropped by a drag/scene change/release -- nothing may be merged any more.
    Abandoned,
}

/// Bookkeeping for the chunk currently on the wire.
struct InFlight {
    request_id: u32,
    dispatched_at: Instant,
    /// `(when, samples_done)` of the first progress report with real progress, the
    /// start point of the steady-state rate measurement (excludes connect/handshake).
    first_progress: Option<(Instant, u32)>,
}

/// See the module doc comment.
pub struct LiveLane {
    epoch: Arc<LiveEpoch>,
    /// `true` for `LiveComputeTarget::Both`: a failed chunk's remainder goes to the
    /// local tracer. `false` for `RemoteOnly`: it goes to [`Self::own_retry`].
    local_lane: bool,
    /// "Final picture" live transfer: the remote sends finished
    /// display frames, never radiance, so the epoch is ONE request over the whole
    /// budget (display frames of separate requests would each show only their own
    /// samples) and nothing is merged -- see [`Self::display_only`].
    display_only: bool,
    rate: Option<f64>,
    consecutive_failures: u32,
    in_flight: Option<InFlight>,
    own_retry: VecDeque<(u32, u32)>,
    state: LaneState,
}

impl LiveLane {
    /// A lane for `epoch`. `local_lane` says whether a local tracer shares the epoch
    /// (see [`Self::local_lane`]'s field doc).
    #[must_use]
    pub const fn new(epoch: Arc<LiveEpoch>, local_lane: bool) -> Self {
        Self {
            epoch,
            local_lane,
            display_only: false,
            rate: None,
            consecutive_failures: 0,
            in_flight: None,
            own_retry: VecDeque::new(),
            state: LaneState::Active,
        }
    }

    /// A lane for a final-picture epoch: no local lane shares it,
    /// its one chunk claims the whole budget (up to [`LIVE_CHUNK_MAX_SAMPLES`], the
    /// per-request limit -- anything beyond stays untraced, local being paused), and a
    /// finished chunk ends the lane without merging anything (the display frames are
    /// the image). A failed chunk is retried whole, like any `RemoteOnly` chunk.
    #[must_use]
    pub const fn display_only(epoch: Arc<LiveEpoch>) -> Self {
        let mut lane = Self::new(epoch, false);
        lane.display_only = true;
        lane
    }

    /// Whether this lane asks for display frames -- see [`Self::display_only`].
    #[must_use]
    pub const fn is_display_only(&self) -> bool {
        self.display_only
    }

    /// The epoch this lane contributes to.
    #[must_use]
    pub const fn epoch(&self) -> &Arc<LiveEpoch> {
        &self.epoch
    }

    /// The lane's current throughput estimate in samples/second.
    #[cfg(test)]
    #[must_use]
    pub const fn rate(&self) -> Option<f64> {
        self.rate
    }

    /// Whether the lane could dispatch a chunk right now (active, nothing in flight).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.state == LaneState::Active && self.in_flight.is_none()
    }

    /// Whether the lane has permanently stopped (exhausted, gave up or abandoned) with
    /// nothing in flight.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.state != LaneState::Active && self.in_flight.is_none()
    }

    /// Claims the next chunk under `request_id`: this lane's own retry queue first
    /// (`RemoteOnly` only), then the epoch's shared pool, sized by
    /// [`live_chunk_samples`]. Registers a fresh accumulator (with the expected range)
    /// as the epoch's in-flight chunk. `None` when the lane is not idle or nothing is
    /// left to claim (the lane then becomes exhausted).
    pub fn next_chunk(&mut self, request_id: u32, now: Instant) -> Option<ChunkRequest> {
        if !self.is_idle() {
            return None;
        }
        let want = if self.display_only {
            LIVE_CHUNK_MAX_SAMPLES
        } else {
            live_chunk_samples(self.rate)
        };
        let claimed = match self.own_retry.pop_front() {
            Some((start, count)) if count > want => {
                self.own_retry.push_front((start + want, count - want));
                Some((start, want))
            }
            Some(range) => Some(range),
            None => self.epoch.claim_remote(want),
        };
        let Some((first_sample, samples)) = claimed else {
            self.state = LaneState::Exhausted;
            return None;
        };
        let (width, height) = self.epoch.dimensions();
        let mut accumulator = Accumulator::new(width, height);
        accumulator.begin_request_for_range(request_id, first_sample, samples);
        let accumulator = Arc::new(Mutex::new(accumulator));
        self.epoch
            .begin_chunk(first_sample, samples, Arc::clone(&accumulator));
        self.in_flight = Some(InFlight {
            request_id,
            dispatched_at: now,
            first_progress: None,
        });
        Some(ChunkRequest {
            request_id,
            first_sample,
            samples,
            accumulator,
        })
    }

    /// Records a `Frame`/`Progress` report for the in-flight chunk (rate measurement
    /// only). Ignored for any other request id.
    pub const fn observe_progress(&mut self, request_id: u32, samples_done: u32, now: Instant) {
        if let Some(f) = self.in_flight.as_mut()
            && f.request_id == request_id
            && f.first_progress.is_none()
            && samples_done > 0
        {
            f.first_progress = Some((now, samples_done));
        }
    }

    /// The in-flight chunk `request_id` finished (`Done`, not cancelled): merges it
    /// into the epoch, refreshes the rate estimate and resets the failure streak.
    /// A short `Done` (fewer samples than assigned) still hands its remainder back like
    /// a failure would, without counting as one. `None` if `request_id` is not the
    /// chunk in flight.
    pub fn chunk_done(&mut self, request_id: u32, now: Instant) -> Option<ChunkEnd> {
        let flight = self.take_in_flight(request_id)?;
        let end = self.epoch.finish_chunk()?;
        if self.display_only {
            // Nothing was summed (display frames carry no radiance): the request is
            // complete as far as this lane is concerned, and it never claims again.
            self.consecutive_failures = 0;
            self.state = LaneState::Exhausted;
            return Some(ChunkEnd {
                done: end.count,
                ..end
            });
        }
        if let Some(measured) = measured_rate(&flight, end.done, now) {
            // A light exponential blend: damps one noisy chunk, still tracks a real
            // change (another job starting on the worker) within a couple of chunks.
            let blended = self
                .rate
                .map_or(measured, |prev| prev.mul_add(0.5, measured * 0.5));
            self.rate = Some(blended);
        }
        if end.done == end.count {
            self.consecutive_failures = 0;
        } else {
            self.hand_back(end);
        }
        Some(end)
    }

    /// The in-flight chunk `request_id` was cut off by a live-remote suspension (live
    /// rendering paused, or an export/batch job starting -- see
    /// `gui::remote::orchestrator::tick::poll::suspend_live_remote`), confirmed by the
    /// worker's `DONE { cancelled: true }`: merges the chunk's valid prefix into the
    /// epoch and hands back any untraced remainder exactly like [`Self::chunk_done`],
    /// but -- unlike `chunk_done` -- touches neither [`Self::rate`] nor the
    /// consecutive-failure streak, since a worker cut off mid-chunk by a local pause
    /// says nothing about its throughput or reliability. The lane is left `Active` and
    /// idle either way (never `Exhausted`/`GaveUp` from this alone), ready for
    /// `resume_idle_lane` to continue it once allowed again. Not expected to be called
    /// for a [`Self::display_only`] lane -- that kind is abandoned outright on
    /// suspension instead, since its one request cannot be resumed mid-stream. `None`
    /// if `request_id` is not the chunk in flight (already superseded by an
    /// `abandon`).
    pub fn chunk_paused(&mut self, request_id: u32) -> Option<ChunkEnd> {
        self.take_in_flight(request_id)?;
        let end = self.epoch.finish_chunk()?;
        if end.done < end.count {
            self.hand_back(end);
        }
        Some(end)
    }

    /// The in-flight chunk `request_id` failed (worker error, transport error or
    /// liveness timeout): keeps its valid prefix, hands the remainder back (see the
    /// module doc comment) and decides whether to continue. `None` if `request_id` is
    /// not the chunk in flight.
    pub fn chunk_failed(&mut self, request_id: u32) -> Option<ChunkVerdict> {
        self.take_in_flight(request_id)?;
        if let Some(end) = self.epoch.finish_chunk() {
            self.hand_back(end);
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures >= MAX_CONSECUTIVE_CHUNK_FAILURES {
            self.state = LaneState::GaveUp;
            return Some(ChunkVerdict::GaveUp);
        }
        Some(ChunkVerdict::Continue)
    }

    /// Stops the lane for good WITHOUT merging the in-flight chunk (a drag, a scene
    /// change or any other release made it worthless). Returns the in-flight request
    /// id, which the caller should cancel on the wire.
    pub fn abandon(&mut self) -> Option<u32> {
        self.state = LaneState::Abandoned;
        self.epoch.abandon_chunk();
        self.own_retry.clear();
        self.in_flight.take().map(|f| f.request_id)
    }

    fn take_in_flight(&mut self, request_id: u32) -> Option<InFlight> {
        if self.state == LaneState::Abandoned {
            return None;
        }
        match &self.in_flight {
            Some(f) if f.request_id == request_id => self.in_flight.take(),
            _ => None,
        }
    }

    /// Returns a chunk's untraced tail to whoever can still trace it.
    fn hand_back(&mut self, end: ChunkEnd) {
        let (start, count) = end.remainder();
        if count == 0 {
            return;
        }
        if self.local_lane {
            self.epoch.return_to_local(start, count);
        } else {
            self.own_retry.push_back((start, count));
        }
    }
}

/// The chunk's steady-state throughput: from its first real progress report to `now`
/// when there is one with a usable span, else over the whole dispatch (which includes
/// connection latency and so errs slow -- the next chunk corrects it).
fn measured_rate(flight: &InFlight, done: u32, now: Instant) -> Option<f64> {
    const MIN_SPAN: Duration = Duration::from_millis(50);
    if let Some((t0, s0)) = flight.first_progress {
        let span = now.saturating_duration_since(t0);
        if done > s0 && span >= MIN_SPAN {
            return Some(f64::from(done - s0) / span.as_secs_f64());
        }
    }
    let span = now.saturating_duration_since(flight.dispatched_at);
    if done == 0 || span.is_zero() {
        return None;
    }
    Some(f64::from(done) / span.as_secs_f64())
}

/// [`LiveLane::chunk_paused`] tests -- kept inline (rather than in [`tests`], this
/// module's own separate file) purely so the live-remote-suspension fix that added
/// `chunk_paused` touches exactly one file under `bridge::remote::live_lane`.
#[cfg(test)]
mod chunk_paused_tests {
    use super::*;
    use crate::bridge::sample_cursor::tests::apply_frame;

    fn t(ms: u64) -> Instant {
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *ORIGIN.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    /// A full chunk paused after it actually finished tracing everything still merges
    /// the whole thing, hands back nothing, and reports no remainder.
    #[test]
    fn a_fully_traced_chunk_paused_at_completion_merges_everything() {
        let epoch = Arc::new(LiveEpoch::new(1, 1, 100));
        let mut lane = LiveLane::new(Arc::clone(&epoch), true);
        let chunk = lane.next_chunk(1, t(0)).unwrap();
        apply_frame(
            &chunk.accumulator,
            1,
            chunk.first_sample,
            chunk.samples,
            2.0,
        );
        let end = lane.chunk_paused(1).expect("in flight");
        assert_eq!(end.remainder(), (chunk.first_sample + chunk.samples, 0));
        assert_eq!(epoch.remote_done(), chunk.samples);
    }

    /// The common case: cancelled mid-chunk. The valid prefix is merged, the untraced
    /// remainder is handed back to the local tracer (`local_lane: true`), and -- unlike
    /// `chunk_failed` -- nothing here counts as a failure: two pauses in a row must
    /// never give up the lane.
    #[test]
    fn a_chunk_paused_mid_flight_merges_the_prefix_and_hands_back_the_remainder() {
        let epoch = Arc::new(LiveEpoch::new(1, 1, 100));
        let mut lane = LiveLane::new(Arc::clone(&epoch), true);
        let chunk = lane.next_chunk(1, t(0)).unwrap();
        assert!(chunk.samples > 1, "need room for a partial trace");
        apply_frame(&chunk.accumulator, 1, chunk.first_sample, 1, 3.0);

        let end = lane.chunk_paused(1).expect("in flight");
        assert_eq!(end.done, 1);
        assert_eq!(epoch.remote_done(), 1, "the valid prefix was merged");
        assert!(lane.is_idle(), "left Active and idle, not GaveUp/Exhausted");

        for _ in 0..=MAX_CONSECUTIVE_CHUNK_FAILURES {
            let c = lane.next_chunk(2, t(10)).expect("still claimable");
            assert!(
                lane.chunk_paused(c.request_id).is_some(),
                "repeated pauses must never exhaust the failure budget"
            );
        }
        assert!(
            lane.is_idle(),
            "chunk_paused must never drive the lane to GaveUp"
        );
    }

    /// A stale/superseded request id (already abandoned, or from a different chunk)
    /// merges nothing and reports no chunk in flight.
    #[test]
    fn a_mismatched_request_id_is_a_no_op() {
        let epoch = Arc::new(LiveEpoch::new(1, 1, 100));
        let mut lane = LiveLane::new(Arc::clone(&epoch), true);
        let chunk = lane.next_chunk(1, t(0)).unwrap();
        assert!(lane.chunk_paused(chunk.request_id + 1).is_none());
        // The real in-flight chunk is untouched -- it can still be paused correctly.
        assert!(lane.chunk_paused(chunk.request_id).is_some());
    }
}
