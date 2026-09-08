//! The property the whole remote-offload design rests on: sample partitioning is
//! additive. Tracing samples `[0, 64)` for a pixel in one batch must sum to bit-for-bit
//! the same radiance as tracing `[0, 32)` and `[32, 64)` in two separate batches and
//! adding the results -- exactly what letting two different nodes each trace a disjoint
//! sample range and summing their contributions requires to be correct.
//!
//! This reproduces the EXACT per-sample seed derivation
//! `apps/indicatrix-cut/src/bridge/render_thread.rs` uses against the real
//! `trace_spectral_ray` (not a stand-in): the seed is a function of `(pixel_index,
//! sample_number)` alone, via `hash_u32`, so it depends only on the pixel and the
//! ABSOLUTE sample number, never on batch boundaries -- which is what makes any
//! partition of `[0, total_samples)` produce the same sum.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{
            Camera, HERO_WAVELENGTH_ROTATION_STREAM, LightingPreset,
            PIXEL_JITTER_X_ROTATION_STREAM, PIXEL_JITTER_Y_ROTATION_STREAM,
            cranley_patterson_rotate, hash_u32, low_discrepancy_base2, radical_inverse_base,
            trace_spectral_ray,
        },
    },
};

/// The fixed per-comparison scene `trace_one_sample`/`sum_samples` trace against --
/// bundled per `clippy::too_many_arguments`. Held constant across one whole
/// partition-correctness comparison; only `(x, y, sample_num)` varies per call.
struct SceneUnderTest<'a> {
    camera: &'a Camera,
    planes: &'a [indicatrix::geometry::GpuFacetPlane],
    material: &'a GemMaterial,
    max_bounces: u32,
    environment: indicatrix::optics::raytracer::EnvironmentSource<'a>,
    width: u32,
    height: u32,
}

/// Traces one sample for pixel `(x, y)` of `scene`, sample number `sample_num`
/// (absolute, not batch-relative) -- byte-for-byte the same seed, jitter, and
/// stratified hero-wavelength derivation as `render_frame_scanlines` in
/// `apps/indicatrix-cut/src/bridge/render_thread.rs`. `low_discrepancy_base2` reads only
/// `sample_num`, and the Cranley-Patterson rotation offsets read only
/// `global_pixel_idx`, so nothing here depends on batch boundaries or call order.
fn trace_one_sample(scene: &SceneUnderTest<'_>, x: u32, y: u32, sample_num: u32) -> Vec3 {
    let global_pixel_idx = y * scene.width + x;
    let seed =
        hash_u32(global_pixel_idx.wrapping_mul(0x9e37_79b9) ^ sample_num.wrapping_mul(0x85eb_ca6b));

    let rot_jx = low_discrepancy_base2(hash_u32(global_pixel_idx ^ PIXEL_JITTER_X_ROTATION_STREAM));
    let rot_jy = low_discrepancy_base2(hash_u32(global_pixel_idx ^ PIXEL_JITTER_Y_ROTATION_STREAM));
    let rot_hero =
        low_discrepancy_base2(hash_u32(global_pixel_idx ^ HERO_WAVELENGTH_ROTATION_STREAM));
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample_num, 3), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample_num, 5), rot_hero);

    let ray = scene.camera.generate_ray(
        x as f32,
        y as f32,
        scene.width as f32,
        scene.height as f32,
        jx,
        jy,
    );
    trace_spectral_ray(
        ray,
        scene.planes,
        scene.material,
        scene.max_bounces,
        scene.environment,
        seed,
        hero_rand,
        None,
    )
}

/// Asserts `a` and `b` agree to within a tight relative tolerance.
///
/// Not `assert_eq!`: floating-point addition is not associative, so summing the same
/// terms in a different grouping can differ in the last bit or two even when the
/// mathematical sums are identical. A real discrepancy (a seed depending on
/// batch-relative state, a dropped sample) would show up orders of magnitude larger
/// than float-rounding noise, so `1e-4` relative is tight enough to catch a real bug
/// while tolerant of summation-order rounding.
fn assert_vec3_approx_eq(a: Vec3, b: Vec3, msg: &str) {
    let diff = (a - b).abs();
    let scale = a.abs().max(b.abs()).max(Vec3::splat(1e-6));
    let rel = diff / scale;
    assert!(
        rel.max_element() < 1e-4,
        "{msg}: left={a:?} right={b:?} rel_diff={rel:?}"
    );
}

