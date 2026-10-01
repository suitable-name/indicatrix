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
//!
//! # Caching the calibration decision (not the split itself)
//!
//! [`calibrate`] is a real measurement: two extra GPU dispatches plus a full-frame CPU
//! trace, thrown away once the decision is made. Nothing above stops a caller running it
//! once per job -- and `stream_emit::tracer::run_tracer` does exactly that, once per
//! `RenderRequest`, per coordinator lane chunk, and per live Direct request. On a
//! long-running stream that repeats every ~1.5s per chunk, re-measuring an answer that
//! only depends on this process's hardware (not the request) every time.
//!
//! [`calibrate_cached`] fixes this: a process-wide cache, keyed by [`JobKey`] (everything
//! the measured ratio actually depends on -- see that type's own doc comment), of the
//! DECISION [`calibrate`] reached, not the raw measurement. A cache hit skips the probe
//! entirely. [`finalize_split_cache`] writes the job's own final blended `gpu_frac` back
//! into the cache when it ends, so the seed keeps improving across jobs exactly as
//! [`hybrid_trace`]'s EMA improves it within one job.

use super::{resolve_facet_finishes, trace_into};
use crate::cli::ComputeMode;
use glam::Vec3;
use indicatrix::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
    renderer::gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
};
use indicatrix_dispatch::SampleRange;
use indicatrix_net::SceneState;
use std::{
    collections::BTreeMap,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG);
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
    // The studio rig, or the HDR map the request path resolved -- see
    // `crate::assets::resolved_hdr_map`.
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_source(scene, hdr_map.as_deref());
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
    // A `false` return (cancelled while queued for CPU permits) is not separately
    // checked here: the next iteration of the caller's own loop re-checks `cancel`
    // before dispatching another sub-batch, same as any other probe timing artifact.
    let _ = trace_into(
        scene,
        SampleRange::new(first_sample + 2, 1),
        threads,
        &camera,
        environment,
        out,
        cancel,
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
    // The studio rig, or the HDR map the request path resolved -- see
    // `crate::assets::resolved_hdr_map`.
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_source(scene, hdr_map.as_deref());

    if gpu_share == 0 {
        if !trace_into(
            scene,
            SampleRange::new(first_sample, samples),
            threads,
            &camera,
            environment,
            &mut buffer,
            cancel,
        ) {
            return None;
        }
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
            // stop offering it work. A `false` return means `cancel` fired while queued
            // for CPU permits: discard the sub-batch exactly as a mid-dispatch GPU
            // cancellation does above, rather than reporting a buffer missing the
            // retraced share.
            if !trace_into(
                scene,
                SampleRange::new(first_sample, gpu_share),
                threads,
                &camera,
                environment,
                &mut buffer,
                cancel,
            ) {
                return None;
            }
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
        // `cancel` plays no finer-grained role here than the between-sub-batches check
        // `run_tracer`'s loop already performs (see this function's doc comment above);
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

/// A resource profile: everything a hybrid calibration decision ([`calibrate_cached`]) or
/// a converged sub-batch size (`stream_emit::sizing`) depends on, and nothing either one
/// ignores. See [`job_key`]'s own doc comment for what each field means and why the
/// omitted axes (scene material/geometry/lighting, which samples are being traced) don't
/// need one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct JobKey {
    /// [`ComputeMode`]'s discriminant, reduced to a small ordered tag (that type derives
    /// neither `Ord` nor `Hash`) -- see [`compute_mode_tag`].
    compute_mode: u8,
    /// `gpu`'s own address: stable for this process's lifetime once acquired (see
    /// `indicatrix::renderer::gpu_backend`'s module doc comment on sharing one
    /// `Arc<GpuBackend>` across every connection), so this distinguishes "no adapter"
    /// from a real one, and one real adapter from another if a process ever held more
    /// than one.
    gpu_id: usize,
    /// The REALIZED thread count (`0`, "let the OS decide", resolved through
    /// [`super::effective_thread_count`]) -- so a request that spelled `0` and one that
    /// spelled out this machine's actual core count share the same entry.
    threads: usize,
    /// `scene.width * scene.height`, bucketed to its enclosing power of two. See
    /// [`job_key`]'s doc comment for why resolution shifts the GPU/CPU throughput ratio.
    resolution_bucket: u32,
}

/// Reduces `mode` to a small, totally-ordered tag for [`JobKey`] -- [`ComputeMode`]
/// itself derives neither `Ord` nor `Hash`, so this is the one place that mapping lives.
const fn compute_mode_tag(mode: ComputeMode) -> u8 {
    match mode {
        ComputeMode::Hybrid => 0,
        ComputeMode::OnlyGpu => 1,
        ComputeMode::OnlyCpu => 2,
    }
}

/// Builds the [`JobKey`] for `gpu`/`scene`/`threads`/`compute_mode`'s resource profile.
///
/// Deliberately keyed on only four things:
/// - `compute_mode`: `OnlyGpu`/`OnlyCpu`/`Hybrid` reach entirely different throughput (and
///   only `Hybrid` ever calibrates a split at all), so they must never share an entry.
/// - `gpu`'s identity: a different physical adapter (or none) has different throughput.
/// - the realized `threads` count.
/// - `scene`'s resolution, bucketed to the enclosing power of two: [`calibrate`]'s
///   measurement folds in fixed per-dispatch/per-call overhead (GPU turnstile admission;
///   the CPU tracer's per-call `build_plane_soa`) that a small preview amortizes far worse
///   than a full-size export, so the measured GPU/CPU ratio genuinely shifts with
///   resolution. Bucketed, not exact, so requests a few pixels apart still share a
///   decision.
///
/// Deliberately NOT keyed on `scene`'s material, geometry, lighting, environment, or which
/// samples are being traced: those shift both engines' per-sample cost together, not
/// their relative split, and both [`calibrate`] and `stream_emit`'s adaptive ramp already
/// re-measure per JOB regardless -- a wrong guess there self-corrects within one job
/// rather than needing a cache dimension of its own.
#[must_use]
pub fn job_key(
    gpu: &GpuBackend,
    scene: &SceneState,
    threads: usize,
    compute_mode: ComputeMode,
) -> JobKey {
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    JobKey {
        compute_mode: compute_mode_tag(compute_mode),
        gpu_id: std::ptr::from_ref(gpu) as usize,
        threads: super::effective_thread_count(threads),
        resolution_bucket: pixels.max(1).ilog2(),
    }
}

/// What [`calibrate_cached`]'s process-wide cache stores per [`JobKey`] -- the DECISION a
/// job should start from, not the raw measurement. Never holds
/// [`CalibrationOutcome::GpuDeclined`] (see that variant's own doc comment: a decline can
/// be per-request-material, not a property of the resource profile a [`JobKey`]
/// captures) or `Cancelled` (not a steady-state decision at all).
#[derive(Debug, Clone, Copy, PartialEq)]
enum CachedDecision {
    /// [`CalibrationOutcome::GpuDominates`] fired: this resource profile's GPU share
    /// meets or exceeds [`HYBRID_MAX_GPU_SHARE`], so later jobs should skip straight to
    /// the single-engine path without probing again.
    GpuOnly,
    /// [`CalibrationOutcome::Split`] fired: later jobs should seed `gpu_frac` at this
    /// value rather than probing from scratch. Kept fresh by [`finalize_split_cache`] as
    /// each job's own EMA blend improves on it.
    Split {
        /// The seed a new job's `gpu_frac` should start from.
        gpu_frac: f64,
    },
}

/// The process-wide cache [`calibrate_cached`] reads and writes -- a `BTreeMap` (not a
/// hash map) purely to keep iteration/debug output deterministic; lookups are by exact
/// key equality either way, not by iteration order.
static CALIBRATION_CACHE: LazyLock<Mutex<BTreeMap<JobKey, CachedDecision>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// What [`calibrate_cached`] decided. Mirrors [`CalibrationOutcome`]'s payloads, but
/// `consumed` is always `0` on a cache hit: a hit skips [`calibrate`]'s 3-sample probe
/// entirely, so there is nothing to fold into `out`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CachedCalibration {
    /// Take the hybrid split path, seeded at `gpu_frac`. `consumed` probe samples (`3` on
    /// a fresh calibration, `0` on a cache hit) are already folded into `out`.
    Split {
        /// The initial `gpu_frac` the caller's sub-batch loop should start from.
        gpu_frac: f64,
        /// Real probe samples already folded into `out` -- fold this into the caller's
        /// own `produced` count exactly as [`CalibrationOutcome::Split`]'s `consumed` is.
        consumed: u32,
    },
    /// Take the single-engine (GPU-only) path for this whole job -- either a fresh
    /// [`CalibrationOutcome::GpuDominates`] or a cached [`CachedDecision::GpuOnly`].
    /// `consumed` is always `0` (see this enum's own doc comment).
    GpuOnly {
        /// Always `0` -- kept for symmetry with the other variants' fold-in contract.
        consumed: u32,
    },
    /// The GPU declined the probe itself, or there weren't enough samples to probe with.
    /// Never cached (see [`CachedDecision`]'s own doc comment), so this only ever comes
    /// from a fresh [`calibrate`] call.
    GpuDeclined,
    /// `cancel` fired mid-probe; `consumed` real samples (`0..=2`) already folded into
    /// `out`. Never produced on a cache hit -- a hit dispatches nothing, so nothing can
    /// cancel mid-probe.
    Cancelled {
        /// Real probe samples already folded into `out` before `cancel` fired.
        consumed: u32,
    },
}

