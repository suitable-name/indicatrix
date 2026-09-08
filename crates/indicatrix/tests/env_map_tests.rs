//! Tests for `indicatrix::renderer::env_map` -- CPU-side HDR environment-map importance
//! sampling. This module is deliberately self-contained and not wired into the tracer
//! (see the module docs on `indicatrix::renderer::env_map`), so these tests exercise it
//! directly rather than through any render path.
//!
//! No external RNG crate is used (indicatrix is deliberately dependency-light); a small
//! deterministic hash-based generator below stands in for one, using stratified
//! `(i + jitter) / n` sequences so every draw lands strictly inside `(0, 1)` -- never
//! exactly at `0.0` or `1.0` -- which matters at the poles: a direction sampled at
//! *exactly* `v == 0.0` sits precisely on the equirectangular singularity where
//! `sin(theta) == 0`, and dividing by that pdf would be `inf`, not a real bug in the
//! library.

use std::f32::consts::PI;

use glam::Vec3;
use indicatrix::renderer::env_map::{EnvironmentMap, rgb_to_spectral_radiance};

/// Cheap, deterministic, decent-quality hash (splitmix64-style finalizer) used only to
/// turn a counter into pseudorandom `f32`s in `(0, 1)` for these tests.
fn hash_to_unit(mut x: u64) -> f32 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    // Keep the low 24 bits (fits exactly in an f32 mantissa) and map to (0, 1),
    // excluding both endpoints.
    let bits = (x & 0x00FF_FFFF) as f32 / (0x0100_0000_u32 as f32);
    bits.mul_add(1.0 - 2.0 / 16_777_216.0, 1.0 / 16_777_216.0)
}

/// Two independent, decorrelated uniforms for sample index `i`.
fn stratified_pair(i: u64) -> (f32, f32) {
    (
        hash_to_unit(i.wrapping_mul(2)),
        hash_to_unit(
            i.wrapping_mul(2)
                .wrapping_add(1)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15),
        ),
    )
}

// ---------------------------------------------------------------------------------
// The white furnace test -- the single most important test in this file.
// ---------------------------------------------------------------------------------
//
// Set the environment to uniform radiance 1.0 everywhere and Monte-Carlo integrate
// `L(w)/pdf(w)` over directions drawn from `EnvironmentMap::sample`. Because `pdf` is
// kept in the standard, MIS-compatible convention of integrating to `1.0` over the
// *whole* sphere (`integral pdf(w) dw == 1`, `w` ranging over all 4*PI steradians --
// this exact convention is separately verified by `pdf_integrates_to_one_over_the_sphere`
// below, independently of the sampler), the raw Monte Carlo estimator
// `mean(L(w_i)/pdf(w_i))` is an unbiased estimator of `integral L(w) dw`, which for a
// constant `L == 1` is mathematically the sphere's own total solid angle, `4*PI`
// steradians -- not `1.0` -- *by definition*, regardless of whether the environment
// sampler has any bugs at all. That is not a subtlety this test tries to route around:
// it is exactly why `raw_mean` is asserted against `4*PI` below, and only the
// *solid-angle-normalized* average (`raw_mean / (4*PI)`, i.e. "what constant radiance
// would produce this total flux") is asserted against `1.0`. Reconstructing that
// normalized average as anything other than the true constant (1.0) is precisely what a
// wrong pdf normalization or a missing/misapplied `sin(theta)` row weighting produces --
// see `build_distribution`'s doc comment in `env_map.rs`.
#[test]
fn white_furnace_test_converges_to_the_true_uniform_radiance() {
    let env = EnvironmentMap::uniform(512, 256, [1.0, 1.0, 1.0]);

    let n: u64 = 400_000;
    let mut sum = 0.0f64;
    let mut max_abs_sample = 0.0f64;
    for i in 0..n {
        let (u0, u1) = stratified_pair(i);
        let (dir, rgb, pdf) = env.sample(u0, u1);
        assert!(
            pdf > 0.0,
            "a uniform furnace's pdf must be strictly positive everywhere it can sample"
        );
        assert!(pdf.is_finite(), "pdf must never be infinite");
        let l = rgb[0]; // R == G == B == 1.0 for this fixture
        assert!(
            (dir.length() - 1.0).abs() < 1e-4,
            "sampled direction must be unit length"
        );
        let estimate = f64::from(l) / f64::from(pdf);
        assert!(estimate.is_finite(), "L/pdf must never be non-finite");
        max_abs_sample = max_abs_sample.max(estimate);
        sum += estimate;
    }
    let raw_mean = sum / n as f64;
    let normalized = raw_mean / (4.0 * std::f64::consts::PI);

    eprintln!(
        "white furnace: N={n}  raw mean(L/pdf)={raw_mean:.6}  (expected 4*PI = {:.6})  normalized (raw_mean/4PI)={normalized:.6}  max single-sample estimate={max_abs_sample:.6}",
        4.0 * std::f64::consts::PI
    );

    assert!(
        4.0f64.mul_add(-std::f64::consts::PI, raw_mean).abs() < 0.02,
        "raw mean(L/pdf) should converge to the sphere's total solid angle 4*PI ~= {:.6} for a correctly \
         normalized full-sphere pdf and constant radiance 1.0; got {raw_mean:.6}. A value near PI, 2*PI, or \
         0.5*(4*PI) instead would indicate a missing factor of 2 (Jacobian) or a missing sin(theta) row weight.",
        4.0 * std::f64::consts::PI
    );
    assert!(
        (normalized - 1.0).abs() < 0.005,
        "solid-angle-normalized average should converge to the true uniform radiance 1.0; got {normalized:.6}"
    );
}

