//! A coordinator job's producer thread: wait for the viewer's turn, take lanes, run the
//! `LanePool` over the request's sample range, and hand every finished chunk to the
//! emitter (see `crate::stream_emit::run_stream_with`), which keeps heartbeating the
//! viewer the whole time.

use super::{
    Coordinator, LaneKey, RateBook,
    limits::{
        JobParkedBudget, Reservation, Turn, contribution_bytes, hdr_bytes, job_bytes,
        job_estimate_bytes, lane_bytes,
    },
    over_budget,
    plan::{InteractivePin, JobPlan, WorkerPick, rank_fastest},
};
use crate::{
    assets::HeldAsset,
    coordinator::{JobLanes, JoinedWorkerLane, LaneNeed, OwnLane, Registry, WorkerInfo},
    stream_emit::{Awaited, ContributionSlot, ProducerOutcome, ProducerSink},
};
use glam::Vec3;
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
    /// v16: the viewer's reserved tail of a `FinalImageRequest`, if it asked to
    /// contribute one. `range` above is the SERVER's own share only; once the pool
    /// completes it, [`run`] waits on this slot and folds it in (or reclaims it) before
    /// finishing.
    pub contribution: Option<Arc<ContributionSlot>>,
}

/// One [`SinkLane`]'s most recently traced, length-valid chunk (`first_sample, done,
/// sum`), held back from the emitter until [`run_pool`]'s `events` closure confirms the
/// pool's [`Merger`] actually merged it.
type PendingChunk = (u32, u32, Vec<Vec3>);

/// Forwards every finished chunk of the wrapped lane to the emitter once [`run_pool`]'s
/// `events` closure confirms the pool's [`Merger`] merged it -- never on the strength of
/// this lane's own validation alone. A chunk `render_chunk` traces here can still be
/// refused by the merger (an overlap after a requeue, or the parked-bytes budget);
/// forwarding it to the viewer first would show a `FRAME` delta for samples the
/// authoritative total never actually counted.
struct SinkLane {
    inner: Arc<dyn WorkerLane>,
    pixels: usize,
    /// This lane's most recent length-valid chunk, awaiting the matching
    /// `PoolEvent::ChunkMerged`. One slot is enough: a lane's next `render_chunk` never
    /// starts before its previous chunk has been merged or discarded (`lane_loop::run_lane`
    /// claims, traces, merges/settles, and only then claims again), so nothing here is
    /// ever overwritten unconfirmed.
    pending: Arc<Mutex<Option<PendingChunk>>>,
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
        // Exactly the results the pool will try to merge: a valid-length prefix of the
        // chunk. Held back, not forwarded yet -- see this type's doc comment.
        if result.done > 0 && result.done <= range.samples && result.sum.len() == self.pixels {
            *self.pending.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((range.first_sample, result.done, result.sum.clone()));
        }
        result
    }
}

