//! Built-in material data: Tanzanite, Synthetic Moissanite, Cubic Zirconia.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Fourth quarter of the built-in material table (Tanzanite through Cubic
    /// Zirconia). See `built_in_materials_diamond_through_emerald` for why the table is
    /// split into several functions.
    pub(super) fn built_in_materials_tanzanite_through_cubic_zirconia() -> Vec<Self> {
        vec![
            // Tanzanite (unheated, trichroic Zoisite, Ca2Al3(SiO4)3(OH):V -- see the
            // CHOICE note below for why this models the unheated stone rather than the
            // heat-treated blue-violet commercial gem)
            //
            // No primary Sellmeier/Cauchy fit for zoisite/tanzanite exists (absent
            // from the refractiveindex.info database, and no dedicated visible-range
            // zoisite dispersion paper found). LOWER-CONFIDENCE 2-parameter Cauchy
            // fallback.
            //
            // n_d = 1.700858, within International Gem Society's cited tanzanite
            // range "1.691-1.70". Dispersion shape: IGS's cited "dispersion .030" is
            // the Fraunhofer B-G interval, not F-C. Converted via the same
            // physically-derived B-G->F-C ratio as Emerald's comment above (0.579):
            // Delta n(F-C) = 0.0301*0.579 = 0.01743. Two data points exactly determine
            // a 2-parameter Cauchy fit (same method as Zircon's comment): A=1.674589,
            // B=0.009123, verified n_d=1.700859, Delta n(F-C)=0.017428, Abbe
            // V_d=40.21. LOWER CONFIDENCE than a primary-literature entry -- flagged
            // for human cross-check.
            Self {
                name: "Tanzanite".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.674_589,
                    b: 0.009_123,
                    c: 0.0,
                },
                birefringence_delta: 0.0130,
                // PLEOCHROISM: a genuine three-set `AbsorptionTensor::biaxial`
                // (`o_ray`=alpha/`beta_ray`=beta/`e_ray`=gamma). This entry
                // deliberately models UNHEATED tanzanite -- the genuinely trichroic
                // mineral, not the heat-treated commercial gem (see the CHOICE note
                // below).
                //
                // Band positions: C.S. Hurlbut, American Mineralogist 54, 702 (1969),
                // citing B.W. Anderson (1968)'s polarized absorption data for blue
                // zoisite (tanzanite): bands at 595nm, 528nm and 455nm. Hurlbut
                // reports tanzanite as trichroic: X-axis red, Y-axis blue, Z-axis
                // yellow-green. Corroborated by K. Schwarzinger, T. Ulatowski & G.R.
                // Rossman's V3+-in-zoisite work, which places V3+ absorption at 530nm
                // and 600nm in all three polarization directions (consistent with the
                // 528/595nm positions used here), and identifies the 455nm band as
                // Fe2+-Ti4+ intervalence charge transfer (IVCT), present only in the
                // gamma ray and destroyed by heat treatment above 550C -- the
                // mechanism by which heating collapses tanzanite's trichroism to the
                // commercial blue-violet dichroic look.
                //
                // AXIS MAPPING: standard optical-mineralogy convention (e.g. Nesse,
                // *Introduction to Optical Mineralogy*) labels a biaxial crystal's
                // X/Y/Z vibration directions as the n_alpha/n_beta/n_gamma directions
                // respectively -- Hurlbut's X=red is alpha, Y=blue is beta,
                // Z=yellow-green is gamma. Maps directly onto this crate's
                // `AbsorptionTensor3` alpha/beta/gamma convention: `o_ray` (alpha) ->
                // red, `beta_ray` (beta) -> blue, `e_ray` (gamma, along `c_axis`
                // below) -> the 455nm-bearing yellow-green ray.
                //
                // CHOICE: this entry models UNHEATED tanzanite, not the heat-treated
                // stone sold as "tanzanite" almost universally -- unheated tanzanite
                // is the textbook trichroic example. The heated form's 455nm-band
                // loss could be modelled later by a "Tanzanite (heated)" entry
                // omitting the third `AbsorptionBand` below.
                //
                // Amplitudes are tuned for plausible saturation; band positions
                // (595nm, 528nm, 455nm) are cited. The qualitative amplitude pattern
                // (beta strongest on both V3+ bands, blocking red and green-yellow to
                // leave blue; alpha weakest, leaving red open; gamma carries the
                // 455nm band exclusively, blocking blue to leave yellow-green)
                // follows directly from the three colors Hurlbut reports.
                absorption: AbsorptionTensor::biaxial(
                    vec![
                        // alpha (o_ray) -- X-axis, red: both V3+ bands weak, leaving
                        // red comparatively open.
                        AbsorptionBand::new(595.0, 45.0, 1.3),
                        AbsorptionBand::new(528.0, 35.0, 1.8),
                    ],
                    vec![
                        // beta (beta_ray) -- Y-axis, blue: both V3+ bands strong,
                        // absorbing red and green-yellow, leaving a blue window.
                        AbsorptionBand::new(595.0, 45.0, 3.2),
                        AbsorptionBand::new(528.0, 35.0, 2.8),
                    ],
                    vec![
                        // gamma (e_ray) -- Z-axis, yellow-green: V3+ absorption weaker
                        // than beta's, plus the gamma-exclusive 455nm Fe2+-Ti4+ IVCT
                        // band, which blocks blue to complete yellow-green.
                        AbsorptionBand::new(595.0, 45.0, 1.6),
                        AbsorptionBand::new(528.0, 35.0, 1.0),
                        AbsorptionBand::new(455.0, 40.0, 2.6),
                    ],
                ),
                c_axis: Vec3::Y,
                // n_beta - n_alpha at the D line. Source: C.S. Hurlbut, American
                // Mineralogist 54, 702 (1969), reporting per-specimen tanzanite
                // indices n_alpha=1.6915, n_beta=1.6935, n_gamma=1.7020. Uses these
                // per-specimen indices rather than independent-range midpoints for
                // n_alpha/n_beta/n_gamma: averaging each index's range separately
                // would discard the correlation between them within a single
                // specimen, and would not be equivalent to a real 3-index
                // measurement. Fractional position of beta between alpha and gamma:
                // (1.6935-1.6915)/(1.7020-1.6915) = 0.19. Applied to this entry's own
                // birefringence_delta (0.0130): delta_beta_alpha = 0.19 * 0.0130 =
                // 0.00247, rounded to 0.0025.
                //
                // SIGN CHECK: `optical_character` above is `BiaxialPositive`, which
                // requires (n_beta - n_alpha) < (n_gamma - n_beta) -- beta sits closer
                // to alpha than to gamma. 0.0025 < (0.0130 - 0.0025) = 0.0105 holds. A
                // value that instead put beta closer to GAMMA would be optically
                // NEGATIVE, contradicting the declared sign; see
                // `biaxial_sign_matches_optical_character` below, which pins this
                // relationship for every biaxial built-in.
                biaxial_delta_beta_alpha: Some(0.0025),
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
            // Moissanite (Synthetic SiC, 6H polytype)
            //
            // Source: S. Wang et al., "4H-SiC: A new nonlinear material for
            // mid-infrared lasers," Laser Photonics Rev. 7, 831 (2013), 6H-SiC
            // ordinary-ray fit (refractiveindex.info "SiC: Wang-6H-o"), valid
            // 0.4358-2.325 um -- 6H is the specific SiC polytype gem moissanite is
            // predominantly cut from, and this is a dedicated 6H measurement (not 4H
            // despite the paper's title):
            //   n^2 = 6.57232 + 0.1401/(l^2-0.03178) - 0.02153*l^2
            // Represented exactly as Sellmeier3 using the same distant-pole /
            // folded-constant technique as Alexandrite's comment above (the
            // "0.1401/(l^2-0.03178)" term expanded via the pole identity
            // b*l^2/(l^2-c) - b = A/(l^2-c) for b=A/c, so it becomes an exact pole):
            //   pole0 (constant): b=1.163887, c=0
            //   pole1 (real UV pole, exact): b=4.408433, c=0.03178
            //   pole2 (linear-term approximation): b=21.53, c=1000
            // Verified: n_d = 2.647434, Delta n(F-C) = 0.063515, Abbe V_d = 25.94 --
            // corroborated by a second, older primary source: S. Singh, J.R.
            // Potopowicz, L.G. Van Uitert & S.H. Wemple, "Nonlinear optical properties
            // of hexagonal silicon carbide," Appl. Phys. Lett. 19, 53 (1971),
            // alpha-SiC (6H) ordinary-ray Sellmeier (n^2-1 =
            // 5.5394*l^2/(l^2-0.026945), SiC/nk/Singh-o.yml), giving n_d=2.646763
            // (agreeing with Wang to <0.03%) and Abbe V_d=25.53 (agreeing to within
            // 1.6%).
            //
            // birefringence_delta: re-derived from Wang 2013's own companion 6H-SiC
            // extraordinary-ray fit (SiC/nk/Wang-6H-e.yml: n_e^2 = 6.7452 +
            // 0.15352/(l^2-0.03597) - 0.02249*l^2), giving n_e(D) = 2.688966 against
            // n_o(D) = 2.647434, i.e. n_e - n_o = +0.041532 at the sodium D line, both
            // indices from the same primary paper. Singh 1971's e-ray fit corroborates
            // with n_e-n_o = +0.045766.
            Self {
                name: "Synthetic Moissanite".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.163_887, 4.408_433, 21.53],
                    c: [0.0, 0.031_78, 1000.0],
                },
                birefringence_delta: 0.0415,
                // colorless (no chromophore): empty band set, zero absorption at
                // every wavelength -- must render identically to before.
                absorption: AbsorptionTensor::isotropic(vec![]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
            // Cubic Zirconia (ZrO2, 12 mol% Y2O3-stabilized)
            //
            // Source: D.L. Wood & K. Nassau, "Refractive index of cubic zirconia
            // stabilized with yttria," Appl. Opt. 21, 2978-2981 (1982), as tabulated by
            // refractiveindex.info ("ZrO2: Wood"), valid 0.361-5.135 um:
            //   n^2-1 = 1.347091*l^2/(l^2-0.062543^2) + 2.117788*l^2/(l^2-0.166739^2)
            //           + 9.452943*l^2/(l^2-24.320570^2)
            // Verified against the paper's own directly-quoted figures: N_D = 2.15847
            // (this fit: n_d = 2.15846) and |N_C-N_F| = 0.03455 (this fit: Delta
            // n(F-C) = 0.03456), both matching to within 1e-5. Abbe V_d = 33.52.
            Self {
                name: "Cubic Zirconia".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.347_091, 2.117_788, 9.452_943],
                    c: [0.003_912, 0.027_802, 591.489],
                },
                birefringence_delta: 0.0,
                // colorless (no chromophore): empty band set, zero absorption at
                // every wavelength -- must render identically to before.
                absorption: AbsorptionTensor::isotropic(vec![]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }
}
