//! Empirical mean and variance comparison of exit-event splitting, via
//! `trace_spectral_ray_inner`'s own `enable_exit_splitting` A/B switch (the same
//! pattern `mode_coupling_tests` uses for `enable_internal_mode_coupling`). Both arms
//! trace the SAME `(seed, hero_rand)` draws, so the paired per-sample differences carry
//! only what the exit split changes; the shared path noise (which hero was drawn, which
//! branch each bounce took) cancels out of them.

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

/// The per-sample XYZ of the splitting-on and splitting-off arms, index for index under
/// the same `(seed, hero_rand)`.
struct PairedArms {
    on: Vec<Vec3>,
    off: Vec<Vec3>,
}

impl PairedArms {
    /// One XYZ component of both arms, widened to `f64` for the moment sums.
    fn component(&self, axis: usize) -> (Vec<f64>, Vec<f64>) {
        let pick = |arm: &Vec<Vec3>| arm.iter().map(|xyz| f64::from(xyz[axis])).collect();
        (pick(&self.on), pick(&self.off))
    }
}

/// Traces `samples` paired draws of both arms (`max_bounces` 12, no NEE).
fn paired_arms(
    material: &GemMaterial,
    ray: Ray,
    planes: &[GpuFacetPlane],
    plane_soa: &crate::simd::PlanesSoA32,
    env: EnvironmentSource<'_>,
    samples: u32,
) -> PairedArms {
    let mut arms = PairedArms {
        on: Vec::with_capacity(samples as usize),
        off: Vec::with_capacity(samples as usize),
    };
    for i in 0..samples {
        let seed = 10_000 + i;
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        let trace = |split: bool| {
            trace_spectral_ray_inner(
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
                split,
                false,
                None,
            )
        };
        arms.on.push(trace(true));
        arms.off.push(trace(false));
    }
    arms
}

/// Mean and population variance of `v`, plus the standard error of that variance
/// estimate, `sqrt((m4 - var^2) / n)` from the fourth central moment. For a heavy-tailed
/// radiance (single Rutile samples reach a hundred times the mean under the ring lights)
/// that error is the only honest scale for "did the variance change".
fn moments(v: &[f64]) -> (f64, f64, f64) {
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let m2 = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let m4 = v.iter().map(|x| (x - mean).powi(4)).sum::<f64>() / n;
    (mean, m2, (m2.mul_add(-m2, m4) / n).max(0.0).sqrt())
}

/// The mean of the paired differences `a - b` and its standard error `sqrt(var(d) / n)`.
fn paired_mean_diff(a: &[f64], b: &[f64]) -> (f64, f64) {
    let d: Vec<f64> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    let (mean, var, _) = moments(&d);
    (mean, (var / d.len() as f64).sqrt())
}

/// `var(a) - var(b)` and the standard error of that difference, from the paired
/// differences of the squared deviations about each arm's own mean.
fn paired_var_diff(a: &[f64], b: &[f64]) -> (f64, f64) {
    let (mean_a, _, _) = moments(a);
    let (mean_b, _, _) = moments(b);
    let d: Vec<f64> = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let (da, db) = (x - mean_a, y - mean_b);
            da.mul_add(da, -db * db)
        })
        .collect();
    let (diff, var, _) = moments(&d);
    (diff, (var / d.len() as f64).sqrt())
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
            // Paired seeds: the SAME (seed, hero_rand) drives both the splitting-on and
            // splitting-off trace of a given sample, so per-sample stochastic path
            // noise (which channel/direction a bounce happened to draw) cancels out of
            // the on/off ratio instead of adding to it -- only a genuine bias from the
            // bounce-budget interaction with the switch state should show up here.
            let seed = 10_000 + i;
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            sum_on += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                max_bounces,
                env,
                seed,
                hero_rand,
                None,
                true,
                true,
                false,
                None,
            );
            sum_off += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                max_bounces,
                env,
                seed,
                hero_rand,
                None,
                true,
                false,
                false,
                None,
            );
        }
        let mean_on = sum_on / SAMPLES as f32;
        let mean_off = sum_off / SAMPLES as f32;
        let ratio = Vec3::new(
            mean_on.x / mean_off.x,
            mean_on.y / mean_off.y,
            mean_on.z / mean_off.z,
        );
        println!(
            "max_bounces={max_bounces}: mean_on={mean_on:?} mean_off={mean_off:?} ratio={ratio:?}"
        );
        for (label, r) in [("X", ratio.x), ("Y", ratio.y), ("Z", ratio.z)] {
            assert!(
                (0.97..1.03).contains(&r),
                "max_bounces={max_bounces}: splitting-on/off diverged sharply on {label} \
                 (ratio={r:.4}) -- likely a bounce-budget truncation regression \
                 (mean_on={mean_on:?}, mean_off={mean_off:?})"
            );
        }
    }
}

