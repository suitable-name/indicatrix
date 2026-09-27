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
//! Jobs charge [`limits::job_bytes`] against the coordinator's memory budget
//! (`--max-job-memory-mib`, default 2 GiB) and are refused past it. Throughput jobs
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

mod limits;
mod plan;
mod producer;

pub use limits::job_bytes;
use limits::{MemoryBudget, ViewerQueues};
pub use plan::InteractivePin;
use plan::{Ask, Route};

use super::{LaneTimeouts, Registry};
use crate::{
    assets::{AssetCache, HeldAsset},
    cli::ComputeMode,
    stream_emit::{self, Output, StreamOutcome, StreamSpec, TimeoutRead, TimeoutWrite},
    validate,
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_dispatch::{ChunkPolicy, PoolConfig, SampleRange};
use indicatrix_net::messages::{
    ErrorMsg, FinalImageRequest, NetError, PayloadEncoding, RenderCapability, RenderRequest,
    RequestIntent, StreamConfig, StreamEvent, TransferMode, error_codes,
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
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
        }
    }
}

/// Which lane a rate belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaneKey {
    /// The own lane.
    Own,
    /// A joined worker connection, by `worker_id`.
    Worker(u32),
}

/// Measured lane rates (samples per second), kept per viewer connection across its
/// requests so a live view's chunk sizing converges.
#[derive(Debug, Default)]
pub struct RateBook {
    rates: HashMap<LaneKey, f64>,
}

impl RateBook {
    /// The measured rate of `key`, if any.
    #[must_use]
    pub fn get(&self, key: LaneKey) -> Option<f64> {
        self.rates.get(&key).copied()
    }

    /// The measured rate of joined worker `worker_id`, if any.
    #[must_use]
    pub fn worker(&self, worker_id: u32) -> Option<f64> {
        self.get(LaneKey::Worker(worker_id))
    }

    /// Records `rate` for `key`.
    pub fn set(&mut self, key: LaneKey, rate: f64) {
        self.rates.insert(key, rate);
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
    /// Lane rates carried across this connection's requests.
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
    stream_request(stream, &request, Output::Radiance, ask, session, asset)
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
    if let Err(message) = checked {
        write_error(
            stream,
            ErrorMsg {
                code: error_codes::VALIDATION_FAILED,
                message,
            },
        )?;
        return Ok(None);
    }
    // Carried through the emitter as the equivalent FinalOnly request: PROGRESS on a 1 s
    // cadence, no FRAME, no PREVIEW; `Output::FinalImage` replaces the final FRAME.
    let as_render = RenderRequest {
        request_id: request.request_id,
        scene: request.scene.clone(),
        first_sample: request.first_sample,
        samples: request.samples,
        stream: StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: 1000,
            preview: None,
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
    stream_request(stream, &as_render, output, ask, session, asset)
}

/// Plans and streams one validated request (direct or as a job).
fn stream_request<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: &RenderRequest,
    output: Output,
    ask: Ask,
    session: &ViewerSession,
    asset: Option<Arc<HeldAsset>>,
) -> Result<Option<RenderRequest>, NetError> {
    let coordinator = &session.coordinator;
    if ask.hdr && asset.is_none() {
        // The request loop holds every HDR scene's map before a request gets here.
        write_error(
            stream,
            ErrorMsg {
                code: error_codes::ASSET_FAILED,
                message: "internal error: the coordinator does not hold this scene's HDR map"
                    .to_string(),
            },
        )?;
        return Ok(None);
    }
    let route = match plan::plan(coordinator, ask) {
        Ok(route) => route,
        Err(refusal) => {
            write_error(stream, refusal)?;
            return Ok(None);
        }
    };
    let spec = StreamSpec {
        request,
        payload_encoding: session.payload_encoding,
        output,
    };
    let (outcome, next) = match (route, coordinator.own.as_ref()) {
        (Route::Direct, Some(own)) => stream_emit::run_stream_with(
            stream,
            &spec,
            stream_emit::local_tracer(request, own.threads, &own.gpu, own.compute_mode),
        )?,
        (Route::Job(plan), _) => {
            let bytes = job_bytes(request.scene.width, request.scene.height);
            let Ok(_reservation) = coordinator.budget.try_reserve(bytes) else {
                write_error(stream, over_budget(coordinator, bytes))?;
                return Ok(None);
            };
            let job = producer::Job {
                coordinator: Arc::clone(coordinator),
                viewer: Arc::clone(&session.viewer),
                rates: Arc::clone(&session.rates),
                scene: request.scene.clone(),
                range: SampleRange::new(request.first_sample, request.samples),
                plan,
                asset,
            };
            stream_emit::run_stream_with(stream, &spec, move |sink| producer::run(&job, sink))?
        }
        // `plan` only routes Direct with an own lane.
        (Route::Direct, None) => return Ok(None),
    };
    report(stream, outcome)?;
    Ok(next)
}

/// The refusal past the memory budget. There is no dedicated v14 code for "busy";
/// `CONNECTION_LIMIT_REACHED` ("this server is at capacity") is the closest, and a
/// viewer treats it as a failed remote request and falls back to local rendering.
fn over_budget(coordinator: &Coordinator, bytes: u64) -> ErrorMsg {
    const MIB: u64 = 1024 * 1024;
    ErrorMsg {
        code: error_codes::CONNECTION_LIMIT_REACHED,
        message: format!(
            "coordinator busy: this job needs {} MiB of in-flight buffers but {} of {} MiB are already in \
             use (--max-job-memory-mib); try again later",
            bytes.div_ceil(MIB),
            coordinator.budget.in_use().div_ceil(MIB),
            coordinator.budget.cap() / MIB
        ),
    }
}
