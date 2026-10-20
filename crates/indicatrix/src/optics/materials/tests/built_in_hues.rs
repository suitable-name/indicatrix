//! Hue and shape relations of the built-in colored materials in the default (per model unit)
//! color mode, from the 2026-10-19 color check: the fits, the measured spectra they use
//! (`crates/indicatrix/data/reference_spectra/*.csv`) and the swatch values are documented in each
//! material's entry comment (`crates/indicatrix/src/optics/materials/*.rs`).
//!
//! No pixel is pinned: every assertion is a relation (a hue class, an ordering of two rays, a
//! path-length trend) on the 7 mm swatch. The swatch is the tensor's transmission over ONE model
//! unit of path (the band peaks are the total optical density over the face-up path of a 7 mm
//! stone, see `render_setup::materials::absorption_path_scale_for`), unpolarised, under D65, as
//! `color::body_color::body_colors` mixes it ((2 `T_o` + `T_e`) / 3 for uniaxial, the mean of the
//! three rays for biaxial).

use crate::{
    color::{
        Illuminant,
        body_color::{BodyColor, BodyColors, body_colors, lab_to_lch},
    },
    optics::{
        absorption::{AbsorptionBand, BandShape},
        materials::{AbsorptionUnit, GemMaterial},
    },
    renderer::buffers::MAX_ABSORPTION_BANDS,
};

const PERIDOT: &str = "Peridot";
const SPESSARTINE: &str = "Spessartine Garnet";
const TANZANITE: &str = "Tanzanite";
const RUBY: &str = "Ruby";
const SAPPHIRE: &str = "Sapphire";
const EMERALD: &str = "Emerald";
const AQUAMARINE: &str = "Aquamarine";
const DEMANTOID: &str = "Andradite Garnet (Demantoid)";
const TSAVORITE: &str = "Grossular Garnet (Tsavorite)";

fn material(name: &str) -> GemMaterial {
    GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} must be a built-in material"))
}

/// The swatch colors of `name` over `units` model units of path, under D65.
fn swatch(name: &str, units: f64) -> BodyColors {
    body_colors(&material(name).absorption, units, Illuminant::D65)
}

/// `[L*, C*ab, h_ab in degrees [0, 360)]` of one body color.
fn lch(color: &BodyColor) -> [f64; 3] {
    lab_to_lch(color.lab)
}

/// The unpolarised `[L*, C*ab, h_ab]` at `units` of path.
fn unpolarised(name: &str, units: f64) -> [f64; 3] {
    lch(&swatch(name, units).unpolarised)
}

/// Napierian absorption coefficient of a band set at `lambda_nm`.
fn alpha(bands: &[AbsorptionBand], lambda_nm: f32) -> f64 {
    f64::from(bands.iter().map(|b| b.evaluate(lambda_nm)).sum::<f32>())
}

/// Transmission of a band set over one model unit at `lambda_nm`.
fn transmission(bands: &[AbsorptionBand], lambda_nm: f32) -> f64 {
    (-alpha(bands, lambda_nm)).exp()
}

#[test]
fn peridot_swatch_is_yellow_green() {
    let [l, c, h] = unpolarised(PERIDOT, 1.0);
    assert!((100.0..=120.0).contains(&h), "Peridot hue {h:.1}");
    assert!(c >= 45.0, "Peridot chroma {c:.1}");
    assert!((65.0..=85.0).contains(&l), "Peridot lightness {l:.1}");
}

/// Caltech GRR 418 gives h 114 / 113 / 111 at 3 / 5 / 8.8 mm: yellow-green at every size.
#[test]
fn peridot_hue_is_stable_with_path() {
    let h1 = unpolarised(PERIDOT, 1.0)[2];
    let h2 = unpolarised(PERIDOT, 2.0)[2];
    assert!(
        (h2 - h1).abs() <= 6.0,
        "Peridot hue moved from {h1:.1} to {h2:.1} between 1 and 2 units"
    );
}

