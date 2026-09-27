//! Tests for [`henyey_greenstein_phase`], [`sample_henyey_greenstein_direction`],
//! [`maybe_scatter_or_extinguish`], and the `scattering_sigma_s` gate in
//! `trace_spectral_ray_inner`'s bounce loop.

use super::{
    super::{
        NUM_CHANNELS,
        absorption::channel_absorption_alphas,
        camera::Camera,
        color::cie_1931_cmf,
        environment::{
            EnvironmentSource, LightingPreset, environment_nee_pdf, sample_environment_for_nee,
        },
        refraction::RayMaterialContext,
        sampling::{DISTANCE_SAMPLE_STREAM, hash_u32},
        transport::trace_spectral_ray,
    },
    balance_heuristic, henyey_greenstein_phase, maybe_scatter_or_extinguish,
    sample_henyey_greenstein_direction,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, polarization::StokesVector},
    renderer::env_map::{EnvironmentMap, rgb_to_spectral_radiance},
};
use glam::Vec3;

/// [`henyey_greenstein_phase`] must integrate to `1.0` over the full sphere for any
/// `g`, via dense quadrature over `cos_theta` (no azimuthal dependence, so the
/// solid-angle integral reduces to `2*pi * integral_{-1}^{1} phase(mu) dmu`).
#[test]
fn hg_phase_integrates_to_one_over_the_sphere() {
    for g in [-0.8f32, -0.3, 0.0, 0.3, 0.7, 0.95] {
        let steps = 200_000u32;
        let dmu = 2.0 / f64::from(steps);
        let mut integral = 0.0f64;
        for i in 0..steps {
            let mu = f64::mul_add(f64::from(i) + 0.5, dmu, -1.0);
            integral = f64::mul_add(
                f64::from(henyey_greenstein_phase(mu as f32, g)),
                dmu,
                integral,
            );
        }
        integral *= 2.0 * std::f64::consts::PI;
        assert!(
            (integral - 1.0).abs() < 1e-3,
            "HG phase function should integrate to 1.0 over the sphere for g={g}, got {integral}"
        );
    }
}

/// `henyey_greenstein_phase(1.0, g)` (exact forward direction) must strictly
/// increase with `g` for `g > 0` -- pins the sign convention ("positive g
/// forward-scatters") against the phase function itself, independent of the sampler.
#[test]
fn hg_phase_is_more_forward_peaked_for_larger_positive_g() {
    let p0 = henyey_greenstein_phase(1.0, 0.0);
    let p5 = henyey_greenstein_phase(1.0, 0.5);
    let p9 = henyey_greenstein_phase(1.0, 0.9);
    assert!(
        p0 < p5 && p5 < p9,
        "forward-direction phase value should strictly increase with g: p(g=0)={p0}, \
         p(g=0.5)={p5}, p(g=0.9)={p9}"
    );
}

/// The decisive check for hazard 3: [`sample_henyey_greenstein_direction`]'s mean
/// `cos_theta` must converge to `g` exactly -- the closed-form mean of the HG
/// distribution. A sign error, swapped `u1`/`u2`, or wrong CDF inversion would
/// converge to the wrong mean and be caught here.
#[test]
fn hg_sampling_mean_cos_theta_converges_to_g() {
    let forward = Vec3::new(0.2, 0.6, 0.77).normalize();
    for g in [-0.7f32, -0.2, 0.0, 0.4, 0.85] {
        let trials = 400_000u32;
        let mut sum = 0.0f64;
        for i in 0..trials {
            let u1 = (hash_u32(i ^ 0x1111_1111) as f32) / 4_294_967_295.0;
            let u2 = (hash_u32(i ^ 0x2222_2222) as f32) / 4_294_967_295.0;
            let dir = sample_henyey_greenstein_direction(u1, u2, g, forward);
            sum += f64::from(dir.dot(forward));
        }
        let mean = (sum / f64::from(trials)) as f32;
        assert!(
            (mean - g).abs() < 0.01,
            "mean cos_theta for g={g} should converge to g itself, got {mean} over {trials} trials"
        );
    }
}

