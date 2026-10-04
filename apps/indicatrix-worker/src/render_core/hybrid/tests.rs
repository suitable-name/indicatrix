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
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: Default::default(),
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
/// `cancel`, so the between-probes checkpoints need a real adapter; no automated test
/// covers them, and the `#[ignore]`d `measure_hybrid_speedup_against_gpu_only` only
/// exercises them incidentally on a machine with one.
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

/// CPU against CPU, not a GPU comparison: with the GPU disabled the GPU share is always
/// 0, so `hybrid_trace` degenerates to the CPU tracer over the whole range and this only
/// pins that degenerate path (and that a stale `gpu_frac` is ignored). It says nothing
/// about a real CPU/GPU split; the only test that drives a real adapter through the
/// hybrid path is the `#[ignore]`d `measure_hybrid_speedup_against_gpu_only`, which
/// reports throughput and does not compare sums.
#[test]
fn hybrid_trace_equals_the_whole_range_on_cpu_with_the_gpu_share_disabled() {
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
/// `#[ignore]`d; it is the only place this module measures a real GPU (and a real
/// GPU/CPU split) against CPU tracing, and it compares throughput, not sums.
///
/// One untimed 1-spp dispatch, purely to pay the GPU's one-time warm-up cost before
/// a measurement starts -- pulled out to keep the caller under clippy's line-count
/// limit.
fn warm_up_gpu(gpu: &GpuBackend, scene: &SceneState) {
    let mut warm = vec![Vec3::ZERO; scene.width as usize * scene.height as usize];
    let _ = gpu.try_accumulate(
        &GpuSceneRef {
            camera: &Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG),
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

// -- calibrate_cached / JobKey / finalize_split_cache -----------------------------
//
// A disabled `GpuBackend` always declines, so none of these can exercise a REAL
// `Split`/`GpuDominates` calibration (that needs `--features gpu` and real hardware;
// only the `#[ignore]`d `measure_hybrid_speedup_against_gpu_only` above reaches one,
// and it checks only that calibration yields a `Split`). Every test below either
// confirms a decline is never cached, or drives the cache type directly -- a
// synthetic `JobKey` with a sentinel `gpu_id` no real pointer will ever collide with,
// pre-seeded by hand -- which needs no GPU at all.

/// A decline (no adapter here) must never be cached: it can depend on the request's
/// own material, not just the resource profile a [`JobKey`] captures -- see
/// [`CachedDecision`]'s own doc comment. Called twice to confirm a decline never
/// somehow answers from a phantom hit either.
#[test]
fn calibrate_cached_never_caches_a_gpu_decline() {
    let gpu = GpuBackend::disabled();
    let scene = tiny_scene();
    let key = JobKey {
        compute_mode: compute_mode_tag(ComputeMode::Hybrid),
        gpu_id: 0xDEAD_0001,
        threads: 1,
        resolution_bucket: 6,
    };
    let mut out = vec![Vec3::ZERO; 64];
    for _ in 0..2 {
        let outcome = calibrate_cached(
            key,
            &gpu,
            &scene,
            SampleRange::new(0, 8),
            1,
            &mut out,
            &never_cancel(),
        );
        assert_eq!(outcome, CachedCalibration::GpuDeclined);
    }
    assert!(
        !CALIBRATION_CACHE.lock().unwrap().contains_key(&key),
        "a decline must never be cached"
    );
}

/// A pre-seeded `Split` entry must answer a matching key WITHOUT reaching
/// `calibrate` at all: `gpu` is disabled here, so if this call fell through to a real
/// probe it would decline, never `Split`. Getting `Split` back, with `out` untouched,
/// proves the cache hit short-circuited the probe entirely.
#[test]
fn calibrate_cached_hits_without_reprobing() {
    let gpu = GpuBackend::disabled();
    let scene = tiny_scene();
    let key = JobKey {
        compute_mode: compute_mode_tag(ComputeMode::Hybrid),
        gpu_id: 0xDEAD_0002,
        threads: 2,
        resolution_bucket: 8,
    };
    CALIBRATION_CACHE
        .lock()
        .unwrap()
        .insert(key, CachedDecision::Split { gpu_frac: 0.42 });

    let mut out = vec![Vec3::ZERO; 64];
    let outcome = calibrate_cached(
        key,
        &gpu,
        &scene,
        SampleRange::new(0, 8),
        2,
        &mut out,
        &never_cancel(),
    );
    assert_eq!(
        outcome,
        CachedCalibration::Split {
            gpu_frac: 0.42,
            consumed: 0
        }
    );
    assert!(
        out.iter().all(|v| *v == Vec3::ZERO),
        "a cache hit dispatches nothing, so `out` must stay untouched"
    );

    CALIBRATION_CACHE.lock().unwrap().remove(&key);
}

/// Same short-circuit, for a pre-seeded `GpuOnly` entry.
#[test]
fn calibrate_cached_hits_gpu_only_the_same_way() {
    let gpu = GpuBackend::disabled();
    let scene = tiny_scene();
    let key = JobKey {
        compute_mode: compute_mode_tag(ComputeMode::Hybrid),
        gpu_id: 0xDEAD_0003,
        threads: 4,
        resolution_bucket: 10,
    };
    CALIBRATION_CACHE
        .lock()
        .unwrap()
        .insert(key, CachedDecision::GpuOnly);

    let mut out = vec![Vec3::ZERO; 64];
    let outcome = calibrate_cached(
        key,
        &gpu,
        &scene,
        SampleRange::new(0, 8),
        4,
        &mut out,
        &never_cancel(),
    );
    assert_eq!(outcome, CachedCalibration::GpuOnly { consumed: 0 });

    CALIBRATION_CACHE.lock().unwrap().remove(&key);
}

/// [`finalize_split_cache`]'s two write-back paths: `Some` overwrites with the job's
/// own final blend, `None` evicts rather than leaving a now-stale split behind.
#[test]
fn finalize_split_cache_overwrites_on_some_and_evicts_on_none() {
    let key = JobKey {
        compute_mode: compute_mode_tag(ComputeMode::Hybrid),
        gpu_id: 0xDEAD_0004,
        threads: 1,
        resolution_bucket: 12,
    };
    CALIBRATION_CACHE
        .lock()
        .unwrap()
        .insert(key, CachedDecision::Split { gpu_frac: 0.1 });

    finalize_split_cache(key, Some(0.77));
    assert_eq!(
        CALIBRATION_CACHE.lock().unwrap().get(&key).copied(),
        Some(CachedDecision::Split { gpu_frac: 0.77 }),
        "a live split at job end must overwrite the cache with the job's final blend"
    );

    finalize_split_cache(key, None);
    assert_eq!(
        CALIBRATION_CACHE.lock().unwrap().get(&key).copied(),
        None,
        "a mid-job GPU decline must evict the cached split, not leave a stale one"
    );
}

/// `--threads 0` ("let the OS decide") and a request that spells out this machine's
/// own realized count must land in the same cache entry.
#[test]
fn job_key_collapses_a_zero_threads_request_onto_its_realized_count() {
    let gpu = GpuBackend::disabled();
    let scene = tiny_scene();
    let realized = super::super::effective_thread_count(0);
    assert_eq!(
        job_key(&gpu, &scene, 0, ComputeMode::Hybrid),
        job_key(&gpu, &scene, realized, ComputeMode::Hybrid),
        "`--threads 0` and its own realized count must share one cache entry"
    );
}

/// Different compute modes, and a far larger resolution, must never collapse onto
/// the same [`JobKey`] -- each has its own achievable throughput.
#[test]
fn job_key_distinguishes_compute_modes_and_resolutions() {
    let gpu = GpuBackend::disabled();
    let scene = tiny_scene();
    let hybrid_key = job_key(&gpu, &scene, 4, ComputeMode::Hybrid);
    let only_gpu_key = job_key(&gpu, &scene, 4, ComputeMode::OnlyGpu);
    assert_ne!(hybrid_key, only_gpu_key);

    let bigger_scene = SceneState {
        width: 4000,
        height: 3000,
        ..tiny_scene()
    };
    let bigger_key = job_key(&gpu, &bigger_scene, 4, ComputeMode::Hybrid);
    assert_ne!(
        hybrid_key, bigger_key,
        "a far larger resolution must land in a different bucket"
    );
}

/// [`calibrate_now`] is also called from `serve`'s and `join`'s start-up sequencing
/// (see its own doc comment), but a disabled `GpuBackend` there never reaches this
/// path since neither calls it without a live adapter -- confirms it runs without
/// panicking and, since a disabled backend always declines, that it leaves no cache
/// entry behind either (the same never-cache-a-decline invariant [`calibrate_cached`]
/// itself upholds).
#[test]
fn calibrate_now_is_callable_and_never_caches_a_decline() {
    let gpu = GpuBackend::disabled();
    let scene = tiny_scene();
    calibrate_now(&gpu, &scene, 1, ComputeMode::Hybrid);
    let key = job_key(&gpu, &scene, 1, ComputeMode::Hybrid);
    assert!(!CALIBRATION_CACHE.lock().unwrap().contains_key(&key));
}
