//! Executing a viewer's request on the coordinator: routing, job fan-out, final
//! pictures and the job limits.
//!
//! # Routes
//!
//! [`plan::plan`] decides per request. A request only the own lane would serve (every
//! `Interactive` request of a `serve --render` coordinator by default) is streamed
//! **directly**, exactly like a plain worker. Anything else is a **job**: a
//! `indicatrix_dispatch::LanePool` over `[first_sample, first_sample + samples)` with one
//! lane per checked-out joined worker plus the own lane, run on a producer thread
//! ([`producer`]) while this connection's thread emits through the same
//! `crate::stream_emit` emitter a plain worker uses.
//!
//! # What the viewer receives from a job
//!
//! - `FRAME`: the sum of every chunk merged since the last emit, `first_sample =
//!   request.first_sample`, `samples` exact -- a SET of samples inside the request, not
//!   a contiguous range (see `indicatrix_net`'s `FrameHeader` docs). Encoded with the
//!   viewer's negotiated encoding. Under `FinalOnly` the one `FRAME` is the pool's
//!   deterministic chunk-order merge (`indicatrix_dispatch::Merger`).
//! - `PREVIEW`: downsampled from the coordinator's own merged total.
//! - `DISPLAY_FRAME` (`TransferMode::DisplayOnly`): the merged sum averaged, denoised
//!   and tone-mapped with the GUI's own `indicatrix::renderer::frame_denoise`, guides
//!   from the coordinator's own primary-ray prepass (computed once per request from its
//!   pose and geometry), at most one denoise in flight -- the same for a job and for the
//!   direct own-lane route (see `crate::stream_emit`'s display module).
//! - `FINAL_IMAGE` (`FinalImageRequest`): the merged sum tone-mapped with the GUI
//!   export's own function (see `crate::stream_emit`'s picture module).
//! - `PROGRESS` at least every 2 s, also while the job waits for its turn or a worker.
//! - `DONE` exactly once, `samples_done == samples`, when complete.
//! - A viewer `CANCEL` or a pipelined request cancels the pool; every joined lane sends
//!   `CANCEL` to its worker and waits (bounded, 10 s) for `DONE { cancelled }`; then
//!   `DONE { cancelled: true }`.
//! - Every lane lost: `ERROR(ALL_WORKERS_LOST)` and no `DONE`.
//!
//! # Limits
//!
//! Jobs charge [`limits::job_estimate_bytes`] against the coordinator's memory budget
//! (`--max-job-memory-mib`, default 2 GiB) and are refused past it: the job's own
//! frame buffers ([`limits::FRAME_BUFFERS_PER_JOB`]), the viewer contribution's and the
//! HDR map's bytes when it has them, and [`limits::FRAME_BUFFERS_PER_LANE`] frame
//! buffers for every lane. The lane part is charged once the lanes are checked out; a
//! job whose lanes do not all fit runs on as many as do and is refused only when not
//! even one fits (a maximum-size 8K job needs about 2.6 GiB with one lane, so it needs
//! a raised `--max-job-memory-mib`). Throughput jobs
//! (`Batch` + `FinalOnly`, and every `FinalImageRequest`) wait in their viewer's FIFO:
//! one active job per viewer certificate. Interactive requests are exempt (they are
//! superseded by the next one within a second, and take no workers by default); so are
//! direct own-lane requests, which behave exactly like a plain worker's.
//!
//! # HDR scenes
//!
//! Before an HDR request gets here the request loop holds its map
//! (`crate::assets::ensure_held`: the coordinator's own cache, else one `NEED_ASSET` to
//! the viewer). The job then takes only joined workers that advertise `hdr`
//! ([`super::LaneNeed`]), and each of them that lacks the map asks the coordinator,
//! which answers from the held copy (see `super::lanes`).
//!
//! # Load balancing across lanes of very different speed
//!
//! [`RateBook`] is now coordinator-PROCESS-wide ([`Coordinator::rates`]), not rebuilt
//! per viewer connection: a GUI export's successive one-shot connections (a fresh
//! `ViewerSession` per chunk, see `crate::serve::connection::worker`) share one book, so
//! a joined worker's calibrated rate survives across them instead of restarting from an
//! 8-sample calibration probe on every single chunk. Keyed by [`WorkerIdentity`] (a
//! worker's certificate label when it has one, else its ephemeral registration id), so
//! the rate also survives that worker's own reconnects.
//!
//! `indicatrix_dispatch::pool::epoch::Epoch::want` sizes each lane's next chunk with
//! `indicatrix_dispatch::ChunkPolicy::tail_aware_samples`: the smaller of the plain
//! target-duration chunk and that lane's proportional share of the run's remaining
//! samples (by rate, summed over every lane of the job). Ordinary chunk sizing is
//! unaffected while samples are plentiful; only a run's genuine tail shrinks, so a
//! slow lane (the coordinator's own GPU, say) can no longer claim an oversized slice of
//! what is left and leave a fast joined worker (an A100 dialed in over `join`) idling in
//! `Epoch::claim` for the whole of that one chunk.