/// Sampled directions must stay unit-length and finite across the full `g` range,
/// including at the isotropic branch threshold.
#[test]
fn hg_sampling_always_produces_finite_unit_directions() {
    let forward = Vec3::new(-0.3, 0.1, 0.94).normalize();
    for g in [-0.999f32, -0.5, -1e-3, 0.0, 1e-3, 0.5, 0.999] {
        for i in 0..2000u32 {
            let u1 = (hash_u32(i ^ 0x3333_3333) as f32) / 4_294_967_295.0;
            let u2 = (hash_u32(i ^ 0x4444_4444) as f32) / 4_294_967_295.0;
            let dir = sample_henyey_greenstein_direction(u1, u2, g, forward);
            assert!(
                dir.is_finite(),
                "non-finite direction for g={g}, i={i}: {dir:?}"
            );
            assert!(
                (dir.length() - 1.0).abs() < 1e-4,
                "non-unit direction for g={g}, i={i}: {dir:?} (len={})",
                dir.length()
            );
        }
    }
}

fn round_brilliant_colourless_scattering_material(sigma_s: f32, g: f32) -> GemMaterial {
    // Colourless, non-dispersive, cubic -- isolates the scattering estimator from
    // the chromatic absorption/dispersion machinery.
    GemMaterial::new_custom("scattering furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_scattering(sigma_s, g)
}

/// Non-negotiable regression guard: a material with `scattering_sigma_s == 0.0`
/// reached via `GemMaterial`'s plain default must trace bit-identically to the
/// same material with scattering explicitly set to `(0.0, g)` for several `g` --
/// proving the `scattering_sigma_s > 0.0` gate disables the new code path, since
/// the estimator is not guaranteed to reduce to the old one algebraically at
/// `sigma_s=0` (see [`maybe_scatter_or_extinguish`] hazard 2).
#[test]
fn default_off_scattering_is_bit_identical_regardless_of_g() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material_default = GemMaterial::ruby();
    assert!(
        material_default.scattering_sigma_s <= 0.0,
        "test premise: Ruby's own built-in scattering_sigma_s must be 0.0"
    );
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let env = || LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

    for g in [-0.6f32, 0.0, 0.4, 0.95] {
        let material_explicit_zero = material_default.clone().with_scattering(0.0, g);
        for iy in 0..6usize {
            for ix in 0..6usize {
                let ray = camera.generate_ray(ix as f32, iy as f32, 6.0, 6.0, 0.5, 0.5);
                for s in 0..4u32 {
                    let pixel_id = (iy as u32) * 6 + (ix as u32);
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x1357_9BDF));
                    let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                    let a = trace_spectral_ray(
                        ray,
                        &planes,
                        &material_default,
                        10,
                        env(),
                        seed,
                        hero_rand,
                        None,
                    );
                    let b = trace_spectral_ray(
                        ray,
                        &planes,
                        &material_explicit_zero,
                        10,
                        env(),
                        seed,
                        hero_rand,
                        None,
                    );
                    assert_eq!(
                        a.to_array(),
                        b.to_array(),
                        "sigma_s=0.0 (default) vs sigma_s=0.0 (explicit, g={g}) must be \
                         BIT identical at pixel ({ix},{iy}) sample {s}"
                    );
                }
            }
        }
    }
}