/// Independent numerical check that `pdf` integrates to `1.0` over the sphere, computed
/// by direct grid quadrature (not by drawing samples from the sampler at all) -- this is
/// deliberately a *different* method than the white furnace test so the two can't share
/// the same bug.
#[test]
fn pdf_integrates_to_one_over_the_sphere() {
    // A non-uniform map so this isn't a degenerate all-equal-pdf check: a smooth
    // gradient plus one brighter patch.
    let (width, height) = (64, 32);
    let mut pixels = vec![[0.1f32, 0.1, 0.1]; width * height];
    for y in 20..26 {
        for x in 10..18 {
            pixels[y * width + x] = [50.0, 40.0, 30.0];
        }
    }
    let env = EnvironmentMap::from_rgb(width, height, pixels).unwrap();

    // Fine lat-long quadrature grid, independent of `width`/`height` above.
    let (grid_theta, grid_phi) = (400usize, 800usize);
    let mut integral = 0.0f64;
    for i in 0..grid_theta {
        let theta = (i as f32 + 0.5) / grid_theta as f32 * PI;
        let sin_theta = theta.sin();
        let d_theta = PI / grid_theta as f32;
        for j in 0..grid_phi {
            let phi = (j as f32 + 0.5) / grid_phi as f32 * 2.0 * PI;
            let d_phi = 2.0 * PI / grid_phi as f32;
            let dir = Vec3::new(
                theta.sin() * phi.sin(),
                theta.cos(),
                theta.sin() * phi.cos(),
            );
            let pdf = env.pdf(dir);
            let d_omega = sin_theta * d_theta * d_phi;
            integral = f64::from(pdf).mul_add(f64::from(d_omega), integral);
        }
    }

    eprintln!("pdf integral over sphere (grid {grid_theta}x{grid_phi}) = {integral:.6}");
    assert!(
        (integral - 1.0).abs() < 0.01,
        "pdf must integrate to 1.0 over the sphere; got {integral:.6}"
    );
}