/// Wraps [`calibrate`] with a process-wide, per-[`JobKey`] cache of the calibration
/// DECISION: a cache hit skips the 3-sample probe entirely and returns the cached
/// split/GPU-only choice immediately, so a long-running stream's many jobs (one per
/// `RenderRequest`, per coordinator lane chunk, or per live Direct request -- see this
/// module's own top doc comment) each pay the probe once per resource profile rather than
/// once per job.
///
/// `key` must be exactly the result of calling [`job_key`] with this same
/// `gpu`/`scene`/`threads`/`compute_mode` -- taken as a parameter (not recomputed here)
/// so a caller that also needs it for `stream_emit::sizing`'s own cache, or for
/// [`finalize_split_cache`] at job end, builds it exactly once.
///
/// `range` bundles `first_sample`/`samples` purely to keep this function's argument
/// count under clippy's limit; only `range.samples` (`>= 3`, see [`calibrate`]'s own
/// floor) matters to the probe itself.
///
/// Logs the decision at `info` the first time a [`JobKey`] is calibrated (cache fill),
/// and at `debug` on every later cache hit for the same key.
#[must_use]
pub fn calibrate_cached(
    key: JobKey,
    gpu: &GpuBackend,
    scene: &SceneState,
    range: SampleRange,
    threads: usize,
    out: &mut [Vec3],
    cancel: &AtomicBool,
) -> CachedCalibration {
    let cached = CALIBRATION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied();
    if let Some(decision) = cached {
        tracing::debug!(
            ?key,
            ?decision,
            "hybrid split: cache hit, reusing calibration decision"
        );
        return match decision {
            CachedDecision::GpuOnly => CachedCalibration::GpuOnly { consumed: 0 },
            CachedDecision::Split { gpu_frac } => CachedCalibration::Split {
                gpu_frac,
                consumed: 0,
            },
        };
    }

    let SampleRange {
        first_sample,
        samples,
    } = range;
    match calibrate(gpu, scene, first_sample, samples, threads, out, cancel) {
        CalibrationOutcome::Split(gpu_frac, consumed) => {
            CALIBRATION_CACHE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, CachedDecision::Split { gpu_frac });
            tracing::info!(
                ?key,
                gpu_frac,
                "hybrid split: calibrated and cached a new decision"
            );
            CachedCalibration::Split { gpu_frac, consumed }
        }
        CalibrationOutcome::GpuDominates { gpu_share_frac } => {
            CALIBRATION_CACHE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, CachedDecision::GpuOnly);
            tracing::info!(
                ?key,
                gpu_share_pct = gpu_share_frac * 100.0,
                "hybrid CPU+GPU split declined: measured GPU share exceeds the hybrid \
                 cutoff, so this job runs GPU-only (splitting measured slower on this \
                 hardware) -- cached for this resource profile"
            );
            CachedCalibration::GpuOnly { consumed: 0 }
        }
        CalibrationOutcome::GpuDeclined => CachedCalibration::GpuDeclined,
        CalibrationOutcome::Cancelled { consumed } => CachedCalibration::Cancelled { consumed },
    }
}