/// The decisive check: a lossless scattering medium (`sigma_a == 0`, `sigma_s > 0`)
/// immersed in a spatially uniform environment must still render at exactly that
/// environment's own radiance -- scattering redirects energy, it must not create
/// or destroy it. Mirrors `tests/raytracer_tests.rs`'s
/// `frosted_girdle_white_furnace_energy_conservation_still_holds`.
#[test]
fn lossless_scattering_white_furnace_energy_conservation_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 96;
    const GRID: usize = 12;
    const TOLERANCE: f32 = 0.08; // generous: CPU-only unit test sample budget

    let planes = StandardGemCuts::standard_round_brilliant();
    let material = round_brilliant_colourless_scattering_material(1.2, 0.4);
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES_PER_PIXEL {
                let pixel_id = (iy as u32) * (GRID as u32) + (ix as u32);
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x5CA1_AB1E));
                sum += trace_spectral_ray(
                    ray,
                    &planes,
                    &material,
                    16,
                    EnvironmentSource::HdrMap(&env_map),
                    seed,
                    (hash_u32(seed) as f32) / 4_294_967_295.0,
                    None,
                );
                count += 1;
            }
        }
    }
    let mean = sum / count as f32;

    let mut target = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        let spec = rgb_to_spectral_radiance([L0, L0, L0], lambda);
        target += cie_1931_cmf(lambda) * spec;
    }
    target /= 106.856;

    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean.x, target.x),
        rel_err(mean.y, target.y),
        rel_err(mean.z, target.z),
    );
    println!(
        "[lossless-scattering furnace] mean={mean:?} target={target:?} rel_err=({ex:.4}, {ey:.4}, {ez:.4}) over {count} samples"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "a lossless scattering medium should still converge to the uniform \
         environment's own radiance (mean={mean:?}, target={target:?}, \
         rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
    );
}

/// The scattering estimator must actually redistribute energy directionally, not
/// just pass an energy-conservation check by coincidence: a strongly forward-biased
/// scattering medium should measurably change a transmissive scene's face-up
/// appearance relative to `sigma_s = 0`.
#[test]
fn scattering_measurably_changes_face_up_appearance() {
    const SAMPLES_PER_PIXEL: u32 = 96;
    const GRID: usize = 14;

    let planes = StandardGemCuts::standard_round_brilliant();
    let clear = GemMaterial::diamond();
    assert!(clear.scattering_sigma_s <= 0.0);
    let hazy = clear.clone().with_scattering(1.5, 0.3);
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let env = || LightingPreset::RingLights.studio(1.0, 0.85, 0.95);

    let mut sum_clear = Vec3::ZERO;
    let mut sum_hazy = Vec3::ZERO;
    let mut count = 0u32;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES_PER_PIXEL {
                let pixel_id = (iy as u32) * (GRID as u32) + (ix as u32);
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x0BAD_F00D));
                let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                sum_clear +=
                    trace_spectral_ray(ray, &planes, &clear, 12, env(), seed, hero_rand, None);
                sum_hazy +=
                    trace_spectral_ray(ray, &planes, &hazy, 12, env(), seed, hero_rand, None);
                count += 1;
            }
        }
    }
    let mean_clear = sum_clear / count as f32;
    let mean_hazy = sum_hazy / count as f32;
    let delta_y = (mean_hazy.y - mean_clear.y).abs();
    let relative_change = delta_y / mean_clear.y.max(1e-6);
    println!(
        "[scattering face-up] clear Y={:.5} hazy Y={:.5} delta_y={:.5} ({:.2}%) over {count} samples",
        mean_clear.y,
        mean_hazy.y,
        delta_y,
        100.0 * relative_change
    );
    assert!(
        relative_change > 0.01,
        "a forward-scattering inclusion medium should measurably change face-up \
         brightness (>1%), not render identically to a clear stone -- got {:.4}% \
         (clear Y={:.5}, hazy Y={:.5})",
        100.0 * relative_change,
        mean_clear.y,
        mean_hazy.y
    );
}