/// Fitted to Caltech GRR 43 (Fe-poor, "Mandarin" orange): the measured 7 mm swatch is a light
/// vivid orange (L* 77, C* 72, h 66), not the darker, over-saturated tuned stone.
#[test]
fn spessartine_swatch_is_orange() {
    let [l, c, h] = unpolarised(SPESSARTINE, 1.0);
    assert!((55.0..=75.0).contains(&h), "Spessartine hue {h:.1}");
    assert!(c >= 60.0, "Spessartine chroma {c:.1}");
    assert!((70.0..=85.0).contains(&l), "Spessartine lightness {l:.1}");
}

/// Heated tanzanite, fitted to Caltech GRR 2292 (blue zone): violet-blue, and the three rays are
/// ordered as in every measured crystal (GRR 1265, GRR 2292, GRR 3045 after heating): gamma
/// (the `e_ray`) is the lightest and bluest, alpha (the `o_ray`) the darkest and most purplish.
#[test]
fn tanzanite_swatch_is_violet_blue() {
    let colors = swatch(TANZANITE, 1.0);
    let [l, _, h] = lch(&colors.unpolarised);
    assert!((280.0..=305.0).contains(&h), "Tanzanite hue {h:.1}");
    assert!((38.0..=55.0).contains(&l), "Tanzanite lightness {l:.1}");
    let [l_alpha, _, h_alpha] = lch(&colors.o_ray);
    let [l_beta, _, h_beta] = lch(&colors.beta_ray.expect("Tanzanite is biaxial"));
    let [l_gamma, _, h_gamma] = lch(&colors.e_ray);
    assert!(
        l_gamma > l_beta && l_beta > l_alpha,
        "lightness must run gamma {l_gamma:.1} > beta {l_beta:.1} > alpha {l_alpha:.1}"
    );
    assert!(
        h_alpha > h_beta && h_beta > h_gamma,
        "hue must run alpha {h_alpha:.1} (most purplish) > beta {h_beta:.1} > gamma {h_gamma:.1}"
    );
}

#[test]
fn ruby_swatch_is_red_not_mauve() {
    let [l, c, h] = unpolarised(RUBY, 1.0);
    assert!(
        (345.0..360.0).contains(&h) || (0.0..=12.0).contains(&h),
        "Ruby hue {h:.1} must be red, not the old mauve (340)"
    );
    assert!(c >= 55.0, "Ruby chroma {c:.1}");
    assert!((33.0..=50.0).contains(&l), "Ruby lightness {l:.1}");
}

/// GIA Appendix 2: the unpolarised hue goes 348 -> 1 -> 11 with growing density.
#[test]
fn ruby_hue_rotates_toward_red_with_path() {
    let h1 = unpolarised(RUBY, 1.0)[2];
    let h2 = unpolarised(RUBY, 2.0)[2];
    let turn = (h2 - h1).rem_euclid(360.0);
    assert!(
        (5.0..=25.0).contains(&turn),
        "Ruby hue turned {turn:.1} degrees from {h1:.1} to {h2:.1}, expected 5 to 25"
    );
}

/// The o-ray (E perp c) carries the stronger Cr3+ U band (GIA sample 1110).
#[test]
fn ruby_o_ray_absorbs_more_than_e_ray_in_the_green() {
    let ruby = material(RUBY);
    let t_o = transmission(&ruby.absorption.o_ray, 560.0);
    let t_e = transmission(&ruby.absorption.e_ray, 560.0);
    assert!(
        t_o < t_e,
        "T_o(560) {t_o:.2e} must be below T_e(560) {t_e:.2e}"
    );
}

