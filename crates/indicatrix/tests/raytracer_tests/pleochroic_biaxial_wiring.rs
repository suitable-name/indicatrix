//! Pleochroic and biaxial-indicatrix absorption wiring tests: every built-in
//! material stays finite through the pleochroic absorption block, Sapphire's
//! uniaxial pleochroism actually attenuates end-to-end, Tanzanite's true biaxial
//! eigenmodes diverge from the uniaxial fallback off-axis, non-biaxial materials'
//! CURRENT render output stays pinned even after the biaxial-indicatrix wiring landed,
//! and the three biaxial built-ins measurably differ from the uniaxial approximation.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        birefringence::{AbsorptionTensor3, BirefringenceParams, effective_pleochroic_alpha},
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};

/// Regression safety net: every built-in material -- including the three
/// biaxial (orthorhombic) species, Alexandrite/Topaz/Tanzanite, which now carry the new
/// `biaxial_delta_beta_alpha` field alongside their existing uniaxial `c_axis` /
/// `birefringence_delta` -- must still trace through `trace_spectral_ray` to a finite,
/// non-negative XYZ across several seeds and an oblique ray direction. This is what
/// would catch a NaN/Inf introduced by the new per-channel pleochroic quadratic-form
/// wiring in `trace_spectral_ray`'s absorption block, which touches every
/// anisotropic material's rendering path, not just the ones exercised by other tests
/// in this file.
#[test]
fn all_builtin_materials_render_finite_through_pleochroic_absorption() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.22, -1.0, -0.13).normalize(),
    };

    for material in GemMaterial::all_materials() {
        for seed in [1u32, 2, 42, 4242] {
            let xyz = trace_spectral_ray(
                ray,
                &planes,
                &material,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            assert!(
                xyz.is_finite(),
                "{}: trace_spectral_ray must produce finite output (seed={seed}), got {xyz:?}",
                material.name
            );
            assert!(
                xyz.x >= 0.0 && xyz.y >= 0.0 && xyz.z >= 0.0,
                "{}: trace_spectral_ray must produce non-negative XYZ (seed={seed}), got {xyz:?}",
                material.name
            );
        }
    }
}

