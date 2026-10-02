//! Sizing a remote chunk from a measured throughput ([`remote_chunk_samples`], held to
//! what one request may carry by [`remote_request_samples`]), the pure
//! throughput-from-elapsed-time arithmetic ([`remote_marginal_rate`]), which span of a
//! finished request that throughput is measured over ([`remote_request_rate`]), how
//! much of an assigned chunk never got traced ([`shortfall`]), and the calibration
//! probe that seeds the very first chunk's rate estimate ([`calibrate_remote_rate`]/
//! [`RemoteCalibration`]). See this group's own `mod.rs` doc comment.

use super::{
    capability::{REMOTE_MIN_SPP, RemoteCapability},
    dispatch::run_remote_batch,
};
use glam::Vec3;
use indicatrix_dispatch::DEFAULT_MAX_CHUNK_SAMPLES;
use indicatrix_net::{SceneState, client::Accumulator};
use std::{
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
    time::{Duration, Instant},
};

/// Sample count for the remote calibration probe -- see [`calibrate_remote_rate`].
/// Large enough to amortize one TLS/network round trip's fixed overhead against real
/// tracing time; small enough not to waste meaningful export time on a measurement.
pub(in crate::bridge::export_thread) const REMOTE_CALIBRATION_SAMPLES: u32 = 8;

/// Target wall-clock duration for one request to a plain remote worker -- long enough
/// that per-request handshake/transfer overhead (~2.5ms locally, more over a real
/// network) is negligible against it; short enough that a slow/loaded worker never
/// holds the tail of the export hostage for minutes while every other engine sits idle.
///
/// Sizing by TIME rather than a fixed sample count makes both ends self-correcting: a
/// slow worker's measured rate is small, so `rate * REMOTE_CHUNK_TARGET_SECS` (see
/// [`remote_chunk_samples`]) comes out small too, automatically; a fast worker gets
/// bigger chunks.
const REMOTE_CHUNK_TARGET_SECS: f64 = 22.0;

/// Target wall-clock duration for one request to a coordinator
/// ([`RemoteCapability::coordinator`]).
///
/// A coordinator cuts every request into chunks of its own (the same 22 second policy)
/// and keeps its lanes busy on them while the merged result streams back, so a long
/// request costs this side no responsiveness; the fixed cost of a request (connection,
/// scene upload, job set-up, the transfer after the last chunk) is paid once per 90
/// seconds of work instead of once per 22.
const COORDINATOR_CHUNK_TARGET_SECS: f64 = 90.0;

/// Floor for [`remote_chunk_samples`] -- reuses [`REMOTE_MIN_SPP`] rather than a second
/// independent number the two could drift apart on.
const REMOTE_CHUNK_MIN_SPP: u32 = REMOTE_MIN_SPP;

/// The most samples one request may ask for: the per-request limit an
/// `indicatrix-worker` (a plain worker or a coordinator) validates every `RenderRequest`
/// against, answering anything above it with a validation error.
const REMOTE_MAX_REQUEST_SAMPLES: u32 = DEFAULT_MAX_CHUNK_SAMPLES;

/// How many samples one remote request should cover to take about the target time for
/// its peer ([`REMOTE_CHUNK_TARGET_SECS`] for a plain worker,
/// [`COORDINATOR_CHUNK_TARGET_SECS`] when `coordinator`), given the CURRENT best
/// estimate of remote's throughput.
///
/// Never below [`REMOTE_CHUNK_MIN_SPP`]: a non-finite, zero, or implausibly small rate
/// must not collapse the chunk size to something per-request overhead would dominate.
/// Not held to the per-request limit and not clamped to the budget remaining: callers
/// size a real request with [`remote_request_samples`], and `SampleCursor::claim`
/// clamps that to what is left.
#[must_use]
pub(in crate::bridge::export_thread) fn remote_chunk_samples(
    rate_samples_per_sec: f64,
    coordinator: bool,
) -> u32 {
    let target_secs = if coordinator {
        COORDINATOR_CHUNK_TARGET_SECS
    } else {
        REMOTE_CHUNK_TARGET_SECS
    };
    let target = rate_samples_per_sec * target_secs;
    if target.is_finite() && target >= f64::from(REMOTE_CHUNK_MIN_SPP) {
        target.round() as u32
    } else {
        REMOTE_CHUNK_MIN_SPP
    }
}