/// Fitted to Caltech GRR 1488: the e-ray (E parallel to c) is a deep violet-blue and the o-ray is
/// nearly colorless, so the unpolarised (2 o + e) / 3 swatch is a light blue (h 247), not cyan.
#[test]
fn aquamarine_swatch_is_light_blue_with_a_colorless_o_ray() {
    let colors = swatch(AQUAMARINE, 1.0);
    let [l, c, h] = lch(&colors.unpolarised);
    assert!((230.0..=260.0).contains(&h), "Aquamarine hue {h:.1}");
    assert!((10.0..=25.0).contains(&c), "Aquamarine chroma {c:.1}");
    assert!((80.0..=90.0).contains(&l), "Aquamarine lightness {l:.1}");
    let [l_o, c_o, _] = lch(&colors.o_ray);
    assert!(
        l_o >= 95.0 && c_o <= 20.0,
        "the o-ray must be near-colorless, got L* {l_o:.1} C* {c_o:.1}"
    );
    let l_e = lch(&colors.e_ray)[0];
    assert!(
        l_e <= 55.0,
        "the e-ray must be the deep blue one, got L* {l_e:.1}"
    );
    // The Fe2+-Fe3+ IVCT band (600-680 nm) is polarised along c: the e-ray is the dark one.
    let aquamarine = material(AQUAMARINE);
    let t_o = transmission(&aquamarine.absorption.o_ray, 680.0);
    let t_e = transmission(&aquamarine.absorption.e_ray, 680.0);
    assert!(
        t_e < t_o,
        "e-ray T(680) {t_e:.3} must be below o-ray T(680) {t_o:.3}"
    );
}

/// Fitted to Caltech GRR 229 (tsavorite) and GIA 2952 (demantoid): both green stones have no
/// transmission window at 650 nm (the 606 nm V3+ band, the 629 nm Cr3+ band) but a measured
/// deep-red window beyond 700 nm (T(740) 0.89 and 0.75); demantoid is the yellower.
#[test]
fn demantoid_and_tsavorite_are_green_with_no_window_at_650_nm() {
    let [_, _, h_demantoid] = unpolarised(DEMANTOID, 1.0);
    let [_, _, h_tsavorite] = unpolarised(TSAVORITE, 1.0);
    for (name, h) in [(DEMANTOID, h_demantoid), (TSAVORITE, h_tsavorite)] {
        assert!((130.0..=160.0).contains(&h), "{name} hue {h:.1}");
        let bands = &material(name).absorption.o_ray;
        let t_650 = transmission(bands, 650.0);
        assert!(t_650 < 0.2, "{name} T(650) {t_650:.3} must be below 0.2");
        let t_740 = transmission(bands, 740.0);
        assert!(
            t_740 > 0.6,
            "{name} T(740) {t_740:.3} must show the measured deep-red window"
        );
    }
    assert!(
        h_demantoid < h_tsavorite,
        "demantoid ({h_demantoid:.1}) must be yellower than tsavorite ({h_tsavorite:.1})"
    );
}

/// The measured-shape refits (owner decision 2026-10-19): the hue class stays blue / green and the
/// refits add the measured o/e dichroism.
#[test]
fn sapphire_and_emerald_refits_keep_their_hue_class() {
    let [l, _, h] = unpolarised(SAPPHIRE, 1.0);
    assert!((275.0..=292.0).contains(&h), "Sapphire hue {h:.1}");
    assert!((50.0..=65.0).contains(&l), "Sapphire lightness {l:.1}");

    let colors = swatch(EMERALD, 1.0);
    let [l, _, h] = lch(&colors.unpolarised);
    assert!((160.0..=175.0).contains(&h), "Emerald hue {h:.1}");
    assert!((58.0..=72.0).contains(&l), "Emerald lightness {l:.1}");
    // Wood & Nassau 1968: o-ray yellowish green, e-ray bluish green.
    let (h_o, h_e) = (lch(&colors.o_ray)[2], lch(&colors.e_ray)[2]);
    assert!(
        h_o < h_e,
        "Emerald o-ray hue {h_o:.1} must be yellower than the e-ray's {h_e:.1}"
    );
}

