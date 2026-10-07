//! Rendering a job file without a window: the entry point `indicatrix-cli render` and
//! `indicatrix-cli tilt-video` call.
//!
//! It runs the very same executor the desktop render queue runs, so a job rendered from a
//! script produces the same picture as one rendered in the app, and reuses the app's
//! remote-worker client (probe, full-data lanes, final picture, HDR map upload). Nothing
//! here creates a window or touches Slint.

use super::{
    convert::local_target_of,
    execute::{ExecContext, execute_job_catching},
};
use crate::settings::WorkerSettings;
use indicatrix_render_jobs::{
    ComputeChoice, JobOutcome, JobSink, LocalEngines, RenderJobFile, TransferChoice,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// A remote render worker: its address and the folder holding its certificate bundle
/// (`ca.pem`, `client.pem` and `client.key`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEndpoint {
    /// `host:port` the worker listens on.
    pub address: String,
    /// The folder with the mutual-TLS certificate bundle.
    pub cert_dir: PathBuf,
}

/// How this computer renders a job: the machine's own choices and the command line's
/// overrides of what the job froze.
#[derive(Debug, Clone, Default)]
pub struct HeadlessOptions {
    /// Which engines of this computer render. Without the `gpu` feature every choice
    /// renders on the processor ([`gpu_compiled_in`]).
    pub local: LocalEngines,
    /// The remote worker, when one is to be used.
    pub remote: Option<RemoteEndpoint>,
    /// Replaces the job's compute target.
    pub compute: Option<ComputeChoice>,
    /// Replaces the job's transfer choice.
    pub transfer: Option<TransferChoice>,
    /// Replaces the job's "this computer renders a share too" choice.
    pub contribute_local: Option<bool>,
    /// A still: the PNG to write. A video: the frame folder. Replaces the job's own.
    pub output: Option<PathBuf>,
    /// A tilt video: delete its frames first and start at frame one.
    pub restart_frames: bool,
}

/// Renders `job` and reports how it ended.
///
/// `job_dir` is the folder of the job file: relative paths in the job (the output, the HDR
/// map) resolve against it. Setting `cancel` stops the render at the next sample batch (a
/// picture) or the next frame (a video); a video keeps its finished frames and resumes
/// from them the next time. `sink` receives the progress. A panic inside the render is
/// reported as a failed outcome, never unwound.
pub fn run_job(
    job: &RenderJobFile,
    job_dir: &Path,
    options: &HeadlessOptions,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    let worker = options.remote.as_ref().map(|remote| WorkerSettings {
        address: remote.address.clone(),
        cert_dir: remote.cert_dir.display().to_string(),
        ..WorkerSettings::default()
    });
    let ctx = ExecContext {
        base_dir: job_dir.to_path_buf(),
        local_compute: local_target_of(options.local),
        worker,
        compute_override: options.compute,
        transfer_override: options.transfer,
        contribute_local_override: options.contribute_local,
        output_override: options.output.clone(),
        restart_frames: options.restart_frames,
    };
    execute_job_catching(job, &ctx, cancel, sink)
}

/// Whether this build can render on the graphics card (the `gpu` feature). Without it,
/// every `--local` choice renders on the processor.
#[must_use]
pub const fn gpu_compiled_in() -> bool {
    cfg!(feature = "gpu")
}
