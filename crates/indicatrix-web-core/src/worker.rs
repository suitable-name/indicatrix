//! [`WorkerHandler`]: everything a Web Worker does with a message, as a plain state
//! machine.
//!
//! The `indicatrix-web-compute` crate only decodes the bytes, calls
//! [`WorkerHandler::handle`] with a `performance.now()` clock, and posts the reply, so
//! the whole Worker behaviour is tested natively here.

use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use glam::Vec3;
use indicatrix::{
    color::metrics::{MetricsCache, SweepProgress},
    renderer::env_map::{EnvironmentMap, environment_from_hdr_bytes},
};
use indicatrix_cut_core::optimize::SearchStage;
use indicatrix_editor::optimize_view::optimize_progress_status;

use crate::{
    display::{DenoiseFrame, Denoiser, export_color_space, export_png},
    hdr::web_hdr_limits,
    protocol::{FromWorker, PROTOCOL_VERSION, PictureKind, ToWorker, WorkerRole},
    render::{ChunkRange, trace_chunk_abortable},
    scene::{OwnedScene, SceneSpec},
    solve::{
        ProgressReport, SolveHooks, SolveRequest, SolveResponse, handle_solve_with,
        run_metrics_with_map, sweep_fraction, sweep_status,
    },
};

#[cfg(test)]
mod hdr_tests;
#[cfg(test)]
mod tests;

/// How often a running search's progress reaches the page at most (a stage change is
/// always reported): a search reports after every tier decision, far more often than a
/// status line can be read.
const PROGRESS_INTERVAL_MS: f64 = 200.0;

/// How often a running search asks its cancel URL whether the page gave up: a probe is
/// a synchronous request, far dearer than a tier decision.
pub const CANCEL_POLL_INTERVAL_MS: f64 = 100.0;

/// What asking a job's cancel URL told the Worker (the answer of the browser's probe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelProbe {
    /// The URL still answers: the page has not cancelled the job.
    Live,
    /// The probe works and the URL no longer answers: the page revoked it, which is how
    /// it cancels.
    Revoked,
    /// The probe could not run at all (no request object, a request the browser or a
    /// content-security policy refuses, an answer with no status). Says nothing about
    /// what the page wants, so the job carries on and the URL is not asked again.
    Unavailable,
}

/// The cached scene and whether it lights with the HDR map.
struct CachedScene {
    scene_id: u64,
    scene: OwnedScene,
    uses_hdr: bool,
}

/// One Worker's state: its role, the cached scene and denoiser (render), the HDR map
/// (render, and solve for the metrics' lighting) and the cancelled-job mark (solve).
#[derive(Default)]
pub struct WorkerHandler {
    role: Option<WorkerRole>,
    worker_index: u32,
    scene: Option<CachedScene>,
    /// The decoded map and its id: a render Worker lights its scene with it, the analysis
    /// Worker scores the current-view metrics under it.
    hdr: Option<(u64, Arc<EnvironmentMap>)>,
    denoiser: Denoiser,
    /// One more than the newest solve job id a [`ToWorker::Cancel`] named (`0`: none).
    /// Atomic because a running job polls it from its progress hooks: where the Worker
    /// can be handed a message mid-job this is how a job learns of a cancel without a
    /// readable cancel URL; in a single-threaded browser Worker a message is only read
    /// between jobs, so there it skips queued jobs.
    cancel_mark: Arc<AtomicU64>,
    /// The `(job, URL)` a [`ToWorker::WatchCancel`] named for the job about to run.
    cancel_watch: Option<(u64, String)>,
    /// The current-view metrics of the last `SolveRequest::Metrics`, so a pose it already
    /// holds is answered without evaluating (the desktop render loop's cache).
    metrics_cache: Option<MetricsCache>,
}

impl WorkerHandler {
    /// A Worker that has not been given a role yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The message a Worker posts once its handler is installed.
    #[must_use]
    pub const fn loaded_message() -> FromWorker {
        FromWorker::Loaded {
            protocol_version: PROTOCOL_VERSION,
        }
    }