mod limits;
mod plan;
mod producer;
mod served;

pub use limits::job_bytes;
use limits::{MemoryBudget, ViewerQueues};
pub use plan::InteractivePin;
use plan::{Ask, JobPlan, Route, WorkerPick};

use super::{LaneTimeouts, Registry, WorkerInfo};
use crate::{
    assets::{AssetCache, HeldAsset},
    cli::ComputeMode,
    stream_emit::{self, Output, StreamOutcome, StreamSpec, TimeoutRead, TimeoutWrite},
    validate,
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_dispatch::{ChunkPolicy, PoolConfig, SampleRange};
use indicatrix_net::messages::{
    ErrorMsg, FinalImageRequest, NetError, PayloadEncoding, PreviewConfig, RenderCapability,
    RenderRequest, RequestIntent, StreamConfig, StreamEvent, TransferMode, error_codes,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// The coordinator's own render lane (`serve --render`).
pub struct OwnLaneSetup {
    /// The process's GPU backend (disabled for `--only-cpu`).
    pub gpu: Arc<GpuBackend>,
    /// CPU tracer threads.
    pub threads: usize,
    /// `--only-gpu`/`--only-cpu`/hybrid.
    pub compute_mode: ComputeMode,
}

/// Scheduling knobs of coordinator jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobConfig {
    /// Throughput jobs: `ChunkPolicy::EXPORT` sizing with a coordinator failure schedule
    /// (see [`Self::default`]).
    pub batch: PoolConfig,
    /// Interactive/progressive jobs: `PoolConfig::INTERACTIVE`.
    pub interactive: PoolConfig,
    /// How long a job with no own lane waits for an idle worker before failing.
    pub lane_wait: Duration,
    /// Joined lanes' liveness deadlines.
    pub lane: LaneTimeouts,
    /// v16: how long a `FinalImageRequest`'s reserved viewer range waits for its
    /// `CONTRIBUTION` once the server's own lanes finish, before the coordinator
    /// renders that range itself (see `stream_emit::ContributionSlot::await_until`).
    /// The wait only starts once the server side is done, and is extended for as long
    /// as an upload is actually in flight, so this only needs to absorb the viewer's
    /// own rate-estimate error, not the whole render.
    pub contribution_wait: Duration,
}

impl Default for JobConfig {
    /// `batch` keeps the export's chunk sizing but not its 15-120 s backoff: a joined
    /// lane re-checks out any idle worker per attempt (and a dropped worker reconnects by
    /// itself), so a short 1-8 s schedule retiring after 4 failures in a row is enough --
    /// and a job whose workers are all gone ends with `ALL_WORKERS_LOST` within seconds
    /// instead of minutes.
    fn default() -> Self {
        Self {
            batch: PoolConfig {
                policy: ChunkPolicy::EXPORT,
                pause_after_failures: 1,
                retire_after_failures: 4,
                backoff_initial: Duration::from_secs(1),
                backoff_max: Duration::from_secs(8),
            },
            interactive: PoolConfig::INTERACTIVE,
            lane_wait: Duration::from_secs(30),
            lane: LaneTimeouts::default(),
            contribution_wait: stream_emit::DEFAULT_CONTRIBUTION_WAIT,
        }
    }
}

/// Which lane a rate belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LaneKey {
    /// The own lane.
    Own,
    /// A joined worker, by [`WorkerIdentity`].
    Worker(WorkerIdentity),
}

