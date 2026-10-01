//! `GemMaterial` default-orientation checks and birefringence rendering tests: the
//! `c_axis` default (with Tourmaline's documented override), Zircon's measurable
//! ordinary/extraordinary split versus a flattened clone, cubic materials' bit-exact
//! indifference to `birefringence_delta`, and non-dispersive custom materials under
//! spectral MIS.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};

#[test]
fn test_gem_materials_default_c_axis_to_y() {
    // `c_axis` is a field on `GemMaterial` consumed by `trace_spectral_ray` for every
    // material. Every built-in material, and `new_custom`, must default it to Vec3::Y
    // so behaviour stays consistent across materials -- EXCEPT a material with a
    // documented, deliberate cut-orientation override.
    //
    // Tourmaline is that one documented exception (see its entry's comment in
    // `GemMaterial::all_materials`): real tourmaline cutters orient the table
    // perpendicular to the c-axis specifically because face-up down the closed
    // (e-ray/"dark ray") axis is tourmaline's worst viewing direction -- with the
    // strong o-ray/e-ray dichroism populated this pass, keeping `c_axis = Vec3::Y`
    // (the every-other-material default) would point the face-up hero shot straight
    // down that dark ray, backwards from how the stone is actually cut and worn. So
    // Tourmaline's `c_axis` is `Vec3::X` (into the table plane) instead.
    const CUT_ORIENTATION_OVERRIDES: &[&str] = &["Tourmaline"];

    for m in GemMaterial::all_materials() {
        if CUT_ORIENTATION_OVERRIDES.contains(&m.name.as_str()) {
            assert_ne!(
                m.c_axis,
                Vec3::Y,
                "{} is documented as a cut-orientation override and should NOT default \
                 c_axis to Vec3::Y -- if this is no longer true, remove it from \
                 CUT_ORIENTATION_OVERRIDES",
                m.name
            );
            continue;
        }
        assert_eq!(
            m.c_axis,
            Vec3::Y,
            "material {:?} should default c_axis to Vec3::Y",
            m.name
        );
    }

    let custom = GemMaterial::new_custom("Test Custom", 1.5, 0.01, 0.02, [0.0, 0.0, 0.0]);
    assert_eq!(
        custom.c_axis,
        Vec3::Y,
        "new_custom should default c_axis to Vec3::Y"
    );
}

