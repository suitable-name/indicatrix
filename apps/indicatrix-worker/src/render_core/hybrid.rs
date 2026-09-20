//! Hybrid CPU+GPU tracing for one `stream_emit::tracer::run_tracer` job: concurrently
//! runs the GPU and CPU tracers over disjoint sample sub-ranges of a batch (summed
//! together, since disjoint sample ranges are additive), rather than
//! [`super::trace_samples_with_gpu`]'s either/or choice of one engine for the whole
//! batch.
//!
//! Ported from `apps/indicatrix-cut/src/bridge/export_thread/batch.rs`'s
//! `calibrate_split`/`hybrid_batch` pattern (this crate cannot depend on that app):
//! [`calibrate`] times one real sample per pixel on each engine to seed an initial GPU
//! throughput share, and [`hybrid_trace`] re-measures its own split every call, blending
//! it into the running estimate with a 0.7-old/0.3-new exponential moving average so the
//! split keeps tracking the machine's actual throughput across a long-running stream.

use super::{VIEWER_FOV_DEG, resolve_facet_finishes, trace_into};
use glam::Vec3;
use indicatrix::{
    optics::raytracer::Camera,
    renderer::gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
};
use indicatrix_net::SceneState;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Instant,
};

/// Below this many total samples in a `RenderRequest`, hybrid calibration (two extra,
/// otherwise-unnecessary dispatches) costs more than splitting could ever save. Also
/// acts as the floor [`calibrate`] itself needs (three real samples).
pub const HYBRID_MIN_SPP: u32 = 8;

/// How many CPU threads to trace with while a GPU dispatch runs concurrently: every
/// core except one, reserved for the thread that submits and polls the GPU dispatch
/// (see the call site). Never returns 0: a single-core machine still traces its share
/// with something, and the reservation is an optimisation, not a requirement.
fn cpu_threads_beside_gpu(threads: usize) -> usize {
    super::effective_thread_count(threads)
        .saturating_sub(1)
        .max(1)
}

/// Above this GPU throughput share, splitting work with the CPU costs more than it can
/// possibly save, so the job goes GPU-only for the rest of its life.
///
/// # Measured, not guessed
///
/// On a real Vulkan adapter (800x600, 6 bounces): GPU-only 79.5 samples/s, CPU-only 7.7
/// samples/s -- GPU 10.3x faster, a theoretical 13% ceiling for splitting. Measured
/// result splitting anyway: **0.58x, a 42% loss**, despite the EMA converging correctly
/// (`gpu_frac` 0.884 vs. a theoretical optimum of 0.912) -- per-sub-batch
/// synchronisation plus the CPU tracer stealing cores/bandwidth from the GPU's
/// submit-and-poll thread costs far more than the theoretical gain.
///
/// `0.7` sits well above break-even: it admits hybrid only when the CPU is within
/// roughly 2.3x of the GPU (~1.43x theoretical gain), leaving headroom for that
/// overhead. A strong CPU beside a weak integrated GPU still benefits; a discrete GPU
/// beside any CPU does not, and now correctly declines to try.
const HYBRID_MAX_GPU_SHARE: f64 = 0.7;

/// Whether a measured GPU share still leaves the CPU enough of a contribution to be
/// worth the synchronisation -- see [`HYBRID_MAX_GPU_SHARE`].
fn cpu_is_worth_splitting_with(gpu_share_frac: f64) -> bool {
    gpu_share_frac < HYBRID_MAX_GPU_SHARE
}

