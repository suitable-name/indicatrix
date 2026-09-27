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

/// One golden-value test case for `studio_rig_refactor_is_bit_identical_to_pre_refactor_baseline`:
/// (ray, rng seed, lighting preset, light yaw, light pitch, expected diamond XYZ bits, expected sapphire XYZ bits).
type GoldenCase = (Ray, u32, LightingPreset, f32, f32, [u32; 3], [u32; 3]);

/// Golden-value regression pinning `trace_spectral_ray`, `sample_studio_environment`,
/// and `evaluate_gem_optical_metrics` against exact `f32::to_bits()` values (not just
/// approximate equality), across a small case table of diamond/sapphire rays, presets
/// and light poses -- any bit drift here means something in the estimator or these
/// metrics changed, intentionally or not.
///
/// The trace tables pin `trace_spectral_ray`'s exit-event spectral splitting; `fire_index`
/// is calibrated against the F/C bifurcation gate and `FIRE_DEGREES_TO_DISPLAY_SCALE`
/// (275). The trace-table values reflect the tabulated CIE 1931 CMF, the assigned-mode
/// pleochroic absorption and the Mueller frame mirror fix -- see the cause-by-cause note
/// on `non_biaxial_materials_render_bit_identical_to_pre_chapter_04_golden_values`. The
/// `sample_studio_environment` grid and the `evaluate_gem_optical_metrics` values are
/// unaffected by that fix, which is what localises the drift to the spectral transport.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a bit-exact golden-value regression test; splitting the case table from \
              its assertions risks the exact kind of accidental drift this test exists \
              to catch"
)]
fn studio_rig_refactor_is_bit_identical_to_pre_refactor_baseline() {
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
            [0x3ff8_5979, 0x4000_e390, 0x4010_b5ac],
            // Same degenerate normal-incidence-along-c_axis geometry as the seed=1337
            // case above.
            [0x3d14_9934, 0x3c30_fa27, 0x3e3f_4fa1],
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
            [0x3D22_0C97, 0x3D2B_789C, 0x3D45_C548],
            [0x3A80_D76B, 0x3958_89D5, 0x3B9B_FBB5],
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
            [0x3cc8_efb2, 0x3cdc_89c6, 0x3cde_018e],
            // Same degenerate normal-incidence-along-c_axis geometry as the seed=1337
            // case above.
            [0x39f8_7715, 0x38d4_1626, 0x3b14_3b29],
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
            "diamond trace drifted from pre-StudioRig-refactor baseline for seed {seed}, preset {preset:?}"
        );
        assert_eq!(
            [xyz_r.x.to_bits(), xyz_r.y.to_bits(), xyz_r.z.to_bits()],
            expected_ruby,
            "sapphire trace drifted from pre-StudioRig-refactor baseline for seed {seed}, preset {preset:?}"
        );
    }

    // sample_studio_environment direct sampling across a grid of directions.
    let env_dirs = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 0.5, 0.5).normalize(),
        Vec3::new(-0.3, 0.2, 0.9).normalize(),
        Vec3::new(1.0, 0.0, 0.0),
    ];
    let env_expected: [[u32; 3]; 4] = [
        [0x3d95_c284, 0x3db0_45c5, 0x3dac_259b],
        [0x4160_fea9, 0x4184_69d0, 0x4181_5070],
        [0x3c9a_2cbc, 0x3cb5_7813, 0x3cb1_38c6],
        [0x3c91_97fd, 0x3cab_5e6d, 0x3ca7_5ba5],
    ];
    for (d, expected) in env_dirs.into_iter().zip(env_expected) {
        for (&lambda, &expected_bits) in [450.0f32, 550.0, 650.0].iter().zip(expected.iter()) {
            let v =
                sample_studio_environment(d, lambda, LightingPreset::RingLights, 1.0, 0.85, 0.95);
            assert_eq!(
                v.to_bits(),
                expected_bits,
                "sample_studio_environment drifted for dir {d:?}, lambda {lambda}"
            );
        }
    }

    // evaluate_gem_optical_metrics golden values.
    let m = evaluate_gem_optical_metrics(&planes, &diamond, 0.0, 0.45, 0.85, 0.95);
    assert_eq!(
        [
            m.brilliance_pct.to_bits(),
            m.fire_index.to_bits(),
            m.scintillation_pct.to_bits(),
            m.windowing_pct.to_bits(),
            m.extinction_pct.to_bits(),
        ],
        [
            0x4222_2153,
            0x419d_904b,
            0x420b_ce0b,
            0x41fa_e339,
            0x41e0_da21
        ],
        "evaluate_gem_optical_metrics drifted from pre-StudioRig-refactor baseline"
    );
}
