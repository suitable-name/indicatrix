//! Workers that become available while a job is running: [`LateLanes`], the
//! [`LaneFeed`] that turns each of them into a lane of the running pool, and
//! [`LaneTable`], the per-lane bookkeeping both the producer and the feed keep by pool
//! index.
//!
//! The feed wakes on the registry's change notifications (a worker registering or being
//! dropped) and, at the latest, every [`LaneFeed`] wait -- a connection that merely became
//! idle again after another job produces no notification. Each wake it applies the same
//! rules as the job's start (`producer::acquire_lanes`): the worker must pass the job's
//! [`LaneNeed`](crate::coordinator::LaneNeed) and not be one that refused its HDR map, a
//! `Fastest(n)` job keeps at most `n` live joined lanes, and every added lane is charged
//! the same frame buffers against the memory budget as a lane that started with the job
//! (a lane that does not fit is not added; its worker stays idle for the next job).

use super::{
    LaneKey,
    limits::{Reservation, lane_bytes},
    plan::WorkerPick,
    producer::{INITIAL_RATE_GUESS, Job, PendingChunk, SinkLane, checkout_ranked},
};
use crate::coordinator::{
    Capacity, JobLanes, JoinedWorkerLane, Registry, WorkerHandle, WorkerInfo,
};
use indicatrix_dispatch::{LaneFeed, RateModel, WorkerLane};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

/// A lane's slot for the chunk held back from the emitter until the merger confirms it.
pub(super) type PendingSlot = Arc<Mutex<Option<PendingChunk>>>;

/// Every lane of one pool run by pool index: its rate-book key and its held-back chunk
/// slot. Filled by the producer for the lanes the run starts with and by [`LateLanes`]
/// for the ones that join it.
#[derive(Default)]
pub(super) struct LaneTable {
    lanes: Mutex<BTreeMap<usize, (LaneKey, PendingSlot)>>,
}

impl LaneTable {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<usize, (LaneKey, PendingSlot)>> {
        self.lanes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records the lane the pool knows as `index`.
    pub(super) fn register(&self, index: usize, key: LaneKey, pending: PendingSlot) {
        self.lock().insert(index, (key, pending));
    }

    /// The held-back chunk slot of lane `index`.
    pub(super) fn pending(&self, index: usize) -> Option<PendingSlot> {
        self.lock()
            .get(&index)
            .map(|(_, pending)| Arc::clone(pending))
    }

    /// Every lane's pool index and rate-book key, in index order.
    pub(super) fn keys(&self) -> Vec<(usize, LaneKey)> {
        self.lock()
            .iter()
            .map(|(index, (key, _))| (*index, key.clone()))
            .collect()
    }
}

/// A joined lane the feed accounts for, with the memory it was charged (none for a lane
/// that started with the job: the producer holds that reservation).
struct HeldLane<'a> {
    lane: Arc<dyn WorkerLane>,
    _frames: Option<Reservation<'a>>,
}

/// The [`LaneFeed`] of one job's pool; see the module doc.
pub(super) struct LateLanes<'a> {
    registry: &'a Arc<Registry>,
    shared: &'a Arc<JobLanes>,
    table: &'a LaneTable,
    budget: &'a super::limits::MemoryBudget,
    rates: &'a Mutex<super::RateBook>,
    pin: Option<&'a super::InteractivePin>,
    workers: WorkerPick,
    size: (u32, u32),
    changes: Mutex<mpsc::Receiver<Capacity>>,
    held: Mutex<Vec<HeldLane<'a>>>,
}