/// A small, very bright region in an otherwise dark map must capture the overwhelming
/// majority of importance-sampled directions -- the entire point of importance sampling
/// over uniform direction sampling.
#[test]
fn bright_spot_dominates_sampling() {
    let (width, height) = (128, 64);
    let mut pixels = vec![[0.001f32, 0.001, 0.001]; width * height];
    // A compact bright patch away from the poles (rows 30..34 out of 64, i.e. near the
    // equator where sin(theta) doesn't itself suppress it) and away from the u seam.
    let (bx0, bx1, by0, by1) = (60usize, 64usize, 30usize, 34usize);
    for y in by0..by1 {
        for x in bx0..bx1 {
            pixels[y * width + x] = [5000.0, 5000.0, 5000.0];
        }
    }
    let env = EnvironmentMap::from_rgb(width, height, pixels).unwrap();

    let n: u64 = 20_000;
    let mut hits = 0u64;
    for i in 0..n {
        let (u0, u1) = stratified_pair(i.wrapping_add(0x1234_5678));
        let (dir, _rgb, _pdf) = env.sample(u0, u1);
        let (u, v) = EnvironmentMap::direction_to_uv(dir);
        let col = ((u * width as f32) as usize).min(width - 1);
        let row = ((v * height as f32) as usize).min(height - 1);
        if (bx0..bx1).contains(&col) && (by0..by1).contains(&row) {
            hits += 1;
        }
    }

    let hit_rate = hits as f64 / n as f64;
    eprintln!("bright spot hit rate: {hits}/{n} = {hit_rate:.4}");
    assert!(
        hit_rate > 0.95,
        "expected the bright spot (covering {}/{} texels) to dominate sampling; got hit rate {hit_rate:.4}",
        (bx1 - bx0) * (by1 - by0),
        width * height
    );
}

#[test]
fn sampled_directions_are_unit_length() {
    let env = EnvironmentMap::uniform(64, 32, [1.0, 2.0, 3.0]);
    for i in 0..2000u64 {
        let (u0, u1) = stratified_pair(i);
        let (dir, _, _) = env.sample(u0, u1);
        assert!(
            (dir.length() - 1.0).abs() < 1e-4,
            "direction {dir:?} is not unit length"
        );
    }
}

#[test]
fn uv_direction_mapping_round_trips_including_poles_and_seam() {
    // Interior points, away from poles/seam: full uv round-trip.
    for &(u, v) in &[
        (0.0f32, 0.25),
        (0.1, 0.5),
        (0.37, 0.6),
        (0.5, 0.5),
        (0.7, 0.33),
        (0.9999, 0.75),
    ] {
        let dir = EnvironmentMap::uv_to_direction(u, v);
        assert!((dir.length() - 1.0).abs() < 1e-5);
        let (u2, v2) = EnvironmentMap::direction_to_uv(dir);
        assert!(
            (u - u2).abs() < 1e-3 || (u - u2).abs() > 1.0 - 1e-3,
            "u round-trip failed: {u} -> {u2}"
        );
        assert!((v - v2).abs() < 1e-4, "v round-trip failed: {v} -> {v2}");
    }

    // Poles: u is undefined there (every u maps to the same point), so only check that
    // the direction round-trips to the correct pole and v lands exactly at the pole.
    for &v in &[0.0f32, 1.0] {
        let dir = EnvironmentMap::uv_to_direction(0.37, v);
        let expected_y = if v == 0.0 { 1.0 } else { -1.0 };
        assert!(
            (dir.y - expected_y).abs() < 1e-5,
            "pole direction should be +-Y, got {dir:?}"
        );
        let (_, v2) = EnvironmentMap::direction_to_uv(dir);
        assert!(
            (v - v2).abs() < 1e-5,
            "pole v should round-trip exactly: {v} -> {v2}"
        );
    }

    // Seam: u==0 and u->1 should map to (nearly) the same direction.
    let d0 = EnvironmentMap::uv_to_direction(0.0, 0.5);
    let d1 = EnvironmentMap::uv_to_direction(0.999_999, 0.5);
    assert!(
        (d0 - d1).length() < 1e-3,
        "the u=0/u=1 seam should be continuous: {d0:?} vs {d1:?}"
    );
}

#[test]
fn direct_direction_to_uv_roundtrip_for_axis_aligned_directions() {
    for dir in [
        Vec3::X,
        Vec3::Y,
        -Vec3::Y,
        Vec3::Z,
        -Vec3::Z,
        -Vec3::X,
        Vec3::new(1.0, 1.0, 1.0).normalize(),
    ] {
        let (u, v) = EnvironmentMap::direction_to_uv(dir);
        assert!((0.0..1.0).contains(&u), "u out of range: {u}");
        assert!((0.0..=1.0).contains(&v), "v out of range: {v}");
        let back = EnvironmentMap::uv_to_direction(u, v);
        assert!(
            (back - dir).length() < 1e-4,
            "{dir:?} -> ({u},{v}) -> {back:?} did not round-trip"
        );
    }
}

