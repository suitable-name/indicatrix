//! Ray-polyhedron intersection, camera generation, and basic polarization
//! (Stokes/Mueller Brewster-angle reflection, birefringent walk-off) unit tests.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        birefringence::BirefringenceParams,
        materials::GemMaterial,
        polarization::{MuellerMatrix, StokesVector},
        raytracer::{Camera, Ray, intersect_polyhedron},
    },
};

#[test]
fn test_standard_round_brilliant_planes() {
    let planes = StandardGemCuts::standard_round_brilliant();
    assert!(
        planes.len() >= 57,
        "Standard round brilliant should have at least 57 facet planes"
    );
}

#[test]
fn test_ray_polyhedron_intersection() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.0, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let hit = intersect_polyhedron(ray, &planes);
    assert!(
        hit.is_some(),
        "Ray directed at top table facet must intersect gem polyhedron"
    );
    let rec = hit.unwrap();
    assert!(rec.t > 0.0, "Hit distance must be positive");
    assert!(
        (rec.normal.y - 1.0).abs() < 1e-3,
        "Normal of top table facet must be +Y"
    );
}

#[test]
fn test_ray_polyhedron_intersection_from_inside_exits() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 0.0, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let hit = intersect_polyhedron(ray, &planes);
    assert!(
        hit.is_some(),
        "Ray starting inside the gem must still find its exit facet"
    );
    let rec = hit.unwrap();
    assert!(
        (rec.t - 0.88).abs() < 1e-2,
        "Ray from origin heading straight down should exit through the culet plane at t ~= 0.88 (got {})",
        rec.t
    );
    assert!(
        (rec.normal - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-3,
        "Exit normal should be the culet plane's outward normal (0,-1,0) (got {:?})",
        rec.normal
    );
}

#[test]
fn test_camera_generation_and_orbit() {
    let cam = Camera::new(0.0, 0.0, 3.0, 45.0);
    let ray = cam.generate_ray(400.0, 300.0, 800.0, 600.0, 0.0, 0.0);
    assert!((ray.origin.z - 3.0).abs() < 1e-3);
    assert!((ray.dir.z - (-1.0)).abs() < 1e-3);
}

#[test]
fn test_stokes_mueller_brewster_polarization() {
    let n1 = 1.0f32;
    let n2 = 2.418f32;
    let theta_b = (n2 / n1).atan();
    let cos_i = theta_b.cos();
    let sin_t = (n1 / n2) * theta_b.sin();
    let cos_t = sin_t.mul_add(-sin_t, 1.0).sqrt();

    let r_s = n2.mul_add(-cos_t, n1 * cos_i) / n2.mul_add(cos_t, n1 * cos_i);
    let r_p = n1.mul_add(-cos_t, n2 * cos_i) / n1.mul_add(cos_t, n2 * cos_i);

    assert!(r_p.abs() < 1e-4, "r_p must be zero at Brewster angle");

    let refl_matrix = MuellerMatrix::fresnel_reflection(r_s, r_p);
    let unpolarized_in = StokesVector::unpolarized(1.0);
    let stokes_out = unpolarized_in.apply_matrix(&refl_matrix);

    assert!(
        (stokes_out.degree_of_polarization() - 1.0).abs() < 1e-3,
        "Reflected light at Brewster angle must be 100% linearly polarized"
    );
}

#[test]
fn test_birefringent_walk_off_moissanite() {
    let moissanite = GemMaterial::by_name("Synthetic Moissanite").unwrap();
    let n_o = moissanite.dispersion.evaluate(589.3);
    let n_e = n_o + moissanite.birefringence_delta;

    let theta = 45.0f32.to_radians();
    let walk_off_rad = BirefringenceParams::walk_off_angle(n_o, n_e, theta);
    let walk_off_deg = walk_off_rad.to_degrees();

    assert!(
        walk_off_deg.abs() > 0.5,
        "Moissanite should exhibit significant extraordinary walk-off angle (|rho| > 0.5 deg)"
    );
}
