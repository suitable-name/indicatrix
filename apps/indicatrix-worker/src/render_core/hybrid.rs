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

use super::{resolve_facet_finishes, scene_uses_gpu, trace_into};
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
    time::Instant,
};

mod split;
// The steady-state sub-batch lives in `split.rs`; re-exported so every path under
// `render_core::hybrid` stays as it was.
#[cfg(test)]
pub use split::hybrid_trace;
pub use split::{HybridBuffers, hybrid_trace_into};

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
    // A concave scene is CPU-only: there is nothing to calibrate a split against.
    if samples < 3 || !scene_uses_gpu(scene) {
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
