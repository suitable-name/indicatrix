//! Native GPU tests for event wavefront rendering.
//!
//! Skips cleanly if no GPU adapter is available, unless `INDICATRIX_REQUIRE_GPU=1` is set.

use glam::Vec3;

use super::{
    BYTES_PER_RAY, CHUNK_BUDGET_BYTES, EwError, chunk_pixels_for, compare::compare_frames,
    next_chunk_pixels,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
    renderer::{
        frame_scene::FrameScene,
        gpu::{
            GpuContext, event_wavefront::EventWavefrontRenderer,
            frame::run_event_wavefront_equivalence,
        },
    },
};

use super::accumulate::FIRST_DISPATCH_MAX_TUPLES;

const REQUIRE_GPU_ENV: &str = "INDICATRIX_REQUIRE_GPU";

fn gpu_required() -> bool {
    std::env::var(REQUIRE_GPU_ENV).is_ok_and(|v| v == "1")
}

fn ew_renderer_or_skip(test_name: &str) -> Option<EventWavefrontRenderer> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(EventWavefrontRenderer::new(ctx)),
        Err(err) => {
            assert!(
                !gpu_required(),
                "{test_name}: {REQUIRE_GPU_ENV}=1 was set, but GPU acquisition failed: {err}"
            );
            println!("skipping {test_name}: no GPU adapter ({err})");
            None
        }
    }
}

#[test]
fn ew_renderer_initializes_cleanly() {
    let Some(renderer) = ew_renderer_or_skip("ew_renderer_initializes_cleanly") else {
        return;
    };
    assert!(renderer.ns_per_tuple_ema().is_none());
    assert_eq!(
        renderer.chunk_budget_bytes(),
        super::accumulate::CHUNK_BUDGET_BYTES
    );
}

