//! Tests for [`super::hg_nee`]: the shadow ray of the Henyey-Greenstein NEE pays the
//! assigned mode's extinction AT THE SHADOW DIRECTION ([`ShadowAlphas`]), over the full
//! distance from the scattering point. Regression for the NEE-on-darker bias
//! `transport::tests::nee_tests::ruby_scattering_nee_on_and_off_agree_within_half_a_percent`
//! measured on the refit ruby (NEE-on 0.6 % darker in X, 0.45 % in Y than NEE-off): the
//! deposit used to pay the alphas of the segment that led INTO the scattering point, over
//! a length `1e-4` short of the continuation's.

use super::{
    super::{
        NUM_CHANNELS,
        absorption::{channel_absorption_alphas_assigned, spectral_absorption},
        build_plane_soa,
        environment::{EnvironmentSource, sample_environment_for_nee},
        refraction::{RayMaterialContext, build_ray_wavelength_cache},
        sampling::{NEE_ENV_DIR_U_STREAM, NEE_ENV_DIR_V_STREAM, hash_u32},
    },
    NeeContext, balance_heuristic, henyey_greenstein_phase,
    hg_nee::{ShadowAlphas, nee_contribution_hg_scatter},
};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{materials::GemMaterial, polarization::StokesVector},
    renderer::env_map::{EnvironmentMap, rgb_to_spectral_radiance},
};
use glam::Vec3;

/// Eight channels across the visible range (the ruby's o/e coefficients differ most
/// around 500 nm and 600 nm).
const LAMBDAS: [f32; NUM_CHANNELS] = [400.0, 450.0, 500.0, 550.0, 600.0, 650.0, 700.0, 750.0];

/// The production context for `material` (`transport::inner::build_ray_material_context`'s
/// shape, hero channel 0).
fn ray_context(material: &GemMaterial, is_anisotropic: bool) -> RayMaterialContext<'_> {
    RayMaterialContext {
        material,
        lambdas: LAMBDAS,
        hero_idx: 0,
        c_axis: material.c_axis,
        is_anisotropic,
        enable_internal_mode_coupling: true,
    }
}

/// A unit direction at polar angle `theta` from `+Y` (the ruby's optic axis) and azimuth
/// `phi` about it.
fn about_c(theta: f32, phi: f32) -> Vec3 {
    let (sin_theta, cos_theta) = theta.sin_cos();
    let (sin_phi, cos_phi) = phi.sin_cos();
    Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
}

/// An axis-aligned cube of half-extent `half` about the origin (`n . p <= half` on every
/// face, the `GpuFacetPlane` convention `n . p + d <= 0`).
fn cube_planes(half: f32) -> Vec<GpuFacetPlane> {
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .into_iter()
    .map(|n| GpuFacetPlane::new(n, -half))
    .collect()
}

/// The outward normal of the cube face a ray from the origin along unit `w` leaves through.
fn cube_face(w: Vec3) -> Vec3 {
    let a = w.abs();
    let axis = if a.x >= a.y && a.x >= a.z {
        Vec3::X
    } else if a.y >= a.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    axis * w.dot(axis).signum()
}

/// Unpolarised Fresnel transmittance and refracted cosine for a ray leaving index `n` at
/// `cos_i`, `None` on total internal reflection -- `hg_nee::exit_transmittance`'s formula,
/// written out again so the oracle does not call the code under test.
fn fresnel_t(n: f32, cos_i: f32) -> Option<(f32, f32)> {
    let sin2_t = (n * n * cos_i.mul_add(-cos_i, 1.0)).min(1.0);
    if sin2_t >= 1.0 {
        return None;
    }
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s = n.mul_add(cos_i, -cos_t) / n.mul_add(cos_i, cos_t);
    let r_p = n.mul_add(-cos_t, cos_i) / n.mul_add(cos_t, cos_i);
    let r_unpol = (0.5 * r_p.mul_add(r_p, r_s * r_s)).clamp(0.0, 1.0);
    Some((1.0 - r_unpol, cos_t))
}

/// The light sample `nee_contribution_hg_scatter` draws for `(rng_seed, bounce)`: the same
/// two hashes on the NEE direction streams, through the same environment sampler.
fn nee_draw(environment: EnvironmentSource<'_>, rng_seed: u32, bounce: u32) -> Option<(Vec3, f32)> {
    let u0 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM)) as f32) / 4_294_967_295.0;
    let u1 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM)) as f32) / 4_294_967_295.0;
    sample_environment_for_nee(environment, u0, u1).map(|sample| (sample.dir, sample.pdf))
}

