//! One lane's claim / trace / merge / settle loop for a [`super::LanePool`] run.

use super::{PoolEvent, epoch::Epoch};
use crate::{ChunkResult, RateModel, SampleRange, WorkerLane};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// What one chunk attempt amounted to after validation and merging.
struct Traced {
    /// Samples merged: the valid prefix, or `0` when the result was discarded.
    done: u32,
    /// The lane's own rate measurement, if it reported a usable one.
    rate: Option<f64>,
    error: Option<String>,
    elapsed: Duration,
}

/// What a lane does after a failed chunk.
enum AfterFailure {
    Retry,
    Stop,
    Retired,
}

/// Runs lane `index` until nothing is left, the run is cancelled, or the lane is
/// retired. See the parent module doc for the loop.
pub(super) fn run_lane(
    epoch: &Epoch<'_>,
    index: usize,
    lane: &dyn WorkerLane,
    rate: &Mutex<RateModel>,
) {
    epoch.emit(PoolEvent::LaneStarted { lane: index });
    let mut failures = 0u32;
    let mut chunks = 0u32;
    let mut samples = 0u32;
    loop {
        let want = rate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .chunk_samples(&epoch.config.policy);
        let Some(range) = epoch.claim(want) else {
            break;
        };
        let traced = trace_chunk(epoch, index, lane, range);
        if traced.done > 0 {
            chunks += 1;
            samples += traced.done;
        }
        // A short chunk's wall time is dominated by however it failed (a liveness
        // timeout, a refused connection), so only a complete chunk -- or the lane's
        // own steady-state measurement -- is evidence of its throughput.
        if traced.done == range.samples || traced.rate.is_some() {
            rate.lock().unwrap_or_else(PoisonError::into_inner).observe(
                traced.done,
                traced.elapsed,
                traced.rate,
            );
        }
        epoch.settle(range, traced.done);
        if traced.done == range.samples {
            failures = 0;
            continue;
        }
        if epoch.cancel.is_cancelled() {
            break;
        }
        failures += 1;
        match after_failure(epoch, index, range, &traced, failures) {
            AfterFailure::Retry => {}
            AfterFailure::Stop => break,
            AfterFailure::Retired => return,
        }
    }
    epoch.emit(PoolEvent::LaneFinished {
        lane: index,
        chunks,
        samples,
    });
}

/// Traces `range` on `lane`, validates the result and merges its prefix. A panic in
/// the lane, an over-long `done`, or a buffer the merger refuses all count as a
/// chunk with nothing traced.
fn trace_chunk(
    epoch: &Epoch<'_>,
    index: usize,
    lane: &dyn WorkerLane,
    range: SampleRange,
) -> Traced {
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| {
        lane.render_chunk(epoch.scene, range, epoch.cancel)
    }))
    .unwrap_or_else(|_| ChunkResult::failed("lane panicked while rendering a chunk".to_owned()));
    let elapsed = started.elapsed();
    let discarded = |error: String| Traced {
        done: 0,
        rate: None,
        error: Some(error),
        elapsed,
    };
    let ChunkResult {
        sum,
        done,
        rate,
        error,
    } = result;
    if done > range.samples {
        return discarded(format!(
            "lane reported {done} samples for a {}-sample chunk; result discarded",
            range.samples
        ));
    }
    if done == 0 {
        return Traced {
            done: 0,
            rate: None,
            error,
            elapsed,
        };
    }
    if sum.len() != epoch.pixels {
        return discarded(format!(
            "lane returned {} pixels for a {}-pixel image; result discarded",
            sum.len(),
            epoch.pixels
        ));
    }
    match epoch.merger.add(range.first_sample, done, sum) {
        Ok(total_done) => {
            epoch.emit(PoolEvent::ChunkMerged {
                lane: index,
                range,
                done,
                total_done,
                target: epoch.target,
            });
            Traced {
                done,
                rate,
                error,
                elapsed,
            }
        }
        Err(refused) => discarded(format!("chunk discarded: {refused}")),
    }
}

/// Reports a failed chunk and decides whether the lane retries (after its pause),
/// stops, or is retired.
fn after_failure(
    epoch: &Epoch<'_>,
    index: usize,
    range: SampleRange,
    traced: &Traced,
    consecutive_failures: u32,
) -> AfterFailure {
    let returned = range.after_prefix(traced.done);
    let error = traced
        .error
        .clone()
        .unwrap_or_else(|| "chunk ended early".to_owned());
    if consecutive_failures >= epoch.config.retire_after_failures.max(1) {
        epoch.emit(PoolEvent::LaneFailed {
            lane: index,
            error,
            returned,
            consecutive_failures,
            pause: None,
        });
        epoch.emit(PoolEvent::LaneRetired {
            lane: index,
            consecutive_failures,
        });
        return AfterFailure::Retired;
    }
    let pause = epoch.config.backoff(consecutive_failures);
    epoch.emit(PoolEvent::LaneFailed {
        lane: index,
        error,
        returned,
        consecutive_failures,
        pause: Some(pause),
    });
    if pause.is_zero() || epoch.pause(pause) {
        AfterFailure::Retry
    } else {
        AfterFailure::Stop
    }
}
