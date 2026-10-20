//! Built-in material data: Peridot, YAG, GGG, Benitoite.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

/// Peridot's trichroic absorption (alpha/beta/gamma -> `o_ray`/`beta_ray`/`e_ray`): the
/// energy-Gaussian fits to Caltech GRR 418 described at the entry below. A separate function so
/// the entry stays within clippy's function-length limit.
fn peridot_absorption() -> AbsorptionTensor {
    AbsorptionTensor::biaxial(
        // alpha (o_ray)
        vec![
            AbsorptionBand::energy(345.0, 10_096.0, 5.38),
            AbsorptionBand::energy(455.0, 1_410.0, 0.53),
            AbsorptionBand::energy(490.0, 2_160.0, 0.44),
            AbsorptionBand::energy(637.0, 1_724.0, 0.80),
            AbsorptionBand::energy(840.0, 3_276.0, 4.76),
            AbsorptionBand::energy(1000.0, 2_534.0, 35.2),
        ],
        // beta (beta_ray)
        vec![
            AbsorptionBand::energy(341.0, 12_000.0, 6.78),
            AbsorptionBand::energy(465.0, 2_526.0, 1.07),
            AbsorptionBand::energy(500.0, 3_000.0, 0.47),
            AbsorptionBand::energy(635.0, 2_500.0, 1.33),
            AbsorptionBand::energy(840.0, 4_000.0, 4.25),
            AbsorptionBand::energy(1000.0, 2_401.0, 35.2),
        ],
        // gamma (e_ray)
        vec![
            AbsorptionBand::energy(376.0, 3_317.0, 4.77),
            AbsorptionBand::energy(440.0, 2_933.0, 2.24),
            AbsorptionBand::energy(500.0, 3_000.0, 0.79),
            AbsorptionBand::energy(627.0, 2_500.0, 0.77),
            AbsorptionBand::energy(840.0, 4_000.0, 2.34),
            AbsorptionBand::energy(1000.0, 3_663.0, 12.7),
        ],
    )
}