impl LaneKey {
    /// The lane key for a joined worker: its certificate label when it has one --
    /// [`WorkerInfo::label`]'s own doc comment calls that "the one identity that
    /// survives reconnects", exactly what a rate carried coordinator-wide (see
    /// [`RateBook`]) needs -- else its ephemeral per-registration id (no TLS label),
    /// which still lets one connection's own rate converge but starts over on reconnect.
    #[must_use]
    pub fn for_worker(info: &WorkerInfo) -> Self {
        Self::Worker(
            info.label
                .clone()
                .map_or_else(|| WorkerIdentity::Id(info.worker_id), WorkerIdentity::Label),
        )
    }
}

/// A joined worker's identity for [`LaneKey::Worker`]. See [`LaneKey::for_worker`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkerIdentity {
    /// The worker certificate's label: stable across `join --slots K` and reconnects.
    Label(String),
    /// The coordinator-assigned per-registration id: the fallback without a label.
    Id(u32),
}

/// Measured lane rates, shared coordinator-wide across every viewer connection.
///
/// Kept in [`Coordinator::rates`] so a live view's chunk sizing converges and a GUI
/// export's successive one-shot connections (one per ~22 s chunk) reuse calibration
/// instead of re-probing from scratch on every single one.
///
/// A `BTreeMap`, not a hash map, purely to keep iteration/debug output deterministic;
/// lookups are by exact key equality either way.
///
/// Stored in PIXEL-samples per second (`rate * pixels`), not plain samples per second
/// -- a lane's throughput is roughly proportional to the image's pixel count (more
/// pixels, more rays per sample), so a rate measured on one request's resolution (a
/// live-view preview, say) read back as-is for a very differently sized later request
/// (a full batch export) would size that request's chunks from a wildly wrong figure.
/// Normalising by pixel count at the edges ([`Self::get`]/[`Self::set`]) keeps
/// the estimate comparable across resolutions.
#[derive(Debug, Default)]
pub struct RateBook {
    rates: BTreeMap<LaneKey, f64>,
}

impl RateBook {
    /// The measured rate of `key` for an image of `pixels` pixels, if any: the stored
    /// pixel-samples/sec figure divided back down to plain samples/sec at this size.
    #[must_use]
    pub fn get(&self, key: &LaneKey, pixels: u32) -> Option<f64> {
        self.rates
            .get(key)
            .map(|&pixel_rate| pixel_rate / f64::from(pixels.max(1)))
    }

    /// The measured rate of joined worker `info` for an image of `pixels` pixels, if
    /// any.
    #[must_use]
    pub fn worker(&self, info: &WorkerInfo, pixels: u32) -> Option<f64> {
        self.get(&LaneKey::for_worker(info), pixels)
    }

    /// Records `rate` samples/sec measured on an image of `pixels` pixels for `key`.
    pub fn set(&mut self, key: LaneKey, pixels: u32, rate: f64) {
        self.rates.insert(key, rate * f64::from(pixels.max(1)));
    }
}

/// The process-wide coordinator state every viewer connection shares.
pub struct Coordinator {
    registry: Option<Arc<Registry>>,
    own: Option<OwnLaneSetup>,
    interactive_workers: u32,
    /// `--pin-interactive-worker` (advanced), see [`InteractivePin`].
    interactive_pin: Option<InteractivePin>,
    budget: MemoryBudget,
    queues: ViewerQueues,
    config: Mutex<JobConfig>,
    /// The HDR asset cache every job's map is held through.
    assets: Option<Arc<AssetCache>>,
    /// Every joined worker's (and the own lane's) measured rate, shared by every
    /// viewer connection this process serves -- see [`Self::rates`].
    rates: Arc<Mutex<RateBook>>,
}