/// Writes back what a job that started on the hybrid split path converged to, once the
/// job ends. Callers only invoke this when [`calibrate_cached`] actually returned
/// `Split` for this job (never for `GpuOnly`/`GpuDeclined`/`Cancelled`, and never for
/// `OnlyGpu`/`OnlyCpu` jobs, which never calibrate at all) -- see
/// `stream_emit::tracer::run_tracer`'s own use.
///
/// `Some(frac)`: the job ended (finished or was cancelled) still on a live split --
/// overwrites the cache with this job's own final EMA-blended fraction (see
/// [`hybrid_trace`]'s `blend_gpu_frac`), so the next job with the same [`JobKey`] seeds
/// from an estimate that keeps tracking the machine's real throughput.
///
/// `None`: the GPU declined mid-job (device loss -- see [`hybrid_trace`]'s handling of
/// [`GpuAccumulate::Declined`], which clears its caller's `gpu_frac` for exactly this
/// reason) and the job fell back to CPU-only for the rest of its life. The cached
/// decision is EVICTED rather than overwritten with a now-stale split, so the next job
/// recalibrates fresh -- cheaply rediscovering the same decline, since a lost GPU device
/// never recovers within one process.
pub fn finalize_split_cache(key: JobKey, gpu_frac: Option<f64>) {
    let mut cache = CALIBRATION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match gpu_frac {
        Some(frac) => {
            cache.insert(key, CachedDecision::Split { gpu_frac: frac });
        }
        None => {
            cache.remove(&key);
        }
    }
}

