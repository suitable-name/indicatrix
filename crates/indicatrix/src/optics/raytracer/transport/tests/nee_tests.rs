//! Next-event-estimation unbiasedness, via `trace_spectral_ray_inner`'s own
//! `enable_nee` A/B switch (the same pattern `mode_coupling_tests`/`exit_splitting_tests`
//! use for their own on/off flags).

use super::super::{
    super::{
        camera::Ray, environment::EnvironmentSource, intersect::build_plane_soa, sampling::hash_u32,
    },
    inner::trace_spectral_ray_inner,
};
use crate::{geometry::cuts::StandardGemCuts, optics::materials::GemMaterial};
use glam::Vec3;

/// The decisive end-to-end measurement: on a uniform ("furnace-like") HDR map,
/// NEE-on and NEE-off must converge to the SAME mean radiance -- MIS is a
/// variance-reduction technique, not a change in expectation. A biased
/// complementary-weight formula (double-counting direct light both ways, or a
/// dropped `pending_light_mis` reset letting it leak into an unrelated later
/// escape) would show up here as a systematic mean shift, not merely extra noise.
#[test]
fn nee_on_and_off_converge_to_the_same_mean_on_a_uniform_hdr_map() {
    const L0: f32 = 2.0;
    const SAMPLES: u32 = 20_000;
    const TOLERANCE: f32 = 0.05;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    // Lossless scattering (sigma_a == 0) so every scatter event is genuinely
    // NEE-eligible and the comparison isolates the NEE/MIS machinery itself, not
    // absorption.
    let material = GemMaterial::new_custom("G7 furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_scattering(1.0, 0.2);
    let env_map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [L0, L0, L0]);
    let environment = EnvironmentSource::HdrMap(&env_map);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.1, -1.0, 0.05).normalize(),
    };

    let mut sum_on = Vec3::ZERO;
    let mut sum_off = Vec3::ZERO;
    for i in 0..SAMPLES {
        let seed = 42_000 + i;
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        sum_on += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            12,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            true,
            None,
        );
        sum_off += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            12,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            false,
            None,
        );
    }
    let mean_on = sum_on / SAMPLES as f32;
    let mean_off = sum_off / SAMPLES as f32;
    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean_on.x, mean_off.x),
        rel_err(mean_on.y, mean_off.y),
        rel_err(mean_on.z, mean_off.z),
    );
    println!(
        "[G7 NEE furnace] mean_on={mean_on:?} mean_off={mean_off:?} \
         rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "NEE-on and NEE-off should converge to the same mean radiance on a uniform \
         HDR map (mean_on={mean_on:?}, mean_off={mean_off:?}, rel_err=({ex}, {ey}, \
         {ez}), tolerance={TOLERANCE})"
    );
}

/// The uniform-map furnace test above cannot catch NEE looking up the
/// environment at the wrong direction -- `rgb_to_spectral_radiance` gives the same
/// answer everywhere on a uniform map regardless of which direction is sampled. A
/// genuinely directional (two-hemisphere: bright `+Y` half, black `-Y` half) map
/// makes that bug visible: if `nee_contribution_hg_scatter` looked up
/// the environment at the UNREFRACTED interior light-sample direction instead of the
/// true refracted exterior one, NEE-on would read a systematically different mean
/// than NEE-off (whose phase-sampled continuation always escapes along the genuinely
/// refracted direction).
#[test]
fn nee_on_and_off_agree_on_a_two_hemisphere_hdr_map() {
    const L0: f32 = 3.0;
    const SAMPLES: u32 = 40_000;
    const TOLERANCE: f32 = 0.05;
    const WIDTH: usize = 8;
    const HEIGHT: usize = 4;

    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    // Lossless scattering, same as the uniform-map furnace test above, so this
    // isolates the NEE lookup-direction check here from the separately-tested
    // medium transmittance.
    let material =
        GemMaterial::new_custom("2c two-hemisphere probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0])
            .with_scattering(1.0, 0.2);

    // Row `0` is `v = 0` (north pole, `+Y`); row `HEIGHT - 1` is `v = 1` (south
    // pole, `-Y`) -- see `EnvironmentMap`'s own struct doc comment. The upper half
    // of the rows (`+Y` hemisphere) is bright, the lower half (`-Y` hemisphere)
    // stays black.
    let mut pixels = vec![[0.0f32, 0.0, 0.0]; WIDTH * HEIGHT];
    for row in 0..HEIGHT / 2 {
        for col in 0..WIDTH {
            pixels[row * WIDTH + col] = [L0, L0, L0];
        }
    }
    let env_map = crate::renderer::env_map::EnvironmentMap::from_rgb(WIDTH, HEIGHT, pixels)
        .expect("WIDTH * HEIGHT pixels for a WIDTH x HEIGHT map");
    let environment = EnvironmentSource::HdrMap(&env_map);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.1, -1.0, 0.05).normalize(),
    };

    let mut sum_on = Vec3::ZERO;
    let mut sum_off = Vec3::ZERO;
    for i in 0..SAMPLES {
        let seed = 77_000 + i;
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        sum_on += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            12,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            true,
            None,
        );
        sum_off += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            12,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            false,
            None,
        );
    }
    let mean_on = sum_on / SAMPLES as f32;
    let mean_off = sum_off / SAMPLES as f32;
    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean_on.x, mean_off.x),
        rel_err(mean_on.y, mean_off.y),
        rel_err(mean_on.z, mean_off.z),
    );
    println!(
        "[2c two-hemisphere NEE] mean_on={mean_on:?} mean_off={mean_off:?} \
         rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "NEE-on and NEE-off should agree within tolerance on a directional \
         (two-hemisphere) HDR map, not only a uniform one (mean_on={mean_on:?}, \
         mean_off={mean_off:?}, rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
    );
}