impl Coordinator {
    /// A coordinator over `registry` (the worker port's, if open) and `own` (with
    /// `--render`), taking at most `interactive_workers` workers per `Interactive`
    /// request and holding at most `max_job_bytes` in in-flight jobs.
    #[must_use]
    pub fn new(
        registry: Option<Arc<Registry>>,
        own: Option<OwnLaneSetup>,
        interactive_workers: u32,
        max_job_bytes: u64,
    ) -> Self {
        Self {
            registry,
            own,
            interactive_workers,
            interactive_pin: None,
            budget: MemoryBudget::new(max_job_bytes),
            queues: ViewerQueues::default(),
            config: Mutex::new(JobConfig::default()),
            assets: None,
            rates: Arc::new(Mutex::new(RateBook::default())),
        }
    }

    /// Gives the coordinator its HDR asset cache: HDR jobs hold their map
    /// through it and forward it to joined workers. Without one, HDR scenes are refused
    /// (see `crate::assets::policy`).
    #[must_use]
    pub fn with_assets(mut self, assets: Option<Arc<AssetCache>>) -> Self {
        self.assets = assets;
        self
    }

    /// The HDR asset cache, if the coordinator keeps one.
    #[must_use]
    pub const fn assets(&self) -> Option<&Arc<AssetCache>> {
        self.assets.as_ref()
    }

    /// Pins `label`'s worker for `Interactive` requests that take workers (`serve
    /// --pin-interactive-worker`, see [`InteractivePin`]); `None` keeps the plain
    /// fastest-first pick.
    #[must_use]
    pub fn with_interactive_pin(mut self, label: Option<String>) -> Self {
        self.interactive_pin = label.map(InteractivePin::new);
        self
    }

    /// The pinned interactive worker, if any.
    #[must_use]
    pub const fn interactive_pin(&self) -> Option<&InteractivePin> {
        self.interactive_pin.as_ref()
    }

    /// The current job scheduling knobs.
    #[must_use]
    pub fn job_config(&self) -> JobConfig {
        *self.config.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the job scheduling knobs for jobs started from now on.
    pub fn set_job_config(&self, config: JobConfig) {
        *self.config.lock().unwrap_or_else(PoisonError::into_inner) = config;
    }

    /// The joined-worker registry, if the worker port is open.
    #[must_use]
    pub const fn registry(&self) -> Option<&Arc<Registry>> {
        self.registry.as_ref()
    }

    /// The own lane, with `--render`.
    #[must_use]
    pub const fn own(&self) -> Option<&OwnLaneSetup> {
        self.own.as_ref()
    }

    /// The in-flight job memory budget.
    #[must_use]
    pub const fn budget(&self) -> &MemoryBudget {
        &self.budget
    }

    /// The process-wide lane rate book, shared by every viewer connection this
    /// coordinator serves -- so a live view's chunk sizing converges across requests,
    /// and a GUI export's successive one-shot connections (a fresh `ViewerSession` per
    /// ~22 s chunk) reuse calibration instead of re-probing every single one from an
    /// 8-sample calibration chunk. Callers clone the `Arc` into their own
    /// `ViewerSession`/`Job`; nothing here is persisted to disk.
    #[must_use]
    pub const fn rates(&self) -> &Arc<Mutex<RateBook>> {
        &self.rates
    }
}

/// One viewer connection's view of the coordinator.
pub struct ViewerSession {
    /// The shared coordinator.
    pub coordinator: Arc<Coordinator>,
    /// The viewer's identity for the per-viewer FIFO: its certificate fingerprint (or
    /// its address without TLS).
    pub viewer: Arc<str>,
    /// The encoding negotiated in this connection's `WELCOME`.
    pub payload_encoding: PayloadEncoding,
    /// What `WELCOME.render` said (the baseline for `CapabilityChanged`).
    pub advertised: Option<RenderCapability>,
    /// The own lane's capability, if any (for re-advertising).
    pub own_capability: Option<RenderCapability>,
    /// The coordinator's process-wide lane rate book (see [`Coordinator::rates`]),
    /// cloned in for this connection -- NOT a fresh book per connection, so a live
    /// view's chunk sizing converges across requests and a GUI export's successive
    /// one-shot connections reuse calibration instead of restarting it every chunk.
    pub rates: Arc<Mutex<RateBook>>,
}

/// Writes `error` as a stream `ERROR`.
fn write_error<S: Write>(stream: &mut S, error: ErrorMsg) -> Result<(), NetError> {
    tracing::info!("coordinator: refusing/failing a request: {}", error.message);
    indicatrix_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)
}

