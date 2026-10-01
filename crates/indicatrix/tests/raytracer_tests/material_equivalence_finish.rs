//! Sapphire/Ruby o-ray-only bit-identity, and facet-finish (bruted/frosted girdle)
//! tests: an all-`Polished` finish must be bit-identical to `trace_spectral_ray`,
//! a bruted girdle must still satisfy white-furnace energy conservation, and it
//! must change face-up appearance measurably.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, LightingPreset, Ray, hash_u32,
            trace_spectral_ray, trace_spectral_ray_with_finish,
        },
    },
    renderer::env_map::EnvironmentMap,
};

use crate::fixtures::{bruted_girdle_finishes, furnace_mean_xyz, uniform_furnace_target};

/// Regression (pleochroism data): Sapphire's and Ruby's face-up render must stay
/// UNAFFECTED by the newly-populated `e_ray` absorption data -- with `c_axis` = `Vec3::Y` and this exact straight-down ray (propagation
/// exactly parallel to `c_axis`), the polarization quadratic form degenerates to pure
/// `alpha_o` for every bounce whose propagation direction stays exactly on-axis (see the
/// hue-shift test's doc comment above for the same degeneracy argument). Verified here
/// to be exactly BIT-IDENTICAL (not merely within a small tolerance) to an
/// otherwise-identical material with its absorption forced to
/// `AbsorptionTensor::isotropic(o_ray)` -- i.e. exactly what these entries are
/// equivalent to with no `e_ray` data populated -- across 500 independent seeds on
/// the full Standard Round Brilliant cut. This is a stronger guarantee than a small
/// tolerance would give, made possible because this specific ray's
/// on-axis symmetry is exact, not approximate.
#[test]
fn sapphire_and_ruby_face_up_render_is_bit_identical_to_o_ray_only_material() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    for name in ["Sapphire", "Ruby"] {
        let real = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let mut o_ray_only = real.clone();
        o_ray_only.absorption = indicatrix::optics::absorption::AbsorptionTensor::isotropic(
            real.absorption.o_ray.clone(),
        );

        for seed in 0u32..500 {
            let xyz_real = trace_spectral_ray(
                ray,
                &planes,
                &real,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            let xyz_o_only = trace_spectral_ray(
                ray,
                &planes,
                &o_ray_only,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            assert_eq!(
                xyz_real, xyz_o_only,
                "{name}: face-up render (seed={seed}) must be bit-identical to an o-ray-only \
                 clone -- the new e_ray data must never be consulted for this exactly-on-axis ray"
            );
        }
    }
}

// ---------------------------------------------------------------------------------
// Girdle finish (bruted/frosted facets).
// ---------------------------------------------------------------------------------

/// `trace_spectral_ray_with_finish` with an all-`Polished` `facet_finishes` slice (the
/// same length as `planes`, every entry the default) must be bit-identical to
/// `trace_spectral_ray` -- the "existing cuts keep polished girdles unless a finish is
/// explicitly set" requirement, pinned as an exact `f32::to_bits()` regression, not a
/// tolerance-based one. Covers several materials (isotropic AND uniaxial, so
/// mode-coupling machinery is also exercised on this code path) and rays.
#[test]
fn frosted_finish_all_polished_is_bit_identical_to_trace_spectral_ray() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let all_polished = vec![FacetFinish::Polished; planes.len()];
    let rays = [
        Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.0, -1.0, 0.0),
        },
        Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
        },
    ];
    for name in ["Diamond", "Zircon", "Sapphire"] {
        let material = GemMaterial::by_name(name).unwrap();
        for ray in rays {
            for seed in [1u32, 42, 9001] {
                let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
                let baseline =
                    trace_spectral_ray(ray, &planes, &material, 12, env, seed, hero_rand, None);
                let env2 = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
                let with_finish = trace_spectral_ray_with_finish(
                    ray,
                    &planes,
                    &all_polished,
                    &material,
                    12,
                    env2,
                    seed,
                    hero_rand,
                    None,
                );
                assert_eq!(
                    baseline.x.to_bits(),
                    with_finish.x.to_bits(),
                    "{name} seed={seed}: all-Polished trace_spectral_ray_with_finish must be \
                     bit-identical to trace_spectral_ray (x component)"
                );
                assert_eq!(
                    baseline.y.to_bits(),
                    with_finish.y.to_bits(),
                    "{name} seed={seed}: y component"
                );
                assert_eq!(
                    baseline.z.to_bits(),
                    with_finish.z.to_bits(),
                    "{name} seed={seed}: z component"
                );
            }
        }
    }
}