    /// The role given by `Init`, if any.
    #[must_use]
    pub const fn role(&self) -> Option<WorkerRole> {
        self.role
    }

    /// The index given by `Init` (0 before it).
    #[must_use]
    pub const fn worker_index(&self) -> u32 {
        self.worker_index
    }

    /// Handles one message; `now_ms` is the Worker's monotonic clock in milliseconds.
    /// Returns the reply to post, if any. A long solve job's progress reports are dropped
    /// (see [`Self::handle_streaming`]).
    pub fn handle(&mut self, message: ToWorker, now_ms: &dyn Fn() -> f64) -> Option<FromWorker> {
        self.handle_streaming(message, now_ms, &|_| {})
    }

    /// [`Self::handle`], with `emit` called for every intermediate message a long job
    /// posts before its reply -- today the [`FromWorker::Progress`] reports of an Optimize
    /// or Retarget search, at most one per [`PROGRESS_INTERVAL_MS`]. The Worker entry
    /// point posts each one as it is emitted (a Worker cannot be interrupted, but it can
    /// post while it computes).
    pub fn handle_streaming(
        &mut self,
        message: ToWorker,
        now_ms: &dyn Fn() -> f64,
        emit: &dyn Fn(FromWorker),
    ) -> Option<FromWorker> {
        self.handle_probed(message, now_ms, emit, &|_| CancelProbe::Live)
    }

    /// [`Self::handle_streaming`], with `probe` answering, for the URL a
    /// [`ToWorker::WatchCancel`] named, whether the page has cancelled the job.
    ///
    /// A running search polls it between tier decisions, and a chunk between its row
    /// groups (at most once per [`CANCEL_POLL_INTERVAL_MS`]); [`CancelProbe::Revoked`]
    /// stops the job, which then answers with its best result so far (a chunk:
    /// [`FromWorker::ChunkAborted`]). The Worker entry point supplies the browser's probe
    /// (a synchronous request to the `blob:` URL, which fails once the page revokes it).
    /// [`CancelProbe::Unavailable`] is not a cancel: the URL is given up on for the rest
    /// of the job, and only a [`ToWorker::Cancel`] can stop it.
    pub fn handle_probed(
        &mut self,
        message: ToWorker,
        now_ms: &dyn Fn() -> f64,
        emit: &dyn Fn(FromWorker),
        probe: &dyn Fn(&str) -> CancelProbe,
    ) -> Option<FromWorker> {
        match message {
            ToWorker::Init {
                protocol_version,
                role,
                worker_index,
            } => Some(self.init(protocol_version, role, worker_index)),
            ToWorker::SetScene { scene_id, spec } => self
                .require(WorkerRole::Render, "SetScene")
                .err()
                .or_else(|| self.set_scene(scene_id, &spec)),
            ToWorker::HdrMap { id, bytes } => Some(
                self.require_one_of(&[WorkerRole::Render, WorkerRole::Solve], "HdrMap")
                    .err()
                    .unwrap_or_else(|| self.load_hdr(id, &bytes)),
            ),
            ToWorker::ClearHdr => {
                self.hdr = None;
                if self.scene.as_ref().is_some_and(|s| s.uses_hdr) {
                    self.scene = None;
                }
                None
            }
            ToWorker::TraceChunk {
                scene_id,
                first_pixel,
                stride,
                sample_offset,
                spp,
            } => Some(
                self.require(WorkerRole::Render, "TraceChunk")
                    .err()
                    .unwrap_or_else(|| {
                        let range = ChunkRange {
                            first_pixel,
                            stride,
                            sample_offset,
                            spp,
                        };
                        self.trace(scene_id, range, now_ms, probe)
                    }),
            ),
            ToWorker::Solve {
                job_id,
                design_toml,
                request,
            } => Some(
                self.require(WorkerRole::Solve, "Solve")
                    .err()
                    .unwrap_or_else(|| {
                        let mark = Arc::clone(&self.cancel_mark);
                        let env = JobEnv {
                            now_ms,
                            emit,
                            watch: None,
                            probe,
                            mark: &mark,
                        };
                        self.solve_job(job_id, &design_toml, &request, &env)
                    }),
            ),
            ToWorker::Cancel { job_id } => {
                self.cancel_mark
                    .fetch_max(job_id.saturating_add(1), Ordering::Relaxed);
                None
            }
            ToWorker::WatchCancel { job_id, url } => {
                self.cancel_watch = Some((job_id, url));
                None
            }
            ToWorker::Picture {
                scene_id,
                sample_count,
                sums,
                kind,
            } => Some(
                self.require(WorkerRole::Render, "Picture")
                    .err()
                    .unwrap_or_else(|| {
                        self.picture_reply(scene_id, sample_count, &sums, kind, now_ms)
                    }),
            ),
        }
    }

