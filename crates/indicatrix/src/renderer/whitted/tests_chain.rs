use glam::Vec3;

use crate::{
    geometry::{GpuFacetPlane, StandardGemCuts, tool::ToolPrimitive},
    optics::{
        fresnel::fresnel_dielectric,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, Ray, build_plane_soa,
            intersect_stone::intersect_stone_soa_side,
        },
    },
    renderer::{env_map::EnvironmentMap, frame_scene::FrameScene},
};

use super::{
    WhittedPrep,
    chain::{trace_chain, trace_chain_with_cutoff},
    fresnel::{fresnel_unpolarised_f32, refract},
};

fn cube(thickness: f32) -> Vec<GpuFacetPlane> {
    [
        (Vec3::X, -0.5),
        (Vec3::NEG_X, -0.5),
        (Vec3::Y, -0.5),
        (Vec3::NEG_Y, -0.5),
        (Vec3::Z, -thickness * 0.5),
        (Vec3::NEG_Z, -thickness * 0.5),
    ]
    .map(|(normal, d)| GpuFacetPlane::new(normal, d))
    .to_vec()
}

/// The f64 reflectance as a function of the transmitted-cosine square `cos2_t`, which is the one
/// quantity the f32 kernel computes by cancellation (`1 - eta^2 sin^2`).
fn reflectance_at_cos2_t(cos_i: f64, eta: f64, cos2_t: f64) -> f64 {
    if cos2_t <= 0.0 {
        return 1.0;
    }
    let transmitted = cos2_t.sqrt();
    let r_perp = eta.mul_add(cos_i, -transmitted) / eta.mul_add(cos_i, transmitted);
    let r_par = eta.mul_add(-transmitted, cos_i) / eta.mul_add(transmitted, cos_i);
    (0.5 * r_par.mul_add(r_par, r_perp * r_perp)).clamp(0.0, 1.0)
}

/// Within 1e-6 of the f64 reference, widened only by the conditioning of `cos2_t`. Near the
/// critical angle `R` falls with infinite slope in `cos2_t` (`cos_t = sqrt(cos2_t)`), so the
/// rounding of `cos2_t` itself, not the kernel, sets the attainable accuracy there. The f32
/// `cos2_t` carries at most 2^-24 * (1.5 eta^2 + 0.5) of error: the rounded `eta * eta` (relative
/// 2^-24, times sin2 <= 1), the rounded `sin2` (absolute 2^-25, times `eta^2`) and the final
/// rounding (below 2^-25). The extra allowance is how far the reference moves over that range.
#[test]
fn fresnel_matches_the_f64_reference() {
    for eta_step in 4..=25 {
        let eta = eta_step as f32 / 10.0;
        for step in 0..=100 {
            let cos_i = step as f32 / 100.0;
            let reference = fresnel_dielectric(f64::from(cos_i), f64::from(eta), 1.0);
            let actual = fresnel_unpolarised_f32(cos_i, eta);
            let (c, e) = (f64::from(cos_i), f64::from(eta));
            let cos2_t = (-(e * e)).mul_add(c.mul_add(-c, 1.0), 1.0);
            let rounding = 2.0_f64.powi(-24) * 1.5f64.mul_add(e * e, 0.5);
            let at = |shift: f64| reflectance_at_cos2_t(c, e, cos2_t + shift);
            let allowance = (at(rounding) - at(0.0))
                .abs()
                .max((at(-rounding) - at(0.0)).abs());
            assert!(
                (f64::from(actual) - reference).abs() <= 1e-6 + allowance,
                "cos={cos_i}, eta={eta}: {actual} vs {reference} (allowance {allowance})"
            );
        }
    }
}

#[test]
fn normal_incidence_reflectance_is_the_textbook_value() {
    for n in [1.0f32, 1.333, 1.5, 1.76, 2.417] {
        let expected = ((n - 1.0) / (n + 1.0)).powi(2);
        for eta in [n, 1.0 / n] {
            assert!((fresnel_unpolarised_f32(1.0, eta) - expected).abs() < 1e-7);
        }
    }
}

#[test]
fn beyond_the_critical_angle_everything_reflects() {
    for n in [1.33f32, 1.5, 1.76, 2.417] {
        let sin_i = (1.0 / n + 0.05).min(1.0);
        let cos_i = sin_i.mul_add(-sin_i, 1.0).sqrt();
        assert_eq!(fresnel_unpolarised_f32(cos_i, n), 1.0);
        assert!(refract(Vec3::new(sin_i, 0.0, -cos_i), Vec3::Z, n).is_none());
    }
}