/// Sums `trace_one_sample` over `sample_range`, exactly what one worker node
/// accumulating its assigned batch of sample indices for one pixel would do.
fn sum_samples(
    scene: &SceneUnderTest<'_>,
    x: u32,
    y: u32,
    sample_range: std::ops::Range<u32>,
) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for sample_num in sample_range {
        sum += trace_one_sample(scene, x, y, sample_num);
    }
    sum
}

#[test]
fn batch_0_64_equals_batch_0_32_plus_batch_32_64() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Ruby").expect("Ruby is a built-in material");
    let camera = Camera::new(0.4, 0.3, 3.0, 45.0);
    let environment = LightingPreset::Daylight.studio(1.0, 0.85, 0.95);
    let width = 64;
    let height = 64;
    let max_bounces = 6;
    let scene = SceneUnderTest {
        camera: &camera,
        planes: &planes,
        material: &material,
        max_bounces,
        environment,
        width,
        height,
    };

    // Dead center (hits the gem), off-center (still inside the silhouette), and a
    // corner that misses the gem and only samples the background.
    for (x, y) in [(32, 32), (20, 40), (2, 2)] {
        let whole: Vec3 = sum_samples(&scene, x, y, 0..64);
        let first_half: Vec3 = sum_samples(&scene, x, y, 0..32);
        let second_half: Vec3 = sum_samples(&scene, x, y, 32..64);
        let split_sum = first_half + second_half;

        assert_vec3_approx_eq(
            whole,
            split_sum,
            &format!("pixel ({x}, {y}): sum over [0,64) must equal sum over [0,32) + [32,64)"),
        );
    }
}

#[test]
fn an_uneven_three_way_split_also_sums_exactly() {
    // [0,7) + [7,19) + [19,64): an uneven three-way partition, closer to how real
    // worker nodes with different throughput would divide up a sample budget.
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(-0.6, 0.5, 3.2, 40.0);
    let environment = LightingPreset::RingLights.studio(1.2, 0.5, 1.1);
    let width = 48;
    let height = 48;
    let max_bounces = 8;
    let x = 24;
    let y = 24;
    let scene = SceneUnderTest {
        camera: &camera,
        planes: &planes,
        material: &material,
        max_bounces,
        environment,
        width,
        height,
    };

    let whole = sum_samples(&scene, x, y, 0..64);
    let part_a = sum_samples(&scene, x, y, 0..7);
    let part_b = sum_samples(&scene, x, y, 7..19);
    let part_c = sum_samples(&scene, x, y, 19..64);

    assert_vec3_approx_eq(whole, part_a + part_b + part_c, "uneven three-way split");
}

#[test]
fn single_sample_batches_summed_one_at_a_time_still_match_the_whole_batch() {
    // Every sample computed in its own separate call, as if each went to a different
    // worker node -- most likely to catch any hidden batch-relative state.
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Sapphire").expect("Sapphire is a built-in material");
    let camera = Camera::new(0.0, 0.0, 3.0, 45.0);
    let environment = LightingPreset::DarkSpotlight.studio(1.5, 0.2, 0.6);
    let width = 32;
    let height = 32;
    let max_bounces = 4;
    let x = 16;
    let y = 16;
    let scene = SceneUnderTest {
        camera: &camera,
        planes: &planes,
        material: &material,
        max_bounces,
        environment,
        width,
        height,
    };

    let whole = sum_samples(&scene, x, y, 0..16);

    let mut one_at_a_time = Vec3::ZERO;
    for sample_num in 0..16 {
        one_at_a_time += sum_samples(&scene, x, y, sample_num..sample_num + 1);
    }

    assert_vec3_approx_eq(
        whole,
        one_at_a_time,
        "16 single-sample batches summed one at a time",
    );
}