/// The closed form of one NEE deposit from the origin of a cube-shaped stone.
struct Oracle<'a> {
    env_map: &'a EnvironmentMap,
    n_hero: f32,
    g: f32,
    sigma_s: f32,
    path_scale: f32,
    stokes: [StokesVector; NUM_CHANNELS],
}

impl Oracle<'_> {
    /// What the estimator deposits for a shadow ray along `w` (drawn with density `pdf`,
    /// the incoming direction being `k_in`) that pays `alphas` over the in-material length
    /// `length`; `None` when the draw is dropped (total internal reflection at the face).
    fn deposit(
        &self,
        k_in: Vec3,
        (w, pdf): (Vec3, f32),
        alphas: &[f32; NUM_CHANNELS],
        length: f32,
    ) -> Option<[f32; NUM_CHANNELS]> {
        let normal = cube_face(w);
        let cos_i = w.dot(normal).clamp(0.0, 1.0);
        let (t_unpol, cos_t) = fresnel_t(self.n_hero, cos_i)?;
        let phase = henyey_greenstein_phase(w.dot(k_in), self.g);
        let mis = balance_heuristic(pdf, phase);
        let refracted =
            (self.n_hero * w - self.n_hero.mul_add(cos_i, -cos_t) * normal).normalize_or_zero();
        let env_rgb = self.env_map.radiance_rgb(refracted);
        let sigma_s_abs = self.sigma_s / self.path_scale;
        let scaled = length * self.path_scale;
        let common = t_unpol * phase * mis / pdf;
        Some(std::array::from_fn(|k| {
            let transmittance = (-(alphas[k] + sigma_s_abs) * scaled).exp();
            self.stokes[k].intensity()
                * transmittance
                * common
                * rgb_to_spectral_radiance(env_rgb, LAMBDAS[k])
        }))
    }
}

fn assert_close(got: &[f32; NUM_CHANNELS], expected: &[f32; NUM_CHANNELS], rel: f32, what: &str) {
    for (k, (g, e)) in got.iter().zip(expected).enumerate() {
        assert!(
            (g - e).abs() <= rel * e.abs(),
            "{what}: channel {k} ({} nm) deposited {g}, closed form {e}",
            LAMBDAS[k]
        );
    }
}

/// `ShadowAlphas::AssignedMode` evaluates the assigned mode's coefficients at the direction
/// it is asked about: the ruby's extraordinary mode reads `alpha_o cos^2 + alpha_e sin^2`
/// of the angle to the optic axis (to within the e-ray's sub-degree walk-off), its
/// ordinary mode the direction-independent `alpha_o`, and `Fixed` hands back its array.
#[test]
fn shadow_alphas_follow_the_shadow_direction() {
    let ruby = GemMaterial::ruby();
    assert!(
        ruby.c_axis.abs_diff_eq(Vec3::Y, 1e-6) && ruby.birefringence_delta.abs() > 1e-4,
        "premise: the built-in ruby is uniaxial about +Y"
    );
    let ctx = ray_context(&ruby, true);
    let cache = build_ray_wavelength_cache(&ctx);
    let alpha_o: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| spectral_absorption(&ruby.absorption.o_ray, LAMBDAS[k]));
    let alpha_e: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| spectral_absorption(&ruby.absorption.e_ray, LAMBDAS[k]));
    assert!(
        alpha_o[3] > 2.0 * alpha_e[3],
        "premise: strongly pleochroic at 550 nm (o {}, e {})",
        alpha_o[3],
        alpha_e[3]
    );
    let e_mode = ShadowAlphas::AssignedMode {
        ctx: &ctx,
        cache: &cache,
        is_extraordinary: true,
    };
    let o_mode = ShadowAlphas::AssignedMode {
        ctx: &ctx,
        cache: &cache,
        is_extraordinary: false,
    };
    for (theta_deg, phi) in [
        (5.0f32, 0.3f32),
        (30.0, 1.0),
        (60.0, 1.7),
        (85.0, 2.4),
        (120.0, 3.1),
        (160.0, 3.8),
    ] {
        let theta = theta_deg.to_radians();
        let dir = about_c(theta, phi);
        let got = e_mode.at(dir);
        let oracle = channel_absorption_alphas_assigned(&ctx, &cache, dir, true);
        assert_eq!(
            got.map(f32::to_bits),
            oracle.map(f32::to_bits),
            "theta {theta_deg}: the AssignedMode arm is the assigned-mode driver itself"
        );
        let (sin_theta, cos_theta) = theta.sin_cos();
        let ordinary = o_mode.at(dir);
        for k in 0..NUM_CHANNELS {
            let closed =
                alpha_o[k].mul_add(cos_theta * cos_theta, alpha_e[k] * sin_theta * sin_theta);
            assert!(
                (got[k] - closed).abs() <= 2e-2 * closed.max(1e-3),
                "theta {theta_deg}, {} nm: e-mode alpha {} vs closed form {closed}",
                LAMBDAS[k],
                got[k]
            );
            assert!(
                (ordinary[k] - alpha_o[k]).abs() <= 1e-3 * alpha_o[k].max(1e-3),
                "theta {theta_deg}, {} nm: o-mode alpha {} vs alpha_o {}",
                LAMBDAS[k],
                ordinary[k],
                alpha_o[k]
            );
        }
    }
    // The dependence this pins is large: along the axis the e-mode pays the ordinary
    // coefficient, 60 degrees off it mostly the extraordinary one.
    let along = e_mode.at(Vec3::Y);
    let across = e_mode.at(about_c(60.0f32.to_radians(), 0.3));
    assert!(
        along[3] > 1.5 * across[3],
        "550 nm e-mode alpha along the axis {} vs 60 degrees off {}",
        along[3],
        across[3]
    );
    let fixed = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    assert_eq!(
        ShadowAlphas::Fixed(&fixed).at(Vec3::X).map(f32::to_bits),
        fixed.map(f32::to_bits)
    );
}