impl<'a> LateLanes<'a> {
    /// The feed for `job`'s pool, or `None` when the job cannot take late workers: a
    /// whole-image job (one lane by design), one that takes no joined workers, or a
    /// coordinator without a registry. `joined` are the joined-worker lanes the pool
    /// started with.
    pub(super) fn for_job(
        job: &'a Job,
        shared: &'a Arc<JobLanes>,
        table: &'a LaneTable,
        joined: Vec<Arc<dyn WorkerLane>>,
    ) -> Option<Self> {
        if job.plan.whole_image || job.plan.workers == WorkerPick::None {
            return None;
        }
        let registry = job.coordinator.registry.as_ref()?;
        Some(Self {
            registry,
            shared,
            table,
            budget: job.coordinator.budget(),
            rates: &job.rates,
            pin: job.coordinator.interactive_pin(),
            workers: job.plan.workers,
            size: (job.scene.width, job.scene.height),
            changes: Mutex::new(registry.subscribe()),
            held: Mutex::new(
                joined
                    .into_iter()
                    .map(|lane| HeldLane {
                        lane,
                        _frames: None,
                    })
                    .collect(),
            ),
        })
    }

    /// Blocks up to `wait` for a registry change, then swallows the ones queued behind it.
    fn wait_for_change(&self, wait: Duration) {
        let changes = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        match changes.recv_timeout(wait) {
            Ok(_) => while changes.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => thread::sleep(wait),
        }
    }

    /// Checks one idle eligible worker out, with the frame buffers of the lane it will
    /// become; `None` when there is no such worker or the memory budget has no room.
    fn take_one(&self) -> Option<(WorkerHandle, Reservation<'a>)> {
        let (width, height) = self.size;
        let frames = self.budget.try_reserve(lane_bytes(width, height, 1)).ok()?;
        let accepts = |w: &WorkerInfo| self.shared.accepts(w);
        let handle = match self.workers {
            WorkerPick::Fastest(_) => checkout_ranked(
                self.registry,
                self.rates,
                width * height,
                accepts,
                1,
                self.pin,
            )
            .into_iter()
            .next(),
            _ => Registry::checkout(self.registry, accepts),
        }?;
        Some((handle, frames))
    }

    /// The rate a late lane starts from: half a chunk out of phase when the book knows
    /// its worker (the pool's tail-aware sizing does the rest), uncalibrated otherwise.
    fn start_rate(&self, key: &LaneKey) -> RateModel {
        let (width, height) = self.size;
        self.rates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key, width * height)
            .map_or_else(|| RateModel::new(INITIAL_RATE_GUESS), RateModel::staggered)
    }
}

impl LaneFeed for LateLanes<'_> {
    fn next(&self, first_index: usize, wait: Duration) -> Vec<(Arc<dyn WorkerLane>, RateModel)> {
        self.wait_for_change(wait);
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        // A lost lane's memory is free again and its worker no longer counts.
        held.retain(|h| !h.lane.lost());
        let room = match self.workers {
            WorkerPick::Fastest(n) => (n as usize).saturating_sub(held.len()),
            _ => usize::MAX,
        };
        let pixels = self.size.0 as usize * self.size.1 as usize;
        let mut late: Vec<(Arc<dyn WorkerLane>, RateModel)> = Vec::new();
        while late.len() < room {
            let Some((handle, frames)) = self.take_one() else {
                break;
            };
            let key = LaneKey::for_worker(handle.info());
            tracing::info!(
                "coordinator job: worker #{} became available; it joins the running job",
                handle.info().worker_id
            );
            let inner: Arc<dyn WorkerLane> = Arc::new(JoinedWorkerLane::new(
                handle,
                Arc::clone(self.registry),
                Arc::clone(self.shared),
            ));
            let pending: PendingSlot = Arc::new(Mutex::new(None));
            let rate = self.start_rate(&key);
            self.table
                .register(first_index + late.len(), key, Arc::clone(&pending));
            let sink = SinkLane {
                inner: Arc::clone(&inner),
                pixels,
                pending,
            };
            held.push(HeldLane {
                lane: inner,
                _frames: Some(frames),
            });
            late.push((Arc::new(sink), rate));
        }
        drop(held);
        late
    }
}