/// [`maybe_scatter_or_extinguish`]'s hero channel must reproduce the textbook
/// single-scattering-albedo/unity-survival identities (hazard 2) exactly, checked
/// directly rather than only inferred from the furnace test above.
#[test]
fn hero_channel_weight_matches_albedo_and_unity_survival_identities() {
    let material = round_brilliant_colourless_scattering_material(0.8, 0.0);
    let alphas = [0.0f32; NUM_CHANNELS]; // sigma_a == 0 for this material, every channel
    let sigma_t_hero = material.scattering_sigma_s; // sigma_a == 0 for this material
    let albedo = material.scattering_sigma_s / sigma_t_hero; // == 1.0 (lossless)

    let mut scatter_trials = 0u32;
    let mut survive_trials = 0u32;
    for seed in 0..4000u32 {
        let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let mut path_pdf = [1.0f32; NUM_CHANNELS];
        let hit_t = 1.0f32; // fixed boundary distance
        let outcome = maybe_scatter_or_extinguish(
            &alphas,
            material.scattering_sigma_s,
            material.scattering_g,
            0,
            Vec3::Z,
            hit_t,
            1.0,
            seed,
            0,
            &mut stokes,
            &mut path_pdf,
        );
        if outcome.is_some() {
            scatter_trials += 1;
            assert!(
                (stokes[0].intensity() - albedo).abs() < 1e-4,
                "hero channel's scatter-branch weight should equal the single-scattering \
                 albedo sigma_s/sigma_t = {albedo} exactly (lossless), got {}",
                stokes[0].intensity()
            );
        } else {
            survive_trials += 1;
            assert!(
                (stokes[0].intensity() - 1.0).abs() < 1e-5,
                "hero channel's no-scatter-branch weight should be exactly 1.0 \
                 (unity-survival identity), got {}",
                stokes[0].intensity()
            );
        }
    }
    assert!(
        scatter_trials > 100 && survive_trials > 100,
        "test premise: both branches should fire many times at sigma_t*hit_t = {} \
         (scatter={scatter_trials}, survive={survive_trials})",
        sigma_t_hero * 1.0
    );
}

