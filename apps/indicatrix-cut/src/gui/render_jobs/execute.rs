//! The one executor that renders a render job, without a window.
//!
//! The desktop queue and `indicatrix-cli render` / `tilt-video` both call [`execute_job`],
//! so a queued job, a job run from a script and a picture exported directly all go through
//! the same code:
//!
//! - a **still** is `bridge::export_thread::run_export`, the function the immediate export
//!   runs;
//! - a **tilt video** is `video_export::frames::render_frames`, the direct export's frame
//!   loop, run with `skip_existing` so a paused or interrupted video continues at its
//!   first missing frame.
//!
//! A video's frame folder carries a marker file naming the job (`FRAME_MARKER_FILE`), so
//! a resume only trusts frames that this job wrote, and never mixes in another export's.

use super::convert;
use crate::{
    bridge::export_thread::{
        ExportOutcome, ExportParams, ExportProgress, RemoteSelection, SceneSnapshot, run_export,
    },
    gui::{
        progress_eta::EtaEstimator,
        tilt::video_export::{
            encode::{self, EncodeOutcome},
            frames::{self, FrameReporter, FramesOutcome},
            params,
        },
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use indicatrix_render_jobs::{
    ComputeChoice, FailureKind, JobKind, JobOutcome, JobProgress, JobSink, RenderJobFile, StillJob,
    TiltVideoJob, TransferChoice,
    paths::{FRAME_MARKER_FILE, marker_text, marker_token, reserve_unique_file},
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Instant,
};

/// Where and how one job runs on this machine: everything that is machine configuration
/// rather than render content, plus the command line's overrides.
pub struct ExecContext {
    /// The folder relative paths in the job resolve against.
    pub base_dir: PathBuf,
    /// The local engines (processor, graphics card or both).
    pub local_compute: LocalComputeTarget,
    /// The remote worker to use, read from the settings (app) or the flags (command line).
    pub worker: Option<WorkerSettings>,
    /// Replaces the job's compute target.
    pub compute_override: Option<ComputeChoice>,
    /// Replaces the job's transfer choice.
    pub transfer_override: Option<TransferChoice>,
    /// Replaces the job's "this computer renders a share too" choice.
    pub contribute_local_override: Option<bool>,
    /// A still: the PNG path. A video: the frame folder. Replaces the job's own.
    pub output_override: Option<PathBuf>,
    /// A video: delete this job's frames first and render from frame one.
    pub restart_frames: bool,
}

/// [`execute_job`], with a panic turned into a failed outcome. The outcome is therefore
/// always delivered, which the queue relies on to release the live viewport.
pub fn execute_job_catching(
    job: &RenderJobFile,
    ctx: &ExecContext,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute_job(job, ctx, cancel, sink)
    }))
    .unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "the render worker panicked".to_string());
        failed(
            FailureKind::Render,
            format!("The render stopped unexpectedly: {message}"),
            0,
        )
    })
}

/// Renders `job` and reports how it ended. Cancelling through `cancel` ends the run as
/// [`JobOutcome::Stopped`] at the next sample batch (still) or the next frame (video).
pub fn execute_job(
    job: &RenderJobFile,
    ctx: &ExecContext,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    if let Err(e) = job.validate() {
        return failed(FailureKind::Input, e.to_string(), 0);
    }
    let env_map = match convert::load_hdr(job, &ctx.base_dir) {
        Ok(env_map) => env_map,
        Err((kind, message)) => return failed(kind, message, 0),
    };
    let scene = convert::snapshot_from_scene(&job.scene, env_map);
    let remote = remote_selection_for(job, ctx);
    match &job.kind {
        JobKind::Still(still) => execute_still(job, still, ctx, &scene, &remote, cancel, sink),
        JobKind::TiltVideo(video) => execute_video(job, video, ctx, scene, remote, cancel, sink),
    }
}

const fn failed(kind: FailureKind, message: String, frames_done: u32) -> JobOutcome {
    JobOutcome::Failed {
        kind,
        message,
        frames_done,
    }
}

