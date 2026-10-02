//! The coordinator's [`WorkerLane`]s: [`JoinedWorkerLane`] over a checked-out joined
//! worker connection and [`OwnLane`] over this machine's own tracer (`serve --render`).
//!
//! # How a joined lane holds its connection
//!
//! A job checks its workers out of the [`Registry`] once, when it starts, and each
//! [`JoinedWorkerLane`] keeps its connection across chunks while they succeed back to
//! back. Holding per job rather than re-checking out per chunk means a lane never loses
//! its worker to another job between two chunks, and a connection's stream is always
//! drained to `DONE`/`ERROR` before anything else is written to it.
//!
//! Between two chunks the connection can genuinely sit idle for a while: a lane with
//! nothing left to claim waits (`indicatrix_dispatch::pool::epoch::Epoch::claim`) while
//! another lane's straggling chunk is still in flight, and the registry's own liveness
//! `PING` only reaches CHECKED-IN (idle-in-the-registry) connections -- a checked-out one
//! is invisible to it. Left alone, that combination lets a perfectly healthy connection
//! sit past the 45 s a joined worker hangs up at (`join::session::JOIN_IDLE_TIMEOUT`)
//! with no traffic at all, and the worker drops it out from under the job. Each
//! `JoinedWorkerLane` therefore runs its own small heartbeat thread
//! ([`held_ping_loop`]) for as long as it holds a connection idle between chunks,
//! `PING`ing it well before that deadline; `render_chunk` holds the same lock for the
//! whole duration of an actual request, so the heartbeat only ever touches a connection
//! that is genuinely between chunks, never one mid-request. The heartbeat itself only
//! holds that lock long enough to lift the connection out and (on a successful ping) put
//! it back, never across the `PING`/`PONG` round trip -- see
//! [`try_heartbeat_once`](self::try_heartbeat_once)'s doc comment for why that is still
//! never racing `render_chunk` over the same connection, at the cost of `render_chunk`
//! occasionally checking out a different idle worker instead of waiting the ping out.
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
//! # Chunk sizing across lanes of very different speed
//!
//! Each lane's next chunk is sized tail-aware and share-aware
//! (`indicatrix_dispatch::pool::epoch::Epoch::want` ->
//! `indicatrix_dispatch::ChunkPolicy::tail_aware_samples`): the smaller of the plain
//! target-duration chunk and this lane's proportional share (by rate) of the run's
//! outstanding samples. A fast joined worker (an A100 dialed in over `join`) can
//! therefore no longer be left idling in `Epoch::claim` while a much slower lane (the
//! coordinator's own GPU, say) works through an oversized last chunk it grabbed simply
//! because its own target-duration size happened to exceed what was left. The rates
//! that sizing (and the coordinator's own fastest-worker ranking for `Interactive`
//! requests) reads come from `super::job::Coordinator::rates`, a book shared
//! coordinator-wide across every viewer connection -- not rebuilt per connection -- so
//! a joined worker's calibration
//! survives a GUI export's successive one-shot connections and, keyed by its
//! certificate label, its own reconnects too. A live-view (`Interactive`) request
//! takes every idle joined worker besides the own lane by default
//! (`serve --interactive-workers all`); `--interactive-workers 0` serves it from the
//! own lane alone and `--interactive-workers <n>` caps it at the `n` fastest.
//!
//! A joined lane reports the rate of its tracing alone: the samples between the worker's
//! first and its last `PROGRESS` that advanced the count, over the time between those
//! two reports. The `FinalOnly` `FRAME` that follows the last advance (encode, upload,
//! decode) and the `DONE` after it are not part of the span, so a worker's rate does not
//! sink with the size of the picture it uploads and the proportional share it is given
//! stays in line with what it traces. A chunk too short for two advancing reports reports
//! first-report-to-`DONE` instead (see `chunk::rate_from_progress`).
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
use indicatrix_net::{
    SceneState,
    messages::{ClientMessage, StreamEvent, error_codes},
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
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

/// A [`JoinedWorkerLane`]'s connection and when it last saw traffic, shared with its own
/// heartbeat thread ([`held_ping_loop`]) -- see the module doc comment.
struct Held {
    handle: Mutex<Option<WorkerHandle>>,
    last_traffic: Mutex<Instant>,
}

/// How long a checked-out connection may go without traffic before a lane pings it
/// itself: comfortably under the 45 s a joined worker hangs up at
/// (`join::session::JOIN_IDLE_TIMEOUT`) and under the registry's own idle-ping cadence
/// (`liveness`'s 10 s), so a lane sitting in `Epoch::claim` waiting for other lanes to
/// finish never looks silent to the worker it is holding.
const HELD_PING_INTERVAL: Duration = Duration::from_secs(15);

/// How often [`held_ping_loop`] wakes to check -- short so [`JoinedWorkerLane`]'s `Drop`
/// never waits long for it to notice `stop`.
const HELD_PING_POLL: Duration = Duration::from_millis(200);

/// How long one heartbeat `PING` may take to get its `PONG`.
const HELD_PING_DEADLINE: Duration = Duration::from_secs(5);

/// A lane over one joined worker connection (see the module doc comment for how it
/// holds and replaces its connection).
pub struct JoinedWorkerLane {
    name: String,
    registry: Arc<Registry>,
    /// The job's shared lane state: what a replacement connection must accept, the
    /// held HDR map, the excluded workers.
    job: Arc<JobLanes>,
    held: Arc<Held>,
    heartbeat_stop: Arc<AtomicBool>,
    heartbeat: Option<thread::JoinHandle<()>>,
}

impl JoinedWorkerLane {
    /// A lane starting on the checked-out `handle`, for the job `job` describes. Spawns
    /// this lane's own heartbeat thread (see the module doc comment); a spawn failure
    /// (vanishingly unlikely) just means no heartbeat, not a construction failure --
    /// the ordinary liveness deadlines still apply, just without the extra margin.
    #[must_use]
    pub fn new(handle: WorkerHandle, registry: Arc<Registry>, job: Arc<JobLanes>) -> Self {
        let name = format!("joined worker #{}", handle.info().worker_id);
        let held = Arc::new(Held {
            handle: Mutex::new(Some(handle)),
            last_traffic: Mutex::new(Instant::now()),
        });
        let heartbeat_stop = Arc::new(AtomicBool::new(false));
        let heartbeat = {
            let held = Arc::clone(&held);
            let stop = Arc::clone(&heartbeat_stop);
            thread::Builder::new()
                .name(format!("{name}-heartbeat"))
                .spawn(move || held_ping_loop(&held, &stop))
                .ok()
        };
        Self {
            name,
            registry,
            job,
            held,
            heartbeat_stop,
            heartbeat,
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
        let mut slot = self
            .held
            .handle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let handle = slot
            .take()
            .or_else(|| Registry::checkout(&self.registry, |w| self.job.accepts(w)));
        let Some(mut handle) = handle else {
            return ChunkResult::failed("no idle joined worker to take the chunk".to_string());
        };
        let (result, broken) = self.run_on(&mut handle, scene, range, cancel);
        *self
            .held
            .last_traffic
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
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

impl Drop for JoinedWorkerLane {
    /// Stops and joins this lane's heartbeat thread -- bounded by [`HELD_PING_POLL`],
    /// not [`HELD_PING_INTERVAL`], so ending a job never waits long per lane.
    fn drop(&mut self) {
        self.heartbeat_stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.heartbeat.take() {
            let _ = thread.join();
        }
    }
}

/// One [`JoinedWorkerLane`]'s heartbeat thread body (see the module doc comment): while
/// `stop` isn't set, every [`HELD_PING_INTERVAL`] with no other traffic, `PING`s the
/// connection currently idling in `held.handle`, if any.
///
/// `render_chunk` holds `held.handle`'s lock for the WHOLE duration of an actual
/// request, so a successful `try_lock` here, finding a connection in it, means the lane
/// is genuinely between chunks right now -- never mid-request, never racing
/// `render_chunk`'s own use of the same connection.
fn held_ping_loop(held: &Held, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(HELD_PING_POLL);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let due = held
            .last_traffic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
            >= HELD_PING_INTERVAL;
        if due {
            try_heartbeat_once(held);
        }
    }
}

/// One heartbeat attempt for [`held_ping_loop`]: pings the connection currently idling
/// in `held.handle`, if any, bumping `held.last_traffic` on success or discarding the
/// connection on failure. A no-op when nothing is checked out there right now, or when
/// `render_chunk` is already using it (`try_lock` finds it busy).
///
/// `held.handle`'s lock is taken only to lift the connection out and, on success, to put
/// it back -- never held across the ping itself (a network round trip): while it is out,
/// `render_chunk`'s own blocking `lock()` sees `None` and checks out a fresh idle worker
/// from the registry rather than waiting, which is fine -- this worker's registry slot
/// stays `CheckedOut` for as long as this `JoinedWorkerLane` exists regardless of whether
/// its handle currently sits in `held.handle` or in this function's local `handle`, so
/// nothing else can ever pick up the SAME connection out from under this ping.
fn try_heartbeat_once(held: &Held) {
    let mut slot = match held.handle.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            // `render_chunk` is using the connection right now: real traffic, no
            // heartbeat needed.
            return;
        }
    };
    let Some(mut handle) = slot.take() else {
        // Nothing checked out right now (lost earlier); `render_chunk` checks out
        // a replacement on its own next call.
        return;
    };
    drop(slot);
    match ping_held(&mut handle) {
        Ok(()) => {
            *held
                .last_traffic
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Instant::now();
            *held.handle.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        }
        Err(why) => {
            tracing::info!(
                "coordinator job: heartbeat ping to worker #{} failed while it idled between \
                 chunks: {why}",
                handle.info().worker_id
            );
            handle.discard(&why);
            // `held.handle` stays `None`; `render_chunk`'s own checkout picks a
            // replacement on its next call.
        }
    }
}

/// Sends `PING` on `handle`'s connection and waits (bounded by [`HELD_PING_DEADLINE`])
/// for the matching `PONG`.
fn ping_held(handle: &mut WorkerHandle) -> Result<(), String> {
    // A fixed nonce is fine: this exchange is fully synchronous, with `held.handle`'s
    // lock excluding `render_chunk` for its duration, so nothing else could be waiting
    // on a PONG of its own over this connection at the same time.
    const NONCE: u64 = 0x4845_4152_5442_4954; // "HEARTBIT" in ASCII hex, arbitrary.
    let mut conn = handle.stream();
    conn.set_timeouts(Some(HELD_PING_DEADLINE), Some(HELD_PING_DEADLINE))
        .map_err(|e| format!("could not arm the heartbeat ping deadline: {e}"))?;
    indicatrix_net::messages::write_message(&mut conn, &ClientMessage::Ping { nonce: NONCE })
        .map_err(|e| format!("heartbeat PING failed: {e}"))?;
    loop {
        let (event, _payload) = indicatrix_net::messages::read_stream_event(&mut conn)
            .map_err(|e| format!("heartbeat: no PONG ({e})"))?;
        match event {
            StreamEvent::Pong { nonce: NONCE } => {
                let _ = conn.set_timeouts(None, None);
                return Ok(());
            }
            other => tracing::debug!("coordinator: ignoring {other:?} from a held idle worker"),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::LivenessConfig;
    use indicatrix_net::messages::{Backend, PayloadEncoding, RenderCapability};
    use std::{
        net::{TcpListener, TcpStream},
        thread,
    };

    /// A checked-out [`WorkerHandle`] over a real loopback TCP pair, and the far end
    /// (kept open so the connection stays live).
    fn checked_out_handle() -> (WorkerHandle, TcpStream) {
        let registry = Registry::new(LivenessConfig::default());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback listener");
        let addr = listener
            .local_addr()
            .expect("a bound listener has a local address");
        let near = TcpStream::connect(addr).expect("connect to our own listener");
        let far = listener
            .accept()
            .expect("accept the connection just made")
            .0;
        let worker_id = registry.allocate_id();
        let info = WorkerInfo {
            worker_id,
            capability: RenderCapability {
                backend: Backend::Cpu { threads: 1 },
                max_pixels: 1_000,
                min_cadence_ms: 100,
                hdr: false,
            },
            peer: None,
            label: None,
            payload_encoding: PayloadEncoding::Raw,
        };
        registry.insert(info, Box::new(near), None);
        let handle =
            Registry::checkout(&registry, |_| true).expect("the just-inserted worker is idle");
        (handle, far)
    }

    /// A peer that reads exactly one `PING` and answers `PONG` with the same nonce,
    /// then stops (a stand-in for a joined worker's own request loop between chunks).
    fn answer_one_ping(mut far: TcpStream) {
        thread::spawn(move || {
            let message: ClientMessage = indicatrix_net::messages::read_message(&mut far)
                .expect("the heartbeat sends exactly one PING");
            let ClientMessage::Ping { nonce } = message else {
                panic!("expected ClientMessage::Ping, got {message:?}");
            };
            indicatrix_net::messages::write_stream_event(
                &mut far,
                &StreamEvent::Pong { nonce },
                None,
            )
            .expect("writing PONG must not fail on a live loopback socket");
        });
    }

    #[test]
    fn ping_held_succeeds_against_a_peer_that_answers_pong() {
        let (mut handle, far) = checked_out_handle();
        answer_one_ping(far);
        ping_held(&mut handle).expect("a live peer answering PONG must succeed");
    }

    #[test]
    fn ping_held_fails_against_a_silent_peer() {
        let (mut handle, _far) = checked_out_handle();
        // `_far` is kept alive (not dropped) but never answers -- `ping_held` must
        // still fail once `HELD_PING_DEADLINE` passes, not hang.
        assert!(ping_held(&mut handle).is_err());
    }
}
