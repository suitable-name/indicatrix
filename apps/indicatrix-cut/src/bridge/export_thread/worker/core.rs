//! [`render_accumulation`]: the shared local/remote render core -- calibrates and runs
//! the local/remote concurrent phase and merges every engine's contribution exactly
//! once into a linear accumulation buffer, without writing anything to disk.

use super::types::{Accumulation, AccumulationCarry, AccumulationOutcome};
use crate::{
    bridge::{
        export_thread::{
            ExportProgress,
            batch::{
                ExportCtx, HYBRID_MIN_SPP, calibrate_split, local_chunk_size, run_local_batches,
            },
            params::{ComputeTarget, ExportParams},
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
use indicatrix::{
    optics::raytracer::{Camera, EnvironmentSource},
    renderer::gpu_backend::{GpuBackend, GpuSceneRef},
};
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
/// exactly as [`render_accumulation`] always did before this helper existed. `Err` is
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
/// [`REMOTE_CALIBRATION_SAMPLES`]-sample probe exactly as [`render_accumulation`]
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

/// Renders `params.samples_per_pixel` samples per pixel of `scene` from the camera pose
/// `(cam_yaw, cam_pitch)` in small batches (so progress can be reported and
/// cancellation checked between batches, rather than after every single sample or only
/// once at the end), returning the finished linear accumulation buffer -- no
/// tone-mapping or disk I/O; see [`super::export::run_export`] for the still-image
/// export's own wrapper that adds both.
///
/// `cam_yaw`/`cam_pitch` are explicit, separate from `scene`'s own stored pose, so a
/// caller sweeping the camera across many calls against the SAME static `scene` (the
/// tilt performance video, one call per swept angle) can pass that call's own pose --
/// [`super::export::run_export`] passes `scene.yaw`/`scene.pitch` unchanged,
/// reproducing today's still-image behaviour exactly. This is the ONE place both the
/// local `Camera` and the remote `SceneState` (via `remote::scene_state_from_snapshot`)
/// get their pose from, so local, GPU and remote engines can never disagree on which
/// frame they are tracing.
///
/// `gpu`/`carry` are caller-owned (not acquired/reset inside this function) so a
/// multi-frame caller can reuse the SAME [`GpuBackend`] and [`AccumulationCarry`] across
/// every frame instead of paying adapter acquisition and remote
/// probing/calibration once per frame -- see [`AccumulationCarry`]'s own doc comment.
///
/// # Local + remote as a third engine, sharing a claim point
///
/// `compute_target` and `worker` add the remote endpoint alongside the CPU/GPU hybrid
/// split (see "Hybrid CPU+GPU export" below). Unlike that split -- a single up-front
/// calibration, since both live in-process -- remote samples are claimed from a
/// [`SampleCursor`] the local loop ALSO claims from concurrently for the whole
/// concurrent phase: see `sample_cursor`'s module doc for why a shared atomic claim
/// point avoids handing an engine a fixed slice that runs out early with nothing
/// further to claim. `ComputeTarget::LocalOnly` skips every remote code path entirely,
/// so its output stays byte-identical to a purely local render.
///
/// Remote is dispatched as a SEQUENCE of chunk requests sized to a target wall-clock
/// duration (see `remote::remote_chunk_samples`), not one request for its whole share
/// -- a giant request is what let remote's slice finish early with nothing further to
/// claim. If a chunk's connection drops partway, [`remote::run_remote_batch`]'s
/// returned `samples_done` is always exactly the valid PREFIX completed for that
/// chunk; the unfinished remainder goes back to the shared cursor for local to pick up
/// (`ComputeTarget::Both`) or fails the render outright (`RemoteOnly`, which has no
/// local lane to hand it to).
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "this is the shared render worker's own top-level orchestration -- \
              scene/pose/output params plus the compute-target/worker/carry choice \
              -- and its length is the real local+remote concurrent-dispatch/\
              fallback/merge logic the task asked for, not padding; it's already \
              split across `batch`/`remote`'s own helper functions \
              (`run_local_batches`, `calibrate_remote_rate`, `run_remote_lane`) and \
              this module's own `resolve_remote_capability`/`resolve_remote_rate` \
              everywhere that split doesn't fight the shared `SampleCursor` this \
              function's whole correctness argument depends on both lanes drawing \
              from"
)]
pub fn render_accumulation(
    scene: &SceneSnapshot,
    cam_yaw: f32,
    cam_pitch: f32,
    params: ExportParams,
    compute_target: ComputeTarget,
    worker: Option<&WorkerSettings>,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    carry: &mut AccumulationCarry,
    cancel: &AtomicBool,
    mut report_progress: impl FnMut(ExportProgress),
) -> AccumulationOutcome {
    // `max_bounces` is deliberately unused here: `scene.max_bounces` is already the
    // render's resolved bounce cap (`gui::render_export`/the tilt video both set it
    // right after `SceneSnapshot::capture`). `ExportParams::max_bounces` exists only so
    // the still-image dialog's choice is validated centrally alongside
    // width/height/samples_per_pixel.
    let ExportParams {
        width,
        height,
        samples_per_pixel,
        max_bounces: _,
    } = params;
    let mut accum = vec![Vec3::ZERO; (width as usize) * (height as usize)];
    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, 42.0);

    // Loop-invariant scene reference, hoisted.
    //
    // A loaded HDR panorama (`SceneSnapshot::env_map`) replaces the analytic studio
    // rig as this render's environment, mirroring `render_thread::mod`'s live render
    // loop. The GPU megakernel has its own `env_mode` for `HdrMap` and
    // renders it directly -- `gpu_scene.environment` built as `HdrMap` here traces on
    // the GPU exactly like any other environment, falling through to the CPU tracer
    // only on the same generic per-frame decline every other scene gets (no adapter,
    // device lost, `gpu` feature off), not an HDR-specific one.
    let environment = scene.env_map.as_deref().map_or_else(
        || {
            scene
                .lighting_preset
                .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
                .with_backdrop(scene.backdrop)
        },
        EnvironmentSource::HdrMap,
    );
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width,
        height,
        planes: &scene.active_planes,
        facet_finishes: &scene.facet_finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment,
    };
    // Shared by every batch this call runs (via `ExportCtx::gpu_retired`) so a joined
    // GPU-thread panic in any one of them retires the backend for the REST of this
    // call, not just the batch it happened in -- see `batch::hybrid_batch`'s own doc
    // comment. Deliberately per-call, not carried in `AccumulationCarry`: a caller
    // sweeping many frames retries the GPU on every frame even after a panic on an
    // earlier one, rather than writing it off for the rest of the sweep.
    let gpu_retired = AtomicBool::new(false);
    let ctx = ExportCtx {
        width,
        height,
        camera: &camera,
        scene,
        gpu,
        gpu_scene: &gpu_scene,
        gpu_retired: &gpu_retired,
    };

    // ---- Remote availability --------------------------------------------------------
    let mut samples_done = 0u32;
    let mut pending_note: Option<String> = None;
    let remote_capability = match resolve_remote_capability(
        carry,
        compute_target,
        worker,
        scene,
        width,
        height,
        &mut pending_note,
    ) {
        Ok(cap) => cap,
        Err(message) => return AccumulationOutcome::Failed(message),
    };
    let scene_state = remote_capability
        .as_ref()
        .map(|_| remote::scene_state_from_snapshot(scene, width, height, cam_yaw, cam_pitch));

    // ---- Remote calibration: an initial throughput estimate, not a fixed split ----
    // `gpu_accum` is declared here because the concurrent phase below needs it
    // regardless of whether remote ends up in play.
    let mut gpu_accum = vec![Vec3::ZERO; accum.len()];
    // `Some(rate)` once a lane is worth running this call -- either freshly
    // calibrated, or carried forward from a prior call on the same `carry`. `None`
    // means no remote lane this call: declined outright (handled above), too little
    // budget left, or a fresh probe failed/was cancelled/ran short.
    let remote_rate: Option<f64> =
        if let (Some(cap), Some(state)) = (&remote_capability, &scene_state) {
            match resolve_remote_rate(
                carry,
                compute_target,
                cap,
                state,
                width,
                height,
                samples_per_pixel,
                &mut samples_done,
                &mut accum,
                cancel,
                &mut pending_note,
            ) {
                Ok(rate) => rate,
                Err(message) => return AccumulationOutcome::Failed(message),
            }
        } else {
            None
        };
    if cancel.load(Ordering::Relaxed) {
        return AccumulationOutcome::Cancelled;
    }

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
        carry.hybrid_frac.or_else(|| {
            calibrate_split(&ctx, &mut samples_done, &mut accum, &mut gpu_accum, cancel)
        })
    } else {
        None
    };
    if cancel.load(Ordering::Relaxed) {
        return AccumulationOutcome::Cancelled;
    }

    // ---- The concurrent phase: local and (if in play) remote share one cursor -----
    // Both calibration probes above are real, already-counted work -- `samples_done`
    // marks where the shared claim point for the REST of the budget begins. A single
    // atomic claim point both lanes draw from concurrently avoids handing a lane a
    // fixed slice that finishes early with nothing further to claim.
    let cursor = SampleCursor::new(samples_done, samples_per_pixel);
    let local_claim_size = local_chunk_size(samples_per_pixel);
    let mut preview_throttle = PreviewThrottle::new();
    let remote_progress = remote_rate.map(|_| RemoteProgress::new(accum.len()));
    // `true` only for `ComputeTarget::Both` -- gates whether a remote chunk's
    // unfinished remainder is handed to the cursor for local to pick up, or fails the
    // render outright. `RemoteOnly` never falls back: no local lane runs under it.
    let fallback_to_local = matches!(compute_target, ComputeTarget::Both);
    // `false` only for `RemoteOnly` -- local claims nothing and the calling thread
    // just polls for progress while remote's lane runs (see the `thread::scope` body).
    let run_local = !matches!(compute_target, ComputeTarget::RemoteOnly);

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

    // `Some` only when `run_remote_lane` ended in a state that must fail the whole
    // render (`RemoteOnly` only).
    let mut remote_fatal: Option<String> = None;
    // Carries `run_remote_lane`'s own final rate estimate back into `carry` below --
    // starts at `remote_rate` (the seed this call started from) so a call that never
    // actually entered the `if let Some(rate) = remote_rate` branch below still leaves
    // `carry.remote_rate` unchanged rather than clobbering it with `None`.
    let mut remote_final_rate = remote_rate;
    if let Some(rate) = remote_rate {
        let cap = remote_capability
            .as_ref()
            .expect("remote_rate implies remote_capability");
        let state = scene_state
            .as_ref()
            .expect("remote_rate implies scene_state");
        let progress = remote_progress
            .as_ref()
            .expect("remote_rate implies remote_progress");
        // Set by `run_remote_lane` as the LAST thing it does, once its claim loop has
        // permanently ended -- `run_local_batches` depends on that ordering to know
        // it's safe to stop waiting for a possible late `return_to_local`.
        let remote_lane_done = AtomicBool::new(false);

        thread::scope(|s| {
            let remote_thread = s.spawn(|| {
                run_remote_lane(
                    &cursor,
                    cap,
                    state,
                    width,
                    height,
                    rate,
                    progress,
                    fallback_to_local,
                    cancel,
                    &remote_lane_done,
                )
            });

            if run_local {
                run_local_batches(
                    &ctx,
                    &cursor,
                    local_claim_size,
                    &remote_lane_done,
                    &mut gpu_frac,
                    &mut accum,
                    &mut gpu_accum,
                    cancel,
                    &mut on_local_batch,
                );
            } else {
                // `ComputeTarget::RemoteOnly` -- nothing for local to claim, so poll
                // for progress on a light timer instead of leaving the progress bar
                // frozen until remote's lane finishes.
                while !remote_thread.is_finished() {
                    on_local_batch(0, &accum, &gpu_accum);
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            }

            let outcome = remote_thread
                .join()
                .unwrap_or_else(|_| remote::RemoteLaneOutcome {
                    fatal: Some("remote render worker thread panicked".to_string()),
                    final_rate: None,
                });
            remote_fatal = outcome.fatal;
            remote_final_rate = outcome.final_rate.or(remote_final_rate);
        });
    } else {
        // No remote lane in play at all -- an already-`true` "remote lane done" flag
        // collapses `run_local_batches`' stop condition to "stop once the cursor is
        // empty", with no separate single-engine code path to keep in sync.
        let no_remote_lane = AtomicBool::new(true);
        run_local_batches(
            &ctx,
            &cursor,
            local_claim_size,
            &no_remote_lane,
            &mut gpu_frac,
            &mut accum,
            &mut gpu_accum,
            cancel,
            &mut on_local_batch,
        );
    }

    // Carries this call's final calibration state forward for the NEXT call on the
    // same `carry` (a no-op for `run_export`'s single-call `AccumulationCarry`, which
    // is dropped right after). `gpu_frac` is written back even when it ended `None`
    // (a declined/failed GPU): see `AccumulationCarry::hybrid_frac`'s own doc comment
    // on why that means "retry next frame", not "give up for the sweep".
    carry.hybrid_frac = gpu_frac;
    carry.remote_rate = remote_final_rate.or(carry.remote_rate);

    // A user-initiated cancel discards the whole render, remote's partial contribution
    // included. `run_remote_batch` already sent `CANCEL` to the worker for whatever
    // chunk was in flight once it observed `cancel`, so this is just bookkeeping
    // catching up.
    if cancel.load(Ordering::Relaxed) {
        return AccumulationOutcome::Cancelled;
    }

    // `RemoteOnly` only: fails the whole render outright rather than returning the
    // partial `accum`, since no local lane ever ran to pick up a failed chunk's
    // remainder.
    if let Some(message) = remote_fatal {
        return AccumulationOutcome::Failed(message);
    }

    // Merge remote's full contribution exactly once, now that the concurrent phase has
    // ended -- merging earlier would race local's CPU threads writing the same pixel
    // indices. Mirrors the GPU buffer's own single end-of-render merge just below.
    if let Some(progress) = remote_progress {
        let remote_buf = progress.into_buffer();
        for (dst, src) in accum.iter_mut().zip(&remote_buf) {
            *dst += *src;
        }
    }

    // Merge the GPU's separate accumulation exactly once. A pure-CPU render leaves
    // `gpu_accum` all zero, making this a no-op.
    for (px, gpu_px) in accum.iter_mut().zip(&gpu_accum) {
        *px += *gpu_px;
    }

    if cancel.load(Ordering::Relaxed) {
        return AccumulationOutcome::Cancelled;
    }

    AccumulationOutcome::Completed(Accumulation {
        accum,
        samples_per_pixel,
    })
}