    /// The reply to a [`ToWorker::Picture`]: the picture, or why it could not be made.
    fn picture_reply(
        &mut self,
        scene_id: u64,
        sample_count: u32,
        sums: &[[f32; 3]],
        kind: PictureKind,
        now_ms: &dyn Fn() -> f64,
    ) -> FromWorker {
        let start = now_ms();
        match self.picture(scene_id, sample_count, sums, kind) {
            Ok(bytes) => FromWorker::Picture {
                scene_id,
                sample_count,
                kind,
                bytes,
                elapsed_ms: now_ms() - start,
            },
            Err(message) => FromWorker::PictureFailed {
                scene_id,
                kind,
                message,
            },
        }
    }

    /// Runs solve job `job_id` unless it was cancelled while queued, taking the cancel URL
    /// a [`ToWorker::WatchCancel`] named for it (`env.watch` is filled in here).
    fn solve_job(
        &mut self,
        job_id: u64,
        design_toml: &str,
        request: &SolveRequest,
        env: &JobEnv<'_>,
    ) -> FromWorker {
        let watch = take_watch(&mut self.cancel_watch, job_id);
        if job_id < self.cancel_mark.load(Ordering::Relaxed) {
            return FromWorker::SolveResult {
                job_id,
                response: SolveResponse::Cancelled,
                elapsed_ms: 0.0,
            };
        }
        let start = (env.now_ms)();
        let env = JobEnv {
            watch: watch.as_deref(),
            ..*env
        };
        let response = match request {
            // Cheap and stateful: answered here, from the Worker's own cache, under the
            // HDR map it holds when the request names it.
            SolveRequest::Metrics { params } => run_metrics_with_map(
                &mut self.metrics_cache,
                params,
                self.hdr.as_ref().map(|(id, map)| (*id, map.as_ref())),
            ),
            _ => run_solve_job(job_id, design_toml, request, start, &env),
        };
        FromWorker::SolveResult {
            job_id,
            response,
            elapsed_ms: (env.now_ms)() - start,
        }
    }

    /// Makes `kind` from a sum of the cached scene `scene_id` (see `crate::display`).
    /// The guides are cached per scene; an export's are dropped afterwards, since an
    /// export frame is large and is not denoised twice.
    fn picture(
        &mut self,
        scene_id: u64,
        sample_count: u32,
        sums: &[[f32; 3]],
        kind: PictureKind,
    ) -> Result<Vec<u8>, String> {
        let Some(cached) = self.scene.as_ref().filter(|s| s.scene_id == scene_id) else {
            return Err(format!("this worker no longer holds scene {scene_id}"));
        };
        let scene = &cached.scene;
        let (width, height) = (scene.width(), scene.height());
        if sums.len() != width as usize * height as usize {
            return Err(format!(
                "a {}-pixel sum was sent for a {width}x{height} scene",
                sums.len()
            ));
        }
        let sum: Vec<Vec3> = sums.iter().copied().map(Vec3::from_array).collect();
        let frame = DenoiseFrame {
            guide_key: scene_id,
            width,
            height,
            camera: scene.camera(),
            planes: scene.planes(),
            sample_count,
            sum: &sum,
        };
        match kind {
            PictureKind::DenoisedLive => Ok(self.denoiser.denoised_rgba(&frame)),
            PictureKind::Png {
                color_space,
                denoise,
            } => {
                let mean = denoise.then(|| self.denoiser.denoised_mean(&frame));
                if denoise {
                    self.denoiser.clear();
                }
                export_png(
                    width,
                    height,
                    sample_count,
                    &sum,
                    export_color_space(color_space),
                    mean.as_deref(),
                )
            }
        }
    }

