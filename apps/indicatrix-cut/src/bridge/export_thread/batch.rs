//! The batch render loop: hybrid CPU+GPU calibration and per-batch dispatch, plus the
//! CPU scanline batch tracer itself.
//!
//! Split out of `bridge::export_thread` purely to keep that module (already sizeable)
//! from growing further.

use super::{sample_cursor::SampleCursor, scene_snapshot::SceneSnapshot};
use glam::Vec3;
use indicatrix::{
    optics::raytracer::{
        Camera, EnvironmentSource, pixel_rotations, sample_draws, trace_spectral_ray_with_finish,
    },
    renderer::gpu_backend::{GpuBackend, GpuSceneRef},
};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

/// Below this many samples per pixel, hybrid calibration costs more than it
/// saves; the export takes the single-engine path instead.
pub(super) const HYBRID_MIN_SPP: u32 = 8;

/// How many batches the local hybrid loop targets across the FULL export, not just
/// whatever share local ends up claiming (see [`local_chunk_size`]).
const TARGET_BATCHES: u32 = 40;

/// The local lane's claim size for [`SampleCursor::claim_local`]. Derived from the
/// export's OVERALL `samples_per_pixel`, not local's dynamically-decided share, so a
/// `LocalOnly` export's batch granularity (and progress-reporting/cancellation
/// latency) stays the same whether or not a remote worker is in play. When remote is
/// claiming a share of the budget, local just runs fewer same-sized claims.
pub(super) fn local_chunk_size(samples_per_pixel: u32) -> u32 {
    (samples_per_pixel / TARGET_BATCHES).max(1)
}

/// How long the local lane sleeps between re-checks of `SampleCursor::claim_local`
/// when it momentarily finds nothing to claim but the remote lane hasn't signalled
/// done yet. Export batches run for seconds at a time, so this poll interval only
/// matters in the closing moments.
const LOCAL_RETRY_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Everything one export batch needs, bundled so the hybrid helpers stay
/// within clippy's argument-count limit.
pub(super) struct ExportCtx<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) camera: &'a Camera,
    pub(super) scene: &'a SceneSnapshot,
    pub(super) gpu: &'a GpuBackend,
    pub(super) gpu_scene: &'a GpuSceneRef<'a>,
    /// Set once a joined GPU-thread panic is observed in [`hybrid_batch`]
    /// and never cleared for the rest of this export -- `GpuBackend` itself only
    /// retires its own internal `lost` flag on a cleanly-reported `DeviceLost`, which a
    /// raw thread panic never reaches. Every `ctx.gpu.try_accumulate` call site in this
    /// module checks this FIRST, so a panic mid-export retires the backend for every
    /// later batch (both the hybrid split and the single-engine fallback in
    /// [`run_local_batches`]), not just the one batch it happened in.
    pub(super) gpu_retired: &'a AtomicBool,
}

/// Times one real export sample per pixel on each engine (GPU, then CPU -- both
/// counted toward the export) and returns the GPU's throughput share for hybrid
/// batches, or `None` when the GPU declines (no adapter, no `gpu` feature, or an HDR
/// environment map, which the megakernel has no `env_mode` for).
///
/// The GPU side actually dispatches TWO 1-spp samples: the first includes
/// output-buffer allocation and driver warm-up (measured cold 102ms vs warm 73ms at
/// 800x600), which would otherwise inflate `gpu_time` into a pessimistic estimate of
/// steady-state throughput. Timing the SECOND sample instead lands that warm-up cost
/// on the untimed (but still real and counted) first one. Checks `cancel` once,
/// between the GPU and CPU measurements, so a cancel-during-calibration export doesn't
/// also pay for the CPU sample.
pub(super) fn calibrate_split(
    ctx: &ExportCtx<'_>,
    samples_done: &mut u32,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
    cancel: &AtomicBool,
) -> Option<f64> {
    // A prior batch's joined GPU-thread panic retired the backend for the
    // rest of this export -- decline exactly like a normal `try_accumulate` decline,
    // without touching the (possibly corrupted) renderer again.
    if ctx.gpu_retired.load(Ordering::Relaxed) {
        return None;
    }
    // Untimed warm-up sample: a real export sample (counted below), just not the one
    // whose wall-clock cost feeds the split.
    if !ctx
        .gpu
        .try_accumulate(ctx.gpu_scene, *samples_done, 1, gpu_accum)
    {
        return None;
    }
    *samples_done += 1;

    let start = std::time::Instant::now();
    if !ctx
        .gpu
        .try_accumulate(ctx.gpu_scene, *samples_done, 1, gpu_accum)
    {
        return None;
    }
    let gpu_time = start.elapsed().as_secs_f64().max(1e-9);
    *samples_done += 1;

    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let start = std::time::Instant::now();
    render_batch(
        ctx.width,
        ctx.height,
        1,
        *samples_done,
        ctx.camera,
        ctx.scene,
        accum,
    );
    let cpu_time = start.elapsed().as_secs_f64().max(1e-9);
    *samples_done += 1;

    // GPU share proportional to measured throughput (1/time per engine).
    let frac = cpu_time / (gpu_time + cpu_time);
    Some(frac.clamp(0.0, 1.0))
}