/// A companion (non-hero) channel's weight, in both branches, against an
/// independently-derived closed form -- not merely re-checking the implementation
/// against itself. Uses two channels with deliberately different chromatic
/// absorption (Tourmaline's o-ray vs. e-ray at 550nm/430nm) so the hero-vs-companion
/// divergence actually exercises the chromatic term, unlike a colourless material
/// where `sigma_t_k == sigma_t_hero` for every k would hide a bug that swaps the
/// shared hero-pdf denominator for a per-channel one.
///
/// Closed forms (independently re-derived, not copied from the implementation):
/// - no-scatter branch: `weight_k = exp(-(sigma_t_k - sigma_t_hero) * hit_t)`
/// - scatter branch: `weight_k = (sigma_s / sigma_t_hero) * exp(-(sigma_t_k -
///   sigma_t_hero) * t_free)`, recovered by re-deriving `t_free` from the SAME
///   `dist_rand` the function drew (`t_free = -ln(1 - dist_rand) / sigma_t_hero`)
///   using a fixed `rng_seed`/`bounce` so the test can predict the exact draw.
#[test]
fn companion_channel_weight_matches_independently_derived_closed_form() {
    let material = GemMaterial::by_name("Tourmaline")
        .expect("\"Tourmaline\" must be a built-in material")
        .with_scattering(0.8, 0.0);
    let lambda_hero = 550.0f32;
    let lambda_companion = 430.0f32; // strongly dichroic vs. 550nm, see module doc comment
    let mat_ctx = RayMaterialContext {
        material: &material,
        lambdas: [
            lambda_hero,
            lambda_companion,
            lambda_hero,
            lambda_hero,
            lambda_hero,
            lambda_hero,
            lambda_hero,
            lambda_hero,
        ],
        hero_idx: 0,
        c_axis: material.c_axis,
        is_anisotropic: true,
        enable_internal_mode_coupling: true,
    };
    let cache = super::super::refraction::build_ray_wavelength_cache(&mat_ctx);
    let ray_dir = Vec3::new(0.3, 0.9, 0.3).normalize();
    let sigma_s = material.scattering_sigma_s;

    // Uses the real `channel_absorption_alphas` for alpha_hero/alpha_companion --
    // legitimate here since this test checks the downstream weight formula, not
    // pleochroic absorption itself.
    let stokes_probe = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
    let alphas = channel_absorption_alphas(&mat_ctx, &cache, Vec3::ZERO, ray_dir, &stokes_probe);
    assert!(
        (alphas[0] - alphas[1]).abs() > 1e-4,
        "test premise: hero (550nm) and companion (430nm) must have genuinely \
         different alpha (got {} vs {})",
        alphas[0],
        alphas[1]
    );
    let sigma_t_hero = alphas[0] + sigma_s;
    let sigma_t_companion = alphas[1] + sigma_s;

    let mut no_scatter_checked = false;
    let mut scatter_checked = false;
    for seed in 0..3000u32 {
        let hit_t = 0.9f32;
        let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let mut path_pdf = [1.0f32; NUM_CHANNELS];
        let outcome = maybe_scatter_or_extinguish(
            &alphas,
            sigma_s,
            material.scattering_g,
            0,
            ray_dir,
            hit_t,
            1.0,
            seed,
            0,
            &mut stokes,
            &mut path_pdf,
        );
        let dist_rand =
            (hash_u32(seed ^ hash_u32(DISTANCE_SAMPLE_STREAM)) as f32) / 4_294_967_295.0;
        let one_minus_u = (1.0 - dist_rand).max(1e-7);
        let t_free = -(one_minus_u.ln()) / sigma_t_hero;

        if outcome.is_some() {
            if t_free >= hit_t {
                continue; // outcome disagreed with our recomputed t_free; skip (guard only)
            }
            scatter_checked = true;
            let expected =
                (sigma_s / sigma_t_hero) * (-(sigma_t_companion - sigma_t_hero) * t_free).exp();
            let actual = stokes[1].intensity();
            assert!(
                (actual - expected).abs() < 1e-3 * expected.abs().max(1.0),
                "scatter-branch companion weight mismatch at seed={seed}: expected \
                 {expected} (closed form), got {actual}"
            );
        } else {
            no_scatter_checked = true;
            let expected = (-(sigma_t_companion - sigma_t_hero) * hit_t).exp();
            let actual = stokes[1].intensity();
            assert!(
                (actual - expected).abs() < 1e-3 * expected.abs().max(1.0),
                "no-scatter-branch companion weight mismatch at seed={seed}: expected \
                 {expected} (closed form), got {actual}"
            );
        }
    }
    assert!(
        no_scatter_checked && scatter_checked,
        "test premise: both branches should be exercised and checked \
         (no_scatter_checked={no_scatter_checked}, scatter_checked={scatter_checked})"
    );
}