/// Sends whatever a stream's outcome still owes the viewer.
fn report<S: Write>(stream: &mut S, outcome: StreamOutcome) -> Result<(), NetError> {
    match outcome {
        StreamOutcome::Completed => Ok(()),
        StreamOutcome::TracePanicked => write_error(
            stream,
            ErrorMsg {
                code: error_codes::TRACE_PANIC,
                message: "internal error while tracing this request".to_string(),
                // `report`'s own callers don't currently thread a request_id through to
                // here; `StreamOutcome::Failed`
                // below already carries whatever its own producer stamped.
                request_id: None,
            },
        ),
        StreamOutcome::Failed(error) => write_error(stream, error),
    }
}

/// Serves one viewer `RenderRequest` (see the module doc comment); returns the viewer's
/// already-pipelined next `RenderRequest`, if the emitter pulled one off the wire.
///
/// `asset` is the HDR map the request loop already holds for an HDR scene
/// (`crate::assets::ensure_held`); joined workers asking for it get it from there.
///
/// # Errors
///
/// [`NetError`] for a transport failure on the viewer connection.
pub fn serve_render<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    mut request: RenderRequest,
    session: &ViewerSession,
    asset: Option<Arc<HeldAsset>>,
) -> Result<Option<RenderRequest>, NetError> {
    if let Err(message) =
        validate::validate_request(&request.scene, request.first_sample, request.samples)
            .and_then(|()| validate::validate_stream_config(&mut request.stream, &request.scene))
    {
        write_error(
            stream,
            ErrorMsg {
                code: error_codes::VALIDATION_FAILED,
                message,
                request_id: Some(request.request_id),
            },
        )?;
        return Ok(None);
    }
    let ask = Ask {
        intent: request.intent,
        transfer_mode: request.stream.transfer_mode,
        pixels: request.scene.width * request.scene.height,
        hdr: request.scene.hdr().is_some(),
    };
    stream_request(
        stream,
        &request,
        Output::Radiance,
        ask,
        session,
        asset,
        None,
    )
}

/// Serves one `FinalImageRequest`.
///
/// The whole range runs as a `Batch` job, then one PNG goes out, tone-mapped for the
/// requested colour space with the GUI export's own `tonemap_accumulation`, per the
/// reply sequence in `indicatrix_net::messages::final_image`. `asset` as in
/// [`serve_render`].
///
/// # Errors
///
/// [`NetError`] for a transport failure on the viewer connection.
pub fn serve_final_image<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: &FinalImageRequest,
    session: &ViewerSession,
    asset: Option<Arc<HeldAsset>>,
) -> Result<Option<RenderRequest>, NetError> {
    let checked = if request.output_matches_scene() {
        validate::validate_request(&request.scene, request.first_sample, request.samples)
    } else {
        Err(format!(
            "FinalImageRequest output {}x{} must equal the scene's {}x{} (no server-side scaling in v14)",
            request.width, request.height, request.scene.width, request.scene.height
        ))
    };
    let checked = checked.and_then(|()| {
        if request.viewer_share_valid() {
            Ok(())
        } else {
            Err(format!(
                "FinalImageRequest viewer_samples ({}) must be at most half of samples ({})",
                request.viewer_samples, request.samples
            ))
        }
    });
    if let Err(message) = checked {
        write_error(
            stream,
            ErrorMsg {
                code: error_codes::VALIDATION_FAILED,
                message,
                request_id: Some(request.request_id),
            },
        )?;
        return Ok(None);
    }
    // v16: the viewer's own reserved tail, if any -- the server only plans
    // `request.server_samples()` of the range, and the job's producer thread waits for
    // this slot to fill once its own lanes finish (see `stream_request`/`producer::run`).
    let slot = request.reserved_range().map(|(first, samples)| {
        Arc::new(stream_emit::ContributionSlot::new(
            SampleRange::new(first, samples),
            request.width,
            request.height,
        ))
    });
    // Carried through the emitter as the equivalent FinalOnly request: PROGRESS on a 1 s
    // cadence, no FRAME; `Output::FinalImage` replaces the final FRAME. A downsampled
    // PREVIEW IS forced on, unlike a plain viewer's `RenderRequest` (whose `preview` is
    // whatever it asked for): a `FinalImageRequest` caller has no `StreamConfig` of its
    // own to set one on, and without this a "final picture only" export would otherwise
    // show nothing at all until the whole job completes (the owner's report this fixes).
    // See `final_image_preview_config` for the size and the emitter's own 1%-of-budget
    // gating (`stream_emit::emitter::emit::emit_tick`) for why this doesn't spam the
    // wire every cadence tick between a coordinator job's chunk merges.
    let as_render = RenderRequest {
        request_id: request.request_id,
        scene: request.scene.clone(),
        first_sample: request.first_sample,
        samples: request.samples,
        stream: StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: 1000,
            preview: Some(final_image_preview_config(request.width, request.height)),
        },
        intent: RequestIntent::Batch,
    };
    let ask = Ask {
        intent: RequestIntent::Batch,
        transfer_mode: TransferMode::FinalOnly,
        pixels: request.scene.width * request.scene.height,
        hdr: request.scene.hdr().is_some(),
    };
    let output = Output::FinalImage(request.color_space.into());
    stream_request(
        stream,
        &as_render,
        output,
        ask,
        session,
        asset,
        slot.as_ref(),
    )
}