/// One hybrid batch: the GPU traces the batch's lower sample range into its
/// own buffer on a scoped thread while every CPU core traces the upper range
/// into `accum`; the ranges are disjoint, so the per-sample stratification
/// each engine derives from the absolute sample index stays consistent. If
/// the GPU declines mid-export (e.g. device loss), its share is retraced on
/// the CPU so the sample count stays exact, and `gpu_frac` is cleared so
/// later batches stop offering it work.
///
/// # Adapting the split as the export runs
///
/// `calibrate_split` only measures one cold(ish) dispatch pair before any real batch
/// runs; freezing that fraction for all ~40 batches would ignore thermal drift and
/// other load changes over a multi-minute export. So every batch that actually
/// measures both engines (`gpu_share > 0 && cpu_share > 0`) blends its own measured
/// split into `gpu_frac` via the same 0.7-old/0.3-new exponential moving average
/// `render_thread::gpu_backend::HybridPacing::blend` uses for the live viewport.
/// Batches that only exercise one engine leave `gpu_frac` unchanged -- there is no
/// second engine's timing to compare against.
pub(super) fn hybrid_batch(
    ctx: &ExportCtx<'_>,
    samples_done: u32,
    this_batch: u32,
    gpu_frac: &mut Option<f64>,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
) {
    // A prior batch's joined GPU-thread panic retired the backend for the
    // rest of this export -- force this batch entirely onto the CPU (like a normal
    // decline) and stop offering the GPU any future share, rather than dispatching
    // into the renderer state a panic may have left mapped/corrupted.
    let frac = if ctx.gpu_retired.load(Ordering::Relaxed) {
        *gpu_frac = None;
        0.0
    } else {
        gpu_frac.unwrap_or(0.0)
    };
    let gpu_share = (f64::from(this_batch) * frac).round() as u32;
    let gpu_share = gpu_share.min(this_batch);
    let cpu_share = this_batch - gpu_share;

    if gpu_share == 0 {
        render_batch(
            ctx.width,
            ctx.height,
            this_batch,
            samples_done,
            ctx.camera,
            ctx.scene,
            accum,
        );
        return;
    }

    let (gpu_ok, gpu_elapsed, cpu_elapsed) = if cpu_share == 0 {
        let start = std::time::Instant::now();
        let ok = ctx
            .gpu
            .try_accumulate(ctx.gpu_scene, samples_done, gpu_share, gpu_accum);
        (ok, start.elapsed(), std::time::Duration::ZERO)
    } else {
        thread::scope(|s| {
            let gpu_task = s.spawn(|| {
                let start = std::time::Instant::now();
                let ok = ctx
                    .gpu
                    .try_accumulate(ctx.gpu_scene, samples_done, gpu_share, gpu_accum);
                (ok, start.elapsed())
            });
            let cpu_start = std::time::Instant::now();
            render_batch(
                ctx.width,
                ctx.height,
                cpu_share,
                samples_done + gpu_share,
                ctx.camera,
                ctx.scene,
                accum,
            );
            let cpu_elapsed = cpu_start.elapsed();
            if let Ok((gpu_ok, gpu_elapsed)) = gpu_task.join() {
                (gpu_ok, gpu_elapsed, cpu_elapsed)
            } else {
                // The GPU thread panicked rather than returning normally --
                // distinct from a plain decline. Retire the backend for the REST of
                // this export (every later batch, via `ctx.gpu_retired`), not just
                // clear this batch's own `gpu_frac` split.
                tracing::error!(
                    "GPU export thread panicked mid-batch; retiring the GPU \
                     backend for the rest of this export"
                );
                ctx.gpu_retired.store(true, Ordering::Relaxed);
                (false, std::time::Duration::ZERO, cpu_elapsed)
            }
        })
    };

    if !gpu_ok {
        render_batch(
            ctx.width,
            ctx.height,
            gpu_share,
            samples_done,
            ctx.camera,
            ctx.scene,
            accum,
        );
        *gpu_frac = None;
        return;
    }

    if cpu_share > 0 {
        let gpu_rate = f64::from(gpu_share) / gpu_elapsed.as_secs_f64().max(1e-9);
        let cpu_rate = f64::from(cpu_share) / cpu_elapsed.as_secs_f64().max(1e-9);
        let measured_frac = gpu_rate / (gpu_rate + cpu_rate);
        let updated = frac.mul_add(0.7, measured_frac * 0.3);
        *gpu_frac = Some(updated.clamp(0.0, 1.0));
    }
}

