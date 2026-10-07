//! One lane's claim / trace / merge / settle loop for a [`super::LanePool`] run.

use super::{PoolEvent, epoch::Epoch};
use crate::{ChunkResult, MergeError, RateModel, SampleRange, WorkerLane};
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
    /// Set when the merger refused to park the chunk because its parked-bytes budget is
    /// exhausted: the merge frontier this attempt started from. The lane is healthy, so
    /// this is not a failure; the chunk is retried once the frontier has advanced.
    deferred_at: Option<u32>,
}

/// A lane reports its first deferral at a frontier and then every this many.
const DEFERRAL_LOG_EVERY: u32 = 50;

/// Consecutive deferrals of one lane at the same frontier, with no chunk merged in
/// between, after which the lane fails the chunk instead of waiting again. Each wait
/// lasts at least one poll interval (25 ms), so this is roughly half a minute of a
/// frontier that is not advancing.
const MAX_DEFERRALS: u32 = 1200;

/// A lane's run of consecutive deferrals at one frontier.
#[derive(Debug, Default)]
struct DeferralRun {
    frontier: u32,
    count: u32,
}

impl DeferralRun {
    /// Records a deferral at `frontier` and returns the run length, this one included;
    /// a different frontier starts a new run.
    const fn note(&mut self, frontier: u32) -> u32 {
        if self.count == 0 || self.frontier != frontier {
            self.frontier = frontier;
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.count
    }

    /// Ends the run (a chunk merged, or the lane gave up).
    const fn reset(&mut self) {
        self.count = 0;
    }
}

/// Whether the `count`th deferral of a run is one to report.
const fn report_deferral(count: u32) -> bool {
    count == 1 || count.is_multiple_of(DEFERRAL_LOG_EVERY)
}

/// What a lane does after a failed chunk.
enum AfterFailure {
    Retry,
    Stop,
    Retired,
}

/// Takes lane `index` out of the epoch when its resource is gone for good and another
/// lane is left to carry on. Its unfinished chunk is already back on the cursor.
fn removed(epoch: &Epoch<'_>, index: usize, lane: &dyn WorkerLane) -> bool {
    if lane.lost() && epoch.roster.leave_unless_last() {
        epoch.emit(PoolEvent::LaneRemoved { lane: index });
        return true;
    }
    false
}

/// Runs lane `index` until nothing is left, the run is cancelled, or the lane is
/// retired or removed. See the parent module doc for the loop. Returns `true` when the
/// lane already counted itself out of the roster (removal), `false` otherwise.
pub(super) fn run_lane(
    epoch: &Epoch<'_>,
    index: usize,
    lane: &dyn WorkerLane,
    rate: &Mutex<RateModel>,
) -> bool {
    epoch.emit(PoolEvent::LaneStarted { lane: index });
    let mut failures = 0u32;
    let mut chunks = 0u32;
    let mut samples = 0u32;
    let mut deferrals = DeferralRun::default();
    loop {
        if removed(epoch, index, lane) {
            return true;
        }
        let want = epoch.want(rate);
        let Some(claim) = epoch.claim(want) else {
            break;
        };
        let range = claim.range();
        let mut traced = trace_chunk(epoch, index, lane, range);
        if traced.done > 0 {
            chunks += 1;
            samples += traced.done;
            deferrals.reset();
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
        claim.settle(traced.done);
        if traced.done == range.samples {
            failures = 0;
            continue;
        }
        if let Some(seen) = traced.deferred_at {
            // A full parked budget is back-pressure from a slow frontier chunk, not a
            // lane fault: wait for the frontier to move, then trace the chunk again --
            // unless the frontier has not moved for `MAX_DEFERRALS` waits, when the
            // chunk fails like any other and the lane's failure count decides.
            let count = deferrals.note(seen);
            if count < MAX_DEFERRALS {
                if report_deferral(count) {
                    epoch.emit(PoolEvent::ChunkDeferred {
                        lane: index,
                        range,
                        frontier: seen,
                        total_done: epoch.merger.total(),
                        deferrals: count,
                    });
                }
                epoch.wait_for_frontier(seen);
                continue;
            }
            deferrals.reset();
            traced.deferred_at = None;
            traced.error = Some(format!(
                "gave up after {count} consecutive deferrals at frontier {seen}: the merge frontier is not advancing"
            ));
        }
        if epoch.cancel.is_cancelled() {
            break;
        }
        if removed(epoch, index, lane) {
            return true;
        }
        failures += 1;
        match after_failure(epoch, index, range, &traced, failures) {
            AfterFailure::Retry => {}
            AfterFailure::Stop => break,
            AfterFailure::Retired => return false,
        }
    }
    epoch.emit(PoolEvent::LaneFinished {
        lane: index,
        chunks,
        samples,
    });
    false
}

/// Traces `range` on `lane`, validates the result and merges its prefix. A panic in
/// the lane or in the merge, an over-long `done`, or a buffer the merger refuses all
/// count as a chunk with nothing traced; a full parked budget is a deferral instead (see
/// [`Traced::deferred_at`]).
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
        deferred_at: None,
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
            deferred_at: None,
        };
    }
    if sum.len() != epoch.pixels {
        return discarded(format!(
            "lane returned {} pixels for a {}-pixel image; result discarded",
            sum.len(),
            epoch.pixels
        ));
    }
    let frontier = epoch.merger.frontier();
    let added = catch_unwind(AssertUnwindSafe(|| {
        epoch.merger.add(range.first_sample, done, sum)
    }));
    match added {
        Ok(Ok(total_done)) => {
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
                deferred_at: None,
            }
        }
        Ok(Err(refused @ MergeError::ParkedBudgetExceeded { .. })) => Traced {
            done: 0,
            rate: None,
            error: Some(format!("chunk deferred: {refused}")),
            elapsed,
            deferred_at: Some(frontier),
        },
        Ok(Err(refused)) => discarded(format!("chunk discarded: {refused}")),
        Err(_) => discarded("merging a chunk panicked; result discarded".to_owned()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_counts_at_one_frontier_and_restarts_on_another() {
        let mut run = DeferralRun::default();
        assert_eq!(run.note(7), 1);
        assert_eq!(run.note(7), 2);
        assert_eq!(run.note(9), 1, "a moved frontier starts a new run");
        run.reset();
        assert_eq!(run.note(9), 1, "a reset starts a new run");
    }

    #[test]
    fn a_run_reaches_the_cap_only_without_progress() {
        let mut run = DeferralRun::default();
        let reached = (1..=MAX_DEFERRALS).map(|_| run.note(3)).last();
        assert_eq!(reached, Some(MAX_DEFERRALS));
        let mut moving = DeferralRun::default();
        assert!(
            (0..MAX_DEFERRALS * 2).all(|n| moving.note(n) < MAX_DEFERRALS),
            "a frontier that keeps moving never hits the cap"
        );
    }

    #[test]
    fn deferrals_are_reported_first_and_then_periodically() {
        assert!(report_deferral(1));
        assert!(!report_deferral(2));
        assert!(!report_deferral(DEFERRAL_LOG_EVERY - 1));
        assert!(report_deferral(DEFERRAL_LOG_EVERY));
        assert!(!report_deferral(DEFERRAL_LOG_EVERY + 1));
        assert!(report_deferral(DEFERRAL_LOG_EVERY * 2));
    }
}
