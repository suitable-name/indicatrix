//! Shared test fixtures for the girdle-finish and white-furnace tests.

use glam::Vec3;
use indicatrix::{
    color::cie1931::CIE_1931_Y_INTEGRAL_5NM,
    geometry::cuts::STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS,
    optics::raytracer::{Camera, FacetFinish, Ray, cie_1931_cmf, hash_u32},
    renderer::env_map::rgb_to_spectral_radiance,
};

/// Pixels per side of the square furnace grid.
pub const FURNACE_GRID: usize = 12;

/// Builds a `facet_finishes` slice sized to `planes.len()`, `Polished` everywhere except
/// the girdle band (`STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS`), which is `Frosted`.
pub fn bruted_girdle_finishes(num_planes: usize) -> Vec<FacetFinish> {
    let mut finishes = vec![FacetFinish::Polished; num_planes];
    for i in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
        finishes[i] = FacetFinish::Frosted;
    }
    finishes
}

/// Mean XYZ over the square `FURNACE_GRID`-sided furnace camera grid with
/// `samples_per_pixel` samples per pixel, plus the sample count.
///
/// `seed_salt` is combined by `^` into the per-sample seed hash; `trace(ray, seed,
/// hero_rand)` traces one sample.
pub fn furnace_mean_xyz(
    samples_per_pixel: u32,
    seed_salt: u32,
    mut trace: impl FnMut(Ray, u32, f32) -> Vec3,
) -> (Vec3, u32) {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let grid = FURNACE_GRID;
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    for iy in 0..grid {
        for ix in 0..grid {
            let ray = camera.generate_ray(ix as f32, iy as f32, grid as f32, grid as f32, 0.5, 0.5);
            for s in 0..samples_per_pixel {
                let pixel_id = (iy as u32) * (grid as u32) + (ix as u32);
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ seed_salt));
                sum += trace(ray, seed, (hash_u32(seed) as f32) / 4_294_967_295.0);
                count += 1;
            }
        }
    }
    (sum / count as f32, count)
}

/// Analytic XYZ of a uniform environment of radiance `[l0; 3]`: the CMF-weighted spectral
/// reconstruction over 380..=780 nm, normalised by the CIE 1931 Y integral.
pub fn uniform_furnace_target(l0: f32) -> Vec3 {
    assert!(
        (CIE_1931_Y_INTEGRAL_5NM - 106.856).abs() < 0.01,
        "CIE_1931_Y_INTEGRAL_5NM drifted from the tabulated 106.856"
    );
    let mut target = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        let spec = rgb_to_spectral_radiance([l0, l0, l0], lambda);
        target += cie_1931_cmf(lambda) * spec;
    }
    target / CIE_1931_Y_INTEGRAL_5NM
}