/// Long-edge cap for a `FinalImageRequest` job's forced `PREVIEW`, matching the GUI's
/// own export-progress thumbnail size (`bridge::export_thread::preview::
/// PREVIEW_MAX_LONG_EDGE` in `indicatrix-cut`) so a server-forced preview never carries
/// more resolution than the client would ever draw.
const FINAL_IMAGE_PREVIEW_MAX_LONG_EDGE: u32 = 360;

/// The downsampled `PREVIEW` size for a `FinalImageRequest` job of `width x height`: the
/// largest size whose long edge is at most [`FINAL_IMAGE_PREVIEW_MAX_LONG_EDGE`], never
/// upsampling a request already smaller than the cap. Mirrors
/// `bridge::export_thread::preview::downsample_preview`'s own scale computation
/// (`indicatrix-cut`), so the two crates agree on what "the GUI's thumbnail size" means
/// without sharing code across the client/server boundary.
#[must_use]
fn final_image_preview_config(width: u32, height: u32) -> PreviewConfig {
    let long_edge = width.max(height).max(1);
    let scale = (f64::from(FINAL_IMAGE_PREVIEW_MAX_LONG_EDGE) / f64::from(long_edge)).min(1.0);
    PreviewConfig {
        width: ((f64::from(width) * scale).round() as u32).max(1),
        height: ((f64::from(height) * scale).round() as u32).max(1),
    }
}

