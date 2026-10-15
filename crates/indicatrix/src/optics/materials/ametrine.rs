//! The built-in Ametrine: bicolour quartz with an amethyst half and a
//! citrine half, both colours taken from the chromophore catalogue's quartz iron recipes.
//!
//! The two zone tensors are resolved once per process (`zone_tensors`), so
//! [`GemMaterial::all_materials`] stays cheap and every call returns the same bytes.

use super::GemMaterial;
use crate::optics::{
    absorption::AbsorptionTensor,
    chromophore::{ChromophoreCatalogue, ColorRecipe, resolve},
    zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
};
use glam::DVec3;
use std::sync::OnceLock;

/// Iron of the amethyst half, in ppma of all atoms (the unit of the quartz host's iron).
///
/// This is 80 ppmw Fe in quartz converted through the host's number density:
/// `80e-6 * 2.65 g/cm3 / 55.845 g/mol * N_A / 7.968712e22 cm^-3 * 1e6 = 28.7`. 80 ppmw is the Anahi
/// (Bolivia) amethyst whose spectrum and stated color the catalogue test
/// `measured_color_tests::recipe_vs_measured_amethyst_e_perp_c` compares the recipe against. The
/// recipe's treatment is `gamma_irradiation`, which turns 10 % of the iron into the `[FeO4]0`
/// amethyst centre.
pub const AMETRINE_AMETHYST_FE_PPMA: f64 = 28.7;

/// Iron of the citrine half, in ppma of all atoms: the same iron as the amethyst half.
///
/// The recipe's treatments are `gamma_irradiation` and then `thermal_anneal_450`: the catalogue's
/// own "heated amethyst is citrine" chain, in which the anneal removes the `[FeO4]0` centre and
/// creates the nano-inclusion `Fe3+` chromophore from 50 % of the iron. With the same iron the
/// citrine half is literally the amethyst half after the anneal, which is what heating a real
/// ametrine does to the whole stone: both sectors grow with the same iron and differ in the centre
/// the iron forms, not in its amount. The catalogue holds no iron measurement of a real ametrine's
/// citrine sector, so this is the one choice that adds no number of its own.
///
/// At the 7 mm face-up path (8.82 mm) this is a golden yellow-orange, CIELAB L* 84, C* 88,
/// hue 88 deg (sRGB `[254, 204, 13]`), against the amethyst half's medium purple, L* 54, C* 41,
/// hue 332 deg (`[174, 105, 162]`). At twice the path the citrine half deepens towards
/// orange (hue about 79 deg) and the amethyst half to a dark purple (L* 30). Lower amounts give a
/// paler lemon: 16 ppma is L* 90, hue 94 deg, and 4 ppma is a near-clear L* 97.
pub const AMETRINE_CITRINE_FE_PPMA: f64 = AMETRINE_AMETHYST_FE_PPMA;

/// Treatments of the amethyst half's recipe.
const AMETHYST_TREATMENTS: &[&str] = &["gamma_irradiation"];

/// Treatments of the citrine half's recipe, in the catalogue's own order.
const CITRINE_TREATMENTS: &[&str] = &["gamma_irradiation", "thermal_anneal_450"];

/// The absorption of quartz holding `iron_ppma` of iron after `treatments`, per millimetre.
///
/// The catalogue is embedded in the binary, so the recipe resolving is an invariant of this
/// module and not something a caller can trigger.
fn quartz_iron_tensor(iron_ppma: f64, treatments: &[&str]) -> AbsorptionTensor {
    let catalogue = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("quartz", catalogue.data_version);
    assert!(
        recipe.set_amount("Fe", iron_ppma),
        "the iron amount is finite"
    );
    recipe.treatments = treatments.iter().map(|id| (*id).to_owned()).collect();
    let (tensor, _warnings) = resolve(&recipe, catalogue)
        .expect("the embedded catalogue resolves the quartz iron recipes");
    tensor
}

/// The amethyst and the citrine tensor, resolved once per process.
fn zone_tensors() -> &'static (AbsorptionTensor, AbsorptionTensor) {
    static TENSORS: OnceLock<(AbsorptionTensor, AbsorptionTensor)> = OnceLock::new();
    TENSORS.get_or_init(|| {
        (
            quartz_iron_tensor(AMETRINE_AMETHYST_FE_PPMA, AMETHYST_TREATMENTS),
            quartz_iron_tensor(AMETRINE_CITRINE_FE_PPMA, CITRINE_TREATMENTS),
        )
    })
}

/// The zoning of the built-in Ametrine: amethyst everywhere (the base zone) and citrine on the
/// `x >= 0` side of the plane `x = 0`.
///
/// The plane contains the crystal's c axis (`+Y`, the table normal), like the boundary between
/// the amethyst and the citrine growth sectors of a real ametrine. The boundary is sharp, as the
/// Anahi boundaries are. A half space through the origin is scale-free, so the same geometry
/// fits a stone of any size and needs no "relative to stone" handling. Both zones are per
/// millimetre.
#[must_use]
pub fn ametrine_zoning() -> ZonedAbsorption {
    let (amethyst, citrine) = zone_tensors();
    let mut zoning = ZonedAbsorption::new(ZoneAbsorption::per_mm(amethyst.clone()));
    zoning.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        },
        absorption: ZoneAbsorption::per_mm(citrine.clone()),
    });
    zoning
}

impl GemMaterial {
    /// The built-in Ametrine: Anahi-type bicolour quartz with one straight boundary, amethyst on
    /// `x < 0` and citrine on `x >= 0` (see [`ametrine_zoning`]).
    ///
    /// The optics are Citrine's, bit for bit (it is taken from
    /// that entry, so the two cannot drift apart): the same quartz host, ordinary and
    /// extraordinary dispersion, birefringence and c axis `+Y`, no scattering and no edge
    /// rounding. The colors are the catalogue's quartz iron recipes per millimetre (see
    /// [`AMETRINE_AMETHYST_FE_PPMA`] and [`AMETRINE_CITRINE_FE_PPMA`]), so the material is
    /// [`AbsorptionUnit::PerMm`](super::AbsorptionUnit::PerMm) and its plain
    /// [`absorption`](Self::absorption) is the amethyst half.
    ///
    /// Not modelled: the six-sector pinwheel of a whole crystal section (a cut stone shows one or
    /// two sectors, not the star), and a diffuse boundary (`boundary_softness_mm` is `0`).
    pub(super) fn built_in_material_ametrine() -> Self {
        let citrine = Self::built_in_materials_amethyst_through_citrine()
            .into_iter()
            .find(|material| material.name == "Citrine")
            .expect("the Citrine built-in is defined next to Amethyst");
        Self {
            name: "Ametrine".to_owned(),
            ..citrine
        }
        .with_zoning(ametrine_zoning())
    }
}
