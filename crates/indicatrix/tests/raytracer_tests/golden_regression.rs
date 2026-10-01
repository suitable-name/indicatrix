//! Bit-exact golden-value regression pinning `trace_spectral_ray`,
//! `sample_studio_environment`, and `evaluate_gem_optical_metrics` against exact
//! `f32::to_bits()` values across a small diamond/sapphire case table.

use glam::Vec3;
use indicatrix::{
    color::metrics::evaluate_gem_optical_metrics,
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};
use std::hint::black_box;

/// One golden-value test case for `studio_rig_trace_output_is_pinned_to_current_values`:
/// (ray, rng seed, lighting preset, light yaw, light pitch, expected diamond XYZ bits, expected sapphire XYZ bits).
type GoldenCase = (Ray, u32, LightingPreset, f32, f32, [u32; 3], [u32; 3]);

/// Golden-value regression pinning `trace_spectral_ray`, `sample_studio_environment`,
/// and `evaluate_gem_optical_metrics` against exact `f32::to_bits()` values (not just
/// approximate equality), across a small case table of diamond/sapphire rays, presets
/// and light poses -- any bit drift here means something in the estimator or these
/// metrics changed, intentionally or not.
///
/// Every table below pins the CURRENT output of the code, captured from a run of this
/// crate: they are drift detectors, not independently derived physics. Re-pin a table
/// only after an intended change to the quantity it covers, and say which change in
/// the commit that re-pins it.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a bit-exact golden-value regression test; splitting the case table from \
              its assertions risks the exact kind of accidental drift this test exists \
              to catch"
)]
fn studio_rig_trace_output_is_pinned_to_current_values() {
    use indicatrix::optics::raytracer::sample_studio_environment;
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let ruby = GemMaterial::sapphire();

    // `expected_diamond` (cubic, `is_anisotropic == false`) stays unchanged across any
    // anisotropic-only physics change; `expected_ruby` (actually `GemMaterial::sapphire()`,
    // genuinely uniaxial) moves whenever entry/internal-reflection/exit birefringence
    // handling changes.
    let cases: [GoldenCase; 4] = [
        (
            Ray {
                origin: Vec3::new(0.0, 2.5, 0.0),
                dir: Vec3::new(0.0, -1.0, 0.0),
            },
            1337,
            LightingPreset::RingLights,
            0.85,
            0.95,
            [0x3c69_75d5, 0x3c73_f69d, 0x3c8a_018e],
            // This ray (dir (0,-1,0)) hits the crown at exactly normal incidence with
            // Sapphire's default `c_axis == Vec3::Y` -- the degenerate wave-normal-
            // parallel-to-optic-axis limit, where both eigenmodes collapse to a single
            // isotropic response at `n_o`.
            [0x3ab2_8ba3, 0x3a3f_8369, 0x3ba5_03d6],
        ),
        (
            Ray {
                origin: Vec3::new(0.0, 2.5, 0.0),
                dir: Vec3::new(0.0, -1.0, 0.0),
            },
            42,
            LightingPreset::Incandescent,
            0.0,
            1.2,
            // Moved by 5 / 3 / 4 ULP on 2026-10-01 when the test profile started
            // optimising this crate (`[profile.test.package.indicatrix]` in the
            // workspace manifest): the studio sampler rounds a few ULP differently
            // between the unoptimised and the optimised build, and a release build had
            // computed these bits all along. Re-pinned to the optimised value, which
            // `cargo test` and `cargo test --release` now share.
            [0x3ff8_597e, 0x4000_e393, 0x4010_b5b0],
            // Same degenerate normal-incidence-along-c_axis geometry as the seed=1337
            // case above. Y moved by exactly one ULP (0x3c30_fa27 -> 0x3c30_fa26) on
            // 2026-09-28 when the degenerate uniaxial entry (k parallel to the c
            // axis) stopped taking its own isotropic fallback and went through the
            // general partial-Fresnel path like the GPU;
            // re-pinned to the new value. Moved again by 6 / 7 / 9 ULP on 2026-10-01
            // with the optimised test profile (see the diamond row above).
            [0x3d14_993a, 0x3c30_fa2d, 0x3e3f_4faa],
        ),
        (
            Ray {
                origin: Vec3::new(0.3, 2.5, 0.1),
                dir: Vec3::new(-0.05, -1.0, 0.02).normalize(),
            },
            9001,
            LightingPreset::DarkSpotlight,
            2.1,
            0.4,
            // This oblique ray (dir = (-0.05, -1.0, 0.02).normalize()) draws the
            // EXTRAORDINARY eigenmode at the air->crystal entry and reaches a real
            // dispersive crystal->air exit with a live companion channel -- the case
            // that exercises exit-event spectral splitting, unlike the other three.
            // Z moved by 2 ULP on 2026-10-01 with the optimised test profile (see the
            // seed=42 diamond row above).
            [0x3d22_0c97, 0x3d2b_789c, 0x3d45_c546],
            [0x3a80_d76b, 0x3958_89d5, 0x3b9b_fbb5],
        ),
        (
            Ray {
                origin: Vec3::new(0.0, 10.0, 0.0),
                dir: Vec3::new(0.0, -1.0, 0.0),
            },
            7,
            LightingPreset::Daylight,
            std::f32::consts::PI,
            0.3,
            // The only case in this table using `LightingPreset::Daylight`, which
            // samples the tabulated CIE D65 measured spectrum rather than a smooth
            // 6500K Planckian.
            [0x3ccc_d14a, 0x3cdc_72bc, 0x3ce4_6a02],
            // Same degenerate normal-incidence-along-c_axis geometry as the seed=1337
            // case above. Y moved by three ULPs (0x38d4_1626 -> 0x38d4_1623) on
            // 2026-09-28 for the same reason as the seed=42 case (the degenerate
            // uniaxial entry now takes the general partial-Fresnel path); re-pinned.
            [0x39fe_e521, 0x38dd_b2d2, 0x3b18_c493],
        ),
    ];

    for (ray, seed, preset, lyaw, lpitch, expected_diamond, expected_ruby) in cases {
        let xyz_d = trace_spectral_ray(
            ray,
            &planes,
            &diamond,
            12,
            preset.studio(1.0, lyaw, lpitch),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        let xyz_r = trace_spectral_ray(
            ray,
            &planes,
            &ruby,
            12,
            preset.studio(1.0, lyaw, lpitch),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        assert_eq!(
            [xyz_d.x.to_bits(), xyz_d.y.to_bits(), xyz_d.z.to_bits()],
            expected_diamond,
            "diamond trace drifted from the pinned baseline for seed {seed}, preset {preset:?}; re-pin only after an intended physics change"
        );
        assert_eq!(
            [xyz_r.x.to_bits(), xyz_r.y.to_bits(), xyz_r.z.to_bits()],
            expected_ruby,
            "sapphire trace drifted from the pinned baseline for seed {seed}, preset {preset:?}; re-pin only after an intended physics change"
        );
    }

    // sample_studio_environment direct sampling across a grid of directions.
    let env_dirs = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 0.5, 0.5).normalize(),
        Vec3::new(-0.3, 0.2, 0.9).normalize(),
        Vec3::new(1.0, 0.0, 0.0),
    ];
    // Re-pinned on 2026-10-01 with the optimised test profile (see the seed=42 diamond
    // row above): the four rows moved by at most 2 ULP per component.
    let env_expected: [[u32; 3]; 4] = [
        [0x3d95_c282, 0x3db0_45c3, 0x3dac_2599],
        [0x4160_feaa, 0x4184_69d1, 0x4181_5071],
        [0x3c9a_2cbb, 0x3cb5_7813, 0x3cb1_38c6],
        [0x3c91_97fc, 0x3cab_5e6d, 0x3ca7_5ba5],
    ];
    // The inputs go through `black_box` so that no build can fold this call at compile
    // time: a release build with LTO otherwise evaluates it in double precision and
    // lands hundreds of ULP from the run-time result the renderer produces.
    for (d, expected) in env_dirs.into_iter().zip(env_expected) {
        for (&lambda, &expected_bits) in [450.0f32, 550.0, 650.0].iter().zip(expected.iter()) {
            let v = sample_studio_environment(
                black_box(d),
                black_box(lambda),
                black_box(LightingPreset::RingLights),
                black_box(1.0),
                black_box(0.85),
                black_box(0.95),
            );
            assert_eq!(
                v.to_bits(),
                expected_bits,
                "sample_studio_environment drifted for dir {d:?}, lambda {lambda}"
            );
        }
    }

    // evaluate_gem_optical_metrics golden values. The bit table below must be re-pinned
    // after the metrics rework (radiance-based illumination test, girdle-scaled sampling
    // fan and the explicit environment argument): every value moves.
    let m = evaluate_gem_optical_metrics(
        &planes,
        &diamond,
        0.0,
        0.45,
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
    );
    assert_eq!(
        [
            m.brilliance_pct.to_bits(),
            m.fire_index.to_bits(),
            m.scintillation_pct.to_bits(),
            m.windowing_pct.to_bits(),
            m.extinction_pct.to_bits(),
        ],
        [
            0x418f_4899,
            0x40b5_ef5a,
            0x421f_d86a,
            0x4213_c2dd,
            0x4234_98d6
        ],
        "evaluate_gem_optical_metrics drifted from the pinned baseline; re-pin only after an intended physics change"
    );
}