/// What [`calibrate`] found. A caller can tell the two decline reasons apart: the GPU
/// itself declining the probe dispatch is unremarkable, but `GpuDominates` is
/// [`HYBRID_MAX_GPU_SHARE`] firing -- a deliberate, measurement-backed cutoff worth
/// logging (see `run_tracer`'s handling of this variant).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalibrationOutcome {
    /// The GPU offered a share worth splitting for: `(gpu_share_frac, samples_consumed)`.
    Split(f64, u32),
    /// The GPU declined the probe dispatch itself (no adapter, or an unsupported
    /// material) -- the caller falls back to [`super::trace_samples_with_gpu`]'s
    /// either/or behavior for the whole job.
    GpuDeclined,
    /// The GPU accepted the probe, but its measured throughput share meets or exceeds
    /// [`HYBRID_MAX_GPU_SHARE`]. Carries the measured share so the caller can log it;
    /// the job still runs GPU-only, exactly as `GpuDeclined` does.
    GpuDominates { gpu_share_frac: f64 },
    /// `cancel` was observed set before calibration finished: a caller-supplied scene
    /// can make even the tiny 3-sample probe take real wall time, so calibration needs
    /// the same cooperative cancellation every other sub-batch gets.
    ///
    /// `consumed` is exactly how many of the 3 probe samples were fully traced and
    /// already folded into `out` before `cancel` was noticed (0, 1, or 2). These are
    /// real, already-spent samples -- the caller must fold them into its own accounting
    /// exactly as [`Self::Split`]'s `consumed` is, before stopping the job.
    Cancelled { consumed: u32 },
}

/// Rebuilds the per-call scene pieces [`GpuSceneRef`]/`trace_into` both need from
/// `scene` alone -- cheap enough to recompute fresh each call rather than threading a
/// persistent struct through the job's lifetime, avoiding lifetime entanglement with
/// `run_tracer`'s loop.
fn scene_pieces(scene: &SceneState) -> (Camera, Vec<indicatrix::optics::raytracer::FacetFinish>) {
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, VIEWER_FOV_DEG);
    (camera, resolve_facet_finishes(scene))
}

/// Times one real sample per pixel on each engine (GPU, then CPU) and returns a
/// [`CalibrationOutcome`].
///
/// Spends exactly 3 real samples from the front of `[first_sample, first_sample +
/// samples)`, folding their contribution into `out` like any other sub-batch -- the
/// caller accounts for them the same way. Requires `samples >= 3`; callers gating on
/// [`HYBRID_MIN_SPP`] never hit that floor.
///
/// The GPU side dispatches two 1-spp samples, not one: the first pays one-time
/// adapter/pipeline warm-up, which would otherwise inflate the estimate into a
/// pessimistic read of steady-state throughput. Timing the second sample instead lands
/// that cost on the untimed (but still real and counted) first one.
///
/// `cancel` is checked before each of the 3 probe samples -- see
/// [`CalibrationOutcome::Cancelled`] for what `consumed` means when it fires.
#[must_use]
pub fn calibrate(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    out: &mut [Vec3],
    cancel: &AtomicBool,
) -> CalibrationOutcome {
    if samples < 3 {
        return CalibrationOutcome::GpuDeclined;
    }
    if cancel.load(Ordering::Relaxed) {
        return CalibrationOutcome::Cancelled { consumed: 0 };
    }
    let (camera, facet_finishes) = scene_pieces(scene);
    let environment = scene
        .lighting_preset
        .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
        .with_backdrop(scene.backdrop);
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width: scene.width,
        height: scene.height,
        planes: &scene.planes,
        facet_finishes: &facet_finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment,
    };

    // Untimed warm-up sample: real and counted, just not the one whose wall-clock cost
    // feeds the split.
    match gpu.try_accumulate_cancellable(&gpu_scene, first_sample, 1, out, cancel) {
        GpuAccumulate::Done => {}
        GpuAccumulate::Declined => return CalibrationOutcome::GpuDeclined,
        // `out` is guaranteed untouched, so `consumed: 0` is exact.
        GpuAccumulate::Cancelled => return CalibrationOutcome::Cancelled { consumed: 0 },
    }
    if cancel.load(Ordering::Relaxed) {
        return CalibrationOutcome::Cancelled { consumed: 1 };
    }

    let start = Instant::now();
    match gpu.try_accumulate_cancellable(&gpu_scene, first_sample + 1, 1, out, cancel) {
        GpuAccumulate::Done => {}
        GpuAccumulate::Declined => return CalibrationOutcome::GpuDeclined,
        // The warm-up sample already folded into `out`, so `consumed: 1` is exact.
        GpuAccumulate::Cancelled => return CalibrationOutcome::Cancelled { consumed: 1 },
    }
    let gpu_time = start.elapsed().as_secs_f64().max(1e-9);
    if cancel.load(Ordering::Relaxed) {
        return CalibrationOutcome::Cancelled { consumed: 2 };
    }

    let start = Instant::now();
    trace_into(
        scene,
        first_sample + 2,
        1,
        threads,
        &camera,
        environment,
        out,
    );
    let cpu_time = start.elapsed().as_secs_f64().max(1e-9);

    // GPU share proportional to measured throughput (1/time per engine).
    let frac = (cpu_time / (gpu_time + cpu_time)).clamp(0.0, 1.0);
    // Decline the split outright when the GPU already dominates (HYBRID_MAX_GPU_SHARE).
    // Returning GpuDominates (not Split(1.0, 3)) sends the caller down
    // trace_samples_with_gpu's plain path, so a dominant GPU never pays split overhead.
    //
    // The three probe samples traced into `out` are discarded on this path:
    // run_tracer only folds `out` in on Split, then re-traces the full range from
    // first_sample -- three wasted samples, once per job, cheap against HYBRID_MIN_SPP.
    if !cpu_is_worth_splitting_with(frac) {
        return CalibrationOutcome::GpuDominates {
            gpu_share_frac: frac,
        };
    }
    CalibrationOutcome::Split(frac, 3)
}