/// Negative control: if a companion channel's `path_pdf` update used the hero's own
/// technique density instead of its own, `spectral_mis_weight` would silently
/// collapse toward `N` instead of reflecting genuine per-channel agreement. Pins
/// the correct behaviour: two channels with different absorption must end up with
/// different `path_pdf` values after a scatter event.
#[test]
fn scatter_event_path_pdf_is_genuinely_per_channel_when_absorption_is_chromatic() {
    // Real built-in Tourmaline (strongly dichroic o-ray vs. e-ray absorption),
    // opted into scattering.
    let material = GemMaterial::by_name("Tourmaline")
        .expect("\"Tourmaline\" must be a built-in material")
        .with_scattering(0.8, 0.0);
    let mat_ctx = RayMaterialContext {
        material: &material,
        lambdas: [550.0, 430.0, 550.0, 550.0, 550.0, 550.0, 550.0, 550.0],
        hero_idx: 0,
        c_axis: material.c_axis,
        is_anisotropic: true,
        enable_internal_mode_coupling: true,
    };
    let cache = super::super::refraction::build_ray_wavelength_cache(&mat_ctx);
    let ray_dir = Vec3::new(0.3, 0.9, 0.3).normalize();
    let probe_stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
    let alphas = channel_absorption_alphas(&mat_ctx, &cache, Vec3::ZERO, ray_dir, &probe_stokes);
    let mut found_scatter = false;
    for seed in 0..2000u32 {
        let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let mut path_pdf = [1.0f32; NUM_CHANNELS];
        let outcome = maybe_scatter_or_extinguish(
            &alphas,
            material.scattering_sigma_s,
            material.scattering_g,
            0,
            ray_dir,
            1.0,
            1.0,
            seed,
            0,
            &mut stokes,
            &mut path_pdf,
        );
        if outcome.is_some() {
            found_scatter = true;
            assert!(
                (path_pdf[0] - path_pdf[1]).abs() > 1e-6,
                "channel 0 (550nm) and channel 1 (430nm, strongly dichroic) must get \
                 DIFFERENT path_pdf values from a scatter event when the material's \
                 absorption is genuinely chromatic -- got path_pdf[0]={}, path_pdf[1]={}",
                path_pdf[0],
                path_pdf[1]
            );
        }
    }
    assert!(
        found_scatter,
        "test premise: at least one trial should hit the scatter branch"
    );
}

/// Veach's balance heuristic is a partition of unity -- for any two
/// positive technique densities, `balance_heuristic(a, b) + balance_heuristic(b, a)
/// == 1.0` exactly (the two divide the same `a + b` denominator). This is the
/// property that makes `nee_contribution_hg_scatter`'s direct sample and
/// `trace_spectral_ray_inner`'s complementary escape-branch weight sum to the true
/// expected value rather than double-counting or losing energy.
#[test]
fn balance_heuristic_weights_sum_to_one() {
    let pairs = [
        (1.0f32, 1.0f32),
        (0.2, 5.0),
        (1e-3, 1e3),
        (12.5, 0.001),
        (7.0, 7.0),
    ];
    for (a, b) in pairs {
        let wa = balance_heuristic(a, b);
        let wb = balance_heuristic(b, a);
        assert!(
            (wa + wb - 1.0).abs() < 1e-5,
            "balance_heuristic({a}, {b}) + balance_heuristic({b}, {a}) should be 1.0, \
             got {wa} + {wb} = {}",
            wa + wb
        );
    }
}

/// Degenerate input (both densities non-positive) must return `0.0`, not `NaN` from
/// a `0.0 / 0.0` division -- the case a fully-occluded shadow ray or an all-black
/// environment row can legitimately produce.
#[test]
fn balance_heuristic_handles_degenerate_zero_densities() {
    assert_eq!(balance_heuristic(0.0, 0.0), 0.0);
    assert!(!balance_heuristic(0.0, 0.0).is_nan());
}

/// [`sample_environment_for_nee`]/[`environment_nee_pdf`] must both decline for
/// [`EnvironmentSource::Studio`] -- NEE is HDR-map-only (see
/// [`NeeContext`](super::NeeContext)'s doc comment): the analytic rig has no
/// importance distribution to draw a light sample from.
#[test]
fn nee_sampling_is_a_no_op_for_the_studio_rig() {
    let studio = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
    assert!(sample_environment_for_nee(studio, 0.3, 0.7).is_none());
    assert_eq!(environment_nee_pdf(studio, Vec3::Y), 0.0);
}

// The decisive NEE-on-vs-off unbiasedness measurement lives in `transport.rs`'s own
// `#[cfg(test)] mod nee_tests` instead of here: it needs `trace_spectral_ray_inner`,
// which is private to that module (this file has no access to it, unlike
// `trace_spectral_ray` -- the public wrapper -- which cannot express an explicit
// NEE-on/off A/B override).
