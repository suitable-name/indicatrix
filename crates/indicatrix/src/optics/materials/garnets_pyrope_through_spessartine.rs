//! Built-in material data: Pyrope, Almandine, Spessartine garnets.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Three of the five garnet-group species (the other two, Grossular and
    /// Andradite, are [`Self::built_in_materials_garnets_grossular_and_andradite`] --
    /// split purely to keep each part under clippy's function-length lint). All five
    /// are cubic (isotropic, `birefringence_delta: 0.0`).
    ///
    /// No primary Sellmeier/Cauchy fit is available for any of the five
    /// (garnet-group gemstones are not present in the refractiveindex.info database);
    /// every dispersion fit below is a 2-parameter Cauchy solved from `n_d` and its
    /// "dispersion" figure treated as the standard gemological Fraunhofer B-G
    /// interval, converted to F-C via the same 0.579 ratio as every other gemological
    /// entry in this file -- see
    /// [`Self::built_in_materials_aquamarine_through_citrine`]'s doc comment for the
    /// full convention note. LOWER CONFIDENCE tier, same as Zircon/Topaz/
    /// Tourmaline/Tanzanite/Emerald above; flagged for human cross-check.
    pub(super) fn built_in_materials_garnets_pyrope_through_spessartine() -> Vec<Self> {
        vec![
            // Pyrope (Mg3Al2(SiO4)3): n_d=1.714, B-G 0.022 -> Delta n(F-C) = 0.01274,
            // A=1.714-0.006668/0.5893^2=1.694804, B=0.006668. Abbe V_d ~ 56.05.
            Self {
                name: "Pyrope Garnet".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.694_804,
                    b: 0.006_668,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: the classic Cr3+ (505nm)/Fe2+ (570nm) pair
                // responsible for pyrope's deep red color. Widths (35nm/45nm) and
                // peaks (1.2/1.4) TUNED for a clear deep red; band CENTRES cited.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(505.0, 35.0, 1.2),
                    AbsorptionBand::new(570.0, 45.0, 1.4),
                ]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Almandine (Fe3Al2(SiO4)3): n_d=1.790, B-G 0.024 -> Delta n(F-C) =
            // 0.01390, A=1.790-0.007274/0.5893^2=1.769057, B=0.007274. Abbe V_d~56.86.
            Self {
                name: "Almandine Garnet".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.769_057,
                    b: 0.007_274,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: the diagnostic Fe2+ 505/520/573nm triplet (almandine's
                // characteristic three-line absorption pattern, the standard
                // gemological identification feature for this species). Widths
                // (20nm each) and peaks (1.3/1.5/1.3) TUNED for a deep red-brown/
                // violet-red; band CENTRES cited.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(505.0, 20.0, 1.3),
                    AbsorptionBand::new(520.0, 20.0, 1.5),
                    AbsorptionBand::new(573.0, 20.0, 1.3),
                ]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Spessartine (Mn3Al2(SiO4)3): n_d=1.800, B-G 0.027 -> Delta n(F-C) =
            // 0.01563, A=1.800-0.008182/0.5893^2=1.776445, B=0.008182. Abbe V_d~51.19.
            Self {
                name: "Spessartine Garnet".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.776_445,
                    b: 0.008_182,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: Mn2+ 410/430/460nm triplet, blocking violet-blue and
                // leaving spessartine's vivid orange transmitted. Widths (20nm each)
                // and peaks (1.0/1.2/1.0) TUNED; band CENTRES cited.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(410.0, 20.0, 1.0),
                    AbsorptionBand::new(430.0, 20.0, 1.2),
                    AbsorptionBand::new(460.0, 20.0, 1.0),
                ]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }
}