/// The split estimator must keep the expectation and must not make the Y (luminance)
/// channel noisier: Y is the channel every visible render weights by (CIE luminous
/// efficiency).
///
/// Means: the paired per-sample differences may move each XYZ mean by one percent plus
/// 3.5 standard errors. Both arms ride every companion channel on the hero's geometry
/// while its own refracted direction stays within `DIRECTION_MATCH_COS_TOL` of the
/// hero's, and splitting leans on that approximation harder (a companion contributes
/// past the exit from every hero in its family, not only from the heroes whose exit
/// direction it shares), so the arms differ by a small systematic amount that scales
/// with the tolerance: at 1,048,576 paired draws Rutile reads +0.4% in X and Y and -0.9%
/// in Z, Zircon and Synthetic Moissanite +0.2 to +0.7%, Diamond nothing; a tolerance of
/// 1e-7 shrinks Rutile to +0.1% in every component (and the variance benefit with it),
/// 1e-5 widens it to +2.4% / -2.3%. The one-percent allowance covers that, a true
/// accounting error would not stay under it.
///
/// Variance: the bound is statistical, not a fixed ratio. With splitting on, every
/// companion channel that is still alive at the exit adds its own environment lookup, a
/// second correlated estimate folded into the same sample, so the Y variance drops
/// clearly for Zircon and Synthetic Moissanite (by 30% and 80% at these counts). Rutile's
/// paths are heavy-tailed under the ring lights and its variance estimate carries a 15%
/// standard error even at 65,536 samples; read as an unpaired ratio it swung between 0.59
/// and 0.89 from one seed range to the next, while the paired difference sits within one
/// standard error of zero. Only a regression that multiplies the variance (the
/// bounce-budget asymmetry guarded above was 2.6x) can fail the three-standard-error
/// bound.
#[test]
fn exit_splitting_keeps_the_mean_within_a_percent_and_does_not_add_variance() {
    const SAMPLES: u32 = 65_536;
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
        let arms = paired_arms(&material, ray, &planes, &plane_soa, env, SAMPLES);
        for (axis, tag) in ["X", "Y", "Z"].iter().enumerate() {
            let (on, off) = arms.component(axis);
            let (diff, se) = paired_mean_diff(&on, &off);
            let (mean_off, _, _) = moments(&off);
            println!(
                "exit splitting -- {name} {tag}: mean off {mean_off:.5}, on - off {diff:+.3e} \
                 (paired se {se:.1e})"
            );
            assert!(
                diff.abs() <= 3.5f64.mul_add(se, 0.01 * mean_off.abs()),
                "{name}: exit-event splitting moved the {tag} mean by {diff:+.3e} against \
                 {mean_off:.5} (paired standard error {se:.1e}): more than one percent plus \
                 noise"
            );
        }
        let (on_y, off_y) = arms.component(1);
        let (_, var_on, se_on) = moments(&on_y);
        let (_, var_off, se_off) = moments(&off_y);
        let (diff, se_diff) = paired_var_diff(&on_y, &off_y);
        println!(
            "exit splitting -- {name}: Y variance on {var_on:.4e} (se {se_on:.1e}), off \
             {var_off:.4e} (se {se_off:.1e}); on - off {diff:.3e} (se {se_diff:.1e})"
        );
        assert!(
            diff <= 3.0 * se_diff,
            "{name}: exit-event splitting raised the Y variance by {diff:.3e}, more than \
             three standard errors ({se_diff:.1e}) of the paired estimate \
             (var_on={var_on:.4e}, var_off={var_off:.4e})"
        );
    }
}
