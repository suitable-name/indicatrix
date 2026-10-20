//! Worker contracts against the shared deterministic image tracer.
use super::*;
use indicatrix::renderer::{
    engine::RenderEngine,
    frame_scene::FrameScene,
    whitted::{WhittedPrep, whitted_pixels_interleaved_geom},
};
fn scene() -> SceneState {
    let mut scene = super::tests::tiny_scene();
    scene.engine.engine = RenderEngine::RayTraced;
    scene.engine.ray.aa_taps = 4;
    scene
}
#[test]
fn ray_worker_matches_the_shared_cpu_tracer_and_thread_counts() {
    let scene = scene();
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG);
    let finishes = resolve_facet_finishes(&scene);
    let frame = FrameScene {
        camera: &camera,
        width: scene.width,
        height: scene.height,
        planes: &scene.planes,
        facet_finishes: &finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment: crate::assets::environment_source(&scene, None),
    };
    let prep = WhittedPrep::new(&frame, &scene.fluorescence, scene.engine.ray).unwrap();
    let expected = whitted_pixels_interleaved_geom(
        &frame,
        &scene.tools,
        &build_plane_soa(&scene.planes),
        &prep,
        0,
        1,
        0,
        4,
    );
    assert_eq!(trace_samples(&scene, 0, 4, 1), expected);
    assert_eq!(trace_samples(&scene, 0, 4, 3), expected);
    // Separate worker ranges cover each absolute tap exactly once.
    let a = trace_samples(&scene, 0, 1, 1);
    let b = trace_samples(&scene, 1, 3, 2);
    for ((a, b), whole) in a.iter().zip(b).zip(expected) {
        assert!((*a + b - whole).abs().max_element() <= 1e-6 * whole.abs().max_element().max(1.0));
    }
}
#[test]
fn a_cancelled_ray_worker_never_commits_a_partial_sum() {
    let scene = scene();
    let mut out = vec![Vec3::ONE; 64];
    assert!(!whitted::trace(
        &scene,
        0,
        4,
        1,
        false,
        &AtomicBool::new(true),
        &mut out
    ));
    assert_eq!(out, vec![Vec3::ONE; 64]);
}

#[test]
fn invalid_ray_ranges_or_buffers_leave_the_output_untouched() {
    let scene = scene();
    for (first, count, len) in [(u32::MAX, 1, 64), (4, 1, 64), (0, 4, 63)] {
        let mut out = vec![Vec3::ONE; len];
        assert!(!whitted::trace(
            &scene,
            first,
            count,
            1,
            false,
            &AtomicBool::new(false),
            &mut out
        ));
        assert_eq!(out, vec![Vec3::ONE; len]);
    }
}