/// One hybrid sub-batch: the GPU traces the lower `[first_sample, first_sample +
/// gpu_share)` sub-range on a scoped thread while every CPU core traces the upper
/// sub-range concurrently (via [`trace_into`]), summed into one buffer -- the same
/// return contract as [`super::trace_samples_with_gpu`], so `run_tracer` doesn't need
/// to know which mode produced it.
///
/// `gpu_share` is `(samples as f64 * gpu_frac.unwrap_or(0.0)).round()`, clamped to
/// `samples`. `*gpu_frac` is updated afterward via the measured-throughput blend, unless
/// the GPU declines mid-job (e.g. device loss), in which case its share is retraced on
/// the CPU and `*gpu_frac` is cleared to `None` so later calls stop offering it work. A
/// batch exercising only one engine leaves `*gpu_frac` untouched: no second engine's
/// timing to compare against.
///
/// Returns `None` if the GPU's share was cancelled mid-dispatch. The whole sub-batch is
/// discarded in that case, including whatever the CPU already traced for its own
/// disjoint share: `samples_done` only advances by a whole sub-batch uniformly covering
/// every pixel, and a cancelled GPU share would otherwise leave a subset of pixels
/// under-sampled relative to the rest of the frame. `cancel` plays no role in the
/// `gpu_share == 0` (CPU-only) path: `trace_into` has no finer-grained cancellation than
/// the between-sub-batches check `run_tracer`'s loop already performs.
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
pub fn hybrid_trace(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    gpu_frac: &mut Option<f64>,
    cancel: &AtomicBool,
) -> Option<Vec<Vec3>> {
    let width = scene.width;
    let height = scene.height;
    let mut buffer = vec![Vec3::ZERO; width as usize * height as usize];
    if samples == 0 || width == 0 || height == 0 {
        return Some(buffer);
    }

    let frac = gpu_frac.unwrap_or(0.0);
    let gpu_share = ((f64::from(samples) * frac).round() as u32).min(samples);
    let cpu_share = samples - gpu_share;

    let (camera, facet_finishes) = scene_pieces(scene);
    let environment = scene
        .lighting_preset
        .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
        .with_backdrop(scene.backdrop);

    if gpu_share == 0 {
        trace_into(
            scene,
            first_sample,
            samples,
            threads,
            &camera,
            environment,
            &mut buffer,
        );
        return Some(buffer);
    }

    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width,
        height,
        planes: &scene.planes,
        facet_finishes: &facet_finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment,
    };
    let mut gpu_buf = vec![Vec3::ZERO; width as usize * height as usize];

    let (gpu_outcome, gpu_elapsed, cpu_elapsed) = if cpu_share == 0 {
        let start = Instant::now();
        let outcome = gpu.try_accumulate_cancellable(
            &gpu_scene,
            first_sample,
            gpu_share,
            &mut gpu_buf,
            cancel,
        );
        (outcome, start.elapsed(), std::time::Duration::ZERO)
    } else {
        // See "One core reserved for the GPU submitter" above.
        let cpu = CpuShare {
            scene,
            first_sample: first_sample + gpu_share,
            samples: cpu_share,
            threads: cpu_threads_beside_gpu(threads),
            buffer: &mut buffer,
        };
        dispatch_concurrently(
            gpu,
            &gpu_scene,
            first_sample,
            gpu_share,
            &mut gpu_buf,
            cancel,
            cpu,
        )
    };

    match gpu_outcome {
        GpuAccumulate::Cancelled => return None,
        GpuAccumulate::Declined => {
            // Retrace the GPU's share on the CPU so the sample count stays exact, and
            // stop offering it work.
            trace_into(
                scene,
                first_sample,
                gpu_share,
                threads,
                &camera,
                environment,
                &mut buffer,
            );
            *gpu_frac = None;
            return Some(buffer);
        }
        GpuAccumulate::Done => {}
    }

    for (b, g) in buffer.iter_mut().zip(&gpu_buf) {
        *b += *g;
    }

    if cpu_share > 0 {
        *gpu_frac = Some(blend_gpu_frac(
            frac,
            gpu_share,
            cpu_share,
            gpu_elapsed,
            cpu_elapsed,
        ));
    }

    Some(buffer)
}

