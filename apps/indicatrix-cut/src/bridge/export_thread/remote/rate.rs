//! Sizing a remote chunk from a measured throughput ([`remote_chunk_samples`]), the
//! pure throughput-from-elapsed-time arithmetic ([`remote_marginal_rate`]), how much of
//! an assigned chunk never got traced ([`shortfall`]), and the calibration probe that
//! seeds the very first chunk's rate estimate ([`calibrate_remote_rate`]/
//! [`RemoteCalibration`]). See this group's own `mod.rs` doc comment.

use super::{
    capability::{REMOTE_MIN_SPP, RemoteCapability},
    dispatch::run_remote_batch,
};
use glam::Vec3;
use indicatrix_net::{SceneState, client::Accumulator};
use std::{
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
    time::{Duration, Instant},
};

/// Sample count for the remote calibration probe -- see [`calibrate_remote_rate`].
/// Large enough to amortize one TLS/network round trip's fixed overhead against real
/// tracing time; small enough not to waste meaningful export time on a measurement.
pub(in crate::bridge::export_thread) const REMOTE_CALIBRATION_SAMPLES: u32 = 8;

/// Target wall-clock duration for one remote chunk request -- long enough that
/// per-request handshake/transfer overhead (~2.5ms locally, more over a real network)
/// is negligible against it; short enough that a slow/loaded worker never holds the
/// tail of the export hostage for minutes while every other engine sits idle.
///
/// Sizing by TIME rather than a fixed sample count makes both ends self-correcting: a
/// slow worker's measured rate is small, so `rate * REMOTE_CHUNK_TARGET_SECS` (see
/// [`remote_chunk_samples`]) comes out small too, automatically; a fast worker gets
/// bigger chunks.
const REMOTE_CHUNK_TARGET_SECS: f64 = 22.0;

/// Floor for [`remote_chunk_samples`] -- reuses [`REMOTE_MIN_SPP`] rather than a second
/// independent number the two could drift apart on.
const REMOTE_CHUNK_MIN_SPP: u32 = REMOTE_MIN_SPP;

/// How many samples one remote chunk request should cover, given the CURRENT best
/// estimate of remote's throughput.
///
/// Never below [`REMOTE_CHUNK_MIN_SPP`]: a non-finite, zero, or implausibly small rate
/// must not collapse the chunk size to something per-request overhead would dominate.
/// `SampleCursor::claim` separately clamps whatever this returns to the budget
/// actually remaining.
#[must_use]
pub(in crate::bridge::export_thread) fn remote_chunk_samples(rate_samples_per_sec: f64) -> u32 {
    let target = rate_samples_per_sec * REMOTE_CHUNK_TARGET_SECS;
    if target.is_finite() && target >= f64::from(REMOTE_CHUNK_MIN_SPP) {
        target.round() as u32
    } else {
        REMOTE_CHUNK_MIN_SPP
    }
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

/// The pure arithmetic behind `super::dispatch::run_remote_batch`'s throughput
/// measurement: `delta_samples` traced over `elapsed` wall time. Takes a plain
/// `Duration` rather than a pair of `Instant`s so it's directly unit-testable.
///
/// This is what [`calibrate_remote_rate`] and every later chunk use INSTEAD OF
/// whole-call wall time, which would bake in one-time connection-setup/upload costs
/// and understate remote's true sustained rate (see [`calibrate_remote_rate`]'s "What
/// the rate measurement excludes").
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
/// # What the rate measurement excludes
///
/// The rate is `super::dispatch::run_remote_batch`'s measured `rate_samples_per_sec`
/// when one came back, falling back to plain `probe / elapsed` otherwise. Either way it
/// excludes connection setup (handshake) and request upload -- one-time costs a real
/// dispatch never repeats -- but still includes ongoing per-`FRAME` transfer cost.
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
        assert_eq!(remote_chunk_samples(1000.0), 22_000);
    }

    #[test]
    fn remote_chunk_samples_never_drops_below_the_floor_for_a_tiny_rate() {
        assert_eq!(remote_chunk_samples(0.01), REMOTE_CHUNK_MIN_SPP);
        assert_eq!(remote_chunk_samples(0.0), REMOTE_CHUNK_MIN_SPP);
    }

    #[test]
    fn remote_chunk_samples_falls_back_to_the_floor_for_non_finite_rates() {
        assert_eq!(remote_chunk_samples(f64::NAN), REMOTE_CHUNK_MIN_SPP);
        assert_eq!(remote_chunk_samples(f64::INFINITY), REMOTE_CHUNK_MIN_SPP);
        assert_eq!(remote_chunk_samples(-100.0), REMOTE_CHUNK_MIN_SPP);
    }

    #[test]
    fn remote_chunk_samples_scales_up_for_a_faster_measured_rate() {
        let slow = remote_chunk_samples(500.0);
        let fast = remote_chunk_samples(1000.0);
        assert!(fast > slow);
        assert!((2.0f64.mul_add(-f64::from(slow), f64::from(fast))).abs() < 2.0);
    }

    // `SampleCursor` guarantees disjointness (see its own test coverage); this proves
    // `remote_chunk_samples`' output is a sane, positive claim size to drive it with.
    #[test]
    fn remote_chunk_samples_output_is_always_a_usable_positive_claim_size() {
        for total in [32u32, 100, 1000, 32768] {
            for rate in [0.0, 0.01, 50.0, 1000.0, 1_000_000.0] {
                let cursor = SampleCursor::new(0, total);
                let want = remote_chunk_samples(rate);
                assert!(want > 0, "a chunk size of 0 would never advance the cursor");
                // `SampleCursor::claim` clamps whatever is asked for to the actual budget.
                if let Some((start, count)) = cursor.claim(want) {
                    assert_eq!(start, 0);
                    assert!(count <= total);
                }
            }
        }
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
