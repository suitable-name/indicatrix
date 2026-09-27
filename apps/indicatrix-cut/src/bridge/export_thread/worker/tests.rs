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
use indicatrix::{optics::raytracer::Camera, renderer::gpu_backend::GpuBackend};
use std::sync::{Mutex, atomic::AtomicBool};

/// The shared core's CPU-only path (`ComputeTarget::LocalOnly`,
/// `LocalComputeTarget::Cpu`) must trace EXACTLY the samples `batch::render_batch`
/// itself would for the same scene/pose/seed -- proving the tilt video's reuse of
/// [`render_accumulation`] for its local CPU frames doesn't take a different,
/// tracer-adjacent path than the still export always has. Both start from an
/// all-zero accumulation buffer and the same `samples_already_done == 0` seed.
#[test]
fn render_accumulation_cpu_only_path_matches_render_batch_directly() {
    let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()));
    let (width, height, spp) = (8u32, 8u32, 4u32);
    let (cam_yaw, cam_pitch) = (0.3f32, 0.2f32);

    let params = ExportParams {
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

    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, 42.0);
    let mut via_batch = vec![Vec3::ZERO; (width * height) as usize];
    render_batch(width, height, spp, 0, &camera, &scene, &mut via_batch);

    assert_eq!(
        via_core, via_batch,
        "the shared core's CPU-only path must be bit-identical to a direct \
         render_batch call for the same scene/pose/seed"
    );
}
