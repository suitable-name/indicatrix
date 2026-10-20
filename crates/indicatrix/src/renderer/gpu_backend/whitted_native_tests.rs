//! Real-kernel checks of frozen uploads and transactional reuse (owner-run).
use super::*;
use crate::{
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, build_plane_soa},
    },
    renderer::{
        engine::RayOptions, env_map::EnvironmentMap, gpu::test_support::require_gpu,
        whitted::whitted_pixels_interleaved_geom,
    },
};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Work limits and cancellation decline or abort without touching the accumulator.
fn assert_limits_and_cancellation_leave_the_accumulator(
    backend: &WhittedGpuBackend,
    uploaded: &WhittedGpuPrepared,
    accum: &mut [Vec3],
    before: &[Vec3],
) {
    backend.set_work_limits(GpuWorkLimits {
        max_frame_pixels: accum.len() as u64 - 1,
        max_samples_per_call: 1,
        chunk_budget_bytes: 12,
        chunks_per_turn: 1,
    });
    assert_eq!(
        backend.try_accumulate_prepared_cancellable(uploaded, 0, 1, accum, &|| false,),
        GpuAccumulate::Declined
    );
    assert_eq!(accum, before);
    backend.set_work_limits(GpuWorkLimits {
        max_frame_pixels: accum.len() as u64,
        max_samples_per_call: 1,
        chunk_budget_bytes: 12,
        chunks_per_turn: 1,
    });
    assert_eq!(
        backend.try_accumulate_prepared_cancellable(uploaded, 0, 2, accum, &|| false,),
        GpuAccumulate::Declined
    );
    let checks = AtomicUsize::new(0);
    assert_eq!(
        backend.try_accumulate_prepared_cancellable(uploaded, 2, 1, accum, &|| checks
            .fetch_add(1, Ordering::Relaxed)
            >= 2,),
        GpuAccumulate::Cancelled
    );
    assert_eq!(
        accum, before,
        "discard earlier successful chunks on cancellation"
    );
}

#[test]
fn prepared_uploads_are_frozen_reusable_and_transactional() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<WhittedGpuPrepared>();
    let Some(context) = require_gpu() else {
        return;
    };
    let renderer =
        pollster::block_on(WhittedGpuRenderer::from_context_async(context)).expect("Ray pipeline");
    let backend = WhittedGpuBackend::with_renderer(Some(renderer));
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let env = EnvironmentMap::uniform(4, 2, [0.7, 0.5, 0.3]);
    let scene = FrameScene {
        camera: &camera,
        width: 8,
        height: 4,
        planes: &[],
        facet_finishes: &[],
        material: &material,
        max_bounces: 16,
        environment: EnvironmentSource::HdrMap(&env),
    };
    let mut prep =
        WhittedPrep::new(&scene, Fluorescence::none(), RayOptions::default()).expect("prep");
    let expected =
        whitted_pixels_interleaved_geom(&scene, &[], &build_plane_soa(&[]), &prep, 0, 1, 0, 2);
    let uploaded = backend
        .prepare_cancellable(&scene, &[], &prep, &|| false)
        .expect("upload");
    // Public CPU prep can change after uploading. This snapshot retains its image.
    prep.rgb_to_xyz = glam::Mat3::ZERO;
    prep.taps = 0;
    prep.chain_count = 0;
    let mut accum = vec![Vec3::ZERO; expected.len()];
    for offset in 0..2 {
        assert_eq!(
            backend
                .try_accumulate_prepared_cancellable(&uploaded, offset, 1, &mut accum, &|| false,),
            GpuAccumulate::Done
        );
    }
    let difference =
        crate::renderer::gpu::event_wavefront::compare::compare_frames(&expected, &accum, 2);
    assert_eq!(difference.non_finite_pixels, 0, "{difference:?}");
    assert_eq!(difference.tol_failing_pixels, 0, "{difference:?}");
    let before = accum.clone();
    assert_eq!(
        backend.try_accumulate_prepared_cancellable(&uploaded, 8, 1, &mut accum, &|| false,),
        GpuAccumulate::Declined
    );
    assert_eq!(accum, before);
    assert_eq!(
        backend.try_accumulate_prepared_cancellable(&uploaded, 0, 1, &mut accum[..1], &|| false,),
        GpuAccumulate::Declined
    );
    assert_eq!(accum, before);
    assert_limits_and_cancellation_leave_the_accumulator(&backend, &uploaded, &mut accum, &before);
    // A replacement renderer/device must not consume a prior device's buffers.
    let Some(replacement) = require_gpu() else {
        return;
    };
    *backend.renderer.lock().unwrap() = Some(
        pollster::block_on(WhittedGpuRenderer::from_context_async(replacement))
            .expect("replacement"),
    );
    assert_eq!(
        backend.try_accumulate_prepared_cancellable(&uploaded, 0, 1, &mut accum, &|| false,),
        GpuAccumulate::Declined
    );
    assert_eq!(accum, before);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = backend.renderer.lock().unwrap();
        panic!("simulate a GPU panic while owning the renderer");
    }));
    assert!(panic.is_err());
    assert!(
        !backend.try_recover(),
        "never reuse poisoned renderer state"
    );
    assert!(matches!(
        backend.prepare_cancellable(&scene, &[], &prep, &|| false),
        Err(GpuAccumulate::Declined)
    ));
    assert_eq!(
        backend.try_accumulate_prepared_cancellable(&uploaded, 0, 1, &mut accum, &|| false,),
        GpuAccumulate::Declined
    );
    assert_eq!(accum, before);
}
