//! Built-in material data: Grossular (Tsavorite) and Andradite (Demantoid) garnets.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Continuation of [`Self::built_in_materials_garnets_pyrope_through_spessartine`]
    /// (split purely to keep each part under clippy's function-length lint):
    /// Grossular (Tsavorite) and Andradite (Demantoid). See that function's doc
    /// comment for the shared garnet-group dispersion-fit convention (2-parameter
    /// Cauchy, B-G->F-C converted).
    pub(super) fn built_in_materials_garnets_grossular_and_andradite() -> Vec<Self> {
        vec![
            // Grossular (Tsavorite variety, Ca3Al2(SiO4)3:V/Cr): n_d=1.734, B-G 0.028
            // -> Delta n(F-C) = 0.01621, A=1.734-0.008486/0.5893^2=1.709572,
            // B=0.008486. Abbe V_d~45.71.
            Self {
                name: "Grossular Garnet (Tsavorite)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.709_572,
                    b: 0.008_486,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: V3+ in the octahedral site, bands at 436 and 606 nm,
                // with the UV edge of the high-vanadium type; transmission windows at
                // 520 nm (green) and beyond 700 nm (deep red, T(740) = 0.89 at 7 mm).
                // FITTED 2026-10-19 by a bounded
                // least-squares energy-Gaussian fit (log residual, 380-780 nm) to the
                // Caltech Mineral Spectroscopy Server spectrum of tsavorite GRR 229
                // (Tanzania; Ca2.97 Al1.75 Cr0.01 V0.11 Ti0.02; 0.758 mm; baseline
                // 0.042 removed),
                // `data/reference_spectra/caltech_tsavorite_grr229_0p758mm.csv`, at
                // 0.5 x the sample's vanadium (about 0.9 wt% V2O3, a fine saturated
                // stone) over the 8.8 mm face-up path of a 7 mm stone. Swatch (7 mm,
                // D65): L*56 C*86 h=147; p = 1.5: h 150, p = 2: h 151. Multiply every
                // peak by s/0.5 for another V content (0.35 x: L*65; 0.18 x ~ 0.32
                // wt% V2O3: L*78).
                absorption: AbsorptionTensor::isotropic(vec![
                    // UV edge (high-V type: total absorption below 430 nm)
                    AbsorptionBand::energy(393.0, 3_729.0, 7.15),
                    // V3+ 3T1g -> 3T1g(P)
                    AbsorptionBand::energy(436.0, 2_341.0, 6.89),
                    // long-wavelength wing of the 436 nm band
                    AbsorptionBand::energy(490.0, 2_500.0, 1.11),
                    // V3+ 3T1g -> 3T2g
                    AbsorptionBand::energy(606.0, 2_727.0, 2.99),
                    // 660-690 nm shoulder (Cr3+ 0.01 apfu / V3+ wing)
                    AbsorptionBand::energy(666.0, 1_166.0, 0.663),
                ]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
            // Andradite (Demantoid variety, Ca3Fe2(SiO4)3:Cr): n_d=1.887, B-G 0.057
            // (demantoid's famously-cited dispersion, HIGHER than diamond's 0.044 B-G-
            // equivalent figure) -> Delta n(F-C) = 0.03300,
            // A=1.887-0.017276/0.5893^2=1.837264, B=0.017276. Abbe V_d~26.88.
            Self {
                name: "Andradite Garnet (Demantoid)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.837_264,
                    b: 0.017_276,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: Fe3+ (440 nm band and the O->Fe3+ charge-transfer
                // edge, total absorption below 450 nm) plus Cr3+ at 578/629 nm;
                // windows at 520-545 nm (green) and beyond 720 nm (deep red,
                // T(740) = 0.75 at 7 mm), an NIR band at ~880 nm. FITTED 2026-10-19
                // by a bounded
                // least-squares energy-Gaussian fit (log residual, 380-780 nm) to the
                // Caltech Mineral Spectroscopy Server spectrum of the Russian
                // demantoid slab GIA 2952 (2.5 mm; scattering baseline 0.816
                // removed),
                // `data/reference_spectra/caltech_demantoid_gia2952_2p5mm.csv`, at
                // the slab's own composition over the 8.8 mm face-up path of a 7 mm
                // stone. Swatch (7 mm, D65): L*41 C*92 h=140; p = 1.5: h 144, p = 2:
                // h 149 (0.75 x all peaks: L*50). The heated Ural demantoid GRR 2667
                // is 2.6 x more absorbing per cm and would be near-black at 7 mm.
                absorption: AbsorptionTensor::isotropic(vec![
                    // O->Fe3+ charge-transfer edge + Fe3+ 440 nm (data saturated
                    // below 450 nm)
                    AbsorptionBand::energy(405.0, 5_035.0, 28.7),
                    // 450-500 nm wing of the edge (FWHM on the bound)
                    AbsorptionBand::energy(455.0, 2_500.0, 3.27),
                    // Cr3+ 4A2 -> 4T2 shoulder
                    AbsorptionBand::energy(578.0, 2_076.0, 2.13),
                    // Cr3+ 4A2 -> 4T2 main band
                    AbsorptionBand::energy(629.0, 1_757.0, 6.0),
                    // 680-700 nm shoulder (Cr3+ 2E region)
                    AbsorptionBand::energy(692.0, 886.0, 1.08),
                    // NIR band (861 nm in Stubna et al. 2019); its tail closes the
                    // 740 nm window slightly
                    AbsorptionBand::energy(880.0, 2_469.0, 1.67),
                ]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }
}
