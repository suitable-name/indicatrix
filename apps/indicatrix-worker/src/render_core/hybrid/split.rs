//! The steady-state hybrid sub-batch: [`hybrid_trace_into`] runs the GPU and CPU tracers
//! concurrently over disjoint sample sub-ranges and sums them, re-measuring the split
//! every call. Split out of `hybrid.rs` (calibration and its cache stay there) to keep
//! both files under the house length limit.

use super::{cpu_is_worth_splitting_with, cpu_threads_beside_gpu, scene_pieces};
use crate::render_core::{scene_uses_gpu, trace_into};
use glam::Vec3;
use indicatrix::{
    optics::raytracer::{Camera, EnvironmentSource, FacetFinish},
    renderer::gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
};
use indicatrix_dispatch::SampleRange;
use indicatrix_net::SceneState;
use std::{
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

/// One hybrid sub-batch: the GPU traces the lower `[first_sample, first_sample +
/// gpu_share)` sub-range on a scoped thread while every CPU core traces the upper
/// sub-range concurrently (via [`trace_into`]), summed into one buffer -- the same
/// return contract as [`crate::render_core::trace_samples_with_gpu`], so `run_tracer`
/// doesn't need to know which mode produced it.
///
/// `gpu_share` is `(samples as f64 * gpu_frac.unwrap_or(0.0)).round()`, clamped to
/// `samples`. `*gpu_frac` is updated afterward via the measured-throughput blend, unless
/// the GPU declines mid-job (e.g. device loss), in which case its share is retraced on
/// the CPU and `*gpu_frac` is cleared to `None` so later calls stop offering it work. A
/// batch exercising only one engine leaves `*gpu_frac` untouched: no second engine's
/// timing to compare against.
///
/// Returns `None` if the GPU's share was cancelled mid-dispatch, or if the CPU-only
/// (`gpu_share == 0`) or GPU-declined-retrace path was still queued for CPU permits when
/// `cancel` fired (`trace_into` now observes cancellation while blocked in
/// `ThreadPermits::acquire`). The whole sub-batch is discarded in that case, including
/// whatever the CPU already traced for its own disjoint share: `samples_done` only
/// advances by a whole sub-batch uniformly covering every pixel, and a partial share
/// would otherwise leave a subset of pixels under-sampled relative to the rest of the
/// frame. The concurrent-dispatch path's own CPU share (inside
/// [`dispatch_concurrently`]) is the one exception: that `trace_into` call's
/// cancellation is not separately observed -- the GPU thread it runs beside is the one
/// under real time pressure, and `run_tracer`'s between-sub-batches check still bounds
/// the CPU side's staleness.
///
/// # One core reserved for the GPU submitter
///
/// Whenever `cpu_share > 0` (both engines dispatch concurrently), the CPU tracer gets
/// `threads` minus one (see [`cpu_threads_beside_gpu`]): a CPU thread has to submit and
/// poll the GPU dispatch, and if every core is busy tracing, that submitter gets
/// descheduled -- `gpu_elapsed` would then measure the wait, and the EMA would read
/// that as "the GPU is slow", shifting more work onto the CPU and starving the
/// submitter further. That feedback loop would drive the split CPU-heavy on exactly the
/// many-core machines where the GPU should dominate. `gpu_share == 0` and a
/// GPU-declined retrace have no concurrent dispatch to protect and get every core.
///
/// # Reused buffers
///
/// The summed radiance lands in `bufs.sum` and the GPU's share is staged in `bufs.gpu`;
/// both are re-zeroed here (both backends ADD into their buffer) but keep their capacity,
/// so a stream's ~100 ms sub-batches stop allocating two ~25 MB buffers each at 1080p.
/// Returns `false` for a discarded (cancelled) sub-batch; `bufs.sum` is then unspecified.
pub fn hybrid_trace_into(
    gpu: &GpuBackend,
    scene: &SceneState,
    (first_sample, samples): (u32, u32),
    threads: usize,
    gpu_frac: &mut Option<f64>,
    cancel: &AtomicBool,
    bufs: &mut HybridBuffers,
) -> bool {
    let width = scene.width;
    let height = scene.height;
    let pixel_count = width as usize * height as usize;
    let HybridBuffers {
        sum: buffer,
        gpu: gpu_buf,
    } = bufs;
    buffer.clear();
    buffer.resize(pixel_count, Vec3::ZERO);
    if samples == 0 || width == 0 || height == 0 {
        return true;
    }

    // A cached `Split` decision is keyed by hardware, not geometry, so it can arrive here
    // for a concave scene the GPU cannot trace: drop it and go CPU-only.
    if !scene_uses_gpu(scene) {
        *gpu_frac = None;
    }
    let frac = gpu_frac.unwrap_or(0.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the product is a non-negative sample count of at most `samples` for a fraction in 0..=1; the saturating cast and the min bound it anyway"
    )]
    let gpu_share = ((f64::from(samples) * frac).round() as u32).min(samples);
    let cpu_share = samples - gpu_share;

    let (camera, facet_finishes) = scene_pieces(scene);
    // The studio rig, or the HDR map the request path resolved -- see
    // `crate::assets::resolved_hdr_map`.
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_source(scene, hdr_map.as_deref());

    if gpu_share == 0 {
        return trace_into(
            scene,
            SampleRange::new(first_sample, samples),
            threads,
            &camera,
            environment,
            buffer.as_mut_slice(),
            cancel,
        );
    }

    let batch = SplitBatch {
        gpu,
        scene,
        camera: &camera,
        facet_finishes: &facet_finishes,
        environment,
        first_sample,
        threads,
        gpu_share,
        cpu_share,
        frac,
        cancel,
    };
    batch.run(buffer, gpu_buf, gpu_frac)
}

