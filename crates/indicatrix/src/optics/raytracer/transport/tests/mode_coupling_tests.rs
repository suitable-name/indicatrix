//! O<->e mode re-coupling at internal reflections.

use super::super::{
    super::{
        NUM_CHANNELS,
        camera::Ray,
        environment::{EnvironmentSource, LightingPreset},
        intersect::build_plane_soa,
        sampling::hash_u32,
        uniaxial_fresnel::{self, UniaxialFrame},
    },
    bounce::apply_internal_mode_coupling,
    inner::{build_ray_material_context, trace_spectral_ray_inner},
};
use crate::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
    optics::{materials::GemMaterial, polarization::StokesVector},
};
use glam::Vec3;

/// Builds the same `(k_hat, normal, frame)` shape `uniaxial_fresnel`'s own tests
/// use (`normal = -Z`, incidence in the XZ plane) so a `UniaxialFrame` can be built
/// directly from an incidence angle and optic axis, without a full bounce/geometry
/// setup.
fn frame_for(ang_deg: f32, c_axis: Vec3) -> (Vec3, Vec3, UniaxialFrame) {
    let theta = ang_deg.to_radians();
    let cos_i = theta.cos();
    let sin_i = theta.sin();
    let k_hat = Vec3::new(sin_i, 0.0, cos_i);
    let normal = Vec3::new(0.0, 0.0, -1.0);
    (
        k_hat,
        normal,
        UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i),
    )
}

/// `apply_internal_mode_coupling`'s relabeling probability uses the exact
/// closed-form `uniaxial_fresnel::internal_solve` Poynting-weighted split
/// `p_o = R_o / (R_o + R_e)` when the caller supplies one, rather than the
/// polarization-projection heuristic.
///
/// Mechanism-level check: for a moderate, non-degenerate uniaxial internal-
/// reflection geometry (25 degrees incidence, c-axis tilted off both the facet
/// normal and the incidence plane), compute `p_o` directly from `internal_solve`'s
/// own `R_o`/`R_e`, then check the empirical draw frequency over many seeds
/// reproduces that value (not a hard-coded 0.5) within binomial sampling error.
#[test]
fn internal_mode_coupling_draw_matches_exact_closed_form_split() {
    let lambdas = [589.3f32; NUM_CHANNELS];
    let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
    let ctx = build_ray_material_context(&zircon, lambdas, 0, true);
    let n_o = zircon.dispersion.evaluate(589.3);
    let n_e = zircon.extraordinary_index_at(589.3, n_o);

    let c_axis = Vec3::new(0.4, 0.5, 0.767_2).normalize();
    let (_, _, frame) = frame_for(25.0, c_axis);
    let sol = uniaxial_fresnel::internal_solve(n_o, n_o, n_e, c_axis, &frame, true);
    let ro_pow = sol.r_o.norm_sqr() * sol.flux_ro;
    let re_pow = sol.r_e.norm_sqr() * sol.flux_re;
    let p_o_exact = ro_pow / (ro_pow + re_pow).max(1e-12);

    let stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
    let mut extraordinary_count = 0u32;
    let trials = 20_000u32;
    for seed in 0..trials {
        if apply_internal_mode_coupling(
            &ctx,
            false,
            Vec3::X,
            Vec3::NEG_Z,
            &stokes,
            Some(p_o_exact),
            seed,
            3,
        ) {
            extraordinary_count += 1;
        }
    }
    let frac = f64::from(extraordinary_count) / f64::from(trials);
    let expected = f64::from(1.0 - p_o_exact);
    // 4-sigma binomial tolerance plus a floor for f32-vs-f64 conversion noise.
    let tol = 4.0f64.mul_add(
        (expected * (1.0 - expected) / f64::from(trials)).sqrt(),
        0.005,
    );
    assert!(
        (frac - expected).abs() < tol,
        "draw frequency should match the exact closed-form split p_o_exact={p_o_exact} \
         (expected extraordinary frac={expected}), got {frac} over {trials} trials \
         (tol={tol})"
    );
}