// ---------------------------------------------------------------------------------
// Degenerate inputs: must not panic or produce NaN.
// ---------------------------------------------------------------------------------

#[test]
fn all_black_map_falls_back_to_uniform_sampling_without_nan() {
    let env = EnvironmentMap::from_rgb(16, 8, vec![[0.0, 0.0, 0.0]; 16 * 8]).unwrap();
    for i in 0..500u64 {
        let (u0, u1) = stratified_pair(i);
        let (dir, rgb, pdf) = env.sample(u0, u1);
        assert!(dir.is_finite() && !dir.x.is_nan());
        assert!(pdf.is_finite() && pdf >= 0.0);
        assert_eq!(rgb, [0.0, 0.0, 0.0]);
    }
    let pdf_at_arbitrary_dir = env.pdf(Vec3::new(0.3, 0.4, 0.5).normalize());
    assert!(pdf_at_arbitrary_dir.is_finite() && !pdf_at_arbitrary_dir.is_nan());
}

#[test]
fn one_by_one_map_does_not_panic() {
    let env = EnvironmentMap::from_rgb(1, 1, vec![[2.0, 3.0, 4.0]]).unwrap();
    for i in 0..200u64 {
        let (u0, u1) = stratified_pair(i);
        let (dir, rgb, pdf) = env.sample(u0, u1);
        assert!(dir.is_finite());
        assert!(pdf.is_finite() && pdf >= 0.0);
        assert_eq!(rgb, [2.0, 3.0, 4.0]);
    }
    assert_eq!(env.radiance_rgb(Vec3::Y), [2.0, 3.0, 4.0]);
}

#[test]
fn single_nonzero_texel_does_not_panic_or_produce_nan() {
    let width = 32;
    let height = 16;
    let mut pixels = vec![[0.0f32, 0.0, 0.0]; width * height];
    pixels[(8 * width) + 5] = [7.0, 8.0, 9.0];
    let env = EnvironmentMap::from_rgb(width, height, pixels).unwrap();
    for i in 0..1000u64 {
        let (u0, u1) = stratified_pair(i);
        let (dir, rgb, pdf) = env.sample(u0, u1);
        assert!(dir.is_finite());
        assert!(pdf.is_finite() && pdf >= 0.0);
        assert!(rgb.iter().all(|c| c.is_finite()));
    }
}

#[test]
fn dimension_mismatch_and_zero_sized_are_reported_as_errors_not_panics() {
    assert!(EnvironmentMap::from_rgb(4, 4, vec![[0.0, 0.0, 0.0]; 10]).is_err());
    assert!(EnvironmentMap::from_rgb(0, 4, vec![]).is_err());
    assert!(EnvironmentMap::from_rgb(4, 0, vec![]).is_err());
}

// ---------------------------------------------------------------------------------
// RGB -> spectrum reconstruction sanity (thorough unit coverage lives in
// `renderer::env_map_spectrum`'s own `#[cfg(test)]` module; this is just an
// integration-level smoke test through the re-exported public function).
// ---------------------------------------------------------------------------------

#[test]
fn rgb_to_spectral_radiance_is_finite_and_nonnegative_across_the_visible_range() {
    for lambda in (380..=780).step_by(20) {
        let v = rgb_to_spectral_radiance([0.8, 0.2, 0.05], lambda as f32);
        assert!(v.is_finite() && v >= 0.0);
    }
}

#[test]
fn radiance_at_matches_rgb_to_spectral_radiance_of_the_bilinear_lookup() {
    let env = EnvironmentMap::uniform(8, 4, [0.5, 0.25, 0.75]);
    let dir = Vec3::new(0.2, 0.6, 0.7).normalize();
    let expected = rgb_to_spectral_radiance([0.5, 0.25, 0.75], 550.0);
    let got = env.radiance_at(dir, 550.0);
    assert!((expected - got).abs() < 1e-5);
}