/// The job's compute choices with the context's overrides applied, and the worker the
/// context names.
pub(super) fn remote_selection_for(job: &RenderJobFile, ctx: &ExecContext) -> RemoteSelection {
    let mut compute = job.compute;
    if let Some(target) = ctx.compute_override {
        compute.target = target;
    }
    if let Some(transfer) = ctx.transfer_override {
        compute.transfer = transfer;
    }
    if let Some(contribute) = ctx.contribute_local_override {
        compute.contribute_local = contribute;
    }
    convert::remote_selection(compute, ctx.worker.clone())
}

fn output_of(job: &RenderJobFile, ctx: &ExecContext) -> PathBuf {
    ctx.output_override
        .clone()
        .unwrap_or_else(|| job.output_path(&ctx.base_dir))
}

// ---- Still ---------------------------------------------------------------------------

fn execute_still(
    job: &RenderJobFile,
    still: &StillJob,
    ctx: &ExecContext,
    scene: &SceneSnapshot,
    remote: &RemoteSelection,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    let params = ExportParams {
        width: job.scene.width,
        height: job.scene.height,
        samples_per_pixel: still.samples_per_pixel,
        max_bounces: job.scene.max_bounces,
    };
    let mut out = output_of(job, ctx);
    if out.exists() {
        out = reserve_unique_file(&out, &[], &|path| path.exists());
    }
    let mut eta = EtaEstimator::default();
    let outcome = run_export(
        scene,
        params,
        convert::color_space_of(still.color_space),
        &out,
        remote,
        ctx.local_compute,
        cancel,
        |progress: ExportProgress| {
            let now = Instant::now();
            eta.observe(now, f64::from(progress.fraction));
            sink.progress(&JobProgress {
                frames_done: 0,
                frames_total: 1,
                fraction: progress.fraction,
                eta_secs: eta.eta(now).map(|left| left.as_secs_f64()),
                note: progress.note,
            });
        },
    );
    match outcome {
        ExportOutcome::Completed(result) => JobOutcome::Done { result, note: None },
        ExportOutcome::Cancelled => JobOutcome::Stopped { frames_done: 0 },
        ExportOutcome::Failed(message) => {
            let kind = if message.starts_with("Could not create output directory")
                || message.starts_with("Failed to write PNG")
            {
                FailureKind::Output
            } else {
                FailureKind::Render
            };
            failed(kind, message, 0)
        }
    }
}

// ---- Tilt video ----------------------------------------------------------------------

fn execute_video(
    job: &RenderJobFile,
    video: &TiltVideoJob,
    ctx: &ExecContext,
    scene: SceneSnapshot,
    remote: RemoteSelection,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    let dir = output_of(job, ctx);
    let total = video.total_frames as usize;
    if params::frame_count(video.start_deg, video.end_deg, video.step_deg) != total {
        return failed(
            FailureKind::Input,
            "The tilt range and step do not give the number of frames the job names.".to_string(),
            0,
        );
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return failed(
            FailureKind::Output,
            format!("Could not create the folder {}: {e}", dir.display()),
            0,
        );
    }
    if let Err(outcome) = prepare_frame_folder(&dir, &job.token, ctx.restart_frames) {
        return outcome;
    }

    let request =
        convert::video_request_from_job(job, video, scene, dir.clone(), remote, ctx.local_compute);
    let frames_on_disk = frames::existing_frames(&dir, total)
        .into_iter()
        .filter(|exists| *exists)
        .count();
    let mut reporter = SinkReporter {
        sink,
        eta: EtaEstimator::default(),
        frames_done: frames_on_disk,
    };
    // A resumed video shows where it continues from before its first frame is done.
    reporter.report(
        frames_on_disk,
        total,
        frames_on_disk as f32 / total.max(1) as f32,
        None,
    );

    match frames::render_frames(&request, true, cancel, &mut reporter) {
        FramesOutcome::Done(paths) => finish_video(video, &dir, &paths, total),
        FramesOutcome::Cancelled { frames_done } => JobOutcome::Stopped {
            frames_done: frames_done as u32,
        },
        FramesOutcome::Failed {
            message,
            frames_done,
        } => failed(FailureKind::Render, message, frames_done as u32),
    }
}