    fn init(&mut self, protocol_version: u32, role: WorkerRole, worker_index: u32) -> FromWorker {
        if protocol_version != PROTOCOL_VERSION {
            return FromWorker::Error {
                message: format!(
                    "worker protocol mismatch: the page speaks version {protocol_version}, \
                     this worker script speaks {PROTOCOL_VERSION} -- reload the page to \
                     fetch matching files"
                ),
            };
        }
        self.role = Some(role);
        self.worker_index = worker_index;
        FromWorker::Ready { role, worker_index }
    }

    /// `Ok` when this Worker has `role`; otherwise the error reply.
    fn require(&self, role: WorkerRole, what: &str) -> Result<(), FromWorker> {
        self.require_one_of(&[role], what)
    }

    /// `Ok` when this Worker has one of `roles`; otherwise the error reply.
    fn require_one_of(&self, roles: &[WorkerRole], what: &str) -> Result<(), FromWorker> {
        if self.role.is_some_and(|role| roles.contains(&role)) {
            Ok(())
        } else {
            let wanted = match roles {
                [only] => format!("{only:?}"),
                _ => format!("one of {roles:?}"),
            };
            Err(FromWorker::Error {
                message: format!(
                    "{what} sent to a worker whose role is {:?}, not {wanted}",
                    self.role
                ),
            })
        }
    }

    fn set_scene(&mut self, scene_id: u64, spec: &SceneSpec) -> Option<FromWorker> {
        // Drop the old scene first so its memory is free before the new one is built.
        self.scene = None;
        let hdr = match (spec.hdr_id, &self.hdr) {
            (Some(wanted), Some((held, map))) if wanted == *held => Some(Arc::clone(map)),
            _ => None,
        };
        match OwnedScene::build(spec, hdr) {
            Ok(scene) => {
                self.scene = Some(CachedScene {
                    scene_id,
                    scene,
                    uses_hdr: spec.hdr_id.is_some(),
                });
                None
            }
            Err(error) => Some(FromWorker::SceneError {
                scene_id,
                message: error.to_string(),
            }),
        }
    }

    fn load_hdr(&mut self, id: u64, bytes: &[u8]) -> FromWorker {
        // Free the old map (and a scene still holding it) before decoding the new one.
        self.hdr = None;
        if self.scene.as_ref().is_some_and(|s| s.uses_hdr) {
            self.scene = None;
        }
        match environment_from_hdr_bytes(bytes, web_hdr_limits()) {
            Ok(map) => {
                let (width, height) = (map.width() as u32, map.height() as u32);
                self.hdr = Some((id, Arc::new(map)));
                FromWorker::HdrLoaded { id, width, height }
            }
            Err(error) => FromWorker::HdrError {
                id,
                message: error.to_string(),
            },
        }
    }