/// The refitted and rewritten entries are energy-domain (wavenumber Gaussian) band sets, as
/// fitted to the measured spectra; the entries the owner kept are still wavelength Gaussians.
#[test]
fn refitted_entries_use_energy_bands_and_kept_entries_do_not() {
    let all_energy = |name: &str| {
        let t = material(name).absorption;
        t.o_ray
            .iter()
            .chain(&t.e_ray)
            .chain(t.beta_ray.iter().flatten())
            .all(|b| b.shape == BandShape::GaussianEnergy)
    };
    for name in [
        PERIDOT,
        SPESSARTINE,
        TANZANITE,
        RUBY,
        SAPPHIRE,
        EMERALD,
        AQUAMARINE,
        DEMANTOID,
        TSAVORITE,
    ] {
        assert!(all_energy(name), "{name} must carry energy-domain bands");
    }
    for name in ["Amethyst", "Citrine", "Tourmaline"] {
        assert!(!all_energy(name), "{name} must keep its wavelength bands");
    }
}

/// Amethyst, Citrine and Tourmaline are deliberately unchanged (Ametrine takes Citrine's optics
/// bit for bit, and the amethyst / citrine pair stays symmetric): a literal copy of the data as
/// it was before the color check.
#[test]
fn unchanged_built_ins_keep_their_bands() {
    let amethyst = material("Amethyst").absorption;
    assert_eq!(amethyst.o_ray, vec![AbsorptionBand::new(545.0, 55.0, 1.8)]);
    assert_eq!(amethyst.e_ray, amethyst.o_ray);

    let citrine = material("Citrine").absorption;
    assert_eq!(citrine.o_ray, vec![AbsorptionBand::new(400.0, 70.0, 1.6)]);
    assert_eq!(citrine.e_ray, citrine.o_ray);

    let tourmaline = material("Tourmaline").absorption;
    assert_eq!(
        tourmaline.o_ray,
        vec![
            AbsorptionBand::new(720.0, 55.0, 3.0),
            AbsorptionBand::new(430.0, 45.0, 1.8),
        ]
    );
    assert_eq!(
        tourmaline.e_ray,
        vec![
            AbsorptionBand::new(720.0, 55.0, 1.0),
            AbsorptionBand::new(430.0, 45.0, 0.6),
        ]
    );
}

/// The GPU material buffer holds [`MAX_ABSORPTION_BANDS`] bands per ray; a longer set would be
/// truncated silently.
#[test]
fn every_built_in_fits_the_gpu_band_limit() {
    for material in GemMaterial::all_materials() {
        let t = &material.absorption;
        let sets = [Some(&t.o_ray), Some(&t.e_ray), t.beta_ray.as_ref()];
        for (ray, set) in ["o", "e", "beta"].into_iter().zip(sets) {
            let count = set.map_or(0, Vec::len);
            assert!(
                count <= MAX_ABSORPTION_BANDS,
                "{}: the {ray} ray has {count} bands, the GPU holds {MAX_ABSORPTION_BANDS}",
                material.name
            );
        }
    }
}

/// The per-millimetre built-in (the zoned Ametrine) is not a band-data entry: the color check
/// must not have turned it into a per-model-unit one, and its optics stay Citrine's.
#[test]
fn per_mm_built_ins_keep_their_unit_and_zoning() {
    let ametrine = material("Ametrine");
    assert_eq!(ametrine.absorption_unit, AbsorptionUnit::PerMm);
    assert!(ametrine.zoning.is_some(), "Ametrine keeps its zoning");
    let citrine = material("Citrine");
    assert_eq!(ametrine.dispersion, citrine.dispersion);
    assert_eq!(ametrine.c_axis, citrine.c_axis);
    assert_eq!(
        ametrine.birefringence_delta.to_bits(),
        citrine.birefringence_delta.to_bits()
    );
}

#[test]
fn colourless_built_ins_stay_empty() {
    for name in ["Diamond", "Synthetic Moissanite", "Cubic Zirconia"] {
        let t = material(name).absorption;
        assert!(
            t.o_ray.is_empty() && t.e_ray.is_empty() && t.beta_ray.is_none(),
            "{name} must stay colorless"
        );
    }
}
