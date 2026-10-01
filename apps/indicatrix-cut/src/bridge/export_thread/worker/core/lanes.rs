//! The phases of one [`super::render_accumulation`] call: resolving the remote lane
//! (availability, then calibration), running the local and remote lanes concurrently
//! over one shared claim point, and merging each engine's buffer exactly once.

use super::super::types::{Accumulation, AccumulationCarry, AccumulationOutcome};
use crate::{
    bridge::{
        export_thread::{
            ExportProgress,
            batch::{
                ExportCtx, HYBRID_MIN_SPP, calibrate_split, local_chunk_size, run_local_batches,
            },
            params::ComputeTarget,
            preview::PreviewThrottle,
            remote::{
                self, REMOTE_CALIBRATION_SAMPLES, REMOTE_MIN_SPP, RemoteCalibration,
                RemoteCapability, RemoteProgress, calibrate_remote_rate, exceeds_pixel_cap,
                probe_remote, run_remote_lane,
            },
            sample_cursor::SampleCursor,
            scene_snapshot::SceneSnapshot,
        },
        remote::remote_can_render,
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use glam::Vec3;
use indicatrix_net::SceneState;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

/// Placeholder initial rate (samples/sec) used when remote's ENTIRE sample budget is
/// smaller than a calibration probe (`ComputeTarget::RemoteOnly` case, see "Remote
/// calibration" below) so there's no measurement to start from. The value barely
/// matters -- `SampleCursor::claim`'s clamp sizes the first (only) chunk to the whole
/// tiny remainder regardless. Deliberately not `0.0`/negative, which
/// `remote_chunk_samples` would treat as "use the floor".
const DEFAULT_REMOTE_RATE_GUESS: f64 = 1000.0;

/// Resolves which remote worker (if any) this call's remote lane should use. Reuses
/// `carry.remote_capability` once a prior call on the SAME carry has already probed
/// (see [`AccumulationCarry::remote_probed`]'s own doc comment); otherwise probes fresh,
/// exactly as [`super::render_accumulation`] always did before this helper existed. `Err` is
/// only returned for `ComputeTarget::RemoteOnly`, which has no local fallback to fail
/// gracefully into.
fn resolve_remote_capability(
    carry: &mut AccumulationCarry,
    compute_target: ComputeTarget,
    worker: Option<&WorkerSettings>,
    scene: &SceneSnapshot,
    width: u32,
    height: u32,
    pending_note: &mut Option<String>,
) -> Result<Option<RemoteCapability>, String> {
    if matches!(compute_target, ComputeTarget::LocalOnly) {
        return Ok(None);
    }
    if carry.remote_probed {
        return Ok(carry.remote_capability.clone());
    }
    // Re-probed here, not trusted from whatever the export dialog observed when it
    // opened, so a worker that went away or came up in the meantime is judged by
    // actual state at dispatch time.
    let resolved = match probe_remote(worker) {
        Ok(cap) => {
            if let Err(refusal) = remote_can_render(scene.env_map.as_ref(), cap.hdr) {
                // The one shared rule (`bridge::remote::guard`), per capability: an HDR
                // map the remote cannot render (or that cannot be sent) makes remote
                // compute UNSOUND, not merely slower -- the remote would light the stone
                // differently than the local half, and `render_accumulation` sums both
                // halves into one buffer. Falling back to local-only for the WHOLE
                // render keeps the image correct and says so plainly -- for the still
                // export and the tilt video alike, even under `RemoteOnly`. Cached like
                // every other outcome below, so the note is not repeated every frame.
                *pending_note = Some(refusal.export_note().to_string());
                carry.remote_probed = true;
                carry.remote_capability = None;
                return Ok(None);
            }
            if exceeds_pixel_cap(width, height, &cap) {
                // Never silently shrink the export. `Both` falls back to local for the
                // WHOLE export with a status message; `RemoteOnly` has no local
                // fallback, so this fails outright instead.
                let pixels = u64::from(width) * u64::from(height);
                let message = format!(
                    "This export ({pixels} px) exceeds the remote worker's maximum of \
                     {} px.",
                    cap.max_pixels
                );
                if matches!(compute_target, ComputeTarget::RemoteOnly) {
                    return Err(message);
                }
                *pending_note = Some(format!("{message} Rendering locally only."));
                None
            } else {
                Some(cap)
            }
        }
        Err(reason) => {
            // Same `RemoteOnly`-has-no-fallback reasoning as above.
            if matches!(compute_target, ComputeTarget::RemoteOnly) {
                return Err(reason.message());
            }
            *pending_note = Some(format!("{} Rendering locally only.", reason.message()));
            None
        }
    };
    carry.remote_probed = true;
    carry.remote_capability.clone_from(&resolved);
    Ok(resolved)
}

/// Resolves the remote lane's starting throughput estimate for this call: reuses
/// `carry.remote_rate` once a prior call on the SAME carry has already measured one
/// (skipping the calibration probe entirely), or times a fresh
/// [`REMOTE_CALIBRATION_SAMPLES`]-sample probe exactly as [`super::render_accumulation`]
/// always did before this helper existed. `Ok(None)` means "no remote lane this call"
/// (not worth it for the SPP remaining, cancelled, or the probe failed/ran short with
/// nothing to seed a lane from); `Err` is fatal only for `ComputeTarget::RemoteOnly`.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one calibration attempt's own \
              identity (the carry to seed from/into, compute target, worker/scene \
              state, resolution, the shared sample budget/accumulator, cancellation, \
              and the pending-note slot) -- bundling them into a struct would just \
              move the same count into field access, not reduce it"
)]
fn resolve_remote_rate(
    carry: &mut AccumulationCarry,
    compute_target: ComputeTarget,
    capability: &RemoteCapability,
    scene_state: &SceneState,
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    samples_done: &mut u32,
    accum: &mut [Vec3],
    cancel: &AtomicBool,
    pending_note: &mut Option<String>,
) -> Result<Option<f64>, String> {
    let remaining = samples_per_pixel - *samples_done;
    // `RemoteOnly` bypasses `REMOTE_MIN_SPP`: that threshold only avoids the overhead
    // of remote when local could handle a tiny remainder alone, which is irrelevant
    // once the user explicitly chose `RemoteOnly`.
    let worth_attempting =
        remaining >= REMOTE_MIN_SPP || matches!(compute_target, ComputeTarget::RemoteOnly);
    if !worth_attempting || remaining == 0 {
        return Ok(None);
    }
    if let Some(seeded) = carry.remote_rate {
        return Ok(Some(seeded));
    }

    // `calibrate_remote_rate` returns a `RemoteCalibration`, not a bare `Option<f64>`,
    // so a failed/short probe's reason reaches the user instead of silently collapsing
    // to "no remote". `Cancelled` gets no note -- the caller's own `cancel` check bails
    // out immediately after.
    let calibration = if remaining >= REMOTE_CALIBRATION_SAMPLES {
        calibrate_remote_rate(
            capability,
            scene_state,
            width,
            height,
            samples_done,
            accum,
            cancel,
        )
    } else {
        // Only reachable via the `RemoteOnly` bypass above, when the ENTIRE sample
        // budget is smaller than one calibration probe. Spending any of that tiny
        // budget on a throwaway timing probe would waste a disproportionate share of
        // it, and the guessed rate barely matters -- `SampleCursor::claim`'s clamp
        // sizes the first (only) chunk to the whole remainder regardless.
        RemoteCalibration::Ready(DEFAULT_REMOTE_RATE_GUESS)
    };
    match calibration {
        RemoteCalibration::Ready(rate) => {
            carry.remote_rate = Some(rate);
            Ok(Some(rate))
        }
        RemoteCalibration::Cancelled => Ok(None),
        RemoteCalibration::Failed(message) => {
            let note = format!("Remote worker rejected this export ({message}).");
            if matches!(compute_target, ComputeTarget::RemoteOnly) {
                return Err(note);
            }
            *pending_note = Some(format!("{note} Rendering locally only."));
            Ok(None)
        }
        RemoteCalibration::Short { done, expected } => {
            let note = format!(
                "Remote worker's calibration probe only completed {done} of {expected} \
                 samples before ending."
            );
            if matches!(compute_target, ComputeTarget::RemoteOnly) {
                return Err(note);
            }
            *pending_note = Some(format!("{note} Rendering locally only."));
            Ok(None)
        }
        RemoteCalibration::Partial {
            rate,
            done,
            expected,
        } => {
            // Enough completed to size a first chunk from; the lane's own failure
            // handling (pause-and-retry, never a permanent write-off) takes it from
            // here.
            carry.remote_rate = Some(rate);
            *pending_note = Some(format!(
                "Remote worker's calibration probe completed {done} of {expected} \
                 samples before ending early -- still using it, and retrying if it \
                 drops out again."
            ));
            Ok(Some(rate))
        }
    }
}

