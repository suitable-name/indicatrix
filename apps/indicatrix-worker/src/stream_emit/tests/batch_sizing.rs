//! Tests for [`next_batch_size`]'s adaptive sub-batch sizing, plus manual (`#[ignore]`d)
//! wall-clock repros against real tracers.

use super::fixtures::tiny_scene;
use crate::{
    render_core,
    stream_emit::{sizing, sizing::next_batch_size},
};
use indicatrix_net::SceneState;
use std::time::Duration;

#[test]
fn next_batch_size_grows_when_well_under_budget() {
    let next = next_batch_size(4, Duration::from_millis(10));
    assert!(next > 4, "expected growth from 4, got {next}");
}

#[test]
fn next_batch_size_shrinks_when_over_budget() {
    let next = next_batch_size(100, Duration::from_millis(400));
    assert!(next < 100, "expected shrink from 100, got {next}");
    assert!(next >= 1);
}

#[test]
fn next_batch_size_never_returns_zero() {
    assert!(next_batch_size(0, Duration::from_millis(1)) >= 1);
    assert!(next_batch_size(1, Duration::from_secs(10)) >= 1);
}

#[test]
fn next_batch_size_never_exceeds_the_absolute_cap_even_under_adversarial_timing() {
    // Every call looks maximally favorable (near-zero elapsed) -- exactly what would
    // make the relative 4x-per-step clamp alone compound unboundedly across calls.
    let mut batch = 1;
    for _ in 0..40 {
        batch = next_batch_size(batch, Duration::from_nanos(1));
        assert!(
            batch <= sizing::MAX_SUBBATCH,
            "batch size {batch} exceeded the absolute cap of {}",
            sizing::MAX_SUBBATCH
        );
    }
    // Also reaches the cap, so this isn't passing vacuously.
    assert_eq!(batch, sizing::MAX_SUBBATCH);
}

#[test]
#[ignore = "manual repro: prints wall-clock batch growth against the real GPU tracer"]
fn repro_batch_growth_runaway_against_real_gpu_tracer() {
    use indicatrix::renderer::gpu_backend::GpuBackend;
    let gpu = GpuBackend::acquire();
    assert!(
        gpu.adapter_label().is_some(),
        "this probe requires a real adapter -- run probe_gpu_adapter first"
    );

    let scene = SceneState {
        width: 800,
        height: 600,
        max_bounces: 6,
        ..tiny_scene()
    };
    let total_samples: u32 = 65_536; // MAX_SAMPLES_PER_REQUEST
    // Skip one untimed warm-up dispatch: the first GPU call pays adapter/pipeline
    // warm-up not representative of steady-state throughput.
    let _ = render_core::trace_samples_with_gpu(&gpu, &scene, 0, 1, 0);
    let mut produced: u32 = 0;
    let mut batch_size: u32 = 1;
    let mut step = 0;
    let wall_start = std::time::Instant::now();
    while produced < total_samples {
        let this_batch = batch_size.min(total_samples - produced);
        let start = std::time::Instant::now();
        let _ = render_core::trace_samples_with_gpu(&gpu, &scene, produced, this_batch, 0);
        let elapsed = start.elapsed();
        eprintln!(
            "step {step}: batch={this_batch} elapsed={elapsed:?} produced_after={} wall_total={:?}",
            produced + this_batch,
            wall_start.elapsed()
        );
        produced += this_batch;
        batch_size = next_batch_size(batch_size, elapsed);
        step += 1;
        if step > 30 {
            break;
        }
    }
}

#[test]
#[ignore = "manual repro: prints wall-clock batch growth against the real GPU tracer, cheap scene"]
fn repro_batch_growth_runaway_against_real_gpu_tracer_cheap_scene() {
    use indicatrix::renderer::gpu_backend::GpuBackend;
    let gpu = GpuBackend::acquire();
    assert!(
        gpu.adapter_label().is_some(),
        "this probe requires a real adapter -- run probe_gpu_adapter first"
    );

    // Small resolution, low bounce count: makes per-sample GPU cost tiny relative to
    // fixed per-dispatch overhead, to see whether that dominance makes batches overshoot.
    let scene = SceneState {
        width: 64,
        height: 48,
        max_bounces: 2,
        ..tiny_scene()
    };
    let total_samples: u32 = 65_536;
    let _ = render_core::trace_samples_with_gpu(&gpu, &scene, 0, 1, 0);
    let mut produced: u32 = 0;
    let mut batch_size: u32 = 1;
    let mut step = 0;
    let wall_start = std::time::Instant::now();
    while produced < total_samples {
        let this_batch = batch_size.min(total_samples - produced);
        let start = std::time::Instant::now();
        let _ = render_core::trace_samples_with_gpu(&gpu, &scene, produced, this_batch, 0);
        let elapsed = start.elapsed();
        eprintln!(
            "step {step}: batch={this_batch} elapsed={elapsed:?} produced_after={} wall_total={:?}",
            produced + this_batch,
            wall_start.elapsed()
        );
        produced += this_batch;
        batch_size = next_batch_size(batch_size, elapsed);
        step += 1;
        if step > 30 {
            break;
        }
    }
}

#[test]
#[ignore = "manual repro: prints wall-clock batch growth against the real CPU tracer"]
fn repro_batch_growth_runaway_against_real_tracer() {
    // Mirrors `run_tracer`'s loop against a scene closer to real usage than this file's
    // other tiny fixtures, to see whether thread-spawn/dispatch overhead alone (no GPU)
    // makes early batches look "free" and compounds into one giant batch.
    let scene = SceneState {
        width: 64,
        height: 48,
        max_bounces: 2,
        ..tiny_scene()
    };
    let total_samples: u32 = 65_536; // MAX_SAMPLES_PER_REQUEST
    let mut produced: u32 = 0;
    let mut batch_size: u32 = 1;
    let mut step = 0;
    while produced < total_samples {
        let this_batch = batch_size.min(total_samples - produced);
        let start = std::time::Instant::now();
        let _ = render_core::trace_samples(&scene, produced, this_batch, 0);
        let elapsed = start.elapsed();
        eprintln!(
            "step {step}: batch={this_batch} elapsed={elapsed:?} produced_after={}",
            produced + this_batch
        );
        produced += this_batch;
        batch_size = next_batch_size(batch_size, elapsed);
        step += 1;
        if step > 30 {
            break;
        }
    }
}
