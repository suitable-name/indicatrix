//! Tests of the built-in Ametrine: the zoning it carries, that the two zones
//! are the catalogue's quartz iron recipes, the color of each half, the optical depth across the
//! boundary, that a color variant drops the zones, and the quartz optics it shares with Citrine.

use crate::{
    color::{Illuminant, body_colors},
    optics::{
        absorption::AbsorptionTensor,
        chromophore::{ChromophoreCatalogue, ColorRecipe, resolve},
        materials::{
            AMETRINE_AMETHYST_FE_PPMA, AMETRINE_CITRINE_FE_PPMA, AbsorptionUnit, GemMaterial,
            ZONE_SWATCH_FALLBACK_MODEL_WIDTH, ametrine_zoning, zone_swatches,
        },
        zoning::{ZoneShape, segment_optical_depth},
    },
    render_setup::{MODEL_UNIT_FACE_UP_PATH, effective_stone_width_mm},
};
use glam::DVec3;

/// Avogadro's number, for the ppmw to ppma conversion check.
const N_AVOGADRO: f64 = 6.022_140_76e23;

/// The quartz absorption for `iron_ppma` of iron (ppma of all atoms) after `treatments`,
/// resolved from the embedded catalogue: the recipe the module documents, written out again.
fn quartz_tensor(iron_ppma: f64, treatments: &[&str]) -> AbsorptionTensor {
    let catalogue = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("quartz", catalogue.data_version);
    assert!(
        recipe.set_amount("Fe", iron_ppma),
        "the iron amount is finite"
    );
    recipe.treatments = treatments.iter().map(|id| (*id).to_owned()).collect();
    let (tensor, _warnings) = resolve(&recipe, catalogue).expect("the quartz iron recipe resolves");
    tensor
}

/// The CIELAB hue angle in degrees, `[0, 360)`.
fn hue_deg(lab: [f64; 3]) -> f64 {
    lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0)
}