/// Runs the local CPU+GPU hybrid loop, claiming successive ranges from the shared
/// `cursor` rather than owning a private `[start, total)` range -- this lets local
/// keep claiming MORE of the budget for as long as any remains, instead of stopping
/// once a fixed one-shot share runs out.
///
/// Extracted out of `export_thread::run_export` so that function can run this on the
/// SAME thread while a concurrently-dispatched remote lane
/// (`export_thread::remote::run_remote_lane`) runs on another, inside one
/// `thread::scope`.
///
/// `local_chunk_size` is derived from the export's OVERALL `samples_per_pixel` (see
/// [`local_chunk_size`]), not from whatever the cursor has left, keeping a local-only
/// export's batch granularity the same whether or not a remote worker is in play.
///
/// # Knowing when to stop
///
/// `cursor.claim_local` momentarily returning `None` does NOT by itself mean this loop
/// is done: the remote lane could still push a failed chunk's remainder into the
/// cursor's retry pile a moment later (`SampleCursor::return_to_local`,
/// `ComputeTarget::Both` only). So this stops only once BOTH the cursor's shared pool
/// and retry pile are empty AND `remote_lane_done` is set, sleeping and re-checking
/// otherwise. Callers with no remote lane in play pass an already-`true`
/// `remote_lane_done`, collapsing this to "stop once the cursor is empty".
///
/// Returns `true` iff cancellation was observed -- callers decide what a cancellation
/// means once they've also learned what the remote lane did, since a cancelled export
/// must NOT be reported done just because the local half stopped.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of the local hybrid loop's own state \
              (scene/GPU context, the shared cursor and its claim size, the CPU/GPU \
              split estimate, the two accumulation buffers, remote-lane-done \
              signalling, cancellation, and the progress callback) -- bundling them \
              into a struct would just move the same count into field access, not \
              reduce it"
)]
pub(super) fn run_local_batches(
    ctx: &ExportCtx<'_>,
    cursor: &SampleCursor,
    local_chunk_size: u32,
    remote_lane_done: &AtomicBool,
    gpu_frac: &mut Option<f64>,
    accum: &mut [Vec3],
    gpu_accum: &mut [Vec3],
    cancel: &AtomicBool,
    mut on_batch: impl FnMut(u32, &[Vec3], &[Vec3]),
) -> bool {
    let mut local_traced = 0u32;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }

        let Some((start, count)) = cursor.claim_local(local_chunk_size) else {
            if remote_lane_done.load(Ordering::Acquire) {
                return false;
            }
            // See "Knowing when to stop" above -- remote might still hand local a
            // failed chunk's remainder any moment.
            thread::sleep(LOCAL_RETRY_POLL_INTERVAL);
            continue;
        };

        if gpu_frac.is_some() {
            hybrid_batch(ctx, start, count, gpu_frac, accum, gpu_accum);
        } else if ctx.gpu_retired.load(Ordering::Relaxed)
            || !ctx
                .gpu
                .try_accumulate(ctx.gpu_scene, start, count, gpu_accum)
        {
            // `ctx.gpu_retired` short-circuits `try_accumulate` entirely
            // once a prior batch's GPU-thread panic retired the backend -- this is the
            // single-engine path a later batch takes once `hybrid_batch` has already
            // cleared `gpu_frac` back to `None`, and it must not call back into the
            // renderer a panic may have left mapped/corrupted.
            render_batch(
                ctx.width, ctx.height, count, start, ctx.camera, ctx.scene, accum,
            );
        }
        local_traced += count;
        on_batch(local_traced, accum, gpu_accum);
    }
}