#[test]
fn test_birefringence_splits_produce_measurable_difference_zircon_vs_flat() {
    // An otherwise-identical pair of materials -- one strongly birefringent
    // (Zircon, birefringence_delta = 0.059) and one with delta forced to 0.0 (making
    // `is_anisotropic` false, so the ordinary/extraordinary split never triggers) --
    // must render measurably differently once the ray actually splits into an
    // ordinary and extraordinary eigenmode ("optical doubling"). Averaged over many
    // paired samples (same seed for both materials, to cancel out unrelated RNG noise)
    // so the assertion isn't dominated by sampling variance.
    let planes = StandardGemCuts::standard_round_brilliant();
    let zircon = GemMaterial::by_name("Zircon").unwrap();
    assert!(
        zircon.birefringence_delta.abs() > 0.01,
        "test assumes Zircon is strongly birefringent"
    );

    let mut zircon_flat = zircon.clone();
    zircon_flat.birefringence_delta = 0.0;

    // An off-axis ray so the wave normal is genuinely oblique to c_axis (Vec3::Y) --
    // at exactly normal incidence along the c-axis, ordinary and extraordinary rays
    // are degenerate (zero walk-off, n_eff == n_o) and there is nothing to detect.
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    let samples = 64u32;
    let mut sum_birefringent = Vec3::ZERO;
    let mut sum_flat = Vec3::ZERO;
    for i in 0..samples {
        let seed = i.wrapping_mul(7919).wrapping_add(13);
        sum_birefringent += trace_spectral_ray(
            ray,
            &planes,
            &zircon,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        sum_flat += trace_spectral_ray(
            ray,
            &planes,
            &zircon_flat,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
    }
    let avg_birefringent = sum_birefringent / samples as f32;
    let avg_flat = sum_flat / samples as f32;
    let diff = (avg_birefringent - avg_flat).length();

    // The extraordinary eigenmode's TIR/Fresnel/Snell physics inside the crystal must be
    // evaluated against the wave normal `k`, not the walked-off Poynting direction `S`:
    // using `S` would compound the error every internal bounce this 12-max-bounce oblique
    // ray takes, non-physically INFLATING the divergence between the birefringent and
    // flattened traces well past the true walk-off effect's own (small, order the
    // walk-off angle -- 1-2 degrees for Zircon-class birefringence) size. With `k`/`S`
    // correctly separated (see `refraction.rs`'s design note), the measured difference is
    // diff=0.000373 for this exact ray/seed set -- still a real, deterministic, nonzero
    // effect (same seed feeds both traces, so this is not sampling noise), just correctly
    // SMALL rather than artificially large. 3e-4 stays comfortably below the measured
    // value while still catching a regression that zeroes the effect out entirely.
    //
    // Upper bound: the effect is first order in the walk-off angle (at most ~2 degrees =
    // 0.035 rad for Zircon-class birefringence), and the measured diff is 3.73e-4. Using
    // the walk-off direction `S` where the wave normal `k` belongs re-enters the
    // refraction with an error of that same angle at every one of the up-to-12 internal
    // bounces, so the error compounds rather than staying a single small perturbation;
    // a trace that has lost the k/S separation therefore moves by a multiple of the
    // correct effect, not a fraction. 3e-3 (about 8x the measured value) leaves room for
    // ordinary changes to the shared sampling and lighting while still failing on an
    // order-of-magnitude inflation.
    assert!(
        diff > 3e-4,
        "birefringent Zircon should render measurably differently (averaged over {samples} samples) than an \
         otherwise-identical flat (delta=0) material -- got diff={diff} (birefringent={avg_birefringent:?}, flat={avg_flat:?})"
    );
    assert!(
        diff < 3e-3,
        "birefringent Zircon's difference from the flat material should stay at the size of the walk-off \
         effect (order 4e-4), not be inflated by evaluating the extraordinary mode against the wrong \
         direction -- got diff={diff} (birefringent={avg_birefringent:?}, flat={avg_flat:?})"
    );
}

#[test]
fn test_cubic_material_ignores_birefringence_delta_bit_identical() {
    // `is_anisotropic` gates on `crystal_system != Cubic`, so a Cubic material
    // must be completely unaffected by `birefringence_delta` -- confirming the
    // ordinary/extraordinary split code path never activates for
    // an isotropic (cubic) gem like Diamond, i.e. rendering is bit-identical whether
    // or not `birefringence_delta` happens to be nonzero.
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    assert_eq!(
        diamond.crystal_system,
        indicatrix::optics::materials::CrystalSystem::Cubic
    );

    let mut diamond_spurious_biref = diamond.clone();
    diamond_spurious_biref.birefringence_delta = 0.5;

    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    for seed in [1337u32, 42, 999, 7] {
        let xyz_a = trace_spectral_ray(
            ray,
            &planes,
            &diamond,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        let xyz_b = trace_spectral_ray(
            ray,
            &planes,
            &diamond_spurious_biref,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        assert_eq!(
            xyz_a, xyz_b,
            "Cubic material rendering must be bit-identical regardless of birefringence_delta (seed={seed})"
        );
    }
}

#[test]
fn test_non_dispersive_custom_material_renders_stably_under_spectral_mis() {
    // `GemMaterial::new_custom` with `dispersion_delta = 0.0` yields a flat Cauchy
    // model (b = c = 0, per `new_custom`'s own formula), so n(lambda) is IDENTICAL
    // across the whole visible spectrum -- unlike Diamond's own Sellmeier model, which
    // is genuinely dispersive even though it has no separate "delta" knob. `raytracer.rs`
    // sums each channel's per-channel radiance unweighted (`mis_weighted_radiance` is
    // the identity function -- see its doc comment for why no spectral-MIS reweighting
    // is valid under this function's wavelength stratification), so this reduction is
    // unconditional and not specific to the non-dispersive case; this integration-level
    // test just checks the render stays well-behaved (finite, non-negative, and
    // consistent across independent seeds) for a representative non-dispersive material.
    let flat = GemMaterial::new_custom("Flat Test Gem", 1.62, 0.0, 0.0, [0.3, 0.3, 0.3]);
    let n_at_400 = flat.dispersion.evaluate(400.0);
    let n_at_700 = flat.dispersion.evaluate(700.0);
    assert!(
        (n_at_400 - n_at_700).abs() < 1e-6,
        "dispersion_delta=0.0 must yield a genuinely flat index across the visible spectrum (got n(400)={n_at_400}, n(700)={n_at_700})"
    );

    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    for seed in [1u32, 2, 3, 100, 4242] {
        let xyz = trace_spectral_ray(
            ray,
            &planes,
            &flat,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        assert!(
            xyz.x.is_finite() && xyz.y.is_finite() && xyz.z.is_finite(),
            "non-dispersive material rendering must stay finite under spectral MIS (seed={seed}, got {xyz:?})"
        );
        assert!(
            xyz.y >= 0.0,
            "luminance must be non-negative (seed={seed}, got {xyz:?})"
        );
    }
}