impl GemMaterial {
    /// Third quarter: Peridot (the one biaxial addition in this quarter), YAG, GGG
    /// and Benitoite.
    pub(super) fn built_in_materials_peridot_through_benitoite() -> Vec<Self> {
        vec![
            // Peridot (Forsterite-rich olivine, (Mg,Fe)2SiO4): n_d=1.654, B-G 0.020 ->
            // Delta n(F-C) = 0.01158, A=1.654-0.006062/0.5893^2=1.636549, B=0.006062.
            // Abbe V_d~56.49. No primary Sellmeier fit available; LOWER CONFIDENCE
            // Cauchy fallback, same convention as the garnets above.
            Self {
                name: "Peridot".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.636_549,
                    b: 0.006_062,
                    c: 0.0,
                },
                birefringence_delta: 0.036,
                // Chromophore: Fe2+ in olivine. Biaxial (trichroic) energy-Gaussian
                // sets, one per principal ray (alpha/beta/gamma -> o_ray/beta_ray/
                // e_ray, as Tanzanite). Each set is a bounded least-squares fit
                // (log residual, 380-780 nm) to the Caltech Mineral Spectroscopy
                // Server San Carlos olivine spectra GRR 418 alpha/beta/gamma (G. R.
                // Rossman; CSVs `crates/indicatrix/data/reference_spectra/
                // caltech_olivine_grr418_{alpha,beta,gamma}.csv`, URLs in their
                // headers), scaled to 0.88 cm = the 8.8 mm face-up path of a 7 mm
                // stone (fitted 2026-10-19; GIA Sardinia peridot
                // characterisation [S4]: 473/490 nm Fe2+ bands, total absorption
                // below ~440 nm, broad ~1050 nm band with an 830 nm shoulder). The
                // 345/341/376 nm bands stand in for the UV edge; the 840/1000 nm
                // bands for the Fe2+ NIR pair (fitted amplitudes sat on the fit
                // bound, only their 650-780 nm tails matter inside the 380-780 nm
                // sampling). Swatch (7 mm, D65): L*76 C*63 h=111 yellow-green; the
                // previous blue-only 450/475/495 nm triplet let red pass and
                // drifted to amber (h=81). Fitted, not hand-tuned.
                absorption: peridot_absorption(),
                c_axis: Vec3::Y,
                // Same fractional-position method as Alexandrite/Topaz/Tanzanite/
                // Chrysoberyl above: real olivine/peridot principal indices are
                // commonly tabulated (e.g. Deer, Howie & Zussman, *An Introduction to
                // the Rock-Forming Minerals*) around n_alpha~1.654, n_beta~1.669,
                // n_gamma~1.690 for gem-quality (Fo~90) peridot -- fraction
                // (1.669-1.654)/(1.690-1.654) = 0.417 -- applied to this entry's own
                // birefringence_delta: 0.417 * 0.036 = 0.01501, rounded to 0.0150.
                biaxial_delta_beta_alpha: Some(0.0150),
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
            // YAG (Yttrium Aluminium Garnet, Y3Al5O12, undoped/colorless laser host)
            //
            // Source: D.E. Zelmon, D.L. Small & R. Page, "Refractive-index
            // measurements of undoped yttrium aluminum garnet from 0.4 to 5.0 um,"
            // Appl. Opt. 37, 4933-4935 (1998), as tabulated by refractiveindex.info
            // ("Y3Al5O12: Zelmon"):
            //   n^2-1 = 2.28200*l^2/(l^2-0.01185) + 3.27644*l^2/(l^2-282.734)
            // Encoded via Sellmeier3 with the unused 3rd pole zeroed (b=0, c=1.0 um^2,
            // same technique as Diamond/Spinel above). Verified: n_d = 1.83263, Delta
            // n(F-C) = 0.01600, Abbe V_d = 52.04 -- matching YAG's widely-cited real
            // Abbe number (~52); a commonly quoted "0.028" dispersion figure for this
            // row is not reproduced by this primary Sellmeier fit and is not used.
            Self {
                name: "YAG".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [2.282_00, 3.276_44, 0.0],
                    c: [0.011_85, 282.734, 1.0],
                },
                birefringence_delta: 0.0,
                // colorless (undoped YAG, the laser-host reference composition):
                // empty band set, zero absorption at every wavelength.
                absorption: AbsorptionTensor::isotropic(vec![]),
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
            // GGG (Gadolinium Gallium Garnet, Gd3Ga5O12, colorless synthetic --
            // historically used as a diamond simulant before Cubic Zirconia).
            //
            // No primary Sellmeier fit for GGG is transcribed here; this is a
            // LOWER-CONFIDENCE 2-parameter Cauchy fit solved from n_d=1.970 and a
            // Delta n(F-C) obtained the same way as the natural gemstone entries
            // above. The 0.045 commonly quoted for GGG is the gemological Fraunhofer
            // B-G dispersion figure (the same kind of figure as Diamond's 0.044 and
            // Benitoite's 0.045), not an F-C value, so it is converted with this
            // file's B-G->F-C ratio of 0.579: Delta n(F-C) = 0.045*0.579 = 0.026055
            // (Diamond 0.044 -> 0.0256 uses the same ratio). B follows from
            // B*(1/0.4861^2 - 1/0.6563^2) = B*1.9104 = 0.026055, so B=0.013639;
            // A=1.970-0.013639/0.5893^2=1.930726, keeping n_d=1.970. Abbe V_d~37.2.
            // n_d's general magnitude (~1.97-2.0) is corroborated by D.L. Wood &
            // K. Nassau, "Optical properties of gadolinium gallium garnet," Appl.
            // Opt. 29, 3704-3707 (1990) -- the same author pair cited for this file's
            // Cubic Zirconia entry -- though this entry's Cauchy coefficients are not
            // transcribed from that paper. Flagged for human cross-check against a
            // primary Sellmeier fit if one becomes available.
            Self {
                name: "GGG".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.930_726,
                    b: 0.013_639,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // colorless (undoped GGG): empty band set, zero absorption at
                // every wavelength.
                absorption: AbsorptionTensor::isotropic(vec![]),
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
            // Benitoite (BaTiSi3O9): n_d=1.757, B-G 0.045 (benitoite's famous
            // "diamond-like fire," commonly cited ~0.046-0.048 B-G) -> Delta n(F-C) =
            // 0.02606, A=1.757-0.013639/0.5893^2=1.717734, B=0.013639. Abbe V_d~29.05.
            // No primary Sellmeier fit available; LOWER CONFIDENCE Cauchy fallback.
            Self {
                name: "Benitoite".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.717_734,
                    b: 0.013_639,
                    c: 0.0,
                },
                birefringence_delta: 0.047,
                // PLEOCHROISM: benitoite is famously STRONGLY dichroic -- an intensely
                // blue e-ray (E-parallel-c) against a near-colourless o-ray
                // (E-perp-c). The blue is the transmission window left by a Fe2+-Ti4+
                // intervalence charge-transfer band in the orange-red, polarised along
                // c (docs/chromophores/garnet_peridot_zircon_benitoite.md section 7:
                // centre 635 nm, FWHM 4500 cm^-1, pol {o 0.05, e 1.0}; Fe-Ti IVCT sits
                // at ~580-620 nm per the general Fe-Ti CT literature, transmitting
                // 420-500 nm; LibreTexts Benitoite / gemdat 624). Modelled as one band
                // at 635 nm, sigma 60 nm (~FWHM 140 nm, i.e. ~560-710 nm), so blue and
                // violet stay transmitted. Peaks (2.0 e-ray vs 0.1 o-ray, 0.05 pol
                // ratio from the dataset) are TUNED for visible saturation. The
                // earlier 440 nm band removed blue and gave a yellow-white stone.
                absorption: AbsorptionTensor::uniaxial(
                    vec![AbsorptionBand::new(635.0, 60.0, 0.1)], // o-ray (E-perp-c)
                    vec![AbsorptionBand::new(635.0, 60.0, 2.0)], // e-ray (E-parallel-c)
                ),
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