    /// Traces `range` of scene `scene_id` in row groups, stopping (with
    /// [`FromWorker::ChunkAborted`]) as soon as the cancel URL a preceding
    /// [`ToWorker::WatchCancel`] named for that scene is revoked.
    fn trace(
        &mut self,
        scene_id: u64,
        range: ChunkRange,
        now_ms: &dyn Fn() -> f64,
        probe: &dyn Fn(&str) -> CancelProbe,
    ) -> FromWorker {
        // Taken first: a watch never outlives the next chunk, whatever that chunk does.
        let watch_url = take_watch(&mut self.cancel_watch, scene_id);
        let Some(cached) = self.scene.as_ref().filter(|s| s.scene_id == scene_id) else {
            return FromWorker::ChunkDropped {
                scene_id,
                first_pixel: range.first_pixel,
                sample_offset: range.sample_offset,
            };
        };
        let start = now_ms();
        // The first look is one poll interval into the chunk: a short chunk never asks.
        let watch = CancelWatch::new(watch_url.as_deref(), probe, None).starting_at(start);
        let Some(sums) =
            trace_chunk_abortable(&cached.scene, range, &|| watch.cancelled_reading(now_ms))
        else {
            return FromWorker::ChunkAborted {
                scene_id,
                first_pixel: range.first_pixel,
                sample_offset: range.sample_offset,
            };
        };
        let elapsed_ms = now_ms() - start;
        FromWorker::ChunkResult {
            scene_id,
            first_pixel: range.first_pixel,
            stride: range.stride,
            sample_offset: range.sample_offset,
            spp: range.spp,
            sums: sums.into_iter().map(<[f32; 3]>::from).collect(),
            elapsed_ms,
        }
    }
}

/// What a running job may call besides its own computation.
struct JobEnv<'a> {
    now_ms: &'a dyn Fn() -> f64,
    emit: &'a dyn Fn(FromWorker),
    /// The cancel URL the page named for this job, if any.
    watch: Option<&'a str>,
    /// Asks what the URL says.
    probe: &'a dyn Fn(&str) -> CancelProbe,
    /// The Worker's [`WorkerHandler::cancel_mark`].
    mark: &'a AtomicU64,
}

/// The URL a [`ToWorker::WatchCancel`] named for `job_id`, consuming the pending one (a
/// watch never outlives the next job, whichever job that is).
fn take_watch(pending: &mut Option<(u64, String)>, job_id: u64) -> Option<String> {
    pending
        .take()
        .and_then(|(watched, url)| (watched == job_id).then_some(url))
}

/// Throttled polling of a job's cancel URL, with the Worker's message-cancel mark as a
/// second, cheaper source.
///
/// A small state machine: the URL is asked at most once per [`CANCEL_POLL_INTERVAL_MS`];
/// [`CancelProbe::Live`] changes nothing, [`CancelProbe::Revoked`] is a cancel, and
/// [`CancelProbe::Unavailable`] retires the URL for the rest of the job (the probe
/// cannot run here, so asking again would only cost time) -- the watch then answers from
/// the message mark alone.
struct CancelWatch<'a> {
    url: Option<&'a str>,
    probe: &'a dyn Fn(&str) -> CancelProbe,
    /// `(mark, job_id)`: the job is cancelled once `job_id < mark`.
    message: Option<(&'a AtomicU64, u64)>,
    last_ms: Cell<f64>,
    /// `false` once the probe reported [`CancelProbe::Unavailable`].
    probe_works: Cell<bool>,
}