/// [`remote_chunk_samples`], held to [`REMOTE_MAX_REQUEST_SAMPLES`]: the size the remote
/// lane claims for its next request. A peer fast enough that the target time would
/// ask for more than one request may carry gets full-size requests back to back
/// instead of one the peer refuses.
#[must_use]
pub(in crate::bridge::export_thread) fn remote_request_samples(
    rate_samples_per_sec: f64,
    coordinator: bool,
) -> u32 {
    remote_chunk_samples(rate_samples_per_sec, coordinator).min(REMOTE_MAX_REQUEST_SAMPLES)
}

/// How many of a remote dispatch's `assigned` samples never got traced -- `0` when it
/// finished cleanly, positive if the connection dropped, the worker errored, or it was
/// cancelled partway. Only the shortfall is ever retraced locally, since every sample
/// the worker did complete is already valid, already-summed radiance. `saturating_sub`
/// as defense-in-depth against `completed` somehow exceeding `assigned`.
#[must_use]
pub(in crate::bridge::export_thread) const fn shortfall(assigned: u32, completed: u32) -> u32 {
    assigned.saturating_sub(completed)
}

/// The pure arithmetic behind [`remote_request_rate`]: `delta_samples` traced over
/// `elapsed` wall time. Takes a plain `Duration` rather than a pair of `Instant`s so
/// it's directly unit-testable.
///
/// `None` when `delta_samples` is `0` (no progress update arrived, or the span was
/// instantaneous) or `elapsed` is zero/negative.
#[must_use]
pub(in crate::bridge::export_thread) fn remote_marginal_rate(
    delta_samples: u32,
    elapsed: Duration,
) -> Option<f64> {
    if delta_samples == 0 {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some(f64::from(delta_samples) / secs)
}

/// The throughput one finished request measured, in samples/sec -- the figure
/// `super::dispatch::run_remote_batch` returns and [`calibrate_remote_rate`] and every
/// later chunk size their next request from.
///
/// `samples_done` over `request_elapsed`, the WHOLE request from sending it to its
/// `DONE`, is the rate of a coordinator peer and the fallback for a plain worker. It
/// counts every fixed cost of a request (connection, scene upload, job set-up, the
/// transfer after the last chunk), which is what a chunk sized to a target duration has
/// to be measured against to take that long. A window that opens at the first progress
/// report and closes at `DONE` would measure neither rendering nor the whole request:
/// against a coordinator, progress advances only when one of its lanes' chunks merges,
/// so such a window opens after the first chunk's work is done and closes after the
/// last chunk's transfer.
///
/// A plain worker with at least two distinct progress reports is measured from them
/// instead: `advance` is the samples gained and the time elapsed between its first and
/// its last report, which is rendering plus the ongoing per-`FRAME` transfer, without
/// the one-time setup and the tail. A worker that reported progress once (`advance` is
/// `None`) gets the whole-request figure.
#[must_use]
pub(in crate::bridge::export_thread) fn remote_request_rate(
    coordinator: bool,
    samples_done: u32,
    request_elapsed: Duration,
    advance: Option<(u32, Duration)>,
) -> Option<f64> {
    let whole_request = || remote_marginal_rate(samples_done, request_elapsed);
    if coordinator {
        return whole_request();
    }
    advance
        .and_then(|(samples, span)| remote_marginal_rate(samples, span))
        .or_else(whole_request)
}

/// The outcome of [`calibrate_remote_rate`]'s probe -- carries WHY a probe didn't
/// produce a trustworthy rate, not just that it didn't, so `run_export` can show the
/// user what happened instead of silently dropping remote. `run_export` matches on
/// every variant: `Ready` seeds the first chunk; `Failed`/`Short` become a
/// `pending_note` (or, under `ComputeTarget::RemoteOnly`, an outright
/// `ExportOutcome::Failed`); `Cancelled` gets no note since `run_export`'s own `cancel`
/// check bails out immediately after.
#[derive(Debug)]
pub(in crate::bridge::export_thread) enum RemoteCalibration {
    /// The probe succeeded and measured a usable throughput estimate, in samples/sec,
    /// for `super::dispatch::run_remote_lane` to size its first chunk from.
    Ready(f64),
    /// The export was cancelled during the probe. No user-facing note is warranted --
    /// `run_export` bails out via its own `cancel` check immediately after.
    Cancelled,
    /// The probe request itself failed outright -- a connection/protocol error, or a
    /// worker-side `ERROR`. Carries the worker's/transport's message text VERBATIM.
    Failed(String),
    /// The probe connected but ended before tracing a single sample -- nothing to
    /// measure a rate from, and no evidence the worker can deliver at this resolution.
    Short { done: u32, expected: u32 },
    /// The probe ended early but DID complete `done` of `expected` samples, enough to
    /// measure `rate` (samples/sec) from. Remote is still worth using -- the lane's own
    /// per-chunk failure handling (`super::dispatch::run_remote_lane`) copes with a
    /// worker that keeps dropping out -- but the user is told the probe was cut short.
    Partial { rate: f64, done: u32, expected: u32 },
}

/// Times a [`REMOTE_CALIBRATION_SAMPLES`]-sample probe against the remote worker and
/// returns its measured throughput in samples/sec -- the initial rate estimate
/// `super::dispatch::run_remote_lane` sizes its first chunk from, before any chunk of
/// its own reports a fresher measurement. Anything other than
/// [`RemoteCalibration::Ready`] means "decline remote for this export": the probe
/// failed, was cancelled, or completed short, with no trustworthy rate to size even a
/// first chunk from.
///
/// No local probe is timed alongside this one: remote runs as a continuously
/// re-claiming lane drawing from a shared `SampleCursor` rather than a fixed split, so
/// only an initial throughput estimate is needed (`batch::hybrid_batch` re-measures its
/// own CPU/GPU split independently every batch).
///
/// The probe's radiance is folded into `accum` for real (never `gpu_accum` -- remote's
/// contribution always lands in the CPU-side buffer), and `*samples_done` is advanced
/// past it.
///
/// # What the rate measurement covers
///
/// The rate is `super::dispatch::run_remote_batch`'s measured `rate_samples_per_sec`
/// when one came back ([`remote_request_rate`]), falling back to plain `probe /
/// elapsed` otherwise. Against a coordinator, and against a worker that reported
/// progress only once, that is the whole request, connection setup, upload and result
/// transfer included: a probe of a few samples reads low, so the first chunk is small,
/// and the next chunk is sized from its own, longer measurement. Against a worker with
/// two or more progress reports it is the span between the first and the last report,
/// which excludes the one-time costs but includes the ongoing per-`FRAME` transfer.
pub(in crate::bridge::export_thread) fn calibrate_remote_rate(
    capability: &RemoteCapability,
    scene_state: &SceneState,
    width: u32,
    height: u32,
    samples_done: &mut u32,
    accum: &mut [Vec3],
    cancel: &AtomicBool,
) -> RemoteCalibration {
    let probe = REMOTE_CALIBRATION_SAMPLES;
    let remote_accumulator = Arc::new(Mutex::new(Accumulator::new(width, height)));
    let start_sample = *samples_done;

    let timer = Instant::now();
    let (done, cancelled, error, measured_rate) = run_remote_batch(
        capability,
        scene_state.clone(),
        start_sample,
        probe,
        width,
        height,
        &remote_accumulator,
        cancel,
    );
    let elapsed = timer.elapsed().as_secs_f64().max(1e-9);

    {
        let acc = remote_accumulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (dst, src) in accum.iter_mut().zip(acc.buffer()) {
            *dst += *src;
        }
    }
    *samples_done += done;

    if cancelled {
        return RemoteCalibration::Cancelled;
    }
    if let Some(message) = error {
        return RemoteCalibration::Failed(message);
    }
    if done == 0 {
        return RemoteCalibration::Short {
            done,
            expected: probe,
        };
    }

    let rate = measured_rate.unwrap_or_else(|| f64::from(done) / elapsed);
    if done < probe {
        return RemoteCalibration::Partial {
            rate,
            done,
            expected: probe,
        };
    }
    RemoteCalibration::Ready(rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::export_thread::sample_cursor::SampleCursor;

    // ---- Chunk sizing -------------------------------------------------------------

    #[test]
    fn remote_chunk_samples_targets_rate_times_the_target_duration() {
        assert_eq!(remote_chunk_samples(1000.0, false), 22_000);
    }

    #[test]
    fn remote_chunk_samples_targets_ninety_seconds_for_a_coordinator() {
        assert_eq!(remote_chunk_samples(1000.0, true), 90_000);
        assert_eq!(remote_chunk_samples(100.0, true), 9_000);
    }

    #[test]
    fn remote_chunk_samples_never_drops_below_the_floor_for_a_tiny_rate() {
        for coordinator in [false, true] {
            assert_eq!(
                remote_chunk_samples(0.01, coordinator),
                REMOTE_CHUNK_MIN_SPP
            );
            assert_eq!(remote_chunk_samples(0.0, coordinator), REMOTE_CHUNK_MIN_SPP);
        }
    }

    #[test]
    fn remote_chunk_samples_falls_back_to_the_floor_for_non_finite_rates() {
        for coordinator in [false, true] {
            assert_eq!(
                remote_chunk_samples(f64::NAN, coordinator),
                REMOTE_CHUNK_MIN_SPP
            );
            assert_eq!(
                remote_chunk_samples(f64::INFINITY, coordinator),
                REMOTE_CHUNK_MIN_SPP
            );
            assert_eq!(
                remote_chunk_samples(-100.0, coordinator),
                REMOTE_CHUNK_MIN_SPP
            );
        }
    }

    #[test]
    fn remote_chunk_samples_scales_up_for_a_faster_measured_rate() {
        let slow = remote_chunk_samples(500.0, false);
        let fast = remote_chunk_samples(1000.0, false);
        assert!(fast > slow);
        assert!((2.0f64.mul_add(-f64::from(slow), f64::from(fast))).abs() < 2.0);
    }

    // `SampleCursor` guarantees disjointness (see its own test coverage); this proves
    // `remote_request_samples`' output is a sane, positive claim size to drive it with.
    #[test]
    fn remote_request_samples_output_is_always_a_usable_positive_claim_size() {
        for total in [32u32, 100, 1000, 32768] {
            for rate in [0.0, 0.01, 50.0, 1000.0, 1_000_000.0] {
                for coordinator in [false, true] {
                    let cursor = SampleCursor::new(0, total);
                    let want = remote_request_samples(rate, coordinator);
                    assert!(want > 0, "a chunk size of 0 would never advance the cursor");
                    // `SampleCursor::claim` clamps whatever is asked for to the actual budget.
                    if let Some((start, count)) = cursor.claim(want) {
                        assert_eq!(start, 0);
                        assert!(count <= total);
                    }
                }
            }
        }
    }

    // The peer answers a request above its per-request limit with a validation error,
    // which the lane would count as a failed chunk: 90 s of a fast coordinator is more
    // than one request may carry.
    #[test]
    fn remote_request_samples_never_exceeds_what_one_request_may_carry() {
        assert_eq!(
            remote_request_samples(1000.0, true),
            REMOTE_MAX_REQUEST_SAMPLES
        );
        assert_eq!(
            remote_request_samples(1_000_000.0, false),
            REMOTE_MAX_REQUEST_SAMPLES
        );
        assert_eq!(remote_request_samples(1000.0, false), 22_000);
        assert_eq!(remote_request_samples(100.0, true), 9_000);
    }

    // ---- Request rate --------------------------------------------------------------

    #[test]
    fn remote_request_rate_with_one_coarse_progress_jump_is_the_whole_request_value() {
        // 1000 samples in a 25 s request whose only progress report landed at 20 s: no
        // span to measure, so a worker gets the whole request, and so does a coordinator.
        let whole = Some(1000.0 / 25.0);
        assert_eq!(
            remote_request_rate(false, 1000, Duration::from_secs(25), None),
            whole
        );
        assert_eq!(
            remote_request_rate(true, 1000, Duration::from_secs(25), None),
            whole
        );
    }

    #[test]
    fn remote_request_rate_of_a_worker_with_two_progress_reports_is_their_span() {
        // 200 samples advanced over 2 s of a 25 s request: the worker renders at
        // 100/s however long its setup and tail took.
        assert_eq!(
            remote_request_rate(
                false,
                1000,
                Duration::from_secs(25),
                Some((200, Duration::from_secs(2)))
            ),
            Some(100.0)
        );
    }

    #[test]
    fn remote_request_rate_of_a_coordinator_ignores_its_progress_span() {
        assert_eq!(
            remote_request_rate(
                true,
                1000,
                Duration::from_secs(25),
                Some((200, Duration::from_secs(2)))
            ),
            Some(40.0)
        );
    }

    #[test]
    fn remote_request_rate_falls_back_to_the_whole_request_for_an_instantaneous_span() {
        assert_eq!(
            remote_request_rate(
                false,
                1000,
                Duration::from_secs(25),
                Some((200, Duration::ZERO))
            ),
            Some(40.0)
        );
    }

    #[test]
    fn remote_request_rate_is_none_when_no_sample_was_traced() {
        assert_eq!(
            remote_request_rate(false, 0, Duration::from_secs(25), None),
            None
        );
        assert_eq!(
            remote_request_rate(true, 0, Duration::from_secs(25), None),
            None
        );
    }

    // ---- Mid-export worker failure -------------------------------------------------

    #[test]
    fn shortfall_is_zero_when_remote_finished_everything_it_was_assigned() {
        assert_eq!(shortfall(1000, 1000), 0);
    }

    #[test]
    fn shortfall_is_the_unfinished_remainder_of_a_partial_completion() {
        // Only the unfinished tail is retraced, never the whole range (which would
        // double-count samples the worker already completed).
        assert_eq!(shortfall(1000, 400), 600);
    }

    #[test]
    fn shortfall_is_the_full_range_when_remote_completed_nothing() {
        assert_eq!(shortfall(1000, 0), 1000);
    }

    #[test]
    fn shortfall_never_underflows_even_if_completed_somehow_exceeded_assigned() {
        assert_eq!(shortfall(100, 150), 0);
    }

    #[test]
    fn remote_marginal_rate_divides_samples_by_elapsed_seconds() {
        assert_eq!(
            remote_marginal_rate(100, Duration::from_secs(2)),
            Some(50.0)
        );
    }

    #[test]
    fn remote_marginal_rate_is_none_when_nothing_was_measured() {
        assert_eq!(remote_marginal_rate(0, Duration::from_secs(1)), None);
    }

    #[test]
    fn remote_marginal_rate_is_none_for_a_non_positive_elapsed_duration() {
        assert_eq!(remote_marginal_rate(100, Duration::ZERO), None);
    }
}