/// For an isotropic material the assigned-mode evaluation does not depend on the
/// direction (or the mode flag), so `try_scatter_step`'s `Fixed` shortcut -- reusing the
/// segment's own array for the shadow ray -- is exact there and keeps such traces
/// bit-identical.
#[test]
fn isotropic_material_alphas_do_not_depend_on_the_shadow_direction() {
    let spinel = GemMaterial::by_name("Spinel").expect("Spinel must be a built-in material");
    let ctx = ray_context(&spinel, false);
    let cache = build_ray_wavelength_cache(&ctx);
    // Directions oblique to every axis (the eigen-polarisation construction the isotropic
    // branch reuses is a geometric no-op here, but it still needs a well-defined basis).
    let reference = channel_absorption_alphas_assigned(&ctx, &cache, about_c(0.9, 1.3), false);
    assert!(
        reference.iter().any(|a| *a > 1e-3),
        "premise: the material absorbs somewhere in the visible ({reference:?})"
    );
    for dir in [
        about_c(1.1, 2.0),
        about_c(2.4, 0.4),
        Vec3::new(0.05, -1.0, 0.02).normalize(),
        Vec3::new(1.0, 0.3, -0.2).normalize(),
        Vec3::new(-0.4, 0.2, 1.0).normalize(),
    ] {
        for is_extraordinary in [false, true] {
            let got = ShadowAlphas::AssignedMode {
                ctx: &ctx,
                cache: &cache,
                is_extraordinary,
            }
            .at(dir);
            // A unit vector's quadratic form against an isotropic tensor is the eigenvalue
            // up to the rounding of `|e|^2 == 1`, hence a tolerance rather than bit equality.
            assert_close(
                &got,
                &reference,
                1e-5,
                &format!("{dir:?} / extraordinary {is_extraordinary}"),
            );
        }
    }
}

/// The inputs of one estimator call that the deposit test holds fixed while it varies the
/// [`ShadowAlphas`] arm: the scattering point is the cube's centre, the bounce index `1`.
struct Call<'a> {
    nee: NeeContext<'a>,
    material: &'a GemMaterial,
    n_hero: f32,
    k_in: Vec3,
    rng_seed: u32,
    stokes: &'a [StokesVector; NUM_CHANNELS],
}

/// Bounce index every estimator call of the deposit test uses (any value works; it only
/// salts the NEE draw).
const BOUNCE: u32 = 1;