#[test]
fn ew_renders_spinel_tiny() {
    let Some(mut renderer) = ew_renderer_or_skip("ew_renders_spinel_tiny") else {
        return;
    };

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in material");
    let (width, height) = (16u32, 16u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = FrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 6,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    let mut accum = vec![Vec3::ZERO; num_pixels];
    pollster::block_on(renderer.accumulate(&scene, 0, spp, &mut accum))
        .expect("accumulate should succeed for Spinel");

    assert!(
        accum.iter().all(|v| v.is_finite()),
        "all accumulated XYZ values must be finite"
    );
    assert!(
        accum.iter().any(|v| v.y > 0.0),
        "at least some pixels must receive light"
    );
}

/// Renders a small Spinel scene with the given sample range into a fresh buffer.
fn render_spinel(
    renderer: &mut EventWavefrontRenderer,
    sample_offset: u32,
    spp: u32,
) -> Result<Vec<Vec3>, EwError> {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in material");
    let (width, height) = (32u32, 32u32);
    let scene = FrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 6,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut accum = vec![Vec3::ZERO; (width * height) as usize];
    pollster::block_on(renderer.accumulate(&scene, sample_offset, spp, &mut accum))?;
    Ok(accum)
}

#[test]
fn ew_sample_offset_ranges_add_up_to_one_larger_pass() {
    let Some(mut renderer) = ew_renderer_or_skip("ew_sample_offset_ranges_add_up") else {
        return;
    };
    let first = render_spinel(&mut renderer, 0, 4).expect("samples 0..4");
    let second = render_spinel(&mut renderer, 4, 4).expect("samples 4..8");
    let whole = render_spinel(&mut renderer, 0, 8).expect("samples 0..8");
    let summed: Vec<Vec3> = first.iter().zip(&second).map(|(a, b)| *a + *b).collect();

    // The seed and jitter come from the absolute sample index, so the two halves are
    // exactly the larger pass up to float summation order.
    let diff = compare_frames(&summed, &whole, 8);
    assert_eq!(diff.non_finite_pixels, 0);
    assert!(
        diff.tol_passed(whole.len()),
        "sample_offset ranges do not add up: {diff:?}"
    );
    // And the second range must really differ from the first (it is not a replay).
    assert!(compare_frames(&first, &second, 4).differing_pixels > 0);
}

#[test]
fn ew_multi_chunk_accumulation_equals_single_chunk() {
    let Some(mut single) = ew_renderer_or_skip("ew_multi_chunk_accumulation") else {
        return;
    };
    let reference = render_spinel(&mut single, 0, 2).expect("single-chunk render");

    let Some(mut chunked) = ew_renderer_or_skip("ew_multi_chunk_accumulation") else {
        return;
    };
    // 64 rays per chunk: 32 pixels at spp 2, so a 32x32 frame needs 32 chunks.
    chunked.set_chunk_budget_bytes(BYTES_PER_RAY * 64);
    let pieces = render_spinel(&mut chunked, 0, 2).expect("multi-chunk render");

    // Every pixel is reduced from its own samples only, so chunking changes nothing.
    let diff = compare_frames(&reference, &pieces, 2);
    assert_eq!(
        diff.differing_pixels, 0,
        "chunking changed the image: {diff:?}"
    );
}

#[test]
fn ew_renderer_recovers_after_a_failed_call() {
    let Some(mut renderer) = ew_renderer_or_skip("ew_renderer_recovers_after_a_failed_call") else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in material");
    let scene = FrameScene {
        camera: &camera,
        width: 16,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut wrong = vec![Vec3::ZERO; 3];
    let err = pollster::block_on(renderer.accumulate(&scene, 0, 1, &mut wrong))
        .expect_err("a wrong-sized accumulator must be refused");
    assert!(matches!(err, EwError::InvalidBufferSize { .. }));
    render_spinel(&mut renderer, 0, 1).expect("renderer must work after an error");
}

#[test]
fn ew_uncaptured_validation_error_becomes_device_lost() {
    use std::sync::atomic::Ordering;

    let Some(renderer) = ew_renderer_or_skip("ew_uncaptured_validation_error") else {
        return;
    };
    assert!(renderer.check_uncaptured_error().is_ok());
    renderer
        .context()
        .validation_error_seen
        .store(true, Ordering::Relaxed);
    let err = renderer
        .check_uncaptured_error()
        .expect_err("a seen validation error must not pass silently");
    assert!(matches!(err, EwError::DeviceLost(_)), "got {err:?}");
    // The flag is consumed by the check.
    assert!(renderer.check_uncaptured_error().is_ok());
}

#[test]
fn ew_matches_megakernel_per_material_class() {
    let Some(renderer) = ew_renderer_or_skip("ew_matches_megakernel_per_material_class") else {
        return;
    };
    for class in [1u32, 2, 3] {
        let result = run_event_wavefront_equivalence(renderer.context(), class);
        assert!(
            result.passed(),
            "class {class}: {} / {} pixels differ, {} beyond tolerance, max |delta| {:e}",
            result.differing_pixels,
            result.total_pixels,
            result.tol_failing_pixels,
            result.max_abs_diff
        );
    }
}

#[test]
fn chunk_pixels_for_respects_budget_binding_and_workgroup_limits() {
    let big_binding = u64::MAX / 2;
    // Budget-bound: 64 MiB / 668 B rays, divided by spp.
    let rays = CHUNK_BUDGET_BYTES / BYTES_PER_RAY;
    assert_eq!(
        chunk_pixels_for(CHUNK_BUDGET_BYTES, 4, usize::MAX, big_binding),
        (rays.min(65535 * 64)) / 4
    );
    // Never more pixels than exist.
    assert_eq!(chunk_pixels_for(CHUNK_BUDGET_BYTES, 1, 10, big_binding), 10);
    // Binding-bound: 288 B per ray in the chan buffer.
    assert_eq!(
        chunk_pixels_for(usize::MAX / 2, 1, usize::MAX, 288 * 100),
        100
    );
    // Always at least one pixel.
    assert_eq!(chunk_pixels_for(1, 1, 50, big_binding), 1);
    assert_eq!(chunk_pixels_for(CHUNK_BUDGET_BYTES, 0, 50, big_binding), 50);
}

#[test]
fn next_chunk_pixels_uses_first_dispatch_cap_then_the_ema() {
    // No EMA yet: bounded by FIRST_DISPATCH_MAX_TUPLES / spp, the byte budget and the pixels left.
    assert_eq!(
        next_chunk_pixels(None, usize::MAX, 4, usize::MAX),
        FIRST_DISPATCH_MAX_TUPLES / 4
    );
    assert_eq!(next_chunk_pixels(None, 10, 4, 1000), 10);
    assert_eq!(next_chunk_pixels(None, 1000, 4, 7), 7);
    // With an EMA: target tuples = TARGET_CHUNK_MS / ns-per-tuple, then / spp.
    let ns = 1000.0;
    let target_tuples = super::TARGET_CHUNK_MS * 1.0e6 / ns;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "target_tuples is a positive, finite count far below usize::MAX"
    )]
    let expected = (target_tuples / 2.0).floor() as usize;
    assert_eq!(
        next_chunk_pixels(Some(ns), usize::MAX, 2, usize::MAX),
        expected
    );
    // Clamped by the byte budget and the pixels left; never zero.
    assert_eq!(next_chunk_pixels(Some(ns), 5, 2, usize::MAX), 5);
    assert_eq!(next_chunk_pixels(Some(ns), usize::MAX, 2, 3), 3);
    assert!(next_chunk_pixels(Some(1.0e12), 100, 1, 100) >= 1);
    // A non-positive EMA is ignored like a missing one.
    assert_eq!(
        next_chunk_pixels(Some(0.0), usize::MAX, 4, usize::MAX),
        FIRST_DISPATCH_MAX_TUPLES / 4
    );
}

