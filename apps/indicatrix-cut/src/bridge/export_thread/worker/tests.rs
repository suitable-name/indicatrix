//! Proves the shared core's CPU-only path traces bit-identically to a direct
//! `batch::render_batch` call.

use super::{
    core::render_accumulation,
    types::{Accumulation, AccumulationCarry, AccumulationOutcome},
};
use crate::{
    bridge::{
        export_thread::{
            batch::render_batch,
            params::{ComputeTarget, ExportParams},
            scene_snapshot::SceneSnapshot,
        },
        render_thread::RenderContext,
    },
    settings::LocalComputeTarget,
};
use glam::Vec3;
use indicatrix::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
    renderer::gpu_backend::GpuBackend,
};
use std::sync::{Mutex, atomic::AtomicBool};

/// The shared core's CPU-only path (`ComputeTarget::LocalOnly`,
/// `LocalComputeTarget::Cpu`) must trace EXACTLY the samples `batch::render_batch`
/// itself would for the same scene/pose/seed -- proving the tilt video's reuse of
/// [`render_accumulation`] for its local CPU frames doesn't take a different,
/// tracer-adjacent path than the still export always has. Both start from an
/// all-zero accumulation buffer and the same `samples_already_done == 0` seed.
#[test]
fn render_accumulation_cpu_only_path_matches_render_batch_directly() {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    let (width, height, spp) = (8u32, 8u32, 4u32);
    let (cam_yaw, cam_pitch) = (0.3f32, 0.2f32);

    let params = ExportParams {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        width,
        height,
        samples_per_pixel: spp,
        max_bounces: scene.max_bounces,
    };
    let gpu = GpuBackend::disabled();
    let mut carry = AccumulationCarry::default();
    let cancel = AtomicBool::new(false);
    let outcome = render_accumulation(
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
    );
    let via_core = match outcome {
        AccumulationOutcome::Completed(Accumulation { accum, .. }) => accum,
        AccumulationOutcome::Cancelled | AccumulationOutcome::Failed(_) => {
            panic!("expected a completed accumulation")
        }
    };

    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, DEFAULT_FOV_DEG);
    let mut via_batch = vec![Vec3::ZERO; (width * height) as usize];
    render_batch(width, height, spp, 0, &camera, &scene, &mut via_batch);

    assert_eq!(
        via_core, via_batch,
        "the shared core's CPU-only path must be bit-identical to a direct \
         render_batch call for the same scene/pose/seed"
    );
}

/// Final-picture contributions and remote retries keep absolute AA tap identity.
#[test]
fn ray_export_parts_reassemble_the_whole_without_hybrid_calibration() {
    use indicatrix::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
    use std::sync::atomic::{AtomicU32, Ordering};
    let mut scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("scene");
    scene.engine = EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: 8,
            fire: true,
        },
    };
    let params = ExportParams {
        engine: scene.engine,
        width: 8,
        height: 8,
        samples_per_pixel: 8,
        max_bounces: scene.max_bounces,
    };
    let gpu = GpuBackend::disabled();
    let stop = AtomicBool::new(false);
    let mut carry = AccumulationCarry::default();
    let outcome = render_accumulation(
        &scene,
        0.3,
        0.2,
        params,
        ComputeTarget::LocalOnly,
        None,
        &gpu,
        LocalComputeTarget::Cpu,
        &mut carry,
        &stop,
        |_| {},
    );
    let AccumulationOutcome::Completed(whole) = outcome else {
        panic!("Ray render completes")
    };
    assert_eq!(whole.samples_per_pixel, 8);
    assert!(carry.hybrid_frac.is_none());
    let mut sums = vec![Vec3::ZERO; 64];
    for (first, count) in [(0, 3), (3, 2), (5, 3)] {
        let done = AtomicU32::new(0);
        let mut hybrid = None;
        let part = super::core::render_local_share(
            &scene,
            (0.3, 0.2),
            params,
            first,
            count,
            &gpu,
            LocalComputeTarget::Cpu,
            &mut hybrid,
            &stop,
            &done,
        )
        .expect("claim completes");
        assert_eq!(done.load(Ordering::Relaxed), count);
        assert!(hybrid.is_none());
        for (sum, value) in sums.iter_mut().zip(part) {
            *sum += value;
        }
    }
    for (whole, assembled) in whole.accum.iter().zip(sums) {
        assert!(
            (*whole - assembled).abs().max_element() <= 1.0e-5 * whole.abs().max_element().max(1.0)
        );
    }
}

#[test]
fn a_ray_override_refuses_diagnostics_before_rendering_a_path_snapshot() {
    use indicatrix::renderer::{engine::RenderEngine, optical_overlay::OpticalPresentation};
    let mut scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("scene");
    scene.optical_diagnostics = Some(OpticalPresentation::default());
    assert!(!scene.engine.is_ray());
    let params = ExportParams {
        engine: indicatrix::renderer::engine::EngineSpec {
            engine: RenderEngine::RayTraced,
            ..Default::default()
        },
        width: 8,
        height: 8,
        samples_per_pixel: 8,
        max_bounces: scene.max_bounces,
    };
    let outcome = super::final_picture::render_image_rgba(
        &scene,
        scene.yaw,
        scene.pitch,
        params,
        indicatrix::color::ColorSpace::Srgb,
        &crate::bridge::export_thread::params::RemoteSelection::local_only(),
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    let super::final_picture::RenderedImage::Failed(reason) = outcome else {
        panic!("diagnostics are refused")
    };
    assert_eq!(reason, "Optical Diagnostics require Path-Traced rendering.");
}
