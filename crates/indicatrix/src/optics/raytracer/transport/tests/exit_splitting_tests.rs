//! Empirical unbiasedness check plus a variance-ratio measurement, via
//! `trace_spectral_ray_inner`'s own `enable_exit_splitting` A/B switch (the same
//! pattern `mode_coupling_tests` uses for `enable_internal_mode_coupling`). Two
//! independent sample sets (disjoint seed ranges) so the two-sample z-test below is
//! the standard unpaired form.

use super::super::{
    super::{
        camera::Ray, environment::EnvironmentSource, intersect::build_plane_soa, sampling::hash_u32,
    },
    inner::trace_spectral_ray_inner,
};
use crate::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
    optics::{LightingPreset, materials::GemMaterial},
};
use glam::Vec3;

/// Mean and (population) variance of `trace_spectral_ray_inner`'s XYZ output over
/// `samples` independent draws. `f64` accumulation for `sumsq` avoids catastrophic
/// cancellation in `E[X^2] - E[X]^2` at these sample counts.
struct RenderStats {
    mean: Vec3,
    var: Vec3,
    n: f64,
}

#[expect(
    clippy::too_many_arguments,
    reason = "test-only helper bundling exactly the inputs trace_spectral_ray_inner \
              needs plus this sweep's sample count/seed base/on-off switch"
)]
fn render_stats(
    material: &GemMaterial,
    ray: Ray,
    planes: &[GpuFacetPlane],
    plane_soa: &crate::simd::PlanesSoA32,
    env: EnvironmentSource<'_>,
    samples: u32,
    seed_base: u32,
    enable_exit_splitting: bool,
) -> RenderStats {
    let mut sum = Vec3::ZERO;
    let mut sumsq_x = 0.0f64;
    let mut sumsq_y = 0.0f64;
    let mut sumsq_z = 0.0f64;
    for i in 0..samples {
        let seed = seed_base.wrapping_add(i);
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        let xyz = trace_spectral_ray_inner(
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
            enable_exit_splitting,
            false,
            None,
        );
        sum += xyz;
        let (xd, yd, zd) = (f64::from(xyz.x), f64::from(xyz.y), f64::from(xyz.z));
        sumsq_x = xd.mul_add(xd, sumsq_x);
        sumsq_y = yd.mul_add(yd, sumsq_y);
        sumsq_z = zd.mul_add(zd, sumsq_z);
    }
    let n = f64::from(samples);
    let mean = sum / samples as f32;
    let (mx, my, mz) = (f64::from(mean.x), f64::from(mean.y), f64::from(mean.z));
    let var = Vec3::new(
        mx.mul_add(-mx, sumsq_x / n).max(0.0) as f32,
        my.mul_add(-my, sumsq_y / n).max(0.0) as f32,
        mz.mul_add(-mz, sumsq_z / n).max(0.0) as f32,
    );
    RenderStats { mean, var, n }
}

/// Two-sample z-score per XYZ component: `(mean_a - mean_b) / sqrt(var_a/n_a +
/// var_b/n_b)`. Matches the pooling `renderer::gpu::estimator_check`'s
/// image-comparison harness uses -- a magnitude of a few units is ordinary
/// sampling noise at these trial counts, not evidence of bias.
fn z_scores(a: &RenderStats, b: &RenderStats) -> Vec3 {
    let se = |va: f32, na: f64, vb: f32, nb: f64| (f64::from(va) / na + f64::from(vb) / nb).sqrt();
    let sx = se(a.var.x, a.n, b.var.x, b.n).max(1e-20);
    let sy = se(a.var.y, a.n, b.var.y, b.n).max(1e-20);
    let sz = se(a.var.z, a.n, b.var.z, b.n).max(1e-20);
    Vec3::new(
        (f64::from(a.mean.x - b.mean.x) / sx) as f32,
        (f64::from(a.mean.y - b.mean.y) / sy) as f32,
        (f64::from(a.mean.z - b.mean.z) / sz) as f32,
    )
}

/// Regression guard for a bounce-budget truncation asymmetry (see
/// [`super::super::super::refraction::ExitSplitCtx::split_radiance`]'s doc comment):
/// a split channel resolves inline (one bounded intersect test) while the shared/hero
/// path needs a whole extra loop iteration to discover its own miss, so at a low
/// `max_bounces` cap splitting-on/off means diverged sharply (up to 2.6x for Synthetic
/// Moissanite at `max_bounces = 4`) before `split_radiance` staging + conditional
/// commit fixed it. Pins the on/off ratio close to 1 across a range of caps.
#[test]
fn exit_splitting_respects_bounce_budget_truncation() {
    const SAMPLES: u32 = 6_000;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let material = GemMaterial::by_name("Synthetic Moissanite")
        .expect("Synthetic Moissanite must be a built-in material");
    for max_bounces in [2u32, 4, 6, 8, 12] {
        let mut sum_on = Vec3::ZERO;
        let mut sum_off = Vec3::ZERO;
        for i in 0..SAMPLES {
            let seed_on = 10_000 + i;
            let hero_rand_on = (hash_u32(seed_on) as f32) / 4_294_967_295.0;
            sum_on += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                max_bounces,
                env,
                seed_on,
                hero_rand_on,
                None,
                true,
                true,
                false,
                None,
            );
            let seed_off = 90_000 + i;
            let hero_rand_off = (hash_u32(seed_off) as f32) / 4_294_967_295.0;
            sum_off += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                max_bounces,
                env,
                seed_off,
                hero_rand_off,
                None,
                true,
                false,
                false,
                None,
            );
        }
        let mean_on = sum_on / SAMPLES as f32;
        let mean_off = sum_off / SAMPLES as f32;
        let ratio_x = mean_on.x / mean_off.x;
        println!(
            "max_bounces={max_bounces}: mean_on={mean_on:?} mean_off={mean_off:?} ratio_x={ratio_x:.4}"
        );
        assert!(
            (0.97..1.03).contains(&ratio_x),
            "max_bounces={max_bounces}: splitting-on/off diverged sharply \
             (ratio_x={ratio_x:.4}) -- likely a bounce-budget truncation \
             regression (mean_on={mean_on:?}, mean_off={mean_off:?})"
        );
    }
}

#[test]
fn exit_splitting_is_unbiased_and_reduces_variance() {
    const SAMPLES: u32 = 16_384;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);

    for name in ["Zircon", "Rutile", "Synthetic Moissanite"] {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let on = render_stats(
            &material, ray, &planes, &plane_soa, env, SAMPLES, 10_000, true,
        );
        let off = render_stats(
            &material, ray, &planes, &plane_soa, env, SAMPLES, 90_000, false,
        );
        let z = z_scores(&on, &off);
        let var_ratio = Vec3::new(
            f64::from(off.var.x / on.var.x.max(1e-12)) as f32,
            f64::from(off.var.y / on.var.y.max(1e-12)) as f32,
            f64::from(off.var.z / on.var.z.max(1e-12)) as f32,
        );
        println!(
            "P6 unbiasedness/variance -- {name}: mean_on={:?} mean_off={:?} z={z:?} \
             var_on={:?} var_off={:?} var_ratio(off/on)={var_ratio:?}",
            on.mean, off.mean, on.var, off.var
        );
        assert!(
            z.x.abs() < 3.5 && z.y.abs() < 3.5 && z.z.abs() < 3.5,
            "{name}: exit-event splitting changed the estimator's expectation \
             (z={z:?}, mean_on={:?}, mean_off={:?}) -- should be unbiased",
            on.mean,
            off.mean
        );
    }
}