/// Runs `job` to its end and finishes `sink` accordingly.
///
/// # The viewer's FIFO turn is held through the contribution wait (v16)
///
/// [`finish_complete`] (not this function) drops `turn`: for a job with a reserved
/// viewer share ([`Job::contribution`]), the picture isn't final the moment the
/// server's own lanes finish -- it still needs the viewer's contribution (or a
/// reclaim). Releasing the viewer's FIFO slot before that would let this same viewer's
/// next queued job start racing a contribution upload still in flight for this one.
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
    // Reserved only now, after the FIFO wait: a job queued behind another one of the
    // same viewer must not hold coordinator-wide memory budget while it hasn't even
    // started. The lane-independent frame buffers come first, with whatever the job
    // pins beside them (see `extra_bytes`); the lanes' own frame buffers are charged
    // once the lanes are known (`reserve_lane_frames`).
    let bytes = job_bytes(job.scene.width, job.scene.height).saturating_add(extra_bytes(job));
    let Ok(_reservation) = job.coordinator.budget().try_reserve(bytes) else {
        let error = over_budget(&job.coordinator, bytes);
        drop(turn);
        return sink.finish(ProducerOutcome::Failed(error));
    };
    let lanes = acquire_lanes(job, &sink);
    if lanes.is_empty() {
        drop(turn);
        let outcome = if sink.is_cancelled() {
            ProducerOutcome::Cancelled
        } else {
            ProducerOutcome::Failed(lost("no joined worker became idle to take the request"))
        };
        return sink.finish(outcome);
    }
    let (lanes, lane_frames) = match reserve_lane_frames(job, lanes) {
        Ok(reserved) => reserved,
        Err(error) => {
            drop(turn);
            return sink.finish(ProducerOutcome::Failed(error));
        }
    };
    let sink = Arc::new(sink);
    let (status, merger) = run_pool(job, lanes, &sink, job.range);
    // The pool and its lanes are gone: release their frames before a reclaim checks out
    // (and charges) lanes of its own.
    drop(lane_frames);
    let Ok(sink) = Arc::try_unwrap(sink) else {
        // Unreachable: the pool (and every lane holding a clone) is gone by now. Dropping
        // the last clone still ends the stream (as an internal error).
        drop(turn);
        return;
    };
    match status {
        PoolStatus::Complete => finish_complete(job, sink, merger, turn),
        PoolStatus::Cancelled { .. } => {
            drop(turn);
            sink.finish(ProducerOutcome::Cancelled);
        }
        PoolStatus::LanesExhausted { missing } => {
            drop(turn);
            sink.finish(ProducerOutcome::Failed(lost(&format!(
                "every render lane failed with {missing} sample(s) of the request left"
            ))));
        }
    }
}