/// Plans and streams one validated request (direct or as a job).
///
/// `slot`, when `Some` (a `FinalImageRequest` that reserved a viewer share), always
/// forces a job route (see the "own-lane-only" fix below) and is passed both to the
/// emitter (which routes an incoming `CONTRIBUTION` into it) and the job's producer
/// (which waits for it once its own lanes finish).
fn stream_request<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: &RenderRequest,
    output: Output,
    ask: Ask,
    session: &ViewerSession,
    asset: Option<Arc<HeldAsset>>,
    slot: Option<&Arc<stream_emit::ContributionSlot>>,
) -> Result<Option<RenderRequest>, NetError> {
    let started = Instant::now();
    let coordinator = &session.coordinator;
    if ask.hdr && asset.is_none() {
        // The request loop holds every HDR scene's map before a request gets here.
        write_error(
            stream,
            ErrorMsg {
                code: error_codes::ASSET_FAILED,
                message: "internal error: the coordinator does not hold this scene's HDR map"
                    .to_string(),
                request_id: Some(request.request_id),
            },
        )?;
        return Ok(None);
    }
    let mut route = match plan::plan(coordinator, ask) {
        Ok(route) => route,
        Err(refusal) => {
            write_error(stream, refusal)?;
            return Ok(None);
        }
    };
    // A reserved viewer share needs a Merger to fold into, even when the own lane
    // would otherwise have served this request directly (no chunking, no pool) -- see
    // `stream_emit::ContributionSlot`'s and `coordinator::job::producer`'s doc
    // comments for how the fold happens.
    if slot.is_some() && matches!(route, Route::Direct) {
        route = Route::Job(JobPlan {
            own: true,
            workers: WorkerPick::None,
            fifo: true,
            pool: coordinator.job_config().batch,
        });
    }
    // Only the direct route is a plain tracer whose silence means "wedged": a job
    // legitimately waits in its viewer's FIFO or for a joined worker with no sample
    // landing, and its lanes carry their own liveness deadlines.
    let direct = matches!(route, Route::Direct);
    let spec = StreamSpec {
        request,
        payload_encoding: session.payload_encoding,
        output,
        contribution: slot.map(Arc::as_ref),
        stall_timeout: direct.then_some(stream_emit::PRODUCER_STALL_TIMEOUT),
    };
    let (outcome, next) = match (route, coordinator.own.as_ref()) {
        (Route::Direct, Some(own)) => stream_emit::run_stream_with(
            stream,
            &spec,
            stream_emit::local_tracer(request, own.threads, &own.gpu, own.compute_mode),
        )?,
        (Route::Job(plan), _) => {
            // The memory-budget reservation itself happens inside `producer::run`,
            // AFTER it waits its turn in the viewer's FIFO -- a job queued behind
            // another one of the same viewer must not hold budget while it hasn't
            // even started.
            //
            // `range` is the SERVER's own share: `request.samples` (the whole range,
            // used unchanged as the emitter's tone-map/progress divisor) minus however
            // many samples `slot` reserved for the viewer.
            let viewer_samples = slot.map_or(0, |s| s.reserved().samples);
            let job = producer::Job {
                coordinator: Arc::clone(coordinator),
                viewer: Arc::clone(&session.viewer),
                rates: Arc::clone(&session.rates),
                scene: request.scene.clone(),
                range: SampleRange::new(request.first_sample, request.samples - viewer_samples),
                plan,
                asset,
                // `spec.contribution` (built above) borrows through `slot` for the
                // emitter's whole run; the job gets its own clone of the same `Arc`.
                contribution: slot.cloned(),
            };
            stream_emit::run_stream_with(stream, &spec, move |sink| producer::run(&job, sink))?
        }
        // `plan` only routes Direct with an own lane; every other combination is a
        // planning bug, not a silent no-op.
        (Route::Direct, None) => {
            write_error(
                stream,
                ErrorMsg {
                    code: error_codes::NO_RENDER_CAPACITY,
                    message: "internal error: the coordinator planned a direct route with no own \
                              render lane"
                        .to_string(),
                    request_id: Some(request.request_id),
                },
            )?;
            return Ok(None);
        }
    };
    served::log(request, ask, direct, started.elapsed(), &outcome);
    report(stream, outcome)?;
    Ok(next)
}

/// The refusal past the memory budget. There is no dedicated v14 code for "busy";
/// `CONNECTION_LIMIT_REACHED` ("this server is at capacity") is the closest, and a
/// viewer treats it as a failed remote request and falls back to local rendering.
pub(super) fn over_budget(coordinator: &Coordinator, bytes: u64) -> ErrorMsg {
    const MIB: u64 = 1024 * 1024;
    ErrorMsg {
        code: error_codes::CONNECTION_LIMIT_REACHED,
        // This helper has no
        // request in scope; its one caller (`producer::run`) could thread one through.
        request_id: None,
        message: format!(
            "coordinator busy: this job needs {} MiB of in-flight buffers but {} of {} MiB are already in \
             use (--max-job-memory-mib); try again later",
            bytes.div_ceil(MIB),
            coordinator.budget.in_use().div_ceil(MIB),
            coordinator.budget.cap() / MIB
        ),
    }
}

#[cfg(test)]
mod tests;
