//! The coordinator's [`WorkerLane`]s: [`JoinedWorkerLane`] over a checked-out joined
//! worker connection and [`OwnLane`] over this machine's own tracer (`serve --render`).
//!
//! # How a joined lane holds its connection
//!
//! A job checks its workers out of the [`Registry`] once, when it starts, and each
//! [`JoinedWorkerLane`] keeps its connection across chunks while they succeed back to
//! back: a job's lanes claim their next chunk immediately after merging the last one, so
//! the connection stays busy (the worker hangs up after 45 s idle; a checked-out
//! connection is never pinged). Holding per job rather than re-checking out per chunk
//! means a lane never loses its worker to another job between two chunks, and a
//! connection's stream is always drained to `DONE`/`ERROR` before anything else is
//! written to it.
//!
//! On any failed chunk the lane lets go before the pool's backoff pause: a broken stream
//! is [`WorkerHandle::discard`]ed (unregistered; `join` reconnects by itself), a worker
//! that merely refused the chunk is checked back in (idle, pinged again). The next chunk
//! then checks out ANY idle eligible worker -- possibly the same machine reconnected
//! under a new id -- so no idle connection is ever held through a pause. When the job
//! ends the lanes are dropped and every connection still held is checked back in.
//!
//! Workers that join while a job runs do not get lanes in it (a `LanePool`'s lanes are
//! fixed per run); they serve the next job. (A lane that lost its connection may still
//! pick one of them up as its replacement, exactly as it picks up a worker that
//! reconnected -- subject to the job's [`LaneNeed`].)
//!
//! # HDR jobs
//!
//! Every lane of a job shares one [`JobLanes`]: what a replacement connection must
//! accept ([`LaneNeed`] -- the pixel count and, for an HDR scene, `hdr`), the map the
//! coordinator holds for the job, and the workers that refused that map. A joined
//! worker lacking the map answers the chunk's `RENDER_REQUEST` with `NEED_ASSET`; the
//! lane answers with `ASSET` from the held copy (see `chunk`). A worker that refuses the
//! map anyway (`ASSET_FAILED`, or `UNSUPPORTED_REQUEST`) costs the lane that one chunk:
//! the chunk returns to the pool and the worker is excluded from the rest of the job,
//! so it never counts toward retiring a lane more than once.

mod chunk;

pub use chunk::LaneTimeouts;
pub(in crate::coordinator) use chunk::{POLL, PatientReader, WRITE_TIMEOUT};

use super::registry::{Registry, WorkerHandle, WorkerInfo};
use crate::{assets::HeldAsset, cli::ComputeMode, validate::MAX_SAMPLES_PER_REQUEST};
use glam::Vec3;
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_dispatch::{CancelToken, ChunkResult, SampleRange, WorkerLane};
use indicatrix_net::{SceneState, messages::error_codes};
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU32, Ordering},
};

/// What a joined worker must accept to take part in a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneNeed {
    /// The job's `width * height`: the worker's `max_pixels` must cover it.
    pub pixels: u32,
    /// The scene is lit by an HDR map: the worker must advertise `hdr`.
    pub hdr: bool,
}

impl LaneNeed {
    /// A studio-lit job of `pixels` pixels.
    #[must_use]
    pub const fn pixels(pixels: u32) -> Self {
        Self { pixels, hdr: false }
    }

    /// Whether the worker `info` describes can take the job.
    #[must_use]
    pub const fn accepts(self, info: &WorkerInfo) -> bool {
        info.capability.max_pixels >= self.pixels && (!self.hdr || info.capability.hdr)
    }
}

/// One job's state shared by all its joined lanes (see the module doc comment).
pub struct JobLanes {
    need: LaneNeed,
    asset: Option<Arc<HeldAsset>>,
    timeouts: LaneTimeouts,
    /// Workers that refused this job's HDR map: never checked out again for it.
    excluded: Mutex<Vec<u32>>,
}

impl JobLanes {
    /// The shared state of a job needing `need`, holding `asset` (an HDR scene's map).
    #[must_use]
    pub const fn new(
        need: LaneNeed,
        asset: Option<Arc<HeldAsset>>,
        timeouts: LaneTimeouts,
    ) -> Self {
        Self {
            need,
            asset,
            timeouts,
            excluded: Mutex::new(Vec::new()),
        }
    }

    /// Whether `info`'s worker can take a chunk of this job now.
    fn accepts(&self, info: &WorkerInfo) -> bool {
        self.need.accepts(info) && !self.lock_excluded().contains(&info.worker_id)
    }

    /// Keeps `worker_id` out of the rest of this job.
    fn exclude(&self, worker_id: u32) {
        let mut excluded = self.lock_excluded();
        if !excluded.contains(&worker_id) {
            excluded.push(worker_id);
        }
    }

