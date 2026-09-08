//! The free-running tracer: [`run_tracer`] traces the requested sample range in
//! adaptively-sized sub-batches, folding each into a shared accumulation buffer --
//! never touching the socket itself. See `crate::serve`'s module docs for why that
//! separation is the whole point.

use super::{emitter::PendingDelta, sizing::next_batch_size};
use crate::{cli::ComputeMode, render_core, render_core::hybrid::CalibrationOutcome};
use glam::Vec3;
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::SceneState;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

/// Shared between `run_tracer` (the sole writer) and `run_stream`'s emitter (the sole
/// reader) via a `Mutex` -- a lock held only for brief updates/drains never stalls
/// either side for long.
///
/// # The double-buffer swap protocol
///
/// `pending_delta` is the only per-pixel buffer this struct carries. The emitter
/// (`emitter::EmitterAccum`) keeps its own `running_total`, built by folding in every
/// delta it takes from `pending_delta` -- the tracer never populates or knows about it.
/// `PendingDelta::swap_with` exchanges its buffer with a spare one the emitter already
/// owns via `mem::swap` (O(1), not a per-pixel copy), so the only work done while this
/// `Mutex` is held is the swap and reading `samples_done`; building the `FRAME` payload,
/// folding into the running total, downsampling, and every socket write happen after
/// the lock is released.
///
/// The buffer left behind after a swap holds stale data, never re-zeroed -- safe since
/// [`PendingDelta::add`]'s first call after a swap always `copy_from_slice`s its
/// contribution over the entire buffer rather than accumulating into it.
pub(super) struct SharedState {
    pub(super) pending_delta: PendingDelta,
    pub(super) samples_done: u32,
    pub(super) finished: bool,
    /// A caller-supplied but validation-passing scene can still make `indicatrix`'s
    /// tracer panic on pathological geometry; tracing runs inside `catch_unwind` as
    /// defense in depth. `run_stream` checks this once `finished` is set and reports
    /// [`StreamOutcome::TracePanicked`] instead of a normal `FRAME`+`DONE` sequence.
    pub(super) panicked: bool,
}

/// One tracer job's fixed inputs, bundled so [`run_tracer`] stays within clippy's
/// argument-count limit.
pub(super) struct TracerJob {
    pub(super) scene: SceneState,
    pub(super) first_sample: u32,
    pub(super) samples: u32,
    pub(super) threads: usize,
    /// `--only-gpu`/`--only-cpu`/hybrid (default). Gates whether [`run_tracer`] ever
    /// attempts [`render_core::hybrid::calibrate`] at all.
    pub(super) compute_mode: ComputeMode,
}

/// Runs the tracer side: free-runs over `[job.first_sample, job.first_sample +
/// job.samples)` in adaptively-sized sub-batches (see [`next_batch_size`]), folding each
/// into `state.pending_delta`, checking `cancel` between sub-batches and mid-dispatch
/// for whichever share of a sub-batch runs on the GPU. Never touches the socket.
///
/// Each sub-batch traces through [`render_core::hybrid::hybrid_trace`] once this job has
/// calibrated a GPU throughput share, or
/// [`render_core::trace_samples_with_gpu_cancellable`] otherwise (prefers `gpu`, falls
/// back to CPU when it declines). A `None` outcome means `cancel` fired mid-dispatch
/// (nothing to fold in), bounding cancellation latency to the GPU's own chunk size
/// rather than a whole sub-batch.
///
/// # Hybrid CPU+GPU, and `--only-gpu`/`--only-cpu`
///
/// `job.compute_mode` decides whether this even attempts a hybrid split:
/// [`render_core::hybrid::calibrate`] is only called for [`ComputeMode::Hybrid`] with
/// `job.samples >= render_core::hybrid::HYBRID_MIN_SPP`. `OnlyGpu`/`OnlyCpu` skip
/// calibration entirely (`gpu_frac` stays `None`), taking the single-engine path
/// (GPU-first with per-frame CPU fallback for `OnlyGpu`; CPU-only for `OnlyCpu`, via a
/// [`GpuBackend::disabled`] backend).
///
/// For `Hybrid`, calibration spends 3 real samples (folded into `state` like any other
/// sub-batch) measuring each engine's throughput. Four outcomes (see
/// [`CalibrationOutcome`]): `Split` calibrates a concurrent hybrid split; `GpuDeclined`
/// (no adapter, or an unsupported material) falls back to the either/or behavior for the
/// whole job; `GpuDominates` is `HYBRID_MAX_GPU_SHARE` firing (measured GPU share so far
/// ahead that splitting would cost more than it saves) and falls back similarly;
/// `Cancelled` folds in whatever probe samples completed, then leaves `gpu_frac` at
/// `None` (moot, since the sub-batch loop stops on the same flag immediately). Once
/// calibrated, every subsequent sub-batch runs GPU and CPU concurrently over disjoint
/// sample sub-ranges, re-measuring and blending the split as the job runs.
///
/// Runs the whole loop inside `catch_unwind`, so a panic anywhere in `indicatrix`'s
/// tracer on pathological geometry sets `state.panicked` for `run_stream` to notice
/// rather than taking down this thread silently.
pub(super) fn run_tracer(
    job: &TracerJob,
    gpu: &GpuBackend,
    state: &Arc<Mutex<SharedState>>,
    cancel: &Arc<AtomicBool>,
    progress_tx: &mpsc::Sender<()>,
) {
    let reporter = Reporter { state, progress_tx };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut produced: u32 = 0;
        let mut batch_size: u32 = 1;

        // Only `Hybrid` ever attempts a split -- `OnlyGpu`/`OnlyCpu` leave `gpu_frac`
        // `None` for the whole job.
        let mut gpu_frac: Option<f64> = if matches!(job.compute_mode, ComputeMode::Hybrid)
            && job.samples >= render_core::hybrid::HYBRID_MIN_SPP
        {
            calibrate_hybrid_split(gpu, job, &reporter, cancel, &mut produced)
        } else {
            None
        };

        while produced < job.samples {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let this_batch = batch_size.min(job.samples - produced);
            let batch_first = job.first_sample + produced;

            let start = Instant::now();
            let dispatch_outcome = if gpu_frac.is_some() {
                render_core::hybrid::hybrid_trace(
                    gpu,
                    &job.scene,
                    batch_first,
                    this_batch,
                    job.threads,
                    &mut gpu_frac,
                    cancel,
                )
            } else {
                render_core::trace_samples_with_gpu_cancellable(
                    gpu,
                    &job.scene,
                    batch_first,
                    this_batch,
                    job.threads,
                    cancel,
                )
            };
            // `None` means `cancel` was observed mid-dispatch; nothing was added for
            // this sub-batch, so this stops the job immediately without touching
            // `produced`/`state`, same as a between-sub-batches cancellation just
            // noticed earlier.
            let Some(buf) = dispatch_outcome else {
                break;
            };
            let elapsed = start.elapsed();

            reporter.fold_and_notify(batch_first, this_batch, &buf, &mut produced);

            batch_size = next_batch_size(batch_size, elapsed);
        }
    }));

    let mut guard = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.finished = true;
    guard.panicked = result.is_err();
    drop(guard);
    let _ = progress_tx.send(());
}