fn run(call: &Call<'_>, alphas: ShadowAlphas<'_>) -> [f32; NUM_CHANNELS] {
    let mut deposit = [0.0f32; NUM_CHANNELS];
    nee_contribution_hg_scatter(
        call.nee,
        &LAMBDAS,
        call.n_hero,
        Vec3::ZERO,
        call.k_in,
        call.material.scattering_g,
        call.rng_seed,
        BOUNCE,
        call.stokes,
        &mut deposit,
        alphas,
        call.material.scattering_sigma_s,
        call.material.absorption_path_scale,
        &[],
    );
    deposit
}

/// From a scattering point at the centre of a cube of ruby, each NEE deposit equals the
/// closed form built from the assigned mode's coefficients AT THE SAMPLED SHADOW
/// DIRECTION and the full distance from the scattering point to the face -- and differs
/// by far more than the tolerance from the old estimator's (the incoming direction's
/// coefficients over the probe's own, `1e-4` shorter, length). The `Fixed` arm pays its
/// array as it is, over the same full length.
#[test]
fn deposit_pays_the_shadow_direction_alphas_over_the_full_distance() {
    const HALF: f32 = 0.25;
    let ruby = GemMaterial::ruby().with_scattering(1.2, 0.4);
    let ctx = ray_context(&ruby, true);
    let cache = build_ray_wavelength_cache(&ctx);
    let planes = cube_planes(HALF);
    let soa = build_plane_soa(&planes);
    let env_map = EnvironmentMap::uniform(4, 2, [2.0, 2.0, 2.0]);
    let environment = EnvironmentSource::HdrMap(&env_map);
    let nee = NeeContext {
        environment,
        plane_soa: &soa,
        tools: &[],
        enabled: true,
    };
    let n_hero = cache.n_o_ch[0];
    let oracle = Oracle {
        env_map: &env_map,
        n_hero,
        g: ruby.scattering_g,
        sigma_s: ruby.scattering_sigma_s,
        path_scale: ruby.absorption_path_scale,
        stokes: std::array::from_fn(|k| StokesVector::unpolarized(0.1f32.mul_add(k as f32, 0.5))),
    };
    // The camera ray through the table: along the optic axis, where the extraordinary
    // mode's coefficient is the ordinary one.
    let k_in = Vec3::new(0.05, -1.0, 0.02).normalize();
    let old_alphas = channel_absorption_alphas_assigned(&ctx, &cache, k_in, true);
    let shadow = ShadowAlphas::AssignedMode {
        ctx: &ctx,
        cache: &cache,
        is_extraordinary: true,
    };
    let mut valid = 0usize;
    let mut max_old_gap = 0.0f32;
    for seed in 0..32u32 {
        let call = Call {
            nee,
            material: &ruby,
            n_hero,
            k_in,
            rng_seed: 91_000 + seed,
            stokes: &oracle.stokes,
        };
        let got = run(&call, shadow);
        let draw =
            nee_draw(environment, call.rng_seed, BOUNCE).expect("a 4x2 uniform map always samples");
        let distance = HALF / draw.0.abs().max_element();
        let shadow_alphas = channel_absorption_alphas_assigned(&ctx, &cache, draw.0, true);
        let Some(expected) = oracle.deposit(k_in, draw, &shadow_alphas, distance) else {
            assert!(
                got.iter().all(|v| v.to_bits() == 0),
                "seed {}: a totally internally reflected draw deposits nothing",
                call.rng_seed
            );
            continue;
        };
        valid += 1;
        assert_close(&got, &expected, 2e-5, &format!("seed {}", call.rng_seed));
        // The old estimator: the incoming direction's alphas over the probe's own `hit.t`.
        let old = oracle
            .deposit(k_in, draw, &old_alphas, distance - 1e-4)
            .expect("same face, same Fresnel verdict");
        for (got_k, old_k) in got.iter().zip(&old) {
            max_old_gap = max_old_gap.max((got_k / old_k - 1.0).abs());
        }
        let fixed = run(&call, ShadowAlphas::Fixed(&old_alphas));
        let expected_fixed = oracle
            .deposit(k_in, draw, &old_alphas, distance)
            .expect("same face, same Fresnel verdict");
        assert_close(
            &fixed,
            &expected_fixed,
            2e-5,
            &format!("seed {}, Fixed arm", call.rng_seed),
        );
    }
    assert!(
        valid >= 8,
        "premise: enough draws leave the cube ({valid} of 32)"
    );
    assert!(
        max_old_gap > 0.05,
        "the incoming direction's alphas must be visibly wrong here (largest gap {max_old_gap})"
    );
}