/// What `job` pins besides the frame buffers every job and lane holds: one full-frame
/// buffer for the viewer's upload while it is in flight ([`Job::contribution`]) and the
/// HDR map ([`Job::asset`], see [`hdr_bytes`]: the encoded bytes once, shared by all
/// lanes, plus the decoded texels when the own lane renders it).
fn extra_bytes(job: &Job) -> u64 {
    let contribution = if job.contribution.is_some() {
        contribution_bytes(job.scene.width, job.scene.height)
    } else {
        0
    };
    let hdr = job.asset.as_ref().map_or(0, |asset| {
        let encoded = asset
            .bytes()
            .map_or(0, |bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let texels = asset
            .map()
            .map(|map| map.width() as u64 * map.height() as u64);
        hdr_bytes(encoded, texels)
    });
    contribution.saturating_add(hdr)
}

/// The lanes [`acquire_lanes`] checked out.
type LaneSet = Vec<(LaneKey, Arc<dyn WorkerLane>)>;

/// Charges `lanes`' frame buffers ([`lane_bytes`]) against the coordinator's memory
/// budget, dropping trailing lanes (joined workers go back to the registry) until what
/// is left fits. The returned reservation is released when dropped.
///
/// # Errors
///
/// [`over_budget`]'s refusal when not even one lane fits next to what the budget
/// already holds.
fn reserve_lane_frames(
    job: &Job,
    mut lanes: LaneSet,
) -> Result<(LaneSet, Reservation<'_>), ErrorMsg> {
    let (width, height) = (job.scene.width, job.scene.height);
    let wanted = lanes.len();
    for count in (1..=wanted).rev() {
        let n = u32::try_from(count).unwrap_or(u32::MAX);
        if let Ok(reservation) = job
            .coordinator
            .budget()
            .try_reserve(lane_bytes(width, height, n))
        {
            if count < wanted {
                tracing::info!(
                    "coordinator job: running on {count} of {wanted} lane(s) -- the frame buffers \
                     of more would exceed the memory budget (--max-job-memory-mib)"
                );
                lanes.truncate(count);
            }
            return Ok((lanes, reservation));
        }
    }
    Err(over_budget(
        &job.coordinator,
        job_estimate_bytes(width, height, 1, extra_bytes(job)),
    ))
}

/// The server's own lanes finished ([`PoolStatus::Complete`]). If this job reserved a
/// viewer contribution, waits for it (or reclaims that range itself) before finishing;
/// otherwise finishes right away. Drops `turn` -- see [`run`]'s doc comment on why that
/// happens here, not in `run` itself.
fn finish_complete(job: &Job, sink: ProducerSink, merger: Merger, turn: Option<Turn<'_>>) {
    let Some(slot) = &job.contribution else {
        drop(turn);
        warn_dropped_pixels(merger.dropped_pixels());
        let (sum, _count) = merger.into_parts();
        return sink.finish(ProducerOutcome::Complete {
            final_total: Some(sum),
            reclaimed_samples: 0,
        });
    };
    let reserved = slot.reserved();
    let wait = job.coordinator.job_config().contribution_wait;
    match slot.await_until(wait, || sink.is_cancelled()) {
        Awaited::Arrived(sum) => {
            drop(turn);
            sink.add_chunk(reserved.samples, &sum);
            // A fresh merger built for this exact image/range cannot mismatch; a
            // failure here would mean the reserved range wasn't actually the merger's
            // frontier, an internal bug rather than something to report to the viewer.
            let _ = merger.add(reserved.first_sample, reserved.samples, sum);
            warn_dropped_pixels(merger.dropped_pixels());
            let (total, _count) = merger.into_parts();
            sink.finish(ProducerOutcome::Complete {
                final_total: Some(total),
                reclaimed_samples: 0,
            });
        }
        Awaited::Cancelled => {
            drop(turn);
            sink.finish(ProducerOutcome::Cancelled);
        }
        Awaited::Reclaim { reason } => {
            tracing::info!("coordinator job: reclaiming viewer range {reserved:?}: {reason}");
            reclaim(job, sink, merger, turn, reserved);
        }
    }
}

/// Logs, once per finished request, how many pixels [`Merger`] zeroed for a non-finite or
/// negative component. Such pixels mean a lane returned corrupt radiance, which the merge
/// hides from the picture; nothing is logged when there were none.
fn warn_dropped_pixels(dropped: u64) {
    if dropped > 0 {
        tracing::warn!(
            "coordinator job: {dropped} pixel(s) of merged chunks were zeroed for a non-finite or negative \
             component (a lane returned invalid radiance)"
        );
    }
}

/// Re-traces `reserved` itself (the viewer's contribution didn't arrive in time, or
/// wasn't valid) with a freshly re-checked-out set of lanes -- the ones `run_pool`
/// already ran are released the moment it returns, so this must ask again.
fn reclaim(
    job: &Job,
    sink: ProducerSink,
    merger: Merger,
    turn: Option<Turn<'_>>,
    reserved: SampleRange,
) {
    let lanes = acquire_lanes(job, &sink);
    if lanes.is_empty() {
        drop(turn);
        let outcome = if sink.is_cancelled() {
            ProducerOutcome::Cancelled
        } else {
            ProducerOutcome::Failed(lost(
                "no joined worker became idle to reclaim the viewer's share",
            ))
        };
        return sink.finish(outcome);
    }
    let (lanes, lane_frames) = match reserve_lane_frames(job, lanes) {
        Ok(reserved) => reserved,
        Err(error) => {
            drop(turn);
            return sink.finish(ProducerOutcome::Failed(error));
        }
    };
    let sink = Arc::new(sink);
    let (status, extra) = run_pool(job, lanes, &sink, reserved);
    drop(lane_frames);
    let Ok(sink) = Arc::try_unwrap(sink) else {
        drop(turn);
        return;
    };
    drop(turn);
    match status {
        PoolStatus::Complete => {
            let extra_dropped = extra.dropped_pixels();
            let (sum, count) = extra.into_parts();
            // As in `finish_complete`: a fresh, correctly-anchored merger cannot
            // mismatch here.
            let _ = merger.add(reserved.first_sample, count, sum);
            warn_dropped_pixels(merger.dropped_pixels().saturating_add(extra_dropped));
            let (total, _count) = merger.into_parts();
            sink.finish(ProducerOutcome::Complete {
                final_total: Some(total),
                reclaimed_samples: reserved.samples,
            });
        }
        PoolStatus::Cancelled { .. } => sink.finish(ProducerOutcome::Cancelled),
        PoolStatus::LanesExhausted { missing } => {
            sink.finish(ProducerOutcome::Failed(lost(&format!(
                "every render lane failed reclaiming the viewer's share, {missing} sample(s) left"
            ))));
        }
    }
}

/// The `ALL_WORKERS_LOST` stream error.
fn lost(why: &str) -> ErrorMsg {
    ErrorMsg {
        code: error_codes::ALL_WORKERS_LOST,
        message: format!("the coordinator lost every lane that could finish this request: {why}"),
        // This helper has no request in scope to stamp; its caller may want to.
        request_id: None,
    }
}

/// Builds the pool from `lanes` (rates from the viewer's book), runs it over `range`
/// with the emitter's cancel flag bridged into the pool's token, and stores the lanes'
/// rates back. `range` is `job.range` for the server's own share, or (v16) the
/// viewer's reserved tail when [`reclaim`] re-runs this for it -- either way the
/// returned [`Merger`] is freshly anchored at `range.first_sample`.
fn run_pool(
    job: &Job,
    lanes: Vec<(LaneKey, Arc<dyn WorkerLane>)>,
    sink: &Arc<ProducerSink>,
    range: SampleRange,
) -> (PoolStatus, Merger) {
    let pixels = job.scene.width as usize * job.scene.height as usize;
    let image_pixels = job.scene.width * job.scene.height;
    let mut pool = LanePool::new(job.plan.pool);
    let mut pendings: Vec<Arc<Mutex<Option<PendingChunk>>>> = Vec::new();
    let keys: Vec<LaneKey> = {
        let book = job.rates.lock().unwrap_or_else(PoisonError::into_inner);
        lanes
            .into_iter()
            .map(|(key, inner)| {
                let rate = book
                    .get(&key, image_pixels)
                    .map_or_else(|| RateModel::new(INITIAL_RATE_GUESS), RateModel::calibrated);
                let pending = Arc::new(Mutex::new(None));
                let lane = SinkLane {
                    inner,
                    pixels,
                    pending: Arc::clone(&pending),
                };
                pool.add_lane(Arc::new(lane), rate);
                pendings.push(pending);
                key
            })
            .collect()
    };
    let merger = Merger::new(pixels, range.first_sample)
        .with_parked_budget(Arc::new(JobParkedBudget(Arc::clone(&job.coordinator))));
    let token = CancelToken::new();
    let running = AtomicBool::new(true);
    // Forwards a lane's held-back chunk to the emitter only once its `ChunkMerged`
    // confirms the authoritative `Merger` accepted it -- see `SinkLane`'s doc comment.
    let events = |event: PoolEvent| {
        if let PoolEvent::ChunkMerged {
            lane, range, done, ..
        } = &event
            && let Some(slot) = pendings.get(*lane)
            && let Some((first_sample, pending_done, sum)) =
                slot.lock().unwrap_or_else(PoisonError::into_inner).take()
            && first_sample == range.first_sample
            && pending_done == *done
        {
            sink.add_chunk(pending_done, &sum);
        }
        log_event(event);
    };
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
        let status = pool.run_into(&job.scene, range, &merger, &token, &events);
        running.store(false, Ordering::Relaxed);
        status
    });
    {
        let mut book = job.rates.lock().unwrap_or_else(PoisonError::into_inner);
        for (index, key) in keys.into_iter().enumerate() {
            if let Some(rate) = pool.rate(index).and_then(|r| r.estimate()) {
                book.set(key, image_pixels, rate);
            }
        }
    }
    // A fresh merger built for this exact image and range cannot mismatch; report a
    // mismatch like lost lanes rather than panicking.
    let status = status.unwrap_or(PoolStatus::LanesExhausted {
        missing: range.samples,
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
            let key = LaneKey::for_worker(handle.info());
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
        need.pixels,
    );
    if let Some(pin) = pin {
        pin.apply(&mut idle);
    }
    idle.iter()
        .filter_map(|w| Registry::checkout(registry, |c| c.worker_id == w.worker_id))
        .take(n as usize)
        .collect()
}