/// The Rust tag/arm/flag constants are twins of `00_common.wgsl`; a drift would route
/// rays to no kernel. Pure text check, no GPU.
#[test]
fn rust_tag_constants_match_the_wgsl_common_unit() {
    use super::{
        ARM_FROSTED, ARM_GENERIC, ARM_TIR, ARM_UNI_ENTRY, ARM_UNI_INTERNAL, TAG_ARM_BASE, TAG_DONE,
        TAG_EXTEND, TAG_FINALIZE, TAG_NEE_HG, TAG_SCATTER, TAG_SPLIT, TAG_SURFACE, TAG_TAIL,
    };

    const WGSL: &str = include_str!("../../shaders/event_wavefront/00_common.wgsl");
    let wgsl_value = |name: &str| -> u32 {
        let needle = format!("const {name}: u32 = ");
        let start = WGSL
            .find(&needle)
            .unwrap_or_else(|| panic!("{name} is missing from 00_common.wgsl"))
            + needle.len();
        let digits: String = WGSL[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().expect("numeric WGSL constant")
    };
    for (name, rust) in [
        ("TAG_DONE", TAG_DONE),
        ("TAG_EXTEND", TAG_EXTEND),
        ("TAG_SCATTER", TAG_SCATTER),
        ("TAG_SURFACE", TAG_SURFACE),
        ("TAG_ARM_BASE", TAG_ARM_BASE),
        ("TAG_SPLIT", TAG_SPLIT),
        ("TAG_TAIL", TAG_TAIL),
        ("TAG_FINALIZE", TAG_FINALIZE),
        ("TAG_NEE_HG", TAG_NEE_HG),
    ] {
        assert_eq!(wgsl_value(name), rust, "{name} drifted from the WGSL unit");
    }
    // Arms are offsets from TAG_ARM_BASE; the WGSL spells them as `ARM_*` constants too
    // when present.
    for (name, rust) in [
        ("ARM_FROSTED", ARM_FROSTED),
        ("ARM_TIR", ARM_TIR),
        ("ARM_UNI_ENTRY", ARM_UNI_ENTRY),
        ("ARM_UNI_INTERNAL", ARM_UNI_INTERNAL),
        ("ARM_GENERIC", ARM_GENERIC),
    ] {
        if WGSL.contains(&format!("const {name}: u32 = ")) {
            assert_eq!(wgsl_value(name), rust, "{name} drifted from the WGSL unit");
        }
    }
}