/// The CPU half of one concurrent hybrid sub-batch -- the upper, disjoint sample
/// sub-range [`hybrid_trace`] hands to [`dispatch_concurrently`] while the GPU traces
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
) -> (GpuAccumulate, std::time::Duration, std::time::Duration) {
    thread::scope(|scope| {
        let gpu_handle = scope.spawn(move || {
            let start = Instant::now();
            let outcome =
                gpu.try_accumulate_cancellable(gpu_scene, first_sample, gpu_share, gpu_buf, cancel);
            (outcome, start.elapsed())
        });
        let cpu_start = Instant::now();
        trace_into(
            cpu.scene,
            cpu.first_sample,
            cpu.samples,
            cpu.threads,
            gpu_scene.camera,
            gpu_scene.environment,
            cpu.buffer,
        );
        let cpu_elapsed = cpu_start.elapsed();
        let (outcome, gpu_elapsed) = gpu_handle
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload));
        (outcome, gpu_elapsed, cpu_elapsed)
    })
}

/// Blends the just-measured throughput split into the running `gpu_frac` estimate via
/// the 0.7-old/0.3-new EMA, pulled out of [`hybrid_trace`] to keep it under clippy's
/// line-count limit.
///
/// Pins to `1.0` (not the raw blended value) once the GPU so outclasses the CPU that
/// splitting can no longer pay for its synchronisation (see [`HYBRID_MAX_GPU_SHARE`]) --
/// keeps the decision stable: `1.0` drives `cpu_share` to 0 on every later call, taking
/// the GPU-only branch and skipping this blend from then on.
fn blend_gpu_frac(
    frac: f64,
    gpu_share: u32,
    cpu_share: u32,
    gpu_elapsed: std::time::Duration,
    cpu_elapsed: std::time::Duration,
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

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    fn tiny_scene() -> SceneState {
        SceneState {
            width: 8,
            height: 8,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
        }
    }

    /// Never set -- a stand-in for callers with no cancellation to exercise, mirroring
    /// `render_core::trace_samples_with_gpu`'s own `never_cancel`.
    fn never_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn calibrate_declines_when_the_gpu_declines() {
        let scene = tiny_scene();
        let gpu = GpuBackend::disabled();
        let mut out = vec![Vec3::ZERO; 64];
        assert_eq!(
            calibrate(&gpu, &scene, 0, 8, 1, &mut out, &never_cancel()),
            CalibrationOutcome::GpuDeclined
        );
        // Nothing written -- decline never touches `out`, so a caller bailing out on
        // GpuDeclined never has to clean up a partial calibration buffer.
        assert!(out.iter().all(|v| *v == Vec3::ZERO));
    }

    /// Too few samples to even spend the 3-sample probe -- must resolve to the same
    /// fallback-to-single-engine outcome a genuine GPU decline does, not panic.
    #[test]
    fn calibrate_declines_when_there_are_too_few_samples_to_probe_with() {
        let scene = tiny_scene();
        let gpu = GpuBackend::disabled();
        let mut out = vec![Vec3::ZERO; 64];
        assert_eq!(
            calibrate(&gpu, &scene, 0, 2, 1, &mut out, &never_cancel()),
            CalibrationOutcome::GpuDeclined
        );
    }

    /// A `cancel` already set before `calibrate` dispatches a probe sample must be
    /// honored immediately -- `consumed: 0`, `out` untouched. The one calibration
    /// cancellation checkpoint testable without real GPU hardware: with
    /// `GpuBackend::disabled`, every dispatch declines immediately regardless of
    /// `cancel`, so the between-probes checkpoints need a real adapter (covered by this
    /// module's `#[ignore]`d hardware measurements instead).
    #[test]
    fn calibrate_honors_a_cancel_already_set_before_the_first_probe_sample() {
        let scene = tiny_scene();
        let gpu = GpuBackend::disabled();
        let mut out = vec![Vec3::ZERO; 64];
        let cancel = AtomicBool::new(true);
        assert_eq!(
            calibrate(&gpu, &scene, 0, 8, 1, &mut out, &cancel),
            CalibrationOutcome::Cancelled { consumed: 0 }
        );
        assert!(out.iter().all(|v| *v == Vec3::ZERO));
    }

    #[test]
    fn hybrid_trace_falls_back_to_cpu_only_when_gpu_frac_is_none() {
        let scene = tiny_scene();
        let gpu = GpuBackend::disabled();
        let mut gpu_frac = None;
        let hybrid = hybrid_trace(&gpu, &scene, 0, 4, 1, &mut gpu_frac, &never_cancel())
            .expect("gpu_share == 0 here, so this can never observe a cancellation");
        let cpu_only = render_core_trace_samples(&scene, 0, 4, 1);
        assert_eq!(hybrid, cpu_only);
        // A disabled backend never reports throughput to blend.
        assert_eq!(gpu_frac, None);
    }

    #[test]
    fn hybrid_trace_sums_to_the_same_result_as_tracing_the_whole_range_on_cpu() {
        // With the GPU disabled, gpu_share is always 0, so this exercises the CPU-only
        // path -- a stand-in for proving the split doesn't change sums, since a real
        // split needs --features gpu and real hardware.
        let scene = tiny_scene();
        let gpu = GpuBackend::disabled();
        let mut gpu_frac = Some(0.8);
        let hybrid = hybrid_trace(&gpu, &scene, 10, 6, 2, &mut gpu_frac, &never_cancel())
            .expect("gpu_share == 0 here, so this can never observe a cancellation");
        let direct = render_core_trace_samples(&scene, 10, 6, 2);
        assert_eq!(hybrid, direct);
    }

    fn render_core_trace_samples(
        scene: &SceneState,
        first: u32,
        samples: u32,
        threads: usize,
    ) -> Vec<Vec3> {
        super::super::trace_samples(scene, first, samples, threads)
    }

    /// A local stand-in for `stream_emit::sizing::next_batch_size` (not reachable from
    /// here, `pub(super)` to `stream_emit` only), same formula, so this module's
    /// throughput measurement drives GPU-only and hybrid through the identical loop
    /// `run_tracer` itself uses.
    fn adaptive_next_batch_size(prev: u32, elapsed: std::time::Duration) -> u32 {
        const TARGET: std::time::Duration = std::time::Duration::from_millis(100);
        const MAX_SUBBATCH: u32 = 2048;
        if elapsed.is_zero() {
            return prev.saturating_mul(4).clamp(1, MAX_SUBBATCH);
        }
        let ratio = TARGET.as_secs_f64() / elapsed.as_secs_f64();
        let scaled = f64::from(prev) * ratio;
        let max_growth = f64::from(prev.saturating_mul(4).clamp(1, MAX_SUBBATCH));
        scaled.clamp(1.0, max_growth).round() as u32
    }

    /// Runs `trace` over `total` samples in adaptively-sized sub-batches -- the same
    /// control loop `run_tracer` uses -- and returns the wall time. Shared by the
    /// GPU-only and CPU-only legs of the measurement below so all three configurations
    /// are timed under an identical batching regime.
    fn time_adaptive_loop(total: u32, mut trace: impl FnMut(u32, u32)) -> std::time::Duration {
        let overall = Instant::now();
        let mut produced = 0u32;
        let mut batch_size = 1u32;
        while produced < total {
            let this_batch = batch_size.min(total - produced);
            let start = Instant::now();
            trace(produced, this_batch);
            let elapsed = start.elapsed();
            produced += this_batch;
            batch_size = adaptive_next_batch_size(batch_size, elapsed);
        }
        overall.elapsed()
    }

    /// Manual measurement, not a correctness check: reports GPU-only vs. hybrid
    /// throughput for a realistic scene on whatever real adapter this machine has,
    /// running both through the same adaptive sub-batch loop `run_tracer` uses rather
    /// than one giant dispatch -- a single huge `hybrid_trace` call would lock in
    /// whatever the first, dispatch-overhead-dominated calibration measured for the
    /// entire batch, while real usage gets many small batches re-measuring and
    /// blending their split, correcting toward the GPU's true bulk throughput within
    /// the first few batches. Requires `--features gpu` and real hardware, so
    /// `#[ignore]`d.
    ///
    /// One untimed 1-spp dispatch, purely to pay the GPU's one-time warm-up cost before
    /// a measurement starts -- pulled out to keep the caller under clippy's line-count
    /// limit.
    fn warm_up_gpu(gpu: &GpuBackend, scene: &SceneState) {
        let mut warm = vec![Vec3::ZERO; scene.width as usize * scene.height as usize];
        let _ = gpu.try_accumulate(
            &GpuSceneRef {
                camera: &Camera::new(
                    scene.yaw,
                    scene.pitch,
                    scene.distance,
                    super::VIEWER_FOV_DEG,
                ),
                width: scene.width,
                height: scene.height,
                planes: &scene.planes,
                facet_finishes: &[],
                material: &scene.material,
                max_bounces: scene.max_bounces,
                environment: scene
                    .lighting_preset
                    .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
                    .with_backdrop(scene.backdrop),
            },
            0,
            1,
            &mut warm,
        );
    }

    #[test]
    #[ignore = "manual measurement: prints GPU-only vs. hybrid throughput on this machine's real adapter"]
    fn measure_hybrid_speedup_against_gpu_only() {
        let gpu = GpuBackend::acquire();
        assert!(
            gpu.adapter_label().is_some(),
            "this measurement requires a real adapter -- run probe_gpu_adapter first"
        );

        let scene = SceneState {
            width: 800,
            height: 600,
            max_bounces: 6,
            ..tiny_scene()
        };
        let total_samples: u32 = 2000;
        let threads = 0;

        // Warm up the GPU (adapter/pipeline compile) before either measurement, so
        // neither one unfairly eats that one-time cost.
        warm_up_gpu(&gpu, &scene);

        // GPU-only baseline, adaptively sub-batched exactly like `run_tracer` without
        // hybrid: repeated `trace_samples_with_gpu` calls, batch size chasing
        // `TARGET_SUBBATCH` via the crate's own `next_batch_size`.
        let gpu_only_elapsed = time_adaptive_loop(total_samples, |first, n| {
            let _ = super::super::trace_samples_with_gpu(&gpu, &scene, first, n, threads);
        });

        // Hybrid, same adaptive sub-batching, but calibrating once up front and then
        // splitting/blending every subsequent sub-batch -- exactly what `run_tracer`
        // itself does once a job clears `HYBRID_MIN_SPP`.
        let hybrid_start = Instant::now();
        {
            let mut calib_buf = vec![Vec3::ZERO; scene.width as usize * scene.height as usize];
            let (frac, consumed) = match calibrate(
                &gpu,
                &scene,
                0,
                total_samples,
                threads,
                &mut calib_buf,
                &never_cancel(),
            ) {
                CalibrationOutcome::Split(frac, consumed) => (frac, consumed),
                other => panic!("gpu available, expected a Split outcome, got {other:?}"),
            };
            let mut gpu_frac = Some(frac);
            let mut produced = consumed;
            let mut batch_size = 1u32;
            while produced < total_samples {
                let this_batch = batch_size.min(total_samples - produced);
                let start = Instant::now();
                let _ = hybrid_trace(
                    &gpu,
                    &scene,
                    produced,
                    this_batch,
                    threads,
                    &mut gpu_frac,
                    &never_cancel(),
                );
                let elapsed = start.elapsed();
                produced += this_batch;
                batch_size = adaptive_next_batch_size(batch_size, elapsed);
            }
            eprintln!("final gpu_frac={gpu_frac:?}");
        }
        let hybrid_elapsed = hybrid_start.elapsed();

        // CPU-only reference, same adaptive loop, no GPU involved -- explains why
        // hybrid does or doesn't help: total wall time per hybrid batch is
        // max(gpu_time, cpu_time), not a weighted average.
        let cpu_only_samples: u32 = 200; // smaller: CPU-only is far slower per sample here.
        let cpu_only_elapsed = time_adaptive_loop(cpu_only_samples, |first, n| {
            let _ = super::super::trace_samples(&scene, first, n, threads);
        });
        let cpu_only_rate = f64::from(cpu_only_samples) / cpu_only_elapsed.as_secs_f64();

        let gpu_only_rate = f64::from(total_samples) / gpu_only_elapsed.as_secs_f64();
        let hybrid_rate = f64::from(total_samples) / hybrid_elapsed.as_secs_f64();
        eprintln!(
            "GPU-only: {total_samples} samples in {gpu_only_elapsed:?} ({gpu_only_rate:.1} samples/s)"
        );
        eprintln!(
            "Hybrid:   {total_samples} samples in {hybrid_elapsed:?} ({hybrid_rate:.1} samples/s)"
        );
        eprintln!(
            "CPU-only: {cpu_only_samples} samples in {cpu_only_elapsed:?} ({cpu_only_rate:.1} samples/s)"
        );
        eprintln!(
            "Speedup (hybrid vs GPU-only): {:.2}x",
            hybrid_rate / gpu_only_rate
        );
        eprintln!(
            "GPU is {:.1}x faster than CPU per-sample on this scene",
            gpu_only_rate / cpu_only_rate
        );
    }

    /// The reservation must never starve the CPU side entirely -- `--threads 1`, or a
    /// single-core machine, still has to trace its share with one thread rather than
    /// zero.
    #[test]
    fn cpu_threads_beside_gpu_never_returns_zero() {
        assert_eq!(cpu_threads_beside_gpu(1), 1, "one core still traces");
        assert_eq!(cpu_threads_beside_gpu(2), 1, "two cores: one reserved");
        assert!(
            cpu_threads_beside_gpu(0) >= 1,
            "0 means `all available`, which must still resolve to at least one thread"
        );
    }

    /// On anything with cores to spare, exactly one is held back -- not a fraction, not
    /// half: only the single submit-and-poll thread needs protecting.
    #[test]
    fn cpu_threads_beside_gpu_reserves_exactly_one_core() {
        for n in 3..=16usize {
            assert_eq!(
                cpu_threads_beside_gpu(n),
                n - 1,
                "with {n} cores the tracer should get {} of them",
                n - 1
            );
        }
    }
}