/// The pleochroic absorption block must actually engage for a real
/// anisotropic built-in (Sapphire, uniaxial) traced end-to-end -- i.e. the new
/// per-channel `pleochroic_channel_alpha` wiring in `trace_spectral_ray` must still
/// attenuate light passing through the gem body (not accidentally zero out or bypass
/// absorption), matching the qualitative behaviour the old propagation-angle blend
/// also had. This exercises the actual call site (argument order, `current_plane_normal`
/// wiring, per-channel Stokes vectors) that the focused unit tests in
/// `birefringence::absorption_tensor_tests` cannot reach on their own.
#[test]
fn sapphire_pleochroic_absorption_still_attenuates_end_to_end() {
    const SAMPLES: u32 = 64;

    let planes = StandardGemCuts::standard_round_brilliant();
    let sapphire = GemMaterial::sapphire();
    let mut sapphire_colorless = sapphire.clone();
    sapphire_colorless.absorption =
        indicatrix::optics::absorption::AbsorptionTensor::isotropic(vec![]);

    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.15, -1.0, 0.09).normalize(),
    };

    let mut sum_absorbing = Vec3::ZERO;
    let mut sum_colorless = Vec3::ZERO;
    for i in 0..SAMPLES {
        let seed = hash_u32(0xC0FF_EE00 ^ hash_u32(i));
        sum_absorbing += trace_spectral_ray(
            ray,
            &planes,
            &sapphire,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        sum_colorless += trace_spectral_ray(
            ray,
            &planes,
            &sapphire_colorless,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
    }
    let luma_absorbing = sum_absorbing.y;
    let luma_colorless = sum_colorless.y;

    assert!(
        luma_absorbing < luma_colorless * 0.99,
        "sapphire's real absorption bands should measurably dim the averaged output relative to an \
         otherwise-identical colorless material (absorbing luma={luma_absorbing}, colorless luma={luma_colorless})"
    );
}

/// headline physical claim is that tanzanite's trichroism -- three
/// different colors down three axes -- is a direct consequence of it being biaxial,
/// not uniaxial. `trace_spectral_ray`'s absorption block (wiring) now
/// sources its two eigen-polarizations from `GemMaterial::biaxial_indicatrix` for
/// tanzanite specifically, instead of the uniaxial ordinary/extraordinary
/// approximation that can only ever distinguish TWO colors. This test pins that
/// substitution down directly: at a generic (non-axis-aligned) propagation direction,
/// tanzanite's true biaxial eigenmodes must give a measurably different pleochroic
/// absorption coefficient than the uniaxial fallback would have -- i.e. the new
/// biaxial data is actually being consulted, not silently ignored.
#[test]
fn tanzanite_biaxial_eigenmodes_diverge_from_uniaxial_fallback_off_axis() {
    let tanzanite = GemMaterial::by_name("Tanzanite").unwrap();
    let lambda = 550.0f32;
    // Synthetic, strongly dichroic coefficients: tanzanite's actual built-in absorption
    // bands are still an uncited isotropic placeholder (see its entry's doc comment in
    // `materials::GemMaterial::all_materials` -- the principal
    // INDICES, not chromophore spectroscopy), so alpha_o == alpha_e there and the
    // quadratic form would be eigenmode-independent regardless of which eigenmode pair
    // is used. Using synthetic values isolates exactly what this test checks: that the
    // wiring feeds in genuinely different eigen-DIRECTIONS, independent of whether the
    // material's own absorption data happens to be dichroic yet.
    let (alpha_o, alpha_e) = (1.0f32, 6.0f32);
    let tensor = AbsorptionTensor3::uniaxial(alpha_o, alpha_e, tanzanite.c_axis);

    let propagation_dir = Vec3::new(0.5, 0.4, 0.6).normalize(); // generic, off every principal axis
    let indicatrix = tanzanite
        .biaxial_indicatrix(lambda)
        .expect("tanzanite must expose a biaxial indicatrix");
    let (biaxial_a, biaxial_b) = indicatrix.eigen_polarizations(propagation_dir);
    let uniaxial_a =
        BirefringenceParams::ordinary_eigen_polarization(propagation_dir, tanzanite.c_axis);
    let uniaxial_b =
        BirefringenceParams::extraordinary_eigen_polarization(propagation_dir, tanzanite.c_axis);

    // Fully polarized light aligned with each pair's own first eigenmode: if the two
    // eigenmode pairs were the same direction, these two coefficients would match.
    let alpha_biaxial = effective_pleochroic_alpha(&tensor, biaxial_a, biaxial_a, biaxial_b, 1.0);
    let alpha_uniaxial =
        effective_pleochroic_alpha(&tensor, uniaxial_a, uniaxial_a, uniaxial_b, 1.0);

    assert!(
        (alpha_biaxial - alpha_uniaxial).abs() > 1e-3,
        "tanzanite's true biaxial eigenmodes should give a measurably different pleochroic \
         coefficient than the uniaxial fallback at an off-axis direction (biaxial={alpha_biaxial}, uniaxial={alpha_uniaxial})"
    );
}

/// Wiring the biaxial indicatrix into refraction/walk-off must not perturb a single
/// bit of output for any NON-biaxial material -- cubic (isotropic) or uniaxial. Pins
/// exact hex-encoded `f32` bit patterns (`to_bits()`, a literal bitwise comparison),
/// to CURRENT values (see this test's own name -- despite the golden's derivation below,
/// these are NOT the values from before biaxial wiring; they are what
/// this test now pins going forward), for two cubic materials (Diamond, Cubic
/// Zirconia) and three uniaxial materials spanning different crystal systems and
/// birefringence signs (Sapphire and Ruby -- both trigonal corundum but opposite
/// pleochroism; Synthetic Moissanite -- hexagonal, the strongest birefringence in the
/// catalogue).
#[test]
fn non_biaxial_materials_render_is_pinned_to_current_post_chapter_04_values() {
    const SEED: u32 = 1;

    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    // (material name, expected x/y/z as f32 bit patterns). Diamond/Cubic Zirconia are
    // cubic (isotropic) and must stay bit-identical across any anisotropic-only
    // physics change; Sapphire/Ruby/Synthetic Moissanite are genuinely birefringent and
    // are expected to move whenever the uniaxial Fresnel/absorption/exit-splitting
    // machinery changes.
    //
    // This golden reflects three physics facts together:
    // (1) `color::cie1931` uses the tabulated CIE 1931 2-degree observer rather than the
    //     Wyman/Sloan/Shirley Gaussian fit -- the sole mover of every isotropic row
    //     (Diamond/CZ/Moissanite reproduce the Gaussian-fit bits at 0 ULP when that fit
    //     is substituted back in; the Gaussian fit is 1-3 % off, 50 % at the x-bar
    //     trough); isotropic rows sit 0.04-0.4 % away from the Gaussian-fit values per
    //     component.
    // (2) `raytracer::absorption` computes pleochroic absorption from the geometric
    //     assigned-mode alpha (`birefringence::assigned_mode_alpha`) rather than the
    //     Stokes degree-of-polarization heuristic; substituting the heuristic back in
    //     (with (1) also reverted) reproduces the axis-aligned sapphire cases in
    //     `studio_rig_trace_output_is_pinned_to_current_values` exactly.
    // (3) the Mueller frame-rotation mirror fix (`polarization.rs`) accounts for what
    //     remains on the OBLIQUE uniaxial rows here: Sapphire +115/+226/+112 % and Ruby
    //     +31/+38/+42 % once (1) and (2) are accounted for -- corundum's absorption acts
    //     on the assigned mode's true E-field, so the correctly-mirrored oblique paths
    //     are brighter than an unmirrored frame would produce. This is not toggleable
    //     and is attributed by elimination: mode re-coupling, the TIR guard, exit
    //     splitting, the observer, the backdrop, the RNG streams and the Studio
    //     lighting were each ruled out by toggle or by proof.
    //
    // What these values are: drift detectors captured from the code's own output, not
    // independently verified physics. They catch an unintended change in the tracer's
    // bits; they cannot say whether the pinned radiance is right. An independent check
    // would need a single-wavelength analytic absorption case: one channel, a
    // non-dispersive slab or a path of known geometric length, with the transmitted
    // fraction compared against the closed-form Beer-Lambert `exp(-alpha * length)`
    // for the assigned mode's absorption coefficient.
    //
    // Re-pinned on 2026-10-01 when the test profile started optimising this crate
    // (`[profile.test.package.indicatrix]` in the workspace manifest): every row moved
    // by 1 to 3 ULP per component, because the studio sampler rounds a few ULP
    // differently between the unoptimised and the optimised build, and a release build
    // had computed these bits all along. `cargo test` and `cargo test --release` now
    // share them.
    let golden: &[(&str, u32, u32, u32)] = &[
        ("Diamond", 0x3F12_4125, 0x3F16_03EE, 0x3F1C_561C),
        ("Cubic Zirconia", 0x3FDA_E16D, 0x3FD1_A43F, 0x4018_44C4),
        ("Sapphire", 0x3B75_3359, 0x3B0C_1C8B, 0x3C95_4BE3),
        ("Ruby", 0x3C12_D2A3, 0x3B9D_0EF1, 0x3C0C_1D99),
        (
            "Synthetic Moissanite",
            0x3E8D_87DA,
            0x3E8C_CAC6,
            0x3EA8_5663,
        ),
    ];

    for &(name, bx, by, bz) in golden {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let xyz = trace_spectral_ray(
            ray,
            &planes,
            &material,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            SEED,
            (hash_u32(SEED) as f32) / 4_294_967_295.0,
            None,
        );
        assert_eq!(
            xyz.x.to_bits(),
            bx,
            "{name}: x component bit pattern changed (got {:?})",
            xyz.x
        );
        assert_eq!(
            xyz.y.to_bits(),
            by,
            "{name}: y component bit pattern changed (got {:?})",
            xyz.y
        );
        assert_eq!(
            xyz.z.to_bits(),
            bz,
            "{name}: z component bit pattern changed (got {:?})",
            xyz.z
        );
    }
}

/// refraction/walk-off wiring must actually be consulted for the three
/// biaxial built-ins (Alexandrite, Topaz, Tanzanite), not silently dead code. Compares
/// each biaxial material's real render against an otherwise-identical clone with
/// `biaxial_delta_beta_alpha` forced to `None` -- which routes that same clone through
/// the pre-existing uniaxial ordinary/extraordinary approximation (`c_axis` as the
/// single optic axis, `birefringence_delta` as `n_gamma - n_o`) instead of the true
/// biaxial Fresnel-equation solve. Averaged over many paired samples (same seed for
/// both variants, to cancel unrelated RNG noise -- a null check confirms comparing a
/// material against ITSELF this way gives exactly zero, so any nonzero diff here is
/// genuine signal, not sampling noise) at an off-axis ray so the wave normal is
/// genuinely oblique to every principal axis.
///
/// The three built-ins are NOT equally biaxial: both Alexandrite and Topaz have
/// `n_beta` sitting noticeably closer to `n_alpha` than to `n_gamma` (see their
/// entries' doc comments in `materials::GemMaterial::all_materials` -- roughly 26% and
/// 30% of the way from alpha to gamma, respectively), so both are only weakly biaxial
/// (nearly uniaxial) and the uniaxial fallback is a fairly good approximation for them
/// specifically -- the measured effect size at this ray is diff~4e-5 (Alexandrite) and
/// diff~1e-5 (Topaz), vs. diff~1e-2 for the more strongly biaxial Tanzanite. The
/// threshold below (`1e-6`) is set an order of magnitude below the smallest of these
/// measured effects, safely above f32 noise for a paired comparison at ~0.1 magnitude
/// -- i.e. this is not a weakened test, just one sized to genuinely small physical
/// effects for two of the three materials.
#[test]
fn biaxial_materials_measurably_differ_from_uniaxial_fallback_in_refraction() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let samples = 64u32;

    for (name, min_diff) in [("Alexandrite", 1e-6), ("Topaz", 1e-6), ("Tanzanite", 1e-6)] {
        let biaxial = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        assert!(
            biaxial.biaxial_delta_beta_alpha.is_some(),
            "{name} must be a genuinely biaxial built-in"
        );

        let mut uniaxial_fallback = biaxial.clone();
        uniaxial_fallback.biaxial_delta_beta_alpha = None;

        let mut sum_biaxial = Vec3::ZERO;
        let mut sum_fallback = Vec3::ZERO;
        for i in 0..samples {
            let seed = i.wrapping_mul(7919).wrapping_add(13);
            sum_biaxial += trace_spectral_ray(
                ray,
                &planes,
                &biaxial,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            sum_fallback += trace_spectral_ray(
                ray,
                &planes,
                &uniaxial_fallback,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
        }
        let avg_biaxial = sum_biaxial / samples as f32;
        let avg_fallback = sum_fallback / samples as f32;
        let diff = (avg_biaxial - avg_fallback).length();

        assert!(
            diff > min_diff,
            "{name}: true biaxial refraction should render measurably differently (averaged over {samples} samples) \
             than the uniaxial ordinary/extraordinary fallback -- got diff={diff} (threshold={min_diff}, \
             biaxial={avg_biaxial:?}, fallback={avg_fallback:?})"
        );
    }
}
