//! `spectral_absorption` / `AbsorptionBand` tests: real material band orientation,
//! single-band peak/symmetry and additive band-summing behaviour, and the
//! colourless built-ins' exact zero-absorption guarantee.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        absorption::AbsorptionBand,
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, spectral_absorption, trace_spectral_ray},
    },
};

/// `spectral_absorption` sums real chromophore `AbsorptionBand`s (see
/// `GemMaterial::all_materials`' doc comments for the cited band positions) instead of
/// blending a `[f32; 3]` RGB triple against three fixed sRGB-primary lobes -- this
/// test reads the actual built-in materials' band sets rather than a synthetic
/// standalone triple, so it stays honest about what the shipped data actually does.
#[test]
fn test_spectral_absorption_orientation() {
    let ruby = GemMaterial::ruby();
    assert!(
        spectral_absorption(&ruby.absorption.o_ray, 650.0)
            < spectral_absorption(&ruby.absorption.o_ray, 500.0),
        "Ruby (Cr3+ bands at 410/550nm) should absorb less at 650nm (in the open red \
         transmission window past both bands) than at 500nm (within the yellow-green band's rising flank)"
    );

    let sapphire = GemMaterial::sapphire();
    assert!(
        spectral_absorption(&sapphire.absorption.o_ray, 650.0)
            > spectral_absorption(&sapphire.absorption.o_ray, 500.0),
        "Sapphire (Fe2+-Ti4+ IVCT band centred 580nm) should absorb more at 650nm (closer \
         to the band centre) than at 500nm (further into the band's blue-side flank)"
    );
}

/// Direct unit-level check of `AbsorptionBand::evaluate` and the band-summing shape of
/// `spectral_absorption` itself, independent of any specific material's data: a single
/// band peaks exactly at its own centre and falls off symmetrically, and summing two
/// non-overlapping bands is additive (not the old model's locally-normalized blend).
#[test]
fn test_absorption_band_peaks_at_its_own_centre_and_bands_sum() {
    let band = AbsorptionBand::new(500.0, 20.0, 4.0);
    assert!(
        (band.evaluate(500.0) - 4.0).abs() < 1e-5,
        "a band's own coefficient at its centre must equal its peak exactly"
    );
    assert!(
        band.evaluate(480.0) < 4.0 && band.evaluate(520.0) < 4.0,
        "absorption must fall off away from the band centre"
    );
    assert!(
        (band.evaluate(480.0) - band.evaluate(520.0)).abs() < 1e-4,
        "a single Gaussian band must be symmetric about its centre (got {} at -20nm vs {} at +20nm)",
        band.evaluate(480.0),
        band.evaluate(520.0)
    );

    // Two well-separated bands should sum (not locally-normalize-blend): far from
    // both centres, the total should be small; at each band's own centre, the total
    // should be close to (but slightly above, from the other band's tail) that band's
    // own peak.
    let bands = vec![
        AbsorptionBand::new(410.0, 20.0, 3.0),
        AbsorptionBand::new(700.0, 20.0, 1.0),
    ];
    let total_at_410 = spectral_absorption(&bands, 410.0);
    assert!(
        (3.0..3.01).contains(&total_at_410),
        "at 410nm the total should be dominated by that band's own peak (got {total_at_410})"
    );
    let total_between = spectral_absorption(&bands, 555.0);
    assert!(
        total_between < 0.1,
        "far from both band centres, summed absorption should be near zero (got {total_between})"
    );
}

/// Diamond, Moissanite and Cubic Zirconia are colourless -- their
/// `AbsorptionTensor`s must be empty band sets, giving EXACTLY zero absorption at every
/// wavelength (not merely "small"), which is what makes their rendering unchanged from
/// the earlier RGB-triple model (the old `[0.0, 0.0, 0.0]` triple also always evaluated
/// to exactly 0.0 in the old `spectral_absorption`, so this is a genuine equivalence,
/// not just a superficially-similar new behaviour).
#[test]
fn test_colourless_materials_have_zero_absorption_everywhere() {
    for name in ["Diamond", "Synthetic Moissanite", "Cubic Zirconia"] {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        assert!(
            material.absorption.o_ray.is_empty(),
            "{name}'s o-ray band set must be empty"
        );
        assert!(
            material.absorption.e_ray.is_empty(),
            "{name}'s e-ray band set must be empty"
        );
        for lambda in [380.0, 450.0, 500.0, 550.0, 589.3, 650.0, 700.0, 780.0] {
            let alpha_o = spectral_absorption(&material.absorption.o_ray, lambda);
            let alpha_e = spectral_absorption(&material.absorption.e_ray, lambda);
            assert_eq!(
                alpha_o, 0.0,
                "{name} must have exactly zero o-ray absorption at {lambda}nm"
            );
            assert_eq!(
                alpha_e, 0.0,
                "{name} must have exactly zero e-ray absorption at {lambda}nm"
            );
        }
    }

    // And the end-to-end consequence: Beer-Lambert transmittance is exp(-0*path_len) =
    // 1.0 for every colourless material regardless of path length, so a full render
    // must be bit-identical to a build with absorption forcibly zeroed out -- exercised
    // here via Diamond through an actual gem cut, matching the "assert a
    // colourless material's output is unchanged" requirement end-to-end rather than
    // only at the `spectral_absorption` unit level above.
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let mut diamond_forced_zero = diamond.clone();
    diamond_forced_zero.absorption =
        indicatrix::optics::absorption::AbsorptionTensor::isotropic(vec![]);
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    for seed in [1u32, 2, 3, 4242] {
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
            &diamond_forced_zero,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        assert_eq!(
            xyz_a, xyz_b,
            "Diamond's actual (empty) absorption bands must render bit-identically to an explicitly-zeroed AbsorptionTensor (seed={seed})"
        );
    }
}