/// Traces `batch_spp` additional samples per pixel across `thread::available_parallelism`
/// CPU threads, adding them into `accum` -- the export-worker analog of
/// `render_thread::render_frame_scanlines`, minus per-frame tone-mapping (export
/// tone-maps once at the end) and progressive-frame bookkeeping.
///
/// # Work distribution: a shared atomic row counter, not contiguous row bands
///
/// Rows through the stone cost far more than background rows, so this claims rows
/// dynamically through a shared `AtomicUsize` counter (`fetch_add` per row) rather
/// than splitting the image into `num_threads` contiguous bands -- same fix as
/// `render_thread::scanline::render_frame_scanlines`. `accum` is pre-split into
/// per-row slices behind a `Mutex<Vec<Option<&mut [Vec3]>>>`; a thread claims row `y`,
/// locks just long enough to `Option::take` that row's slice, then accumulates the
/// whole row without the lock held. Every row is claimed by exactly one thread, so
/// per-pixel sums stay bit-identical regardless of which thread renders which row.
// `pub`, not `pub(super)`: `bridge::preview_render` reuses this exact tracer for its
// own CPU fallback rather than re-deriving the same per-pixel jitter/hero-wavelength
// sampling -- one implementation, not two that could drift apart.
pub fn render_batch(
    width: u32,
    height: u32,
    batch_spp: u32,
    samples_already_done: u32,
    camera: &Camera,
    scene: &SceneSnapshot,
    accum: &mut [Vec3],
) {
    let num_threads = thread::available_parallelism().map_or(8, std::num::NonZero::get);
    let width_usize = width as usize;

    let rows: Vec<Option<&mut [Vec3]>> = accum.chunks_mut(width_usize).map(Some).collect();
    let rows = Mutex::new(rows);
    let next_row = AtomicUsize::new(0);

    // Hoisted out of the per-sample loop: it cannot change within one `render_batch`
    // call. `scene.env_map` replaces the studio rig with a loaded HDR panorama when
    // one was active at capture time -- the same selection `run_export`'s
    // `environment` binding uses for the GPU path, so CPU and GPU always agree on
    // which environment this export uses.
    let environment = scene.env_map.as_deref().map_or_else(
        || {
            scene
                .lighting_preset
                .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
                .with_backdrop(scene.backdrop)
        },
        EnvironmentSource::HdrMap,
    );

    thread::scope(|s| {
        for _ in 0..num_threads {
            let rows = &rows;
            let next_row = &next_row;

            s.spawn(move || {
                loop {
                    let y = next_row.fetch_add(1, Ordering::Relaxed);
                    if y >= height as usize {
                        break;
                    }

                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("each row index is claimed by exactly one thread via fetch_add");

                    for (x, pixel) in row.iter_mut().enumerate() {
                        let global_pixel_idx = (y * width_usize + x) as u32;
                        let mut sample_sum = Vec3::ZERO;

                        // Per-pixel Cranley-Patterson rotations for the stratified
                        // pixel-jitter/hero-wavelength draws below -- must stay in
                        // sync with `apps/indicatrix-worker/src/render_core.rs::
                        // trace_into` (both go through the shared
                        // `indicatrix::optics::raytracer::sampling` functions).
                        let rot = pixel_rotations(global_pixel_idx);

                        for s_idx in 0..batch_spp {
                            let sample_num = samples_already_done + s_idx;
                            let draws = sample_draws(global_pixel_idx, sample_num, &rot);

                            let ray = camera.generate_ray(
                                x as f32,
                                y as f32,
                                width as f32,
                                height as f32,
                                draws.jitter_x,
                                draws.jitter_y,
                            );

                            // Frosted girdle: `scene.facet_finishes` is empty
                            // whenever the toggle was off at capture time, which is
                            // exactly equivalent to `trace_spectral_ray`.
                            sample_sum += trace_spectral_ray_with_finish(
                                ray,
                                &scene.active_planes,
                                &scene.facet_finishes,
                                &scene.material,
                                scene.max_bounces,
                                environment,
                                draws.seed,
                                draws.hero_rand,
                                None,
                            );
                        }

                        *pixel += sample_sum;
                    }
                }
            });
        }
    });
}
