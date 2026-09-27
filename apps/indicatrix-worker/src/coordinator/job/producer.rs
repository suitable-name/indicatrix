//! A coordinator job's producer thread: wait for the viewer's turn, take lanes, run the
//! `LanePool` over the request's sample range, and hand every finished chunk to the
//! emitter (see `crate::stream_emit::run_stream_with`), which keeps heartbeating the
//! viewer the whole time.

use super::{
    Coordinator, LaneKey, RateBook,
    plan::{InteractivePin, JobPlan, WorkerPick, rank_fastest},
};
use crate::{
    assets::HeldAsset,
    coordinator::{JobLanes, JoinedWorkerLane, LaneNeed, OwnLane, Registry, WorkerInfo},
    stream_emit::{ProducerOutcome, ProducerSink},
};
use indicatrix_dispatch::{
    CancelToken, ChunkResult, LanePool, Merger, PoolEvent, PoolStatus, RateModel, SampleRange,
    WorkerLane,
};
use indicatrix_net::{
    SceneState,
    messages::{ErrorMsg, error_codes},
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often a job waiting for an idle worker looks again.
const LANE_RECHECK: Duration = Duration::from_millis(50);

/// The rate a never-measured lane starts from (reporting only; its first chunk is the
/// policy's calibration chunk regardless).
const INITIAL_RATE_GUESS: f64 = 10.0;

/// Everything one job's producer thread owns.
pub struct Job {
    /// The process's coordinator state.
    pub coordinator: Arc<Coordinator>,
    /// The viewer's FIFO key (its certificate).
    pub viewer: Arc<str>,
    /// The viewer connection's lane rates, carried across its requests.
    pub rates: Arc<Mutex<RateBook>>,
    /// The scene every lane renders, byte for byte.
    pub scene: SceneState,
    /// The request's sample range.
    pub range: SampleRange,
    /// Lanes and scheduling.
    pub plan: JobPlan,
    /// The HDR map held for the job: forwarded to joined workers that ask,
    /// and keeping the own lane's decoded map pinned until the job ends.
    pub asset: Option<Arc<HeldAsset>>,
}

/// Forwards every finished chunk of the wrapped lane to the emitter as it is merged, so
/// `FRAME` deltas, `PREVIEW`s and `PROGRESS` follow the pool's progress.
struct SinkLane {
    inner: Arc<dyn WorkerLane>,
    sink: Arc<ProducerSink>,
    pixels: usize,
}

impl WorkerLane for SinkLane {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn render_chunk(
        &self,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
    ) -> ChunkResult {
        let result = self.inner.render_chunk(scene, range, cancel);
        // Exactly the results the pool will merge: a valid-length prefix of the chunk.
        if result.done > 0 && result.done <= range.samples && result.sum.len() == self.pixels {
            self.sink.add_chunk(result.done, &result.sum);
        }
        result
    }
}

/// Runs `job` to its end and finishes `sink` accordingly.
pub fn run(job: &Job, sink: ProducerSink) {
    let turn = if job.plan.fifo {
        let Some(turn) = job
            .coordinator
            .queues
            .wait_turn(&job.viewer, || sink.is_cancelled())
        else {
            return sink.finish(ProducerOutcome::Cancelled);
        };
        Some(turn)
    } else {
        None
    };
    let lanes = acquire_lanes(job, &sink);
    if lanes.is_empty() {
        let outcome = if sink.is_cancelled() {
            ProducerOutcome::Cancelled
        } else {
            ProducerOutcome::Failed(lost("no joined worker became idle to take the request"))
        };
        return sink.finish(outcome);
    }
    let sink = Arc::new(sink);
    let (status, merger) = run_pool(job, lanes, &sink);
    drop(turn);
    let Ok(sink) = Arc::try_unwrap(sink) else {
        // Unreachable: the pool (and every lane holding a clone) is gone by now. Dropping
        // the last clone still ends the stream (as an internal error).
        return;
    };
    match status {
        PoolStatus::Complete => {
            let (sum, _count) = merger.into_parts();
            sink.finish(ProducerOutcome::Complete {
                final_total: Some(sum),
            });
        }
        PoolStatus::Cancelled { .. } => sink.finish(ProducerOutcome::Cancelled),
        PoolStatus::LanesExhausted { missing } => sink.finish(ProducerOutcome::Failed(lost(
            &format!("every render lane failed with {missing} sample(s) of the request left"),
        ))),
    }
}

/// The `ALL_WORKERS_LOST` stream error.
fn lost(why: &str) -> ErrorMsg {
    ErrorMsg {
        code: error_codes::ALL_WORKERS_LOST,
        message: format!("the coordinator lost every lane that could finish this request: {why}"),
    }
}

/// Builds the pool from `lanes` (rates from the viewer's book), runs it with the
/// emitter's cancel flag bridged into the pool's token, and stores the lanes' rates back.
fn run_pool(
    job: &Job,
    lanes: Vec<(LaneKey, Arc<dyn WorkerLane>)>,
    sink: &Arc<ProducerSink>,
) -> (PoolStatus, Merger) {
    let pixels = job.scene.width as usize * job.scene.height as usize;
    let mut pool = LanePool::new(job.plan.pool);
    let keys: Vec<LaneKey> = {
        let book = job.rates.lock().unwrap_or_else(PoisonError::into_inner);
        lanes
            .into_iter()
            .map(|(key, inner)| {
                let rate = book
                    .get(key)
                    .map_or_else(|| RateModel::new(INITIAL_RATE_GUESS), RateModel::calibrated);
                let lane = SinkLane {
                    inner,
                    sink: Arc::clone(sink),
                    pixels,
                };
                pool.add_lane(Arc::new(lane), rate);
                key
            })
            .collect()
    };
    let merger = Merger::new(pixels, job.range.first_sample);
    let token = CancelToken::new();
    let running = AtomicBool::new(true);
    let status = thread::scope(|scope| {
        scope.spawn(|| {
            while running.load(Ordering::Relaxed) {
                if sink.is_cancelled() {
                    token.cancel();
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let status = pool.run_into(&job.scene, job.range, &merger, &token, &log_event);
        running.store(false, Ordering::Relaxed);
        status
    });
    {
        let mut book = job.rates.lock().unwrap_or_else(PoisonError::into_inner);
        for (index, key) in keys.into_iter().enumerate() {
            if let Some(rate) = pool.rate(index).and_then(|r| r.estimate()) {
                book.set(key, rate);
            }
        }
    }
    // A fresh merger built for this exact image and range cannot mismatch; report a
    // mismatch like lost lanes rather than panicking.
    let status = status.unwrap_or(PoolStatus::LanesExhausted {
        missing: job.range.samples,
    });
    (status, merger)
}

/// Logs the pool's lane failures (a worker's own error text never reaches the viewer).
fn log_event(event: PoolEvent) {
    match event {
        PoolEvent::LaneFailed {
            lane,
            error,
            returned,
            pause,
            ..
        } => tracing::info!(
            "coordinator job: lane {lane} failed ({error}); {} sample(s) returned to the pool, pause {pause:?}",
            returned.samples
        ),
        PoolEvent::LaneRetired {
            lane,
            consecutive_failures,
        } => tracing::warn!(
            "coordinator job: lane {lane} retired after {consecutive_failures} failed chunk(s) in a row"
        ),
        other => tracing::trace!("coordinator job: {other:?}"),
    }
}

/// The job's lanes: the own lane (if planned) and the planned joined workers, checked
/// out now. A job without an own lane waits (cancellably) up to the configured
/// `lane_wait` for at least one idle eligible worker.
fn acquire_lanes(job: &Job, sink: &ProducerSink) -> Vec<(LaneKey, Arc<dyn WorkerLane>)> {
    let config = job.coordinator.job_config();
    let deadline = Instant::now() + config.lane_wait;
    let need = LaneNeed {
        pixels: job.scene.width * job.scene.height,
        hdr: job.scene.hdr().is_some(),
    };
    let shared = Arc::new(JobLanes::new(need, job.asset.clone(), config.lane));
    let mut lanes: Vec<(LaneKey, Arc<dyn WorkerLane>)> = Vec::new();
    if job.plan.own
        && let Some(own) = &job.coordinator.own
    {
        let lane = OwnLane::new(Arc::clone(&own.gpu), own.threads, own.compute_mode);
        lanes.push((LaneKey::Own, Arc::new(lane)));
    }
    let Some(registry) = job.coordinator.registry.as_ref() else {
        return lanes;
    };
    loop {
        let handles = match job.plan.workers {
            WorkerPick::None => Vec::new(),
            WorkerPick::All => {
                std::iter::from_fn(|| Registry::checkout(registry, |w| need.accepts(w))).collect()
            }
            WorkerPick::Fastest(n) => checkout_fastest(
                registry,
                &job.rates,
                need,
                n,
                job.coordinator.interactive_pin.as_ref(),
            ),
        };
        for handle in handles {
            let key = LaneKey::Worker(handle.info().worker_id);
            let lane = JoinedWorkerLane::new(handle, Arc::clone(registry), Arc::clone(&shared));
            lanes.push((key, Arc::new(lane)));
        }
        if !lanes.is_empty() || sink.is_cancelled() || Instant::now() >= deadline {
            return lanes;
        }
        thread::sleep(LANE_RECHECK);
    }
}

/// Checks out up to `n` idle eligible workers, fastest first (see [`rank_fastest`]) --
/// with `pin`, the pinned worker first when one of its connections is idle and eligible,
/// else the plain fastest-first pick (see [`InteractivePin::apply`]).
pub(super) fn checkout_fastest(
    registry: &Arc<Registry>,
    rates: &Mutex<RateBook>,
    need: LaneNeed,
    n: u32,
    pin: Option<&InteractivePin>,
) -> Vec<crate::coordinator::WorkerHandle> {
    let mut idle: Vec<WorkerInfo> = registry
        .workers()
        .into_iter()
        .filter(|(w, idle)| *idle && need.accepts(w))
        .map(|(w, _)| w)
        .collect();
    rank_fastest(
        &mut idle,
        &rates.lock().unwrap_or_else(PoisonError::into_inner),
    );
    if let Some(pin) = pin {
        pin.apply(&mut idle);
    }
    idle.iter()
        .filter_map(|w| Registry::checkout(registry, |c| c.worker_id == w.worker_id))
        .take(n as usize)
        .collect()
}