/// The exact split's real payoff over the old heuristic: at an incidence angle
/// between the ordinary and extraordinary modes' own critical angles, one mode is
/// in genuine TIR while the other only partially reflects -- the sharpest
/// asymmetry `R_o`/`R_e` can produce, which the old heuristic (which never looked
/// at either mode's reflectance) could not represent. Found by an angle sweep
/// rather than a hand-derived critical angle.
#[test]
fn internal_mode_coupling_draw_is_strongly_biased_near_a_modes_own_critical_angle() {
    let lambdas = [589.3f32; NUM_CHANNELS];
    let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
    let ctx = build_ray_material_context(&zircon, lambdas, 0, true);
    let n_o = zircon.dispersion.evaluate(589.3);
    let n_e = zircon.extraordinary_index_at(589.3, n_o);
    let c_axis = Vec3::new(0.4, 0.5, 0.767_2).normalize();

    let mut best_angle = 0.0f32;
    let mut best_p_o = 0.5f32;
    let mut best_skew = 0.0f32;
    let steps: u16 = 4000;
    for i in 0..=steps {
        let ang = 20.0f32.mul_add(f32::from(i) / f32::from(steps), 20.0);
        let (_, _, frame) = frame_for(ang, c_axis);
        let sol = uniaxial_fresnel::internal_solve(n_o, n_o, n_e, c_axis, &frame, true);
        let ro_pow = sol.r_o.norm_sqr() * sol.flux_ro;
        let re_pow = sol.r_e.norm_sqr() * sol.flux_re;
        let p_o = ro_pow / (ro_pow + re_pow).max(1e-12);
        let skew = (p_o - 0.5).abs();
        if skew > best_skew {
            best_skew = skew;
            best_angle = ang;
            best_p_o = p_o;
        }
    }
    assert!(
        best_skew > 0.45,
        "test premise: sweep should find a strongly asymmetric angle between the \
         two modes' own critical angles -- best found: angle={best_angle} \
         p_o={best_p_o} skew={best_skew}"
    );

    let stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
    let mut extraordinary_count = 0u32;
    let trials = 20_000u32;
    for seed in 0..trials {
        if apply_internal_mode_coupling(
            &ctx,
            false,
            Vec3::X,
            Vec3::NEG_Z,
            &stokes,
            Some(best_p_o),
            seed,
            7,
        ) {
            extraordinary_count += 1;
        }
    }
    let frac = f64::from(extraordinary_count) / f64::from(trials);
    let expected = f64::from(1.0 - best_p_o);
    let tol = 4.0f64.mul_add(
        (expected * (1.0 - expected) / f64::from(trials)).sqrt(),
        0.005,
    );
    assert!(
        (frac - expected).abs() < tol,
        "at angle={best_angle} (p_o_exact={best_p_o}), draw frequency should match \
         expected={expected}, got {frac} over {trials} trials (tol={tol})"
    );
}

/// Runs `samples` trials of `trace_spectral_ray_inner` at `ray`/`planes`/
/// `plane_soa`/`material`/`env`, seeded `seed_base + i`, once with
/// `enable_internal_mode_coupling` forced on and once forced off (same seed both
/// times), returning `(sum_with, sum_without)`. Split out of
/// `internal_mode_coupling_changes_zircon_render` purely to keep that test under
/// clippy's function-length lint -- both call sites there share this exact A/B
/// sweep, once for Zircon and once for the Diamond null check.
fn sum_with_and_without_mode_coupling(
    material: &GemMaterial,
    ray: Ray,
    planes: &[GpuFacetPlane],
    plane_soa: &crate::simd::PlanesSoA32,
    env: EnvironmentSource<'_>,
    samples: u32,
    seed_base: u32,
) -> (Vec3, Vec3) {
    let mut sum_with = Vec3::ZERO;
    let mut sum_without = Vec3::ZERO;
    for i in 0..samples {
        let seed = seed_base + i;
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        sum_with += trace_spectral_ray_inner(
            ray,
            planes,
            plane_soa,
            &[],
            material,
            12,
            env,
            seed,
            hero_rand,
            None,
            true,
            true,
            false,
            None,
        );
        sum_without += trace_spectral_ray_inner(
            ray,
            planes,
            plane_soa,
            &[],
            material,
            12,
            env,
            seed,
            hero_rand,
            None,
            false,
            true,
            false,
            None,
        );
    }
    (sum_with, sum_without)
}

/// The decisive measurement: render the same real Zircon material (the largest
/// `birefringence_delta` in the built-in catalogue) at the same ray and seeds,
/// averaged over many samples, with internal mode coupling forced on vs. off. Any
/// nonzero, reproducible difference is caused by exactly one thing: whether the
/// eigenmode governing internal TIR/reflect bounces is re-rolled after entry.
#[test]
fn internal_mode_coupling_changes_zircon_render() {
    let planes = StandardGemCuts::standard_round_brilliant();
    // Built once, reused across every call below -- `planes` is fixed for this test.
    let plane_soa = build_plane_soa(&planes);
    let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
    assert!(
        zircon.birefringence_delta.abs() > 0.01,
        "test assumes Zircon is strongly birefringent"
    );

    // Oblique ray (not aligned with c_axis == Y), 12 max_bounces -- produces real
    // internal TIR trains.
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let samples = 256u32;

    let (sum_with, sum_without) =
        sum_with_and_without_mode_coupling(&zircon, ray, &planes, &plane_soa, env, samples, 1000);
    let mean_with = sum_with / samples as f32;
    let mean_without = sum_without / samples as f32;
    let diff = (mean_with - mean_without).length();
    assert!(
        diff > 1e-4,
        "internal mode coupling should measurably change Zircon's rendered XYZ \
         (mean_with={mean_with:?}, mean_without={mean_without:?}, diff={diff})"
    );

    // Null check: the identical comparison for a CUBIC (isotropic) material must
    // show exactly zero difference, proving this is the `is_anisotropic` gate.
    let diamond = GemMaterial::by_name("Diamond").expect("Diamond must be a built-in material");
    let (sum_with_diamond, sum_without_diamond) =
        sum_with_and_without_mode_coupling(&diamond, ray, &planes, &plane_soa, env, samples, 1000);
    assert_eq!(
        sum_with_diamond, sum_without_diamond,
        "Diamond (cubic, is_anisotropic == false) must be bit-identical with the flag \
         either way -- the mechanism must not leak into isotropic materials"
    );
}
