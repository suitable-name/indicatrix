//! A declined deterministic range must fail instead of completing a partial picture.
use super::fixtures::tiny_scene;
use crate::{
    cli::ComputeMode,
    stream_emit::{
        trace_range,
        tracer::{SharedState, TracerJob, run_tracer},
    },
};
use indicatrix::renderer::{engine::RenderEngine, gpu_backend::GpuBackend};
use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};

#[test]
fn a_declined_ray_range_is_an_explicit_stream_and_own_lane_failure() {
    let mut scene = tiny_scene();
    scene.engine.engine = RenderEngine::RayTraced;
    scene.engine.ray.aa_taps = 1;
    // Bypass ingress validation to exercise the renderer's defensive refusal.
    let job = TracerJob {
        scene: scene.clone(),
        first_sample: 1,
        samples: 1,
        threads: 1,
        compute_mode: ComputeMode::OnlyCpu,
    };
    let state = Arc::new(Mutex::new(SharedState::new(16)));
    let (tx, _rx) = mpsc::channel();
    let cancel = AtomicBool::new(false);
    let gpu = GpuBackend::disabled();
    run_tracer(&job, &gpu, &state, &cancel, &tx);
    let guard = state.lock().unwrap();
    assert!(guard.finished);
    assert!(!guard.panicked);
    assert_eq!(guard.samples_done, 0);
    assert!(guard.failed.is_some());
    drop(guard);
    let range = trace_range(&scene, (1, 1), 1, ComputeMode::OnlyCpu, &gpu, &cancel);
    assert_eq!(range.done, 0);
    assert!(range.failed.is_some());
    assert!(!range.panicked);
}

#[test]
fn cancelling_a_ray_range_does_not_report_a_renderer_failure() {
    let mut scene = tiny_scene();
    scene.engine.engine = RenderEngine::RayTraced;
    let range = trace_range(
        &scene,
        (0, 1),
        1,
        ComputeMode::OnlyCpu,
        &GpuBackend::disabled(),
        &AtomicBool::new(true),
    );
    assert_eq!(range.done, 0);
    assert!(range.failed.is_none());
    assert!(!range.panicked);
}