/// Checks the frame folder belongs to this job (or is fresh), clears it when a restart was
/// asked for, removes the leftovers of an interrupted frame and writes the marker.
fn prepare_frame_folder(dir: &Path, token: &str, restart: bool) -> Result<(), JobOutcome> {
    let marker = dir.join(FRAME_MARKER_FILE);
    if marker.exists() {
        let owner = std::fs::read_to_string(&marker).ok();
        if owner.as_deref().and_then(marker_token) != Some(token) {
            return Err(failed(
                FailureKind::Input,
                format!(
                    "The folder {} belongs to another render job.",
                    dir.display()
                ),
                0,
            ));
        }
    } else if frames::holds_frames(dir) && !restart {
        return Err(failed(
            FailureKind::Input,
            format!(
                "The folder {} already holds frames from another export. Choose another \
                 folder or delete those frames.",
                dir.display()
            ),
            0,
        ));
    }
    if restart && let Err(e) = frames::clear_frames(dir) {
        return Err(failed(
            FailureKind::Output,
            format!("Could not clear the old frames in {}: {e}", dir.display()),
            0,
        ));
    }
    frames::remove_partials(dir);
    std::fs::write(&marker, marker_text(token)).map_err(|e| {
        failed(
            FailureKind::Output,
            format!("Could not write in the folder {}: {e}", dir.display()),
            0,
        )
    })
}

/// Encodes the finished frames. A video that was made (and not asked to keep its frames)
/// takes the frames and the marker with it.
fn finish_video(video: &TiltVideoJob, dir: &Path, paths: &[PathBuf], total: usize) -> JobOutcome {
    let outcome = encode::encode(
        dir,
        paths,
        params::frame_number_digits(total),
        video.fps,
        &video.video_name,
    );
    let (result, message) = frames::encode_outcome_message(&outcome, dir);
    if !video.keep_frames && matches!(outcome, EncodeOutcome::Mp4(_) | EncodeOutcome::Gif { .. }) {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(dir.join(FRAME_MARKER_FILE));
    }
    let note = match outcome {
        EncodeOutcome::Mp4(_) => None,
        EncodeOutcome::Gif { .. } | EncodeOutcome::FramesOnly { .. } => Some(message),
    };
    JobOutcome::Done { result, note }
}

/// Forwards the frame loop's progress to a job sink.
struct SinkReporter<'a> {
    sink: &'a mut dyn JobSink,
    eta: EtaEstimator,
    /// Frames on disk, including the ones that were there before this run.
    frames_done: usize,
}

impl SinkReporter<'_> {
    fn report(&mut self, frames_done: usize, total: usize, fraction: f32, note: Option<String>) {
        let eta_secs = self.eta.eta(Instant::now()).map(|left| left.as_secs_f64());
        self.sink.progress(&JobProgress {
            frames_done: frames_done as u32,
            frames_total: total as u32,
            fraction,
            eta_secs,
            note,
        });
    }
}

impl FrameReporter for SinkReporter<'_> {
    fn frame_progress(&mut self, _index: usize, total: usize, fraction: f32, note: Option<String>) {
        let overall = (self.frames_done as f32 + fraction) / total.max(1) as f32;
        self.report(self.frames_done, total, overall, note);
    }

    fn frame_saved(&mut self, _index: usize, frames_done: usize, total: usize, _secs: f32) {
        self.frames_done = frames_done;
        self.eta
            .observe(Instant::now(), frames_done as f64 / total.max(1) as f64);
        self.report(
            frames_done,
            total,
            frames_done as f32 / total.max(1) as f32,
            None,
        );
        self.sink.frame_finished(frames_done as u32, total as u32);
    }
}
