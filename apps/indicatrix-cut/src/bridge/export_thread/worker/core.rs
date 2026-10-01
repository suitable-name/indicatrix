//! [`render_accumulation`]: the shared local/remote render core -- calibrates and runs
//! the local/remote concurrent phase and merges every engine's contribution exactly
//! once into a linear accumulation buffer, without writing anything to disk.

mod lanes;

use super::types::{AccumulationCarry, AccumulationOutcome};
use crate::{
    bridge::export_thread::{
        ExportProgress,
        batch::{ExportCtx, HYBRID_MIN_SPP, calibrate_split, local_chunk_size, run_local_batches},
        params::{ComputeTarget, ExportParams},
        sample_cursor::SampleCursor,
        scene_snapshot::SceneSnapshot,
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use glam::Vec3;
use indicatrix::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG, EnvironmentSource},
    renderer::gpu_backend::{GpuBackend, GpuSceneRef},
};
use lanes::{RenderJob, render_on_ctx};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Builds the per-call scene/camera/environment context ([`ExportCtx`]) one render's
/// batches share, and calls `f` with it. Extracted out of [`render_accumulation`] so
/// [`render_local_share`] (the viewer's own local-share render for a final-picture
/// export, v16) builds this identically -- the camera pose, environment resolution and
/// `GpuSceneRef` wiring must never drift between the two, or local and remote (or a
/// local-share render and `render_accumulation`'s own local lane) would light the stone
/// differently.
fn with_export_ctx<R>(
    scene: &SceneSnapshot,
    cam_yaw: f32,
    cam_pitch: f32,
    width: u32,
    height: u32,
    gpu: &GpuBackend,
    f: impl FnOnce(&ExportCtx<'_>) -> R,
) -> R {
    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, DEFAULT_FOV_DEG);

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
    f(&ctx)
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
    reason = "this is the shared render worker's own top-level entry point -- \
              scene/pose/output params plus the compute-target/worker/carry choice -- \
              and its body is already split across `batch`/`remote`'s own helper \
              functions and this module's `prepare_remote_lane`/`run_lanes`; bundling \
              the arguments into a struct would just move the same count into field \
              access, not reduce it"
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
    let job = RenderJob {
        scene,
        cam_yaw,
        cam_pitch,
        width,
        height,
        samples_per_pixel,
        compute_target,
        worker,
        cancel,
    };

    let rendered = with_export_ctx(scene, cam_yaw, cam_pitch, width, height, gpu, |ctx| {
        render_on_ctx(ctx, &job, local_compute, carry, &mut report_progress)
    });
    match rendered {
        Ok(done) => AccumulationOutcome::Completed(done),
        Err(early) => early,
    }
}

/// Traces absolute samples `[first, first + samples)` of `scene` with the export's own
/// local lanes ONLY (CPU, or CPU+GPU under `LocalComputeTarget::CpuGpu`) -- no remote
/// lane, no shared cursor with anything else. This is the viewer's own contribution to a
/// final-picture export that reserved this tail for it (v16 -- see
/// `remote::final_image`'s module doc comment and `worker::final_picture::final_picture`,
/// which runs this concurrently with the remote's own `FinalImageRequest` for the
/// server's share via `thread::scope`).
///
/// Returns `None` if `stop` was raised before the range finished (a user cancel, or the
/// remote lane already completed and this contribution is no longer needed); otherwise
/// `Some` is always exactly `samples` samples deep, a plain `width * height` radiance
/// sum (not yet divided by `samples`), ready to serialise and upload as one
/// `CONTRIBUTION`.
///
/// `done` is updated (absolute count relative to `first`: `0` at the start, `samples`
/// once finished) after every batch so a concurrently-polling caller can report combined
/// progress. `hybrid_frac` is both read (a carried-forward split estimate skips
/// recalibrating) and written back, the same convention `render_accumulation` uses via
/// `AccumulationCarry::hybrid_frac` -- taken here as a plain `&mut Option<f64>` instead,
/// since a local-share render and the export's own full render never run in the same
/// call and share no other carried state.
///
/// Sample identity matches the server exactly: both `render_batch` (via
/// `run_local_batches`) and `indicatrix-worker`'s own tracer derive each sample's
/// stratification from the ABSOLUTE global pixel/sample index, never a
/// range-relative one, so a viewer-rendered tail and a server-rendered one are the same
/// bits whichever side ends up tracing it.
#[expect(
    clippy::too_many_arguments,
    reason = "one render's own identity (scene, pose, output params, the absolute \
              sample range, GPU backend, local compute choice, the split-estimate \
              carry, cancellation and a progress counter) -- the same shape \
              `render_accumulation` itself takes, split differently because this is a \
              single-engine-choice, single-range, local-only render"
)]
pub(in crate::bridge::export_thread) fn render_local_share(
    scene: &SceneSnapshot,
    pose: (f32, f32),
    params: ExportParams,
    first: u32,
    samples: u32,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    hybrid_frac: &mut Option<f64>,
    stop: &AtomicBool,
    done: &AtomicU32,
) -> Option<Vec<Vec3>> {
    let (cam_yaw, cam_pitch) = pose;
    let ExportParams { width, height, .. } = params;

    with_export_ctx(scene, cam_yaw, cam_pitch, width, height, gpu, |ctx| {
        let mut accum = vec![Vec3::ZERO; (width as usize) * (height as usize)];
        let mut gpu_accum = vec![Vec3::ZERO; accum.len()];
        // Absolute cursor for the calibration probe (if any) -- starts at `first`, the
        // reserved tail's own first absolute sample, exactly as `render_accumulation`'s
        // own `samples_done` starts at `0` (its own range's first absolute sample).
        let mut next = first;
        let mut gpu_frac =
            if local_compute == LocalComputeTarget::CpuGpu && samples >= HYBRID_MIN_SPP {
                hybrid_frac
                    .or_else(|| calibrate_split(ctx, &mut next, &mut accum, &mut gpu_accum, stop))
            } else {
                None
            };

        let cursor = SampleCursor::new(next, first + samples);
        // This render has no remote lane of its own to wait on -- an already-`true`
        // "remote lane done" flag collapses `run_local_batches`' stop condition to
        // "stop once the cursor is empty", the same single-engine convention
        // `render_accumulation`'s own no-remote-lane branch uses.
        let no_remote = AtomicBool::new(true);
        let stopped = run_local_batches(
            ctx,
            &cursor,
            local_chunk_size(samples),
            &no_remote,
            &mut gpu_frac,
            &mut accum,
            &mut gpu_accum,
            stop,
            |traced, _accum, _gpu_accum| {
                done.store((next - first) + traced, Ordering::Relaxed);
            },
        );
        *hybrid_frac = gpu_frac;
        if stopped || stop.load(Ordering::Relaxed) {
            return None;
        }

        // Merge the GPU's separate accumulation exactly once, mirroring
        // `render_accumulation`'s own end-of-render merge. A pure-CPU share leaves
        // `gpu_accum` all zero, making this a no-op.
        for (px, gpu_px) in accum.iter_mut().zip(&gpu_accum) {
            *px += *gpu_px;
        }
        Some(accum)
    })
}