/// The same decisive NEE-on-vs-off unbiasedness measurement as
/// [`nee_on_and_off_converge_to_the_same_mean_on_a_uniform_hdr_map`] above, but for a
/// FROSTED-GIRDLE scene instead of a Henyey-Greenstein scattering medium -- isolates
/// `apply_frosted_bounce`'s own NEE contribution
/// (`scattering::nee_contribution_frosted_exterior`) and its `pending_light_mis`
/// carry from the HG path's identical-shaped machinery, which this scene's material
/// (`scattering_sigma_s == 0.0`, the `GemMaterial::new_custom` default) never
/// exercises at all. A camera-grid sweep (mirroring
/// `frosted_girdle_white_furnace_energy_conservation_still_holds` in
/// `tests/raytracer_tests.rs`) rather than one fixed ray, so both the entry-facet-
/// reflect and exit-facet-transmit NEE-eligible branches (see
/// `apply_frosted_bounce`'s "Sign convention for NEE eligibility" doc section) get
/// exercised across many incidence angles.
#[test]
fn frosted_girdle_nee_on_and_off_converge_to_the_same_mean_on_a_uniform_hdr_map() {
    use super::super::super::camera::Camera;

    const L0: f32 = 2.0;
    const SAMPLES_PER_PIXEL: u32 = 48;
    const GRID: usize = 10;
    const TOLERANCE: f32 = 0.06;

    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let finishes = crate::geometry::girdle_facet_finishes(&planes);
    // Colourless, non-dispersive, no volumetric scattering -- isolates the
    // frosted-facet NEE machinery from both chromatic absorption and the HG
    // scattering NEE path.
    let material =
        GemMaterial::new_custom("G8 frosted furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0]);
    let env_map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [L0, L0, L0]);
    let environment = EnvironmentSource::HdrMap(&env_map);

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let mut sum_on = Vec3::ZERO;
    let mut sum_off = Vec3::ZERO;
    let mut count = 0u32;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES_PER_PIXEL {
                let pixel_id = (iy as u32) * (GRID as u32) + (ix as u32);
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x6667_8899));
                let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                sum_on += trace_spectral_ray_inner(
                    ray,
                    &planes,
                    &plane_soa,
                    &finishes,
                    &material,
                    12,
                    environment,
                    seed,
                    hero_rand,
                    None,
                    true,
                    true,
                    true,
                    None,
                );
                sum_off += trace_spectral_ray_inner(
                    ray,
                    &planes,
                    &plane_soa,
                    &finishes,
                    &material,
                    12,
                    environment,
                    seed,
                    hero_rand,
                    None,
                    true,
                    true,
                    false,
                    None,
                );
                count += 1;
            }
        }
    }
    let mean_on = sum_on / count as f32;
    let mean_off = sum_off / count as f32;
    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean_on.x, mean_off.x),
        rel_err(mean_on.y, mean_off.y),
        rel_err(mean_on.z, mean_off.z),
    );
    println!(
        "[G8 frosted-girdle NEE furnace] mean_on={mean_on:?} mean_off={mean_off:?} \
         rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "frosted-girdle NEE-on and NEE-off should converge to the same mean radiance \
         on a uniform HDR map (mean_on={mean_on:?}, mean_off={mean_off:?}, \
         rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
    );
}