/// A `render_accumulation` call's fixed inputs, shared by its phases.
pub(super) struct RenderJob<'a> {
    /// The scene being rendered.
    pub(super) scene: &'a SceneSnapshot,
    /// The camera yaw this call renders from.
    pub(super) cam_yaw: f32,
    /// The camera pitch this call renders from.
    pub(super) cam_pitch: f32,
    /// Output width in pixels.
    pub(super) width: u32,
    /// Output height in pixels.
    pub(super) height: u32,
    /// Samples per pixel to accumulate.
    pub(super) samples_per_pixel: u32,
    /// Which engines may render.
    pub(super) compute_target: ComputeTarget,
    /// The configured remote worker, if any.
    pub(super) worker: Option<&'a WorkerSettings>,
    /// Raised to abandon the render.
    pub(super) cancel: &'a AtomicBool,
}

/// What the remote lane needs for one call: the worker's capability, the scene as it is
/// sent and the starting throughput estimate. All `None` when no remote lane runs.
struct RemoteLanePlan {
    /// The worker's capability, once it is in play.
    capability: Option<RemoteCapability>,
    /// The scene state sent to the worker.
    scene_state: Option<SceneState>,
    /// `Some(rate)` once a lane is worth running this call -- either freshly
    /// calibrated, or carried forward from a prior call on the same carry.
    rate: Option<f64>,
}