impl<'a> CancelWatch<'a> {
    fn new(
        url: Option<&'a str>,
        probe: &'a dyn Fn(&str) -> CancelProbe,
        message: Option<(&'a AtomicU64, u64)>,
    ) -> Self {
        Self {
            url,
            probe,
            message,
            last_ms: Cell::new(f64::NEG_INFINITY),
            probe_works: Cell::new(true),
        }
    }

    /// Delays the first look at the URL to one poll interval after `start_ms`.
    fn starting_at(self, start_ms: f64) -> Self {
        self.last_ms.set(start_ms);
        self
    }

    /// Whether a [`ToWorker::Cancel`] named the job.
    fn cancelled_by_message(&self) -> bool {
        self.message
            .is_some_and(|(mark, job_id)| job_id < mark.load(Ordering::Relaxed))
    }

    /// Whether the job is cancelled, at time `now_ms`. Asks the URL at most once per
    /// [`CANCEL_POLL_INTERVAL_MS`]; in between it answers `false` for the URL.
    fn cancelled(&self, now_ms: f64) -> bool {
        if self.cancelled_by_message() {
            return true;
        }
        let Some(url) = self.url.filter(|_| self.probe_works.get()) else {
            return false;
        };
        if now_ms - self.last_ms.get() < CANCEL_POLL_INTERVAL_MS {
            return false;
        }
        self.last_ms.set(now_ms);
        match (self.probe)(url) {
            CancelProbe::Live => false,
            CancelProbe::Revoked => true,
            CancelProbe::Unavailable => {
                self.probe_works.set(false);
                false
            }
        }
    }

    /// [`Self::cancelled`] for a caller with no clock reading of its own: the clock is
    /// read only when the URL could still be asked.
    fn cancelled_reading(&self, clock: &dyn Fn() -> f64) -> bool {
        if self.url.is_none() || !self.probe_works.get() {
            return self.cancelled_by_message();
        }
        self.cancelled(clock())
    }
}

/// Runs one solve job, turning its search progress into throttled
/// [`FromWorker::Progress`] messages through `env.emit`, and stopping the search when
/// the job's cancel URL says the page gave up.
fn run_solve_job(
    job_id: u64,
    design_toml: &str,
    request: &SolveRequest,
    start_ms: f64,
    env: &JobEnv<'_>,
) -> SolveResponse {
    let JobEnv {
        now_ms,
        emit,
        watch,
        probe,
        mark,
    } = *env;
    let last_ms = Cell::new(f64::NEG_INFINITY);
    let last_stage = Cell::new(None::<SearchStage>);
    // Set from the progress hook below, which the search runs between tier decisions and
    // which is therefore the one place a busy Worker can look at the outside world.
    let cancel = AtomicBool::new(false);
    let watch = CancelWatch::new(watch, probe, Some((mark, job_id)));
    let on_progress = |report: ProgressReport| {
        let now = now_ms();
        if watch.cancelled(now) {
            cancel.store(true, Ordering::Relaxed);
        }
        let stage_changed = last_stage.get() != Some(report.stage);
        if !stage_changed && now - last_ms.get() < PROGRESS_INTERVAL_MS {
            return;
        }
        last_ms.set(now);
        last_stage.set(Some(report.stage));
        let elapsed_secs = ((now - start_ms) / 1000.0) as f32;
        let counting = matches!(
            report.stage,
            SearchStage::Screening | SearchStage::Coordinate | SearchStage::Polish
        );
        let fraction = (counting && report.max_evaluations > 0)
            .then(|| (report.evaluations as f32 / report.max_evaluations as f32).clamp(0.0, 1.0));
        emit(FromWorker::Progress {
            job_id,
            message: optimize_progress_status(
                report.stage,
                report.evaluations,
                report.max_evaluations,
                // The web runs one start (no `on_start` report to show).
                None,
                elapsed_secs,
            ),
            fraction,
        });
    };
    // A tilt sweep reports before every evaluation (about every 2 ms): the same throttle,
    // and the same look at the cancel URL, as a search's progress hook.
    let last_axis = Cell::new(None::<usize>);
    let on_sweep = |progress: SweepProgress| {
        let now = now_ms();
        if watch.cancelled(now) {
            cancel.store(true, Ordering::Relaxed);
        }
        let axis_changed = last_axis.get() != Some(progress.axis);
        if !axis_changed && now - last_ms.get() < PROGRESS_INTERVAL_MS {
            return;
        }
        last_ms.set(now);
        last_axis.set(Some(progress.axis));
        emit(FromWorker::Progress {
            job_id,
            message: sweep_status(progress),
            fraction: Some(sweep_fraction(progress)),
        });
    };
    handle_solve_with(
        design_toml,
        request,
        &SolveHooks {
            cancel: &cancel,
            on_progress: &on_progress,
            on_sweep: &on_sweep,
        },
    )
}