/// Regression: the two tests above use a colourless, NON-dispersive scattering
/// probe, which cannot see either bug fixed alongside this test -- (a) the exit-split
/// radiance a scatter event's continuation picks up had no MIS weight against a live
/// NEE carry, and (b) the scattering-point NEE deposit was weighted by the FINAL
/// post-loop `path_pdf` rather than its own moment's `path_pdf`/`compat`. Both biases
/// are chromatic MIS-family effects that a same-index-every-channel material cannot
/// exercise (every channel shares one family). `GemMaterial::diamond()` is genuinely
/// dispersive (`Sellmeier3`) and colourless (`sigma_a == 0`), isolating both fixes from
/// chromatic absorption -- see `ruby_scattering_nee_on_and_off_agree_within_half_a_percent`
/// below for the absorbing-medium companion. Before the fix this measured NEE-on/NEE-off
/// disagreeing by several percent (review-measured: 1.0598 vs 0.9911 relative to the
/// furnace's own true value); after the fix both sides agree with each other (each
/// converges independently to the analytic furnace target -- see
/// `scattering_tests::dispersive_lossless_scattering_white_furnace_energy_conservation_holds`).
#[test]
fn dispersive_scattering_nee_on_and_off_converge_to_the_same_mean() {
    const SAMPLES: u32 = 20_000;
    const TOLERANCE: f32 = 0.02;
    const MAX_BOUNCES: u32 = 64;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let material = GemMaterial::diamond().with_scattering(1.5, 0.3);
    let env_map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [2.0, 2.0, 2.0]);
    let environment = EnvironmentSource::HdrMap(&env_map);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.12, -1.0, 0.06).normalize(),
    };

    let mut sum_on = Vec3::ZERO;
    let mut sum_off = Vec3::ZERO;
    for i in 0..SAMPLES {
        let seed = 77_000 + i;
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        sum_on += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            MAX_BOUNCES,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            true,
            None,
        );
        sum_off += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            MAX_BOUNCES,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            false,
            None,
        );
    }
    let mean_on = sum_on / SAMPLES as f32;
    let mean_off = sum_off / SAMPLES as f32;
    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean_on.x, mean_off.x),
        rel_err(mean_on.y, mean_off.y),
        rel_err(mean_on.z, mean_off.z),
    );
    println!(
        "[dispersive scattering NEE] mean_on={mean_on:?} mean_off={mean_off:?} \
         rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "a dispersive scattering material's NEE-on and NEE-off means should converge \
         (mean_on={mean_on:?}, mean_off={mean_off:?}, rel_err=({ex}, {ey}, {ez}), \
         tolerance={TOLERANCE})"
    );
}

/// F-09b regression, absorbing-medium companion to the dispersive test above: an
/// ABSORBING scattering material (`GemMaterial::ruby()`, chromatic per-channel
/// absorption) makes the NEE deposit's mis-weighting visible even without dispersion,
/// since absorption alone already makes each channel's own `path_pdf` diverge over the
/// path -- measured at ~+0.4-0.9% before this fix (0.6006 -> 0.6062), within
/// ~0.1% after (-> 0.6008). Tolerance kept at 0.5% (10x the measured residual)
/// for CPU-only sample-budget headroom.
#[test]
fn ruby_scattering_nee_on_and_off_agree_within_half_a_percent() {
    const SAMPLES: u32 = 20_000;
    const TOLERANCE: f32 = 0.005;
    const MAX_BOUNCES: u32 = 64;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let material = GemMaterial::ruby().with_scattering(1.2, 0.4);
    let env_map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [2.0, 2.0, 2.0]);
    let environment = EnvironmentSource::HdrMap(&env_map);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.1, -1.0, 0.05).normalize(),
    };

    let mut sum_on = Vec3::ZERO;
    let mut sum_off = Vec3::ZERO;
    for i in 0..SAMPLES {
        let seed = 91_000 + i;
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        sum_on += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            MAX_BOUNCES,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            true,
            None,
        );
        sum_off += trace_spectral_ray_inner(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            MAX_BOUNCES,
            environment,
            seed,
            hero_rand,
            None,
            true,
            true,
            false,
            None,
        );
    }
    let mean_on = sum_on / SAMPLES as f32;
    let mean_off = sum_off / SAMPLES as f32;
    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean_on.x, mean_off.x),
        rel_err(mean_on.y, mean_off.y),
        rel_err(mean_on.z, mean_off.z),
    );
    println!(
        "[ruby scattering NEE] mean_on={mean_on:?} mean_off={mean_off:?} \
         rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "ruby's absorbing scattering medium NEE-on and NEE-off means should agree \
         within {TOLERANCE} (mean_on={mean_on:?}, mean_off={mean_off:?}, \
         rel_err=({ex}, {ey}, {ez}))"
    );
}
