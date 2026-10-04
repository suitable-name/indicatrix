//! End-to-end smoke test of the hybrid CPU+GPU export path, kept separate from
//! [`super::tests`] and moved out of `mod.rs` alongside [`super::worker::run_export`]
//! to keep that file from growing further.

use super::{
    worker::{Accumulation, AccumulationOutcome, render_accumulation, render_local_share},
    *,
};
use crate::bridge::render_thread::RenderContext;
use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, LightingPreset},
    },
    renderer::gpu_backend::GpuBackend,
};
use std::sync::{Mutex, atomic::AtomicU32};

/// End-to-end smoke test of the hybrid export path: a small frame rendered
/// through the real `run_export` (calibration, batching, merge, PNG write).
/// Without a GPU adapter or the `gpu` feature this exercises the CPU-only
/// path; with both, it exercises calibration plus concurrent hybrid
/// batches. Asserts completion and a non-empty file, not pixel values (the
/// hybrid sample-to-engine assignment is calibration-dependent by design).
#[test]
fn export_completes_via_hybrid_or_cpu_path() {
    let scene = SceneSnapshot {
        yaw: 0.6,
        pitch: -0.4,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::by_name("Zircon").expect("built-in material"),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 12,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        tools: Vec::new(),
        fluorescence: Default::default(),
        facet_finishes: Vec::new(),
        env_map: None,
        surface_glare: 1.0,
    };
    let params = ExportParams {
        width: 48,
        height: 48,
        samples_per_pixel: 12,
        max_bounces: 12,
    };
    let out = std::env::temp_dir().join("indicatrix_hybrid_export_smoke.png");
    let cancel = AtomicBool::new(false);
    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &out,
        &RemoteSelection::local_only(),
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {},
    );
    match outcome {
        ExportOutcome::Completed(path) => {
            let len = std::fs::metadata(&path).map_or(0, |m| m.len());
            assert!(len > 0, "export wrote an empty file");
            let _ = std::fs::remove_file(path);
        }
        ExportOutcome::Cancelled => panic!("export reported cancelled without a cancel"),
        ExportOutcome::Failed(e) => panic!("export failed: {e}"),
    }
}

// ---- v16: `worker::render_local_share` -- the viewer's own local-share render -------

/// `render_local_share`'s reserved-tail sum, added to a plain `render_batch` trace of
/// the COMPLEMENT range `[0, first)`, must match a single `render_accumulation` call
/// over the WHOLE range `[0, first + viewer)` -- within a RELATIVE tolerance, not bit
/// for bit: `render_accumulation` claims its local chunks from a `SampleCursor` in a
/// different batch/thread order than this test's own head-then-tail split, so the two
/// sides fold the same per-sample values together in a different order, and IEEE 754
/// float addition is not perfectly associative. CPU-only (`GpuBackend::disabled()`),
/// per this lane's own rule against ever touching a real GPU adapter in a test.
#[test]
fn a_local_share_plus_its_complement_matches_render_accumulation() {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    let (width, height, spp) = (6u32, 6u32, 12u32);
    let (cam_yaw, cam_pitch) = (0.3f32, 0.2f32);
    let params = ExportParams {
        width,
        height,
        samples_per_pixel: spp,
        max_bounces: scene.max_bounces,
    };
    let gpu = GpuBackend::disabled();

    // The reference: the whole range, traced by `render_accumulation` itself
    // (`LocalOnly`/`Cpu`, so no remote and no GPU split to introduce non-determinism of
    // its own beyond ordinary chunked summation).
    let mut carry = AccumulationCarry::default();
    let cancel = AtomicBool::new(false);
    let whole = match render_accumulation(
        &scene,
        cam_yaw,
        cam_pitch,
        params,
        ComputeTarget::LocalOnly,
        None,
        &gpu,
        LocalComputeTarget::Cpu,
        &mut carry,
        &cancel,
        |_| {},
    ) {
        AccumulationOutcome::Completed(Accumulation { accum, .. }) => accum,
        AccumulationOutcome::Cancelled | AccumulationOutcome::Failed(_) => {
            panic!("expected a completed accumulation")
        }
    };

    // The SAME samples, split into a head range `[0, first)` traced directly with
    // `render_batch` (standing in for "everything before the reserved tail") and a
    // tail range `[first, spp)` traced through `render_local_share`, exactly as
    // `worker::final_picture::final_picture` forks a v16 contribution.
    let viewer = 5u32;
    let first = spp - viewer;
    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, DEFAULT_FOV_DEG);
    let mut combined = vec![Vec3::ZERO; (width * height) as usize];
    batch::render_batch(width, height, first, 0, &camera, &scene, &mut combined);

    let mut hybrid_frac: Option<f64> = None;
    let stop = AtomicBool::new(false);
    let done = AtomicU32::new(0);
    let tail = render_local_share(
        &scene,
        (cam_yaw, cam_pitch),
        params,
        first,
        viewer,
        &gpu,
        LocalComputeTarget::Cpu,
        &mut hybrid_frac,
        &stop,
        &done,
    )
    .expect("not stopped");
    assert_eq!(
        done.load(Ordering::Relaxed),
        viewer,
        "must finish the whole tail"
    );

    for (dst, src) in combined.iter_mut().zip(&tail) {
        *dst += *src;
    }

    for (whole_px, combined_px) in whole.iter().zip(&combined) {
        let scale = whole_px.length().max(combined_px.length()).max(1e-6);
        let relative_error = (*whole_px - *combined_px).length() / scale;
        assert!(
            relative_error < 1e-3,
            "{whole_px:?} vs {combined_px:?} (relative error {relative_error})"
        );
    }
}

/// `stop` already raised before the first batch is claimed must end the render with
/// `None`, not a partial `Some` -- the reclaim path (`run_final_image_request` raising
/// `LocalShare::stop` the moment the remote's own `DONE` arrives first) depends on this:
/// a local render cut short must never be mistaken for a finished contribution.
#[test]
fn render_local_share_returns_none_when_stopped() {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    let params = ExportParams {
        width: 4,
        height: 4,
        samples_per_pixel: 8,
        max_bounces: scene.max_bounces,
    };
    let gpu = GpuBackend::disabled();
    let mut hybrid_frac: Option<f64> = None;
    let stop = AtomicBool::new(true);
    let done = AtomicU32::new(0);

    let result = render_local_share(
        &scene,
        (0.3, 0.2),
        params,
        0,
        8,
        &gpu,
        LocalComputeTarget::Cpu,
        &mut hybrid_frac,
        &stop,
        &done,
    );
    assert!(result.is_none());
}