/// The existing white-furnace energy-conservation invariant (a colourless,
/// non-dispersive gem immersed in a spatially UNIFORM environment must render at exactly
/// that environment's own radiance, regardless of its internal optics -- reflectance and
/// transmittance always sum to 1 at every interface, so a lossless system can neither
/// gain nor lose energy no matter how many bounces or which directions they go) must
/// still hold with a BRUTED girdle. This is the concrete check that
/// `apply_frosted_bounce`'s reflect/transmit split (`r_unpol` / `1 - r_unpol`, the SAME
/// total energy budget as the polished formula, just redirected diffusely) and its
/// cosine-weighted-hemisphere direction sampling (whose pdf exactly cancels the assumed
/// Lambertian `albedo = 1.0` BRDF/BTDF -- see that function's doc comment) are actually
/// energy-conserving, not merely "doesn't crash".
#[test]
fn frosted_girdle_white_furnace_energy_conservation_still_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 96;
    // Measured on 2026-10-01 by `furnace_noise_floor` at this sample budget: 1.35
    // percent truncation loss at cap 12 (paths still inside the gem at the cap are
    // dropped; nothing is lost at cap 256) plus about 0.15 percent noise; five standard
    // deviations of headroom. A branch mis-weighted by a few percent fails this.
    const TOLERANCE: f32 = 0.025;

    let planes = StandardGemCuts::standard_round_brilliant();
    let finishes = bruted_girdle_finishes(planes.len());
    // Colourless, non-dispersive, cubic -- matches
    // `renderer::gpu::estimator_check::furnace_material`'s own construction (not
    // reusable directly: that function is behind the `gpu` feature).
    let material = GemMaterial::new_custom("CPU furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0]);
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

    let (mean, count) = furnace_mean_xyz(SAMPLES_PER_PIXEL, 0x4657_5230, |ray, seed, hero| {
        trace_spectral_ray_with_finish(
            ray,
            &planes,
            &finishes,
            &material,
            12,
            EnvironmentSource::HdrMap(&env_map),
            seed,
            hero,
            None,
        )
    });

    // Analytic target: EnvironmentMap::uniform's spectral reconstruction of [L0,L0,L0],
    // integrated against the real CIE 1931 CMF over the visible range.
    let target = uniform_furnace_target(L0);

    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean.x, target.x),
        rel_err(mean.y, target.y),
        rel_err(mean.z, target.z),
    );
    println!(
        "[frosted-girdle furnace] mean={mean:?} target={target:?} rel_err=({ex:.4}, {ey:.4}, {ez:.4}) over {count} samples"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "frosted-girdle furnace should still converge to the uniform environment's own \
         radiance (mean={mean:?}, target={target:?}, rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
    );
}

/// The decisive measurement: a bruted girdle must change the gem's face-up
/// appearance, not merely run without crashing.
///
/// The specific claim is that the girdle "feeds a soft bright ring into
/// the pavilion" -- extra light-gathering paths a purely specular mirror girdle can only
/// reach through a single, much narrower, delta direction (outside light scattering
/// diffusely IN through the girdle; internally-trapped light scattering back OUT through
/// it). That is a REDISTRIBUTION of energy (the furnace test above confirms total energy
/// is still conserved), not a claim that every possible viewing angle gets strictly
/// brighter: probing several camera framings while developing this test found some
/// (steep near-vertical pitches that look almost straight down the table) where mean
/// face-up brightness measurably DECREASES with a frosted girdle -- physically sensible,
/// since replacing a crisp specular glint with a diffuse spread can starve a viewing
/// direction that sits exactly in that glint's narrow cone, even while other
/// directions gain. For the standard face-up studio framing this crate already uses
/// elsewhere as its reference camera (`renderer::gpu::estimator_check::test_camera`:
/// `Camera::new(0.35, 0.28, 5.0, 18.0)`), the effect is a strong, clean INCREASE,
/// matching that "soft bright ring" framing -- that is the camera used below.
///
/// Averages luminance (Y) over a small grid of pixels and many samples per pixel, for the
/// SAME material/seeds with only the girdle's finish differing.
#[test]
fn frosted_girdle_changes_face_up_appearance_measurably() {
    const SAMPLES_PER_PIXEL: u32 = 96;
    const GRID: usize = 16;

    let planes = StandardGemCuts::standard_round_brilliant();
    let polished = vec![FacetFinish::Polished; planes.len()];
    let frosted = bruted_girdle_finishes(planes.len());
    let material = GemMaterial::by_name("Diamond").expect("Diamond must be a built-in material");
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let env = || LightingPreset::RingLights.studio(1.0, 0.85, 0.95);

    let mut sum_polished = Vec3::ZERO;
    let mut sum_frosted = Vec3::ZERO;
    let mut count = 0u32;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES_PER_PIXEL {
                let pixel_id = (iy as u32) * (GRID as u32) + (ix as u32);
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x9A7E_C6F1));
                let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                sum_polished += trace_spectral_ray_with_finish(
                    ray,
                    &planes,
                    &polished,
                    &material,
                    12,
                    env(),
                    seed,
                    hero_rand,
                    None,
                );
                sum_frosted += trace_spectral_ray_with_finish(
                    ray,
                    &planes,
                    &frosted,
                    &material,
                    12,
                    env(),
                    seed,
                    hero_rand,
                    None,
                );
                count += 1;
            }
        }
    }
    let mean_polished = sum_polished / count as f32;
    let mean_frosted = sum_frosted / count as f32;
    let delta_y = mean_frosted.y - mean_polished.y;
    println!(
        "[frosted-girdle face-up] polished Y={:.5} frosted Y={:.5} delta_y={:.5} ({:.2}%) over {count} samples",
        mean_polished.y,
        mean_frosted.y,
        delta_y,
        100.0 * delta_y / mean_polished.y.max(1e-6)
    );

    assert!(
        delta_y > 0.0,
        "a bruted girdle should make face-up brightness (Y) INCREASE, not decrease or stay \
         flat -- extra diffuse light-gathering paths through the girdle should add energy, \
         not remove it (polished Y={:.5}, frosted Y={:.5})",
        mean_polished.y,
        mean_frosted.y
    );
    let relative_change = delta_y.abs() / mean_polished.y.max(1e-6);
    assert!(
        relative_change > 0.01,
        "the brightness change from a frosted girdle should be clearly measurable (>1%), \
         not noise-level -- got {:.4}% (polished Y={:.5}, frosted Y={:.5})",
        100.0 * relative_change,
        mean_polished.y,
        mean_frosted.y
    );
}