/// Pre-warms [`calibrate_cached`]'s process-wide cache for `gpu`/`scene`/`threads`/
/// `compute_mode` before any real request needs the answer -- e.g. `serve`/`join`'s own
/// start-up, once either has a representative scene (the default preview resolution is a
/// reasonable choice -- see [`job_key`]'s doc comment on why resolution BUCKETS, not exact
/// dimensions, share a decision) to probe with.
///
/// Called from `serve`'s and `join`'s own start-up sequencing once the compute mode is
/// `Hybrid` and a GPU adapter is present. Discards the probe's own contribution (a
/// throwaway buffer, never folded into anything a real caller is accumulating) and never
/// cancels early (`cancel` is fixed `false`), so this is meant for a quiet moment before
/// real traffic, not mid-request.
///
/// A no-op (besides a `debug` log from [`calibrate_cached`]) if this resource profile is
/// already cached; otherwise runs the same real 3-sample probe a first live
/// [`calibrate_cached`] call would.
pub fn calibrate_now(
    gpu: &GpuBackend,
    scene: &SceneState,
    threads: usize,
    compute_mode: ComputeMode,
) {
    let key = job_key(gpu, scene, threads, compute_mode);
    let pixel_count = scene.width as usize * scene.height as usize;
    let mut throwaway = vec![Vec3::ZERO; pixel_count];
    let never_cancel = AtomicBool::new(false);
    let _ = calibrate_cached(
        key,
        gpu,
        scene,
        SampleRange::new(0, HYBRID_MIN_SPP),
        threads,
        &mut throwaway,
        &never_cancel,
    );
}

#[cfg(test)]
mod tests;