    fn lock_excluded(&self) -> std::sync::MutexGuard<'_, Vec<u32>> {
        self.excluded.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Coordinator-local `request_id`s for requests sent to joined workers -- unique per
/// process, so a late event of an earlier chunk can never be mistaken for the current
/// chunk's.
static NEXT_WORKER_REQUEST_ID: AtomicU32 = AtomicU32::new(1);

/// A fresh coordinator-local request id.
pub(in crate::coordinator) fn next_worker_request_id() -> u32 {
    NEXT_WORKER_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// A lane over one joined worker connection (see the module doc comment for how it
/// holds and replaces its connection).
pub struct JoinedWorkerLane {
    name: String,
    registry: Arc<Registry>,
    /// The job's shared lane state: what a replacement connection must accept, the
    /// held HDR map, the excluded workers.
    job: Arc<JobLanes>,
    handle: Mutex<Option<WorkerHandle>>,
}

impl JoinedWorkerLane {
    /// A lane starting on the checked-out `handle`, for the job `job` describes.
    #[must_use]
    pub fn new(handle: WorkerHandle, registry: Arc<Registry>, job: Arc<JobLanes>) -> Self {
        Self {
            name: format!("joined worker #{}", handle.info().worker_id),
            registry,
            job,
            handle: Mutex::new(Some(handle)),
        }
    }

    /// Runs `range` on `handle`, split into requests of at most
    /// [`MAX_SAMPLES_PER_REQUEST`] samples, summing their radiance; stops at the first
    /// short one. Also returns why the connection broke, if it did.
    fn run_on(
        &self,
        handle: &mut WorkerHandle,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
    ) -> (ChunkResult, Option<String>) {
        let pixels = scene.width as usize * scene.height as usize;
        let worker_id = handle.info().worker_id;
        let mut total = ChunkResult::complete(vec![Vec3::ZERO; pixels], 0, None);
        let mut rest = range;
        let mut parts = 0;
        while !rest.is_empty() {
            let take = rest.samples.min(MAX_SAMPLES_PER_REQUEST);
            let request =
                chunk::worker_request(next_worker_request_id(), scene, rest.first_sample, take);
            let exchange = chunk::Exchange {
                worker_id,
                cancel,
                timeouts: self.job.timeouts,
                asset: self.job.asset.as_deref(),
            };
            let reply = chunk::run_request(handle.stream(), &request, exchange);
            parts += 1;
            if reply.assets_sent > 0 {
                self.registry.note_assets_forwarded(reply.assets_sent);
            }
            if self.job.need.hdr
                && matches!(
                    reply.worker_code,
                    Some(error_codes::ASSET_FAILED | error_codes::UNSUPPORTED_REQUEST)
                )
            {
                tracing::info!(
                    "coordinator job: worker #{worker_id} refused the job's HDR map; it takes no further \
                     chunk of this job"
                );
                self.job.exclude(worker_id);
            }
            let usable = if reply.done == take || reply.prefix {
                reply.done
            } else {
                0
            };
            if usable > 0 {
                for (acc, v) in total.sum.iter_mut().zip(&reply.sum) {
                    *acc += *v;
                }
                total.done += usable;
            }
            total.rate = if parts == 1 { reply.rate } else { None };
            if usable < take || reply.broken.is_some() {
                total.rate = None;
                total.error = reply
                    .error
                    .or_else(|| cancel.is_cancelled().then(|| "cancelled".to_string()))
                    .or_else(|| Some(format!("worker #{worker_id} ended the chunk early")));
                return (total, reply.broken);
            }
            rest = rest.after_prefix(take);
        }
        (total, None)
    }
}

impl WorkerLane for JoinedWorkerLane {
    fn name(&self) -> &str {
        &self.name
    }

    fn render_chunk(
        &self,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
    ) -> ChunkResult {
        let mut slot = self.handle.lock().unwrap_or_else(PoisonError::into_inner);
        let handle = slot
            .take()
            .or_else(|| Registry::checkout(&self.registry, |w| self.job.accepts(w)));
        let Some(mut handle) = handle else {
            return ChunkResult::failed("no idle joined worker to take the chunk".to_string());
        };
        let (result, broken) = self.run_on(&mut handle, scene, range, cancel);
        match broken {
            Some(why) => handle.discard(&why),
            // A clean chunk keeps the connection for the next one; a refused one goes
            // back to the registry before the pool's backoff pause.
            None if result.error.is_none() => *slot = Some(handle),
            None => drop(handle),
        }
        drop(slot);
        result
    }
}

/// The coordinator's own CPU/GPU lane (`serve --render`).
///
/// Each chunk runs the same tracer a streamed request uses
/// ([`crate::stream_emit::trace_range`]), the shared [`GpuBackend`] taking its FIFO turn
/// with every other request on this machine.
pub struct OwnLane {
    gpu: Arc<GpuBackend>,
    threads: usize,
    compute_mode: ComputeMode,
}

impl OwnLane {
    /// The own lane over `gpu` (disabled for `--only-cpu`) and `threads` CPU threads.
    #[must_use]
    pub const fn new(gpu: Arc<GpuBackend>, threads: usize, compute_mode: ComputeMode) -> Self {
        Self {
            gpu,
            threads,
            compute_mode,
        }
    }
}

impl WorkerLane for OwnLane {
    fn name(&self) -> &'static str {
        "coordinator's own lane"
    }

    fn render_chunk(
        &self,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
    ) -> ChunkResult {
        let traced = crate::stream_emit::trace_range(
            scene,
            (range.first_sample, range.samples),
            self.threads,
            self.compute_mode,
            &self.gpu,
            cancel.as_flag(),
        );
        if traced.panicked {
            return ChunkResult::partial(
                traced.sum,
                traced.done,
                "the own lane's tracer panicked on this scene".to_string(),
            );
        }
        if traced.done < range.samples {
            return ChunkResult::partial(traced.sum, traced.done, "cancelled".to_string());
        }
        ChunkResult::complete(traced.sum, traced.done, None)
    }
}