#[test]
fn refraction_obeys_snell() {
    for eta in [0.4f32, 0.65, 1.0, 1.25, 1.5, 2.4] {
        for sin_i in [0.0f32, 0.1, 0.2, 0.35] {
            let dir = Vec3::new(sin_i, 0.0, -sin_i.mul_add(-sin_i, 1.0).sqrt());
            let transmitted = refract(dir, Vec3::Z, eta).expect("below critical angle");
            assert!(eta.mul_add(-sin_i, transmitted.x).abs() < 1e-6);
            assert!((transmitted.length() - 1.0).abs() < 1e-6);
            assert!(transmitted.z < 0.0);
        }
    }
}

#[test]
fn leaf_weights_conserve_energy_without_absorption() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let env = EnvironmentMap::uniform(4, 2, [1.0; 3]);
    let scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 256,
        environment: EnvironmentSource::HdrMap(&env),
    };
    let soa = build_plane_soa(&planes);
    let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::ONE, 256, 1);
    for pixel in [8 * 24 + 12, 6 * 24 + 10, 10 * 24 + 14] {
        let ray = camera.generate_ray(
            (pixel % 24) as f32,
            (pixel / 24) as f32,
            24.0,
            16.0,
            0.0,
            0.0,
        );
        let sum = trace_chain_with_cutoff(&scene, &[], &soa, &prep, ray, 1.5, -ray.dir, None, 0.0);
        assert!(
            sum.min_element() >= 0.999 && sum.max_element() <= 1.0 + 1e-5,
            "{sum:?}"
        );
    }
}

#[test]
fn the_tint_is_beer_lambert() {
    let alpha = Vec3::new(0.2, 0.4, 0.8);
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let env = EnvironmentMap::uniform(4, 2, [1.0; 3]);
    for thickness in [0.5f32, 1.0, 2.0] {
        let planes = cube(thickness);
        let scene = FrameScene {
            camera: &camera,
            width: 2,
            height: 2,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 64,
            environment: EnvironmentSource::HdrMap(&env),
        };
        let soa = build_plane_soa(&planes);
        let prep = WhittedPrep::for_tests(1.0, alpha, Vec3::ONE, 64, 1);
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 3.0),
            dir: Vec3::NEG_Z,
        };
        let rgb = trace_chain(&scene, &[], &soa, &prep, ray, 1.0, Vec3::Z, None);
        let length = thickness - 1e-4; // the shared surface restart moves inside
        let expected = Vec3::new(
            (-alpha.x * length).exp(),
            (-alpha.y * length).exp(),
            (-alpha.z * length).exp(),
        );
        assert!(
            (rgb - expected).abs().max_element() < 1e-6,
            "{rgb:?} vs {expected:?}"
        );
    }
}

#[test]
fn a_concave_cavity_reenters_the_stone() {
    let planes = cube(1.0);
    let tools = [ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.3)];
    let soa = build_plane_soa(&planes);
    let ray = Ray {
        origin: Vec3::new(0.45, 0.4, 0.0),
        dir: Vec3::NEG_X,
    };
    let (exit, side) =
        intersect_stone_soa_side(ray, &soa, planes.len(), &tools).expect("cavity exit");
    assert_eq!(side, Some(false));
    let leaving = Ray {
        origin: ray.origin + ray.dir * (exit.t + 1e-4),
        dir: ray.dir,
    };
    let (entry, side) = intersect_stone_soa_side(leaving, &soa, planes.len(), &tools)
        .expect("opposite cavity wall");
    assert_eq!(side, Some(true));
    assert_eq!(entry.facet_idx, planes.len());
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let env = EnvironmentMap::uniform(4, 2, [1.0; 3]);
    let scene = FrameScene {
        camera: &camera,
        width: 2,
        height: 2,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 128,
        environment: EnvironmentSource::HdrMap(&env),
    };
    let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::ONE, 128, 1);
    let rgb = trace_chain(&scene, &tools, &soa, &prep, ray, 1.5, -ray.dir, None);
    assert!(rgb.is_finite());
    assert!(
        rgb.min_element() > 0.998 && rgb.max_element() <= 1.00001,
        "{rgb:?}"
    );
}

#[test]
fn a_frosted_entry_returns_the_prepared_diffuse_radiance() {
    let planes = cube(1.0);
    let finishes = vec![FacetFinish::Frosted; planes.len()];
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let env = EnvironmentMap::uniform(4, 2, [1.0; 3]);
    let scene = FrameScene {
        camera: &camera,
        width: 2,
        height: 2,
        planes: &planes,
        facet_finishes: &finishes,
        material: &material,
        max_bounces: 64,
        environment: EnvironmentSource::HdrMap(&env),
    };
    let mut prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::ONE, 64, 1);
    let expected = Vec3::new(0.1, 0.3, 0.7);
    prep.frost = vec![expected; planes.len()];
    let ray = Ray {
        origin: Vec3::new(0.0, 0.0, 3.0),
        dir: Vec3::NEG_Z,
    };
    let rgb = trace_chain(
        &scene,
        &[],
        &build_plane_soa(&planes),
        &prep,
        ray,
        1.5,
        -ray.dir,
        None,
    );
    assert_eq!(rgb, expected);
}