impl RemoteLanePlan {
    /// The borrowed handles the concurrent phase runs the remote lane with; `None` when
    /// no remote lane is in play.
    fn lane<'a>(&'a self, progress: Option<&'a RemoteProgress>) -> Option<RemoteLane<'a>> {
        let rate = self.rate?;
        Some(RemoteLane {
            rate,
            capability: self
                .capability
                .as_ref()
                .expect("remote_rate implies remote_capability"),
            scene_state: self
                .scene_state
                .as_ref()
                .expect("remote_rate implies scene_state"),
            progress: progress.expect("remote_rate implies remote_progress"),
        })
    }
}

/// The remote lane's borrowed inputs for the concurrent phase.
#[derive(Clone, Copy)]
struct RemoteLane<'a> {
    /// The lane's starting throughput estimate (samples/sec).
    rate: f64,
    /// The worker's capability.
    capability: &'a RemoteCapability,
    /// The scene state sent to the worker.
    scene_state: &'a SceneState,
    /// Where the lane merges its finished chunks.
    progress: &'a RemoteProgress,
}

/// What the concurrent phase's local and remote lanes share.
struct LaneSetup<'a, 'b> {
    /// The scene/camera/environment context the local batches render with.
    ctx: &'a ExportCtx<'b>,
    /// The claim point both lanes draw samples from.
    cursor: SampleCursor,
    /// How many samples the local lane claims at a time.
    local_claim_size: u32,
    /// Output width in pixels.
    width: u32,
    /// Output height in pixels.
    height: u32,
    /// Raised to abandon the render.
    cancel: &'a AtomicBool,
    /// `true` only for `ComputeTarget::Both` -- gates whether a remote chunk's
    /// unfinished remainder is handed to the cursor for local to pick up, or fails the
    /// render outright. `RemoteOnly` never falls back: no local lane runs under it.
    fallback_to_local: bool,
    /// `false` only for `RemoteOnly` -- local claims nothing and the calling thread
    /// just polls for progress while remote's lane runs.
    run_local: bool,
}

