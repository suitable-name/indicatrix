//! Built-in material data: Tanzanite, Synthetic Moissanite, Cubic Zirconia.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

/// Heated tanzanite's trichroic absorption (alpha/beta/gamma -> `o_ray`/`beta_ray`/`e_ray`),
/// fitted to measured polarised spectra; see the comment at the entry below for the sources and
/// the one rescale. A separate function so the entry stays within clippy's function-length limit.
fn tanzanite_absorption() -> AbsorptionTensor {
    AbsorptionTensor::biaxial(
        vec![
            // alpha (o_ray) -- red-violet direction, the darkest ray after heating; b-ray
            // x 2.119 (see the comment at the entry)
            AbsorptionBand::energy(350.0, 5_083.0, 4.81),
            AbsorptionBand::energy(505.0, 5_500.0, 2.11),
            AbsorptionBand::energy(577.0, 3_037.0, 2.36),
            AbsorptionBand::energy(751.0, 2_191.0, 0.326),
        ],
        vec![
            // beta (beta_ray) -- the blue direction
            AbsorptionBand::energy(340.0, 5_683.0, 12.2),
            AbsorptionBand::energy(565.0, 5_493.0, 2.1),
            AbsorptionBand::energy(610.0, 1_526.0, 0.873),
            AbsorptionBand::energy(771.0, 2_423.0, 2.26),
        ],
        vec![
            // gamma (e_ray, along c_axis) -- yellow-green before heating; blue after (no
            // 445 nm band left)
            AbsorptionBand::energy(385.0, 2_560.0, 9.86),
            AbsorptionBand::energy(440.0, 3_095.0, 0.508),
            AbsorptionBand::energy(538.0, 3_664.0, 1.35),
            AbsorptionBand::energy(612.0, 2_209.0, 1.73),
            AbsorptionBand::energy(742.0, 2_551.0, 1.33),
        ],
    )
}

impl GemMaterial {
    /// Fourth quarter of the built-in material table (Tanzanite through Cubic
    /// Zirconia). See `built_in_materials_diamond_through_emerald` for why the table is
    /// split into several functions.
    pub(super) fn built_in_materials_tanzanite_through_cubic_zirconia() -> Vec<Self> {
        vec![
            // Tanzanite (HEATED, Zoisite, Ca2Al3(SiO4)3(OH):V -- see the CHOICE note
            // below; the entry models the heat-treated violet-blue commercial gem)
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
                // (`o_ray`=alpha/`beta_ray`=beta/`e_ray`=gamma). This entry models
                // the HEATED commercial stone (see the CHOICE note below).
                //
                // Band positions: C.S. Hurlbut, American Mineralogist 54, 702 (1969),
                // citing B.W. Anderson (1968)'s polarized absorption data for blue
                // zoisite (tanzanite): bands at 595nm, 528nm and 455nm. Hurlbut
                // reports tanzanite as trichroic: X-axis red, Y-axis blue, Z-axis
                // yellow-green. Corroborated by K. Schwarzinger, T. Ulatowski & G.R.
                // Rossman's V3+-in-zoisite work, which places V3+ absorption at 530nm
                // and 600nm in all three polarization directions (the literature
                // positions; the fitted band centres below differ, see FITTED), and
                // identifies the 455nm band as
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
                // below) -> the ray that carries the 455nm band in UNHEATED stones.
                //
                // CHOICE (changed 2026-10-19): "Tanzanite" is the HEATED stone, the
                // form sold as "tanzanite" almost universally (the unheated trichroic
                // stone, whose gamma ray carries a strong 455nm band, mixes to a
                // near-grey and was the previous model; an "(unheated)" entry would
                // be appended to `all_materials` later, an owner decision).
                //
                // FITTED 2026-10-19 by bounded
                // least-squares energy-Gaussian fits (log residual, 380-780 nm) to
                // the three polarised Caltech Mineral Spectroscopy Server spectra of
                // zoisite GRR 2292, blue zone (Arusha, Tanzania; E||a = gamma and
                // E||c = beta from a 3.150 mm piece, E||b = alpha from a 5.142 mm
                // piece; NIR baselines removed), the files
                // `caltech_zoisite_grr2292_blue_a_gamma_3p150mm.csv`,
                // `caltech_zoisite_grr2292_blue_b_alpha_5p142mm.csv` and
                // `caltech_zoisite_grr2292_blue_c_beta_3p150mm.csv` in
                // `data/reference_spectra/`. The blue zone's gamma ray has no
                // 445 nm Fe2+-Ti4+ band, the band that heating removes, so this
                // natural blue is the heated stone's spectrum; the digitised
                // Caltech GRR 3045 after-heating curves
                // (`caltech_zoisite_grr3045_after_heating_digitized_5p747mm.csv`)
                // agree ray for ray. The alpha ray is scaled by 2.119 so that
                // alpha(540 nm)/gamma(600 nm) = 1.48, the ratio measured on single
                // crystals (GRR 1265: 1.49, GRR 3045: 1.47; the two-piece 2292 pair
                // gives 0.70). Concentration: the crystal's own (about 0.6 wt%
                // V2O3, a top-colour blue) over the 8.8 mm face-up path of a 7 mm
                // stone. V3+ bands at ~540 and ~600 nm in all rays, an NIR band at
                // 740-770 nm (beta, gamma), UV edges. Swatch (7 mm, D65): L*45 C*49
                // h=292; p = 1.5: h 296, p = 2: h 301. Alpha is the darkest and most
                // purplish ray (L*31 h 317), beta blue (L*45 h 291), gamma the
                // lightest (L*54 h 268).
                absorption: tanzanite_absorption(),
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
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }
}