/// Bundles the shared-state plumbing a calibration/sub-batch result needs to fold in and
/// announce, so [`calibrate_hybrid_split`] stays within clippy's argument-count limit.
struct Reporter<'a> {
    state: &'a Arc<Mutex<SharedState>>,
    progress_tx: &'a mpsc::Sender<()>,
}

impl Reporter<'_> {
    /// Folds `contribution` (produced for the sample sub-range starting at
    /// `first_sample`, `consumed` samples long) into `state.pending_delta`, advances
    /// `*produced`, and wakes the emitter -- the fold-then-notify sequence every
    /// sub-batch and calibration outcome performs, pulled into one place.
    ///
    /// Does not maintain a running total here -- that's the emitter's job (see
    /// [`SharedState`]), built by folding in whatever it takes out of `pending_delta`.
    fn fold_and_notify(
        &self,
        first_sample: u32,
        consumed: u32,
        contribution: &[Vec3],
        produced: &mut u32,
    ) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .pending_delta
            .add(first_sample, consumed, contribution);
        *produced += consumed;
        guard.samples_done = *produced;
        drop(guard);
        let _ = self.progress_tx.send(());
    }
}

/// Runs [`render_core::hybrid::calibrate`] for a job that has already cleared
/// `job.compute_mode == ComputeMode::Hybrid && job.samples >= HYBRID_MIN_SPP` (checked
/// by [`run_tracer`], not re-checked here). Folds whatever `calibrate` produced via
/// `reporter`, advances `*produced`, and returns the initial `gpu_frac` the sub-batch
/// loop should start from (`Some` only for `Split`; every other outcome leaves the job
/// on the single-engine path).
fn calibrate_hybrid_split(
    gpu: &GpuBackend,
    job: &TracerJob,
    reporter: &Reporter<'_>,
    cancel: &AtomicBool,
    produced: &mut u32,
) -> Option<f64> {
    let pixel_count = job.scene.width as usize * job.scene.height as usize;
    let mut calib_buf = vec![Vec3::ZERO; pixel_count];
    match render_core::hybrid::calibrate(
        gpu,
        &job.scene,
        job.first_sample,
        job.samples,
        job.threads,
        &mut calib_buf,
        cancel,
    ) {
        CalibrationOutcome::Split(frac, consumed) => {
            reporter.fold_and_notify(job.first_sample, consumed, &calib_buf, produced);
            Some(frac)
        }
        // Falls through to the single-engine path below.
        CalibrationOutcome::GpuDeclined => None,
        CalibrationOutcome::GpuDominates { gpu_share_frac } => {
            // HYBRID_MAX_GPU_SHARE firing. Runs at most once per job, before the
            // sub-batch loop begins.
            tracing::info!(
                gpu_share_pct = gpu_share_frac * 100.0,
                "hybrid CPU+GPU split declined: measured GPU share exceeds the hybrid \
                 cutoff, so this job runs GPU-only (splitting measured slower on this \
                 hardware)"
            );
            None
        }
        CalibrationOutcome::Cancelled { consumed } => {
            // Cancelled during calibration itself -- consumed samples are folded in
            // like Split's accounting, so progress is never silently dropped.
            // Returning `None` is moot: the sub-batch loop checks the same flag on its
            // first iteration and breaks immediately regardless.
            if consumed > 0 {
                reporter.fold_and_notify(job.first_sample, consumed, &calib_buf, produced);
            }
            None
        }
    }
}
