//! Built-in material data: Peridot, YAG, GGG, Benitoite.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

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
                // Chromophore: the classic Fe2+ 450/475/495nm triplet, blocking
                // violet-blue and leaving peridot's yellow-green transmitted. Widths
                // (25nm each) and peaks (1.0/1.3/1.0) TUNED; band CENTRES cited.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(450.0, 25.0, 1.0),
                    AbsorptionBand::new(475.0, 25.0, 1.3),
                    AbsorptionBand::new(495.0, 25.0, 1.0),
                ]),
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
                // PLEOCHROISM: benitoite is famously STRONGLY dichroic -- a deep
                // sapphire-blue o-ray (E-perp-c) against a near-colorless e-ray
                // (E-parallel-c), the standard gemological description of this
                // species (Ti/Fe-related blue chromophore spanning roughly
                // 380-500nm). Modelled as one broad band (width 60nm, centred 440nm
                // to span that range) present strongly in the o-ray and only weakly
                // in the e-ray -- peaks (2.0 vs 0.4, a 5x dichroic ratio) TUNED for a
                // clearly, strongly dichroic stone; the qualitative "blue one
                // direction, near-colorless the other" pattern and the 380-500nm
                // span are the cited, non-tuned features.
                absorption: AbsorptionTensor::uniaxial(
                    vec![AbsorptionBand::new(440.0, 60.0, 2.0)], // o-ray (E-perp-c)
                    vec![AbsorptionBand::new(440.0, 60.0, 0.4)], // e-ray (E-parallel-c)
                ),
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
