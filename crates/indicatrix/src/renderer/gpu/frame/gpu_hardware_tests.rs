//! Hardware tests for renderer poisoning, wavefront chunk-sizing, HDR
//! storage-binding declines, and the reduction's non-finite rule -- unlike
//! `tests` above, every test here needs a real `wgpu` adapter, so each one acquires its
//! OWN `GpuContext`/[`GpuFrameRenderer`] and prints a note and returns (a clean skip,
//! not a failure) when `GpuContext::acquire` finds none. Run with:
//!
//! ```text
//! cargo test -p indicatrix --features gpu -- gpu_hardware_tests
//! ```

use std::sync::atomic::Ordering;

use glam::Vec3;

use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, LightingPreset, add_finite_sample},
    },
    renderer::{env_map::EnvironmentMap, gpu::compute},
};

use super::{GpuFrameError, GpuFrameScene, GpuPipelineKind, renderer::GpuFrameRenderer};

/// A tiny scene good enough to dispatch a real chunk with -- the same fixture
/// [`super::equivalence::run_pipeline_equivalence`] above already uses, reused rather
/// than reinvented.
fn tiny_scene_material() -> GemMaterial {
    GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material")
}

/// An environment map whose texel buffer would exceed the device's
/// `max_storage_buffer_binding_size` (the WebGPU baseline this crate requests is 128
/// MiB, see `GpuContext::acquire_async`; 4100x2050 vec4-padded texels is
/// `4100*2050*16 = 134_480_000` bytes, just over `134_217_728`) must decline
/// cleanly -- [`GpuFrameError::UnsupportedEnvironment`] -- not panic or hang.
#[test]
fn oversized_hdr_environment_is_declined_not_panicked() {
    let Ok(mut renderer) = GpuFrameRenderer::new() else {
        println!("skipping oversized_hdr_environment_is_declined_not_panicked: no GPU adapter");
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = tiny_scene_material();
    let map = EnvironmentMap::from_rgb(4100, 2050, vec![[0.0, 0.0, 0.0]; 4100 * 2050])
        .expect("4100x2050 black texels is a self-consistent buffer");
    let scene = GpuFrameScene {
        camera: &camera,
        width: 8,
        height: 8,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: EnvironmentSource::HdrMap(&map),
    };
    let mut accum = vec![Vec3::ZERO; 64];
    let result = renderer.accumulate(&scene, 0, 1, &mut accum);
    assert!(
        matches!(result, Err(GpuFrameError::UnsupportedEnvironment)),
        "expected UnsupportedEnvironment, got {result:?}"
    );
    // Must be a clean, per-call decline -- NOT a poisoned/DeviceLost renderer. A
    // later call with an in-bounds map on the SAME renderer must still work.
    let small_map = EnvironmentMap::uniform(1024, 512, [1.0, 1.0, 1.0]);
    let scene2 = GpuFrameScene {
        environment: EnvironmentSource::HdrMap(&small_map),
        ..scene
    };
    renderer
        .accumulate(&scene2, 0, 1, &mut accum)
        .expect("a 1024x512 HDR map is well within the device's binding limit");
}

/// The in-bounds counterpart to the oversized-map test above: a 1024x512 map --
/// comfortably under 128 MiB (`1024*512*16 = 8_388_608` bytes) -- must render,
/// standalone (not merely as the second half of that test).
#[test]
fn in_bounds_hdr_environment_renders() {
    let Ok(mut renderer) = GpuFrameRenderer::new() else {
        println!("skipping in_bounds_hdr_environment_renders: no GPU adapter");
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = tiny_scene_material();
    let map = EnvironmentMap::uniform(1024, 512, [1.0, 1.0, 1.0]);
    let scene = GpuFrameScene {
        camera: &camera,
        width: 8,
        height: 8,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: EnvironmentSource::HdrMap(&map),
    };
    let mut accum = vec![Vec3::ZERO; 64];
    renderer
        .accumulate(&scene, 0, 1, &mut accum)
        .expect("a 1024x512 HDR map is well within the device's binding limit");
    assert!(
        accum.iter().any(|v| v.length_squared() > 0.0),
        "a lit uniform environment must leave SOME nonzero radiance in accum"
    );
}

/// A full-size (1920x1080) wavefront-pipeline dispatch must not error --
/// the chunking hazard this guards against (a chunk's `stokes` buffer request
/// exceeding the device's storage-buffer-binding limit) only grows the EMA-driven
/// chunk size AFTER the first chunk or two drains, so several `accumulate` calls (not
/// just one) are needed to actually exercise it. See
/// `GpuFrameRenderer::next_chunk_pixels`/`GpuFrameRenderer::cap_byte_budget_for_wavefront`'s
/// own doc comments.
#[test]
fn wavefront_1080p_dispatch_does_not_error_as_the_ema_grows() {
    let Ok(mut renderer) = GpuFrameRenderer::new() else {
        println!(
            "skipping wavefront_1080p_dispatch_does_not_error_as_the_ema_grows: no GPU adapter"
        );
        return;
    };
    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    if renderer.pipeline_kind() != GpuPipelineKind::Wavefront {
        println!(
            "skipping wavefront_1080p_dispatch_does_not_error_as_the_ema_grows: this \
             device cannot bind enough buffers for the wavefront pipeline \
             (set_pipeline_kind fell back to Megakernel)"
        );
        return;
    }
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = tiny_scene_material();
    let (width, height) = (1920u32, 1080u32);
    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut accum = vec![Vec3::ZERO; (width * height) as usize];
    // Several calls, not one: the chunk-timing EMA (`ns_per_tuple_ema`) only starts
    // growing the per-chunk pixel budget toward the full frame after earlier chunks
    // have drained and reported their throughput -- see `next_chunk_pixels`'s own
    // doc comment. `FIRST_DISPATCH_MAX_TUPLES` bounds the very first chunk well
    // under the old 2.1M-tuple hazard regardless, so this loop is what actually
    // drives the cap toward its limit.
    for sample in 0..4u32 {
        renderer
            .accumulate(&scene, sample, 1, &mut accum)
            .unwrap_or_else(|e| panic!("wavefront 1920x1080 dispatch #{sample} errored: {e}"));
    }
    assert!(
        accum.iter().any(|v| v.length_squared() > 0.0),
        "a lit studio-rig scene must leave SOME nonzero radiance in accum"
    );
}

/// After [`GpuFrameRenderer::abandon_in_flight`], the renderer is
/// permanently poisoned -- `GpuFrameRenderer::accumulate_turn` (reached here via
/// the public [`GpuFrameRenderer::accumulate`]) must return
/// [`GpuFrameError::DeviceLost`], every time, never dispatch into it again.
#[test]
fn abandon_in_flight_poisons_the_renderer_permanently() {
    let Ok(mut renderer) = GpuFrameRenderer::new() else {
        println!("skipping abandon_in_flight_poisons_the_renderer_permanently: no GPU adapter");
        return;
    };
    renderer.abandon_in_flight();

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = tiny_scene_material();
    let scene = GpuFrameScene {
        camera: &camera,
        width: 4,
        height: 4,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 2,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut accum = vec![Vec3::ZERO; 16];
    let first = renderer.accumulate(&scene, 0, 1, &mut accum);
    assert!(
        matches!(first, Err(GpuFrameError::DeviceLost(_))),
        "expected DeviceLost right after abandon_in_flight, got {first:?}"
    );
    // A SECOND call must decline identically -- poisoning is permanent, not a
    // one-shot signal that clears itself.
    let second = renderer.accumulate(&scene, 0, 1, &mut accum);
    assert!(
        matches!(second, Err(GpuFrameError::DeviceLost(_))),
        "expected DeviceLost again on a second call, got {second:?}"
    );
}

/// `GpuContext::validation_error_seen`, set directly (standing
/// in for a real uncaptured `wgpu::Error` the `on_uncaptured_error` handler would
/// otherwise have to be provoked to actually observe), turns the very next turn into
/// [`GpuFrameError::DeviceLost`] -- the mechanism `GpuFrameRenderer::
/// accumulate_turn_body` uses to convert what would otherwise be a wgpu-panicking
/// uncaptured error into an ordinary `Err` return.
#[test]
fn validation_error_seen_flag_turns_the_next_turn_into_device_lost() {
    let Ok(mut renderer) = GpuFrameRenderer::new() else {
        println!(
            "skipping validation_error_seen_flag_turns_the_next_turn_into_device_lost: no \
             GPU adapter"
        );
        return;
    };
    renderer
        .ctx
        .validation_error_seen
        .store(true, Ordering::Relaxed);

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = tiny_scene_material();
    let scene = GpuFrameScene {
        camera: &camera,
        width: 4,
        height: 4,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 2,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut accum = vec![Vec3::ZERO; 16];
    let result = renderer.accumulate(&scene, 0, 1, &mut accum);
    assert!(
        matches!(result, Err(GpuFrameError::DeviceLost(_))),
        "expected DeviceLost, got {result:?}"
    );
}

/// `reduce_xyz_main` applies the shared non-finite rule: a tuple with any NaN/±Inf
/// component is dropped from its pixel's sum (the caller still counts it), exactly as
/// its CPU twin `add_finite_sample` does. Dispatches the real `reduce_xyz.wgsl` source
/// standalone over hand-written tuples -- the transport kernel has no reachable NaN
/// producer to provoke one with -- and checks every pixel sum is finite and
/// bit-identical to the CPU rule's sum over the same tuples in the same order.
#[test]
fn reduce_drops_non_finite_samples_like_its_cpu_twin() {
    let Ok(renderer) = GpuFrameRenderer::new() else {
        println!("skipping reduce_drops_non_finite_samples_like_its_cpu_twin: no GPU adapter");
        return;
    };
    let ctx = &renderer.ctx;
    let nan = f32::NAN;
    let inf = f32::INFINITY;
    // 3 pixels x 4 samples. Pixel 0 mixes finite, NaN and Inf samples; pixel 1 is all
    // finite; pixel 2 is all non-finite (must come out exactly zero, not NaN).
    let samples: [[f32; 3]; 12] = [
        [1.0, 2.0, 3.0],
        [nan, 1.0, 1.0],
        [4.0, 5.0, 6.0],
        [1.0, inf, 1.0],
        [0.5, 0.25, 0.125],
        [0.5, 0.25, 0.125],
        [0.5, 0.25, 0.125],
        [0.5, 0.25, 0.125],
        [nan, nan, nan],
        [-inf, 0.0, 0.0],
        [0.0, 0.0, inf],
        [f32::from_bits(0x7fc0_1234), 1.0, 1.0],
    ];
    let (num_pixels, num_samples) = (3u32, 4u32);

    let params = super::GpuReduceParams {
        num_pixels,
        num_samples,
        _pad0: 0,
        _pad1: 0,
    };
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "reduce non-finite test",
        super::REDUCE_SHADER_SRC,
        "reduce_xyz_main",
    );
    let params_buf = compute::upload(
        &ctx.device,
        "reduce non-finite test params",
        std::slice::from_ref(&params),
        wgpu::BufferUsages::UNIFORM,
    );
    let input_buf = compute::upload(
        &ctx.device,
        "reduce non-finite test input",
        samples.as_flattened(),
        wgpu::BufferUsages::STORAGE,
    );
    let output_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "reduce non-finite test output",
        (num_pixels * 3) as usize,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "reduce non-finite test bind group",
        &pipeline,
        &[(0, &params_buf), (1, &input_buf), (2, &output_buf)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));
    let gpu: Vec<f32> = compute::readback(
        &ctx.device,
        &ctx.queue,
        &output_buf,
        (num_pixels * 3) as usize,
    );

    for (pixel, tuples) in samples.chunks(num_samples as usize).enumerate() {
        let mut cpu = Vec3::ZERO;
        for t in tuples {
            add_finite_sample(&mut cpu, Vec3::from_array(*t));
        }
        let gpu_sum = Vec3::new(gpu[pixel * 3], gpu[pixel * 3 + 1], gpu[pixel * 3 + 2]);
        assert!(
            gpu_sum.is_finite(),
            "pixel {pixel}: GPU sum {gpu_sum} is not finite"
        );
        assert_eq!(
            gpu_sum.to_array().map(f32::to_bits),
            cpu.to_array().map(f32::to_bits),
            "pixel {pixel}: GPU reduction {gpu_sum} differs from the CPU rule's {cpu}"
        );
    }
    assert_eq!(
        Vec3::from_slice(&gpu[0..3]),
        Vec3::new(5.0, 7.0, 9.0),
        "pixel 0 keeps only its finite samples"
    );
    assert_eq!(
        Vec3::from_slice(&gpu[6..9]),
        Vec3::ZERO,
        "an all-non-finite pixel sums to zero"
    );
}

/// Guards against the live viewport freezing: showing an initial image but rendering
/// nothing further, and ignoring camera drags from then on. Mimics
/// `apps::indicatrix-cut`'s `ViewportGpu`/render-loop pattern directly against this
/// renderer: several full-frame `accumulate` calls at a realistic viewport
/// resolution, each with a DIFFERENT camera pose (as `on_camera_orbit` produces on
/// every drag `moved` event) and an advancing `sample_offset` (as the render loop's
/// `accum_samples` does) -- plus a resize partway through (preview-then-settle: the
/// render loop drops to a reduced resolution while `camera_moving` is true, then
/// returns to full size once the drag ends). Every call must succeed; a
/// `DeviceLost` on any call after the first is exactly that freeze, so its captured
/// `last_uncaptured_error` text is printed to help find the real validation error if
/// this ever fails.
#[test]
fn repeated_turns_with_camera_changes_never_poison_the_renderer() {
    let Ok(mut renderer) = GpuFrameRenderer::new() else {
        println!(
            "skipping repeated_turns_with_camera_changes_never_poison_the_renderer: no \
             GPU adapter"
        );
        return;
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = tiny_scene_material();
    let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

    // Full viewport size, then a reduced "camera_moving" preview size, then back to
    // full -- see `apps::indicatrix-cut::bridge::render_thread::local_preview::
    // effective_dimensions`.
    let full = (480u32, 360u32);
    let preview = (160u32, 120u32);
    let sizes = [full, full, preview, preview, full, full, full];

    let mut sample_offset = 0u32;
    for (turn, &(width, height)) in sizes.iter().enumerate() {
        // A fresh yaw/pitch every turn -- exactly what `on_camera_orbit` writes into
        // `RenderContext` on every `moved` event mid-drag.
        #[allow(
            clippy::cast_precision_loss,
            reason = "turn index is tiny; precision loss is not a concern in this test"
        )]
        let yaw = (turn as f32).mul_add(0.37, 0.1);
        #[allow(
            clippy::cast_precision_loss,
            reason = "turn index is tiny; precision loss is not a concern in this test"
        )]
        let pitch = (turn as f32).mul_add(0.11, 0.2).clamp(-1.4, 1.4);
        let camera = Camera::new(yaw, pitch, 5.0, 18.0);
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 4,
            environment,
        };
        let spp = 2u32;
        let mut accum = vec![Vec3::ZERO; (width * height) as usize];
        let result = renderer.accumulate(&scene, sample_offset, spp, &mut accum);
        if let Err(GpuFrameError::DeviceLost(ref why)) = result {
            panic!(
                "turn {turn} ({width}x{height}, yaw={yaw}, pitch={pitch}) was declared \
                 DeviceLost -- this is the exact poisoning the bug report describes: \
                 {why}"
            );
        }
        result.unwrap_or_else(|e| panic!("turn {turn} failed: {e}"));
        sample_offset += spp;
    }
}