/// `Err(Cancelled)` once `cancel` is set.
fn check_not_cancelled(cancel: &AtomicBool) -> Result<(), AccumulationOutcome> {
    if cancel.load(Ordering::Relaxed) {
        Err(AccumulationOutcome::Cancelled)
    } else {
        Ok(())
    }
}

/// Resolves the remote lane's availability and starting throughput estimate for this
/// call (remote availability, then remote calibration), spending any calibration
/// samples from `job`'s budget into `accum`/`samples_done`. `Err` carries the message
/// that fails the whole render (`RemoteOnly` only).
fn prepare_remote_lane(
    job: &RenderJob<'_>,
    carry: &mut AccumulationCarry,
    samples_done: &mut u32,
    accum: &mut [Vec3],
    pending_note: &mut Option<String>,
) -> Result<RemoteLanePlan, String> {
    // ---- Remote availability --------------------------------------------------------
    let capability = resolve_remote_capability(
        carry,
        job.compute_target,
        job.worker,
        job.scene,
        job.width,
        job.height,
        pending_note,
    )?;
    let scene_state = capability.as_ref().map(|_| {
        remote::scene_state_from_snapshot(
            job.scene,
            job.width,
            job.height,
            job.cam_yaw,
            job.cam_pitch,
        )
    });

    // ---- Remote calibration: an initial throughput estimate, not a fixed split ----
    // `Some(rate)` once a lane is worth running this call -- either freshly
    // calibrated, or carried forward from a prior call on the same `carry`. `None`
    // means no remote lane this call: declined outright (handled above), too little
    // budget left, or a fresh probe failed/was cancelled/ran short.
    let rate: Option<f64> = if let (Some(cap), Some(state)) = (&capability, &scene_state) {
        resolve_remote_rate(
            carry,
            job.compute_target,
            cap,
            state,
            job.width,
            job.height,
            job.samples_per_pixel,
            samples_done,
            accum,
            job.cancel,
            pending_note,
        )?
    } else {
        None
    };
    Ok(RemoteLanePlan {
        capability,
        scene_state,
        rate,
    })
}

/// Runs the concurrent phase: the local lane (unless `RemoteOnly`) and, if `remote` is
/// in play, the remote lane on its own thread, both claiming from `setup.cursor`.
/// Returns what the remote lane ended with (nothing without one).
fn run_lanes(
    setup: &LaneSetup<'_, '_>,
    remote_lane: Option<RemoteLane<'_>>,
    gpu_frac: &mut Option<f64>,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
    mut on_local_batch: impl FnMut(u32, &[Vec3], &[Vec3]),
) -> remote::RemoteLaneOutcome {
    let cursor = &setup.cursor;
    let cancel = setup.cancel;
    let Some(RemoteLane {
        rate,
        capability,
        scene_state,
        progress,
    }) = remote_lane
    else {
        // No remote lane in play at all -- an already-`true` "remote lane done" flag
        // collapses `run_local_batches`' stop condition to "stop once the cursor is
        // empty", with no separate single-engine code path to keep in sync.
        let no_remote_lane = AtomicBool::new(true);
        run_local_batches(
            setup.ctx,
            cursor,
            setup.local_claim_size,
            &no_remote_lane,
            gpu_frac,
            accum,
            gpu_accum,
            cancel,
            on_local_batch,
        );
        return remote::RemoteLaneOutcome {
            fatal: None,
            final_rate: None,
        };
    };
    let (width, height, fallback_to_local) = (setup.width, setup.height, setup.fallback_to_local);
    // Set by `run_remote_lane` as the LAST thing it does, once its claim loop has
    // permanently ended -- `run_local_batches` depends on that ordering to know
    // it's safe to stop waiting for a possible late `return_to_local`.
    let remote_lane_done = AtomicBool::new(false);

    thread::scope(|s| {
        let remote_thread = s.spawn(|| {
            run_remote_lane(
                cursor,
                capability,
                scene_state,
                width,
                height,
                rate,
                progress,
                fallback_to_local,
                cancel,
                &remote_lane_done,
            )
        });

        if setup.run_local {
            run_local_batches(
                setup.ctx,
                cursor,
                setup.local_claim_size,
                &remote_lane_done,
                gpu_frac,
                accum,
                gpu_accum,
                cancel,
                on_local_batch,
            );
        } else {
            // `ComputeTarget::RemoteOnly` -- nothing for local to claim, so poll
            // for progress on a light timer instead of leaving the progress bar
            // frozen until remote's lane finishes.
            while !remote_thread.is_finished() {
                on_local_batch(0, accum, gpu_accum);
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }
        }

        remote_thread
            .join()
            .unwrap_or_else(|_| remote::RemoteLaneOutcome {
                fatal: Some("remote render worker thread panicked".to_string()),
                final_rate: None,
            })
    })
}

