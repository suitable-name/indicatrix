//! [`run_export`]: the export worker's own top-level orchestration -- calibrates and
//! runs the local/remote concurrent phase, merges every engine's contribution exactly
//! once, and writes the final PNG. Moved out of `mod.rs` purely to keep that file from
//! growing further; see this crate's `export_thread` module doc comment for the wider
//! layout this fits into.

use super::{
    ExportOutcome, ExportProgress,
    batch::{ExportCtx, HYBRID_MIN_SPP, calibrate_split, local_chunk_size, run_local_batches},
    params::{ComputeTarget, ExportParams},
    preview::PreviewThrottle,
    remote::{
        self, REMOTE_CALIBRATION_SAMPLES, REMOTE_MIN_SPP, RemoteCalibration, RemoteCapability,
        RemoteProgress, calibrate_remote_rate, exceeds_pixel_cap, probe_remote, run_remote_lane,
    },
    sample_cursor::SampleCursor,
    scene_snapshot::SceneSnapshot,
    tonemap_png::{save_png, tonemap_to_rgba, tonemap_wide_gamut},
};
use crate::settings::{LocalComputeTarget, WorkerSettings};
use glam::Vec3;
use indicatrix::{
    color::ColorSpace,
    optics::raytracer::{Camera, EnvironmentSource},
    renderer::gpu_backend::{GpuBackend, GpuSceneRef},
};
use std::{
    path::Path,
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

/// Renders `params.samples_per_pixel` samples per pixel in small batches (so progress
/// can be reported and cancellation checked between batches, rather than after every
/// single sample or only once at the end), tone-maps the finished accumulation, and
/// writes it to `output_path` as PNG.
///
/// # Local + remote as a third engine, sharing a claim point
///
/// `compute_target` and `workers` add a remote worker alongside the pre-existing
/// CPU/GPU hybrid split (see "Hybrid CPU+GPU export" below). Unlike that split -- a
/// single up-front calibration, since both live in-process -- remote samples are
/// claimed from a [`SampleCursor`] the local loop ALSO claims from concurrently for
/// the whole concurrent phase: see `sample_cursor`'s module doc for why a shared
/// atomic claim point avoids handing an engine a fixed slice that runs out early with
/// nothing further to claim. `ComputeTarget::LocalOnly` skips every remote code path
/// entirely, so its output stays byte-identical to before remote existed.
///
/// Remote is dispatched as a SEQUENCE of chunk requests sized to a target wall-clock
/// duration (see `remote::remote_chunk_samples`), not one request for its whole share
/// -- a giant request is what let remote's slice finish early with nothing further to
/// claim. If a chunk's connection drops partway, [`remote::run_remote_batch`]'s
/// returned `samples_done` is always exactly the valid PREFIX completed for that
/// chunk; the unfinished remainder goes back to the shared cursor for local to pick up
/// (`ComputeTarget::Both`) or fails the export outright (`RemoteOnly`, which has no
/// local lane to hand it to).
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "this is the export worker's own top-level orchestration -- scene/output \
              params plus the compute-target/workers choice -- and its length is the \
              real local+remote concurrent-dispatch/fallback/merge logic the task \
              asked for, not padding; it's already split across `batch`/`remote`'s own \
              helper functions (`run_local_batches`, `calibrate_remote_rate`, \
              `run_remote_lane`) everywhere that split doesn't fight the shared \
              `SampleCursor` this function's whole correctness argument depends on \
              both lanes drawing from"
)]
pub(super) fn run_export(
    scene: &SceneSnapshot,
    params: ExportParams,
    color_space: ColorSpace,
    output_path: &Path,
    compute_target: ComputeTarget,
    workers: &[WorkerSettings],
    local_compute: LocalComputeTarget,
    cancel: &AtomicBool,
    mut report_progress: impl FnMut(ExportProgress),
) -> ExportOutcome {
    // `max_bounces` is deliberately unused here: `scene.max_bounces` is already the
    // export's resolved bounce cap (`gui::render_export` sets it right after
    // `SceneSnapshot::capture`). `ExportParams::max_bounces` exists only so the
    // dialog's choice is validated centrally alongside width/height/samples_per_pixel.
    let ExportParams {
        width,
        height,
        samples_per_pixel,
        max_bounces: _,
    } = params;
    let mut accum = vec![Vec3::ZERO; (width as usize) * (height as usize)];
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, 42.0);
    // Acquired once per export, not per batch: adapter acquisition and shader
    // compilation are far too slow to repeat.
    //
    // `LocalComputeTarget::Cpu` never acquires an adapter -- a disabled backend
    // declines every `try_accumulate`, so `calibrate_split` returns `None` and
    // `run_local_batches` takes its CPU-only path. `Gpu` and `CpuGpu` both acquire;
    // what separates them is the `calibrate_split` gate below, not the backend.
    let gpu = match local_compute {
        LocalComputeTarget::Cpu => GpuBackend::disabled(),
        LocalComputeTarget::CpuGpu | LocalComputeTarget::Gpu => GpuBackend::acquire(),
    };

    // Loop-invariant scene reference, hoisted.
    //
    // A loaded HDR panorama (`SceneSnapshot::env_map`) replaces the analytic studio
    // rig as this export's environment, mirroring `render_thread::mod`'s live render
    // loop. `GpuBackend::try_accumulate` already declines any `EnvironmentSource::
    // HdrMap` scene, so building `gpu_scene.environment` as `HdrMap` here reuses that
    // existing decline path automatically.
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
    let ctx = ExportCtx {
        width,
        height,
        camera: &camera,
        scene,
        gpu: &gpu,
        gpu_scene: &gpu_scene,
    };

    // ---- Remote availability -------------------------------------------------------
    // Re-probed here, not trusted from whatever the export dialog observed when it
    // opened, so a worker that went away or came up in the meantime is judged by
    // actual state at dispatch time.
    let mut samples_done = 0u32;
    let mut pending_note: Option<String> = None;
    let remote_capability: Option<RemoteCapability> =
        if matches!(compute_target, ComputeTarget::LocalOnly) {
            None
        } else if scene.env_map.is_some() {
            // An HDR environment map makes remote compute UNSOUND, not merely slower:
            // `indicatrix_net::SceneState` carries only the analytic studio rig (see
            // `remote::scene_state_from_snapshot`), so a remote worker would light the
            // stone differently than the local half, and `run_export` sums both halves
            // into one buffer -- a physically wrong, silently-varying composite, not a
            // quality tradeoff. Falling back to local-only for the WHOLE export keeps
            // the image correct and says so plainly.
            pending_note = Some(
                "This export uses an HDR environment map, which the remote render \
                 protocol cannot carry -- rendering locally only so the whole image is \
                 lit by the same environment."
                    .to_string(),
            );
            None
        } else {
            match probe_remote(workers) {
                Ok(cap) => {
                    if exceeds_pixel_cap(width, height, &cap) {
                        // Never silently shrink the export. `Both` falls back to local
                        // for the WHOLE export with a status message; `RemoteOnly` has
                        // no local fallback, so this fails outright instead.
                        let pixels = u64::from(width) * u64::from(height);
                        let message = format!(
                            "This export ({pixels} px) exceeds the remote worker's \
                             maximum of {} px.",
                            cap.max_pixels
                        );
                        if matches!(compute_target, ComputeTarget::RemoteOnly) {
                            return ExportOutcome::Failed(message);
                        }
                        pending_note = Some(format!("{message} Rendering locally only."));
                        None
                    } else {
                        Some(cap)
                    }
                }
                Err(reason) => {
                    // Same `RemoteOnly`-has-no-fallback reasoning as above.
                    if matches!(compute_target, ComputeTarget::RemoteOnly) {
                        return ExportOutcome::Failed(reason.message());
                    }
                    pending_note = Some(format!("{} Rendering locally only.", reason.message()));
                    None
                }
            }
        };
    let scene_state = remote_capability
        .as_ref()
        .map(|_| remote::scene_state_from_snapshot(scene, width, height));

    // ---- Remote calibration: an initial throughput estimate, not a fixed split ----
    // `gpu_accum` is declared here because the concurrent phase below needs it
    // regardless of whether remote ends up in play.
    let mut gpu_accum = vec![Vec3::ZERO; accum.len()];
    // `Some(rate)` once a calibration probe measures a usable remote throughput
    // estimate -- `remote::run_remote_lane` sizes its first chunk from this, then
    // adapts from every chunk after. `None` means no remote lane this export: declined
    // outright (handled above), too little budget left, or the probe failed/was
    // cancelled/ran short.
    let mut remote_rate: Option<f64> = None;
    if let (Some(cap), Some(state)) = (&remote_capability, &scene_state) {
        let remaining = samples_per_pixel - samples_done;
        // `RemoteOnly` bypasses `REMOTE_MIN_SPP`: that threshold only avoids the
        // overhead of remote when local could handle a tiny remainder alone, which is
        // irrelevant once the user explicitly chose `RemoteOnly`.
        let worth_attempting =
            remaining >= REMOTE_MIN_SPP || matches!(compute_target, ComputeTarget::RemoteOnly);
        if worth_attempting && remaining > 0 {
            // `calibrate_remote_rate` returns a `RemoteCalibration`, not a bare
            // `Option<f64>`, so a failed/short probe's reason reaches the user instead
            // of silently collapsing to "no remote". `Cancelled` gets no note -- the
            // `cancel` check just below bails the whole export out immediately after.
            let calibration = if remaining >= REMOTE_CALIBRATION_SAMPLES {
                calibrate_remote_rate(
                    cap,
                    state,
                    width,
                    height,
                    &mut samples_done,
                    &mut accum,
                    cancel,
                )
            } else {
                // Only reachable via the `RemoteOnly` bypass above, when the ENTIRE
                // sample budget is smaller than one calibration probe. Spending any of
                // that tiny budget on a throwaway timing probe would waste a
                // disproportionate share of it, and the guessed rate barely matters --
                // `SampleCursor::claim`'s clamp sizes the first (only) chunk to the
                // whole remainder regardless.
                RemoteCalibration::Ready(DEFAULT_REMOTE_RATE_GUESS)
            };
            match calibration {
                RemoteCalibration::Ready(rate) => remote_rate = Some(rate),
                RemoteCalibration::Cancelled => {}
                RemoteCalibration::Failed(message) => {
                    let note = format!("Remote worker rejected this export ({message}).");
                    if matches!(compute_target, ComputeTarget::RemoteOnly) {
                        return ExportOutcome::Failed(note);
                    }
                    pending_note = Some(format!("{note} Rendering locally only."));
                }
                RemoteCalibration::Short { done, expected } => {
                    let note = format!(
                        "Remote worker's calibration probe only completed {done} of \
                         {expected} samples before ending."
                    );
                    if matches!(compute_target, ComputeTarget::RemoteOnly) {
                        return ExportOutcome::Failed(note);
                    }
                    pending_note = Some(format!("{note} Rendering locally only."));
                }
                RemoteCalibration::Partial {
                    rate,
                    done,
                    expected,
                } => {
                    // Enough completed to size a first chunk from; the lane's own
                    // failure handling (pause-and-retry, never a permanent write-off)
                    // takes it from here.
                    remote_rate = Some(rate);
                    pending_note = Some(format!(
                        "Remote worker's calibration probe completed {done} of {expected} \
                         samples before ending early -- still using it, and retrying if \
                         it drops out again."
                    ));
                }
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return ExportOutcome::Cancelled;
    }

    // Hybrid CPU+GPU export: the GPU and all CPU cores trace DISJOINT sample ranges of
    // the same frame concurrently, and radiance sums merge -- the same
    // disjoint-sample-range convention `indicatrix::renderer::gpu::hybrid` uses. The
    // GPU owns its own accumulation buffer, summed into `accum` once at the end, so
    // the two engines never write one buffer concurrently. `gpu_frac == None` reads as
    // "single engine, no split": under `Gpu` the backend accepts every batch; under
    // `Cpu` the disabled backend declines every batch and the CPU tracer carries it.
    // Only `CpuGpu` measures a split and hands the two engines disjoint ranges.
    let mut gpu_frac: Option<f64> = if local_compute == LocalComputeTarget::CpuGpu
        && samples_per_pixel - samples_done >= HYBRID_MIN_SPP
    {
        calibrate_split(&ctx, &mut samples_done, &mut accum, &mut gpu_accum, cancel)
    } else {
        None
    };
    if cancel.load(Ordering::Relaxed) {
        return ExportOutcome::Cancelled;
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
    // export outright. `RemoteOnly` never falls back: no local lane runs under it.
    let fallback_to_local = matches!(compute_target, ComputeTarget::Both);
    // `false` only for `RemoteOnly` -- local claims nothing and the calling thread
    // just polls for progress while remote's lane runs (see the `thread::scope` body).
    let run_local = !matches!(compute_target, ComputeTarget::RemoteOnly);

    // Reports one progress tick. `local_done` is the LOCAL lane's own running total
    // (not a cursor position, which can run ahead of completed work). Adding remote's
    // independently-tracked total gives the export's true combined completion,
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
    // export (`RemoteOnly` only).
    let mut remote_fatal: Option<String> = None;
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
                });
            remote_fatal = outcome.fatal;
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

    // A user-initiated cancel discards the whole export, remote's partial contribution
    // included. `run_remote_batch` already sent `CANCEL` to the worker for whatever
    // chunk was in flight once it observed `cancel`, so this is just bookkeeping
    // catching up.
    if cancel.load(Ordering::Relaxed) {
        return ExportOutcome::Cancelled;
    }

    // `RemoteOnly` only: fails the whole export outright rather than writing the
    // partial `accum`, since no local lane ever ran to pick up a failed chunk's
    // remainder.
    if let Some(message) = remote_fatal {
        return ExportOutcome::Failed(message);
    }

    // Merge remote's full contribution exactly once, now that the concurrent phase has
    // ended -- merging earlier would race local's CPU threads writing the same pixel
    // indices. Mirrors the GPU buffer's own single end-of-export merge just below.
    if let Some(progress) = remote_progress {
        let remote_buf = progress.into_buffer();
        for (dst, src) in accum.iter_mut().zip(&remote_buf) {
            *dst += *src;
        }
    }

    // Merge the GPU's separate accumulation exactly once. A pure-CPU export leaves
    // `gpu_accum` all zero, making this a no-op.
    for (px, gpu_px) in accum.iter_mut().zip(&gpu_accum) {
        *px += *gpu_px;
    }

    if cancel.load(Ordering::Relaxed) {
        return ExportOutcome::Cancelled;
    }

    // `Srgb` keeps the exact pre-existing tone-mapping call (byte-identical output);
    // any other space routes through `tonemap_wide_gamut` -- see this module's doc.
    let rgba = if color_space == ColorSpace::Srgb {
        tonemap_to_rgba(width, height, samples_per_pixel, &accum)
    } else {
        tonemap_wide_gamut(width, height, samples_per_pixel, &accum, color_space)
    };

    if let Some(parent) = output_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return ExportOutcome::Failed(format!("Could not create output directory: {e}"));
    }

    match save_png(output_path, width, height, &rgba, color_space) {
        Ok(()) => ExportOutcome::Completed(output_path.to_path_buf()),
        Err(e) => ExportOutcome::Failed(format!("Failed to write PNG: {e}")),
    }
}