/// The built-in is a per-millimetre quartz with one half-space zone on `x >= 0`, and its optics
/// are Citrine's.
#[test]
fn ametrine_is_a_zoned_per_mm_quartz() {
    let ametrine = GemMaterial::by_name("Ametrine").expect("Ametrine is a built-in");
    assert_eq!(ametrine.name, "Ametrine");
    assert_eq!(
        ametrine,
        GemMaterial::ametrine(),
        "the table is deterministic"
    );
    let zoning = ametrine
        .zoning
        .as_ref()
        .expect("Ametrine carries color zones");
    assert_eq!(zoning.zones.len(), 1, "exactly one shaped zone");
    let ZoneShape::HalfSpace { normal, offset } = &zoning.zones[0].shape else {
        panic!("the shaped zone is a half space");
    };
    assert_eq!(*normal, DVec3::X);
    assert_eq!(*offset, 0.0);
    assert_eq!(zoning.boundary_softness_mm, 0.0, "a sharp boundary");
    assert_ne!(
        zoning.base.tensor, zoning.zones[0].absorption.tensor,
        "the two halves differ"
    );
    assert_eq!(ametrine.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(
        ametrine.absorption, zoning.base.tensor,
        "the plain absorption is the base zone"
    );
    assert_eq!(zoning.validate(), Ok(()));

    let citrine = GemMaterial::by_name("Citrine").expect("Citrine is a built-in");
    assert_eq!(ametrine.dispersion, citrine.dispersion);
    assert_eq!(
        ametrine.birefringence_delta.to_bits(),
        citrine.birefringence_delta.to_bits()
    );
    assert_eq!(
        ametrine.uniaxial_extraordinary_dispersion,
        citrine.uniaxial_extraordinary_dispersion
    );
    assert_eq!(ametrine.c_axis, citrine.c_axis);
    assert_eq!(ametrine.crystal_system, citrine.crystal_system);
    assert_eq!(ametrine.optical_character, citrine.optical_character);
    assert!(ametrine.gpu_supported(), "the GPU zone table renders it");
}

/// The two zone tensors are the quartz host's iron recipes, the amethyst one after irradiation and
/// the citrine one after irradiation and the 450 C anneal, both of the same iron (the citrine half
/// is the amethyst half annealed), and that iron is the 80 ppmw of the Anahi amethyst expressed in
/// ppma of all atoms.
#[test]
fn ametrine_zones_follow_the_catalogue_recipes() {
    let catalogue = ChromophoreCatalogue::global();
    let quartz = catalogue.host("quartz").expect("the quartz host");
    assert_eq!(
        quartz.element_unit("Fe").as_deref(),
        Some("ppma_all"),
        "the iron amounts are ppma of all atoms"
    );
    let anahi_ppma =
        80.0e-6 * quartz.density_g_cm3 / 55.845 * N_AVOGADRO / quartz.n_site_for_unit("ppma_all");
    assert!(
        (AMETRINE_AMETHYST_FE_PPMA - anahi_ppma).abs() < 0.05,
        "80 ppmw Fe is {anahi_ppma:.2} ppma, the constant is {AMETRINE_AMETHYST_FE_PPMA}"
    );
    assert_eq!(
        AMETRINE_CITRINE_FE_PPMA.to_bits(),
        AMETRINE_AMETHYST_FE_PPMA.to_bits(),
        "the citrine half is the amethyst half annealed: the same iron"
    );

    let amethyst = quartz_tensor(AMETRINE_AMETHYST_FE_PPMA, &["gamma_irradiation"]);
    let citrine = quartz_tensor(
        AMETRINE_CITRINE_FE_PPMA,
        &["gamma_irradiation", "thermal_anneal_450"],
    );
    let zoning = ametrine_zoning();
    assert_eq!(zoning.base.tensor, amethyst);
    assert_eq!(zoning.zones[0].absorption.tensor, citrine);
}

/// Face-up, the base zone is a purple (amethyst) and the shaped zone a yellow-orange (citrine),
/// both with a visible chroma and neither black nor clear. The CIELAB triples are printed for
/// tuning the citrine iron with `--nocapture`.
#[test]
fn ametrine_halves_are_purple_and_yellow() {
    let ametrine = GemMaterial::ametrine();
    let swatches = zone_swatches(&ametrine, 0.0);
    assert_eq!(swatches.len(), 2, "the base zone and one shaped zone");

    // The path `zone_swatches` uses for an unsized material: 2.52 model units of face-up path at
    // 7 mm over the fallback model width of 2.0, i.e. 8.82 mm.
    let mm_per_unit = f64::from(effective_stone_width_mm(0.0)) / ZONE_SWATCH_FALLBACK_MODEL_WIDTH;
    let path_mm = f64::from(MODEL_UNIT_FACE_UP_PATH) * mm_per_unit;
    let zoning = ametrine
        .zoning
        .as_ref()
        .expect("Ametrine carries color zones");
    let halves = [&zoning.base, &zoning.zones[0].absorption]
        .map(|zone| body_colors(&zone.tensor, path_mm, Illuminant::D65).unpolarised);
    for (name, half) in ["amethyst half", "citrine half"].iter().zip(&halves) {
        eprintln!("{name}: Lab {:.1?}, sRGB {:?}", half.lab, half.srgb);
    }
    for (swatch, half) in swatches.iter().zip(&halves) {
        assert_eq!(*swatch, half.srgb.map(|v| f32::from(v) / 255.0));
    }

    let [purple, yellow] = halves.map(|half| half.lab);
    let (hue_purple, hue_yellow) = (hue_deg(purple), hue_deg(yellow));
    assert!(
        (270.0..=345.0).contains(&hue_purple),
        "amethyst half hue {hue_purple:.1} deg (Lab {purple:.1?}), expected purple 270-345"
    );
    assert!(
        (55.0..=100.0).contains(&hue_yellow),
        "citrine half hue {hue_yellow:.1} deg (Lab {yellow:.1?}), expected yellow-orange 55-100"
    );
    for (name, lab) in [("amethyst half", purple), ("citrine half", yellow)] {
        assert!(
            lab[1].hypot(lab[2]) >= 10.0,
            "{name}: chroma {:.1} is too pale (Lab {lab:.1?})",
            lab[1].hypot(lab[2])
        );
        assert!(
            (15.0..=95.0).contains(&lab[0]),
            "{name}: L* {:.1} is black or clear (Lab {lab:.1?})",
            lab[0]
        );
    }
}

/// A straight segment in the amethyst half (`x < 0`) has that zone's optical depth, one in the
/// citrine half (`x >= 0`) the other zone's, and one across the plane `x = 0` the sum of the two.
#[test]
fn a_segment_on_each_side_of_the_boundary_takes_that_halfs_absorption() {
    const LAMBDA_NM: f64 = 550.0;
    let zoning = ametrine_zoning();
    let amethyst = zoning.base.alpha(LAMBDA_NM, None);
    let citrine = zoning.zones[0].absorption.alpha(LAMBDA_NM, None);
    assert!(
        (amethyst - citrine).abs() > 1e-6,
        "test premise: the halves absorb differently at {LAMBDA_NM} nm ({amethyst} vs {citrine})"
    );
    let depth = |from: f64, to: f64| {
        segment_optical_depth(
            &zoning,
            DVec3::new(from, 0.0, 0.0),
            DVec3::new(to, 0.0, 0.0),
            LAMBDA_NM,
        )
    };
    assert!(
        (depth(-2.0, -1.0) - amethyst).abs() < 1e-9,
        "x < 0 is amethyst"
    );
    assert!(
        (depth(1.0, 2.0) - citrine).abs() < 1e-9,
        "x >= 0 is citrine"
    );
    assert!(
        (depth(-1.0, 1.0) - (amethyst + citrine)).abs() < 1e-9,
        "a segment across the boundary has one millimetre in each half"
    );
}

/// A color variant replaces the color, and a zoned material's color is its zones: both setters
/// drop the zones, and keep their own unit.
#[test]
fn a_body_colour_drops_the_zones() {
    let triple = GemMaterial::ametrine().with_body_color([0.2, 0.3, 0.1]);
    assert!(triple.zoning.is_none());
    assert_eq!(triple.absorption_unit, AbsorptionUnit::ModelUnit);

    let banded = GemMaterial::ametrine().with_body_color_bands(&[[550.0, 30.0, 0.2]], 1.0);
    assert!(banded.zoning.is_none());
    assert_eq!(banded.absorption_unit, AbsorptionUnit::PerMm);
}

/// The quartz targets of the other two quartz built-ins hold here too (instead of adding the
/// name to the fixed tables of `dispersion.rs` and `biaxial.rs`): `n_d` 1.544 and a
/// birefringence of 0.0091.
#[test]
fn ametrine_matches_the_quartz_targets() {
    let ametrine = GemMaterial::ametrine();
    let n_d = ametrine.dispersion.evaluate(589.3);
    assert!((n_d - 1.544).abs() <= 0.001, "n_d {n_d}");
    assert!(
        (ametrine.birefringence_delta - 0.0091).abs() <= 1e-4,
        "birefringence {}",
        ametrine.birefringence_delta
    );
}