/// Merges remote's and the GPU's separate accumulations into `accum`, each exactly once,
/// now that the concurrent phase has ended.
fn merge_lane_buffers(accum: &mut [Vec3], remote: Option<RemoteProgress>, gpu_accum: &[Vec3]) {
    // Merge remote's full contribution exactly once, now that the concurrent phase has
    // ended -- merging earlier would race local's CPU threads writing the same pixel
    // indices. Mirrors the GPU buffer's own single end-of-render merge just below.
    if let Some(progress) = remote {
        let remote_buf = progress.into_buffer();
        for (dst, src) in accum.iter_mut().zip(&remote_buf) {
            *dst += *src;
        }
    }

    // Merge the GPU's separate accumulation exactly once. A pure-CPU render leaves
    // `gpu_accum` all zero, making this a no-op.
    for (px, gpu_px) in accum.iter_mut().zip(gpu_accum) {
        *px += *gpu_px;
    }
}

/// The body of [`super::render_accumulation`], run once [`super::with_export_ctx`] has built `ctx`.
/// `Err` is an early end of the render (cancelled or failed); `Ok` the finished buffer.
pub(super) fn render_on_ctx(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    carry: &mut AccumulationCarry,
    mut report_progress: impl FnMut(ExportProgress),
) -> Result<Accumulation, AccumulationOutcome> {
    let (width, height, samples_per_pixel, cancel) =
        (job.width, job.height, job.samples_per_pixel, job.cancel);
    let mut accum = vec![Vec3::ZERO; (width as usize) * (height as usize)];
    let mut samples_done = 0u32;
    let mut pending_note: Option<String> = None;
    let remote_plan =
        prepare_remote_lane(job, carry, &mut samples_done, &mut accum, &mut pending_note)
            .map_err(AccumulationOutcome::Failed)?;
    // `gpu_accum` is declared here because the concurrent phase below needs it
    // regardless of whether remote ends up in play.
    let mut gpu_accum = vec![Vec3::ZERO; accum.len()];
    check_not_cancelled(cancel)?;

    // Hybrid CPU+GPU export: the GPU and all CPU cores trace DISJOINT sample ranges of
    // the same frame concurrently, and radiance sums merge -- the same
    // disjoint-sample-range convention `indicatrix::renderer::gpu::hybrid` uses. The
    // GPU owns its own accumulation buffer, summed into `accum` once at the end, so
    // the two engines never write one buffer concurrently. `gpu_frac == None` reads as
    // "single engine, no split": under `Gpu` the backend accepts every batch; under
    // `Cpu` the disabled backend declines every batch and the CPU tracer carries it.
    // Only `CpuGpu` measures a split and hands the two engines disjoint ranges. A
    // seed already carried from a prior call skips the timing probe entirely and
    // starts the concurrent phase from it directly -- `hybrid_batch`'s own per-batch
    // EMA keeps adapting it exactly as if it had just been measured.
    let mut gpu_frac: Option<f64> = if local_compute == LocalComputeTarget::CpuGpu
        && samples_per_pixel - samples_done >= HYBRID_MIN_SPP
    {
        carry
            .hybrid_frac
            .or_else(|| calibrate_split(ctx, &mut samples_done, &mut accum, &mut gpu_accum, cancel))
    } else {
        None
    };
    check_not_cancelled(cancel)?;

    // ---- The concurrent phase: local and (if in play) remote share one cursor -----
    // Both calibration probes above are real, already-counted work -- `samples_done`
    // marks where the shared claim point for the REST of the budget begins. A single
    // atomic claim point both lanes draw from concurrently avoids handing a lane a
    // fixed slice that finishes early with nothing further to claim.
    let setup = LaneSetup {
        ctx,
        cursor: SampleCursor::new(samples_done, samples_per_pixel),
        local_claim_size: local_chunk_size(samples_per_pixel),
        width,
        height,
        cancel,
        fallback_to_local: matches!(job.compute_target, ComputeTarget::Both),
        run_local: !matches!(job.compute_target, ComputeTarget::RemoteOnly),
    };
    let mut preview_throttle = PreviewThrottle::new();
    let remote_progress = remote_plan.rate.map(|_| RemoteProgress::new(accum.len()));

    // Reports one progress tick. `local_done` is the LOCAL lane's own running total
    // (not a cursor position, which can run ahead of completed work). Adding remote's
    // independently-tracked total gives the render's true combined completion,
    // correct by construction since the shared cursor can never double-count a
    // sample. Reduces to the pre-remote formula when `remote_progress` is `None`.
    let mut on_local_batch = |local_done: u32, local_accum: &[Vec3], local_gpu_accum: &[Vec3]| {
        let (remote_done_so_far, remote_preview) = remote_progress
            .as_ref()
            .map_or((0, None), |p| (p.samples_done(), Some(p.preview_buffer())));
        let total_done = (local_done + remote_done_so_far).min(samples_per_pixel);
        let preview = preview_throttle.maybe_generate(
            width,
            height,
            local_accum,
            local_gpu_accum,
            remote_preview.as_deref(),
            total_done,
        );
        // A pre-dispatch decline/calibration note takes priority on the first tick;
        // once drained, later ticks surface whatever `run_remote_lane` itself queued
        // (a recovered mid-export chunk failure, or "giving up on remote").
        let note = pending_note
            .take()
            .or_else(|| remote_progress.as_ref().and_then(RemoteProgress::take_note));
        report_progress(ExportProgress {
            fraction: total_done as f32 / samples_per_pixel as f32,
            samples_done: total_done,
            samples_total: samples_per_pixel,
            preview,
            note,
        });
    };

    // `outcome.fatal` is `Some` only when `run_remote_lane` ended in a state that must
    // fail the whole render (`RemoteOnly` only).
    let outcome = run_lanes(
        &setup,
        remote_plan.lane(remote_progress.as_ref()),
        &mut gpu_frac,
        &mut accum,
        &mut gpu_accum,
        &mut on_local_batch,
    );

    // Carries this call's final calibration state forward for the NEXT call on the
    // same `carry` (a no-op for `run_export`'s single-call `AccumulationCarry`, which
    // is dropped right after). `gpu_frac` is written back even when it ended `None`
    // (a declined/failed GPU): see `AccumulationCarry::hybrid_frac`'s own doc comment
    // on why that means "retry next frame", not "give up for the sweep". The remote
    // rate starts at `remote_plan.rate` (the seed this call started from) so a call that
    // never ran a remote lane still leaves `carry.remote_rate` unchanged rather than
    // clobbering it with `None`.
    carry.hybrid_frac = gpu_frac;
    carry.remote_rate = outcome
        .final_rate
        .or(remote_plan.rate)
        .or(carry.remote_rate);

    // A user-initiated cancel discards the whole render, remote's partial contribution
    // included. `run_remote_batch` already sent `CANCEL` to the worker for whatever
    // chunk was in flight once it observed `cancel`, so this is just bookkeeping
    // catching up.
    check_not_cancelled(cancel)?;

    // `RemoteOnly` only: fails the whole render outright rather than returning the
    // partial `accum`, since no local lane ever ran to pick up a failed chunk's
    // remainder.
    if let Some(message) = outcome.fatal {
        return Err(AccumulationOutcome::Failed(message));
    }

    merge_lane_buffers(&mut accum, remote_progress, &gpu_accum);
    check_not_cancelled(cancel)?;

    Ok(Accumulation {
        accum,
        samples_per_pixel,
    })
}