/// The two full-frame buffers one hybrid sub-batch needs, kept across sub-batches by
/// [`hybrid_trace_into`]'s caller so they are allocated once per job, not once per ~100 ms.
#[derive(Default)]
pub struct HybridBuffers {
    /// The sub-batch's summed radiance (CPU share plus GPU share) after a successful call.
    pub sum: Vec<Vec3>,
    /// Staging for the GPU's share before it is added into `sum`.
    gpu: Vec<Vec3>,
}

/// [`hybrid_trace_into`] with freshly allocated buffers: returns the summed radiance, or
/// `None` for a discarded (cancelled) sub-batch. The streaming tracer uses the `_into`
/// form with one reused [`HybridBuffers`]; this is the convenient one-shot form.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the one-shot form; the streaming tracer uses `hybrid_trace_into`, the tests this"
    )
)]
pub fn hybrid_trace(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    gpu_frac: &mut Option<f64>,
    cancel: &AtomicBool,
) -> Option<Vec<Vec3>> {
    let mut bufs = HybridBuffers::default();
    hybrid_trace_into(
        gpu,
        scene,
        (first_sample, samples),
        threads,
        gpu_frac,
        cancel,
        &mut bufs,
    )
    .then_some(bufs.sum)
}

/// The fixed inputs of one sub-batch that really splits across both engines
/// (`gpu_share > 0`), bundled so [`Self::run`] stays within the argument-count limit.
struct SplitBatch<'a> {
    gpu: &'a GpuBackend,
    scene: &'a SceneState,
    camera: &'a Camera,
    facet_finishes: &'a [FacetFinish],
    environment: EnvironmentSource<'a>,
    first_sample: u32,
    threads: usize,
    gpu_share: u32,
    cpu_share: u32,
    /// The `gpu_frac` this batch's shares were cut from, blended with the measurement.
    frac: f64,
    cancel: &'a AtomicBool,
}

impl SplitBatch<'_> {
    /// Dispatches both shares, adds the GPU's into `buffer` and blends the measured split
    /// into `*gpu_frac`; a mid-dispatch GPU decline retraces its share on the CPU and clears
    /// `*gpu_frac`, a cancellation discards the batch (`false`).
    fn run(
        &self,
        buffer: &mut [Vec3],
        gpu_buf: &mut Vec<Vec3>,
        gpu_frac: &mut Option<f64>,
    ) -> bool {
        let Self {
            gpu,
            scene,
            camera,
            environment,
            first_sample,
            threads,
            gpu_share,
            cpu_share,
            cancel,
            ..
        } = *self;
        let pixel_count = scene.width as usize * scene.height as usize;
        let gpu_scene = GpuSceneRef {
            camera,
            width: scene.width,
            height: scene.height,
            planes: &scene.planes,
            facet_finishes: self.facet_finishes,
            material: &scene.material,
            max_bounces: scene.max_bounces,
            environment,
        };
        gpu_buf.clear();
        gpu_buf.resize(pixel_count, Vec3::ZERO);

        let (gpu_outcome, gpu_elapsed, cpu_elapsed) = if cpu_share == 0 {
            let start = Instant::now();
            let outcome = gpu.try_accumulate_cancellable(
                &gpu_scene,
                first_sample,
                gpu_share,
                gpu_buf.as_mut_slice(),
                cancel,
            );
            (outcome, start.elapsed(), Duration::ZERO)
        } else {
            // See "One core reserved for the GPU submitter" on `hybrid_trace_into`.
            let cpu = CpuShare {
                scene,
                first_sample: first_sample + gpu_share,
                samples: cpu_share,
                threads: cpu_threads_beside_gpu(threads),
                buffer: &mut *buffer,
            };
            dispatch_concurrently(
                gpu,
                &gpu_scene,
                first_sample,
                gpu_share,
                gpu_buf.as_mut_slice(),
                cancel,
                cpu,
            )
        };

        match gpu_outcome {
            GpuAccumulate::Cancelled => return false,
            GpuAccumulate::Declined => {
                // Retrace the GPU's share on the CPU so the sample count stays exact, and
                // stop offering it work. A `false` return means `cancel` fired while queued
                // for CPU permits: discard the sub-batch exactly as a mid-dispatch GPU
                // cancellation does above, rather than reporting a buffer missing the
                // retraced share.
                if !trace_into(
                    scene,
                    SampleRange::new(first_sample, gpu_share),
                    threads,
                    camera,
                    environment,
                    &mut *buffer,
                    cancel,
                ) {
                    return false;
                }
                *gpu_frac = None;
                return true;
            }
            GpuAccumulate::Done => {}
        }

        for (b, g) in buffer.iter_mut().zip(gpu_buf.iter()) {
            *b += *g;
        }

        if cpu_share > 0 {
            *gpu_frac = Some(blend_gpu_frac(
                self.frac,
                gpu_share,
                cpu_share,
                gpu_elapsed,
                cpu_elapsed,
            ));
        }

        true
    }
}

