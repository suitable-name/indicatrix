//! Built-in material data: Amethyst, Citrine.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Continuation of [`Self::built_in_materials_aquamarine_through_citrine`] (split
    /// purely to keep each part under clippy's function-length lint): Amethyst and
    /// Citrine, both quartz.
    pub(super) fn built_in_materials_amethyst_through_citrine() -> Vec<Self> {
        vec![
            // Amethyst (alpha-Quartz, SiO2, color centre) -- physically the same
            // SiO2 crystal as the colorless "Quartz" entry above: reuses that
            // entry's exact Ghosh 1999 o-ray Sellmeier3 fit and its e-ray
            // `uniaxial_extraordinary_dispersion` and `birefringence_delta`
            // bit-for-bit, differing only in absorption. n_d = 1.54421, Delta n(F-C)
            // = 0.00781 (quartz's well-known "0.013" figure is the B-G interval, not
            // F-C; x0.579 gives 0.00753 -- see the Quartz entry above. The stored
            // convention is F-C, 486.1-656.3 nm).
            Self {
                name: "Amethyst".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // Chromophore: an irradiation-induced Fe-related color centre (not a
                // simple Fe3+/Fe4+ d-d transition -- broadly analogous to blue
                // topaz's color centre above but a different defect), a broad band
                // centred ~545nm (green-yellow), leaving violet/purple transmitted.
                // Width (55nm) and peak (1.8) tuned; band centre cited.
                absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                    545.0, 55.0, 1.8,
                )]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
            },
            // Citrine (alpha-Quartz, SiO2, Fe3+) -- same host crystal/dispersion
            // reuse as Amethyst immediately above; see that entry's comment.
            Self {
                name: "Citrine".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // Chromophore: Fe3+ substitution, a broad absorption edge rising from
                // the near-UV into blue (a tail rather than a discrete band) --
                // modelled as a broad Gaussian centred just below the visible band
                // (400nm, width 70nm) so its red-side tail absorbs violet-blue while
                // leaving yellow-orange transmitted, citrine's characteristic color.
                // Peak (1.6) tuned; the UV-blue placement is the cited feature.
                absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                    400.0, 70.0, 1.6,
                )]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
            },
        ]
    }
}