/// The CPU half of one concurrent hybrid sub-batch -- the upper, disjoint sample
/// sub-range [`hybrid_trace_into`] hands to [`dispatch_concurrently`] while the GPU traces
/// the lower one. Bundled so that function stays under clippy's argument limit.
struct CpuShare<'a> {
    scene: &'a SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    buffer: &'a mut [Vec3],
}

/// Runs the GPU's `[first_sample, first_sample + gpu_share)` share on a scoped thread
/// while the calling thread traces `cpu`'s own share via [`trace_into`], then joins.
/// Returns the GPU outcome plus each engine's wall time, measured independently so
/// [`blend_gpu_frac`] compares real throughputs rather than the join's slower half.
///
/// A panic on the GPU thread is re-raised here rather than swallowed: it means a bug,
/// not a device loss (which surfaces as [`GpuAccumulate::Declined`]).
#[expect(
    clippy::needless_pass_by_value,
    reason = "CpuShare::buffer is an exclusive &mut [Vec3] that must be MOVED into \
              trace_into below; taking `&CpuShare` instead would make that reborrow \
              impossible from behind a shared reference, so by-value is the only option"
)]
fn dispatch_concurrently(
    gpu: &GpuBackend,
    gpu_scene: &GpuSceneRef<'_>,
    first_sample: u32,
    gpu_share: u32,
    gpu_buf: &mut [Vec3],
    cancel: &AtomicBool,
    cpu: CpuShare<'_>,
) -> (GpuAccumulate, Duration, Duration) {
    thread::scope(|scope| {
        let gpu_handle = scope.spawn(move || {
            let start = Instant::now();
            let outcome =
                gpu.try_accumulate_cancellable(gpu_scene, first_sample, gpu_share, gpu_buf, cancel);
            (outcome, start.elapsed())
        });
        let cpu_start = Instant::now();
        // `cancel` plays no finer-grained role here than the between-sub-batches check
        // `run_tracer`'s loop already performs (see `hybrid_trace_into`'s doc comment);
        // a `false` return only means CPU permits never freed before cancellation, and
        // the joined GPU outcome below is what this call site actually acts on.
        let _ = trace_into(
            cpu.scene,
            SampleRange::new(cpu.first_sample, cpu.samples),
            cpu.threads,
            gpu_scene.camera,
            gpu_scene.environment,
            cpu.buffer,
            cancel,
        );
        let cpu_elapsed = cpu_start.elapsed();
        let (outcome, gpu_elapsed) = gpu_handle
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload));
        (outcome, gpu_elapsed, cpu_elapsed)
    })
}

/// Blends the just-measured throughput split into the running `gpu_frac` estimate via
/// the 0.7-old/0.3-new EMA, pulled out of [`SplitBatch::run`] to keep it short.
///
/// Pins to `1.0` (not the raw blended value) once the GPU so outclasses the CPU that
/// splitting can no longer pay for its synchronisation (see `HYBRID_MAX_GPU_SHARE`) --
/// keeps the decision stable: `1.0` drives `cpu_share` to 0 on every later call, taking
/// the GPU-only branch and skipping this blend from then on.
fn blend_gpu_frac(
    frac: f64,
    gpu_share: u32,
    cpu_share: u32,
    gpu_elapsed: Duration,
    cpu_elapsed: Duration,
) -> f64 {
    let gpu_rate = f64::from(gpu_share) / gpu_elapsed.as_secs_f64().max(1e-9);
    let cpu_rate = f64::from(cpu_share) / cpu_elapsed.as_secs_f64().max(1e-9);
    let measured_frac = gpu_rate / (gpu_rate + cpu_rate);
    let updated = frac.mul_add(0.7, measured_frac * 0.3).clamp(0.0, 1.0);
    if cpu_is_worth_splitting_with(updated) {
        updated
    } else {
        1.0
    }
}
