//! Built-in material data: Zircon, Alexandrite, Topaz.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Second quarter of the built-in material table (Zircon through Topaz). Split out
    /// of `built_in_materials_diamond_through_emerald` purely to keep each part under
    /// clippy's function-length lint -- this is plain data, not logic, so the split
    /// point carries no significance.
    pub(super) fn built_in_materials_zircon_through_topaz() -> Vec<Self> {
        vec![
            // Zircon (High Zircon, ZrSiO4)
            //
            // n_o = 1.925, corroborated by three independent sources: (1) Mineral Data
            // Publishing's "Handbook of Mineralogy" (2001), zircon entry: omega =
            // 1.925-1.961; (2) International Gem Society: "High zircon: n_o =
            // 1.920-1.940 (often 1.925); n_e = 1.970-2.010 (often 1.984)"; (3) the
            // widely-reproduced gem-trade n_o=1.9250/n_e=1.9840 pairing, also the
            // standard GIA reference point for gem-quality (heat-treated) colourless
            // high zircon. birefringence_delta = n_e - n_o = 1.984 - 1.925 = +0.0590.
            //
            // No primary Sellmeier/Cauchy fit for zircon exists in the optics
            // literature, so this is a LOWER-CONFIDENCE 2-parameter Cauchy fallback.
            // Dispersion shape: zircon's "0.039" figure is near-universal across
            // gemological references (GIA, IGS) and is the Fraunhofer B-G interval,
            // not F-C. Converted via the same physically-derived B-G->F-C ratio as the
            // Emerald entry's comment (0.579, range 0.569-0.587): Delta n(F-C) =
            // 0.039*0.579 = 0.02258. Two data points (n_o=1.925, converted Delta n)
            // exactly determine a 2-parameter Cauchy fit:
            //   A = n_d - B/lambda_d^2,  B = (n_d-1)/(V_d * (1/lambda_F^2 - 1/lambda_C^2))
            // giving A=1.890963, B=0.011820, verified n_d=1.925, Delta n(F-C)=0.02258,
            // Abbe V_d=40.96. LOWER CONFIDENCE than a primary-literature entry --
            // flagged for human cross-check. n_d drives the critical angle
            // (brilliance/windowing/extinction), so a >0.002 change here is
            // significant: this value differs from an earlier, unsourced 1.956878 by
            // -0.0319, well above that threshold.
            Self {
                name: "Zircon".to_string(),
                crystal_system: CrystalSystem::Tetragonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.890_963,
                    b: 0.011_820,
                    c: 0.0,
                },
                birefringence_delta: 0.0590,
                // Chromophore: high (gem-quality) zircon's colour and characteristic
                // sharp lines come from trace U4+ substituting for Zr4+, producing
                // narrow lines rather than a broad transition-metal band. Source: the
                // U4+ absorption spectrum of zircon, e.g. P.E. Fielding (1970) and
                // Nasdala et al., "Spectroscopic methods applied to zircon," Reviews
                // in Mineralogy & Geochemistry 53 (2003), reporting a dominant line
                // near 653nm plus a weaker set at 691/589/562/537nm. Widths (8nm
                // dominant, 6nm weaker -- U4+ lines are sharp) and peaks (dominant 2.4,
                // weaker 0.5 each) tuned; band positions cited. No per-axis split is
                // cited, so this stays isotropic.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(537.0, 6.0, 0.5),
                    AbsorptionBand::new(562.0, 6.0, 0.5),
                    AbsorptionBand::new(589.0, 6.0, 0.5),
                    AbsorptionBand::new(653.0, 8.0, 2.4),
                    AbsorptionBand::new(691.0, 6.0, 0.5),
                ]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Alexandrite (Chrysoberyl, BeAl2O4:Cr)
            //
            // Source: J. Walling et al., "Tunable alexandrite lasers," IEEE J. Quantum
            // Electron. 16, 1302 (1980), n(alpha)-direction Sellmeier
            // (refractiveindex.info "BeAl2O4: Walling-alpha"), valid 0.25-2.6 um:
            //   n^2 = 1.78522 + 1.21202*l^2/(l^2-0.01262) - 0.01681*l^2
            // Represented as Sellmeier3 with the "-0.01681*l^2" linear term encoded as
            // a distant pole (b/c = 0.01681, c=1000 um^2, outside the visible domain)
            // and the leading constant folded into a c=0 pole:
            //   pole0 (constant): b=0.78522, c=0
            //   pole1 (real UV pole, exact): b=1.21202, c=0.01262
            //   pole2 (linear-term approximation): b=16.81, c=1000
            // Verified: n_d = 1.742730, Delta n(F-C) = 0.010051, Abbe V_d = 73.90.
            //
            // birefringence_delta: Walling 1980 also tabulates beta and gamma
            // Sellmeier fits (Walling-beta.yml, Walling-gamma.yml, same repo). Note:
            // Walling's alpha/beta/gamma file labels are the paper's crystallographic
            // axis labels and do NOT sort by index magnitude the way the mineralogical
            // optical-indicatrix convention (n_alpha <= n_beta <= n_gamma) requires --
            // at the D line the three curves evaluate to alpha=1.742730,
            // beta=1.748360, gamma=1.740779, i.e. numerically gamma < alpha < beta.
            // Sorting by magnitude gives the true optical indicatrix: n_alpha(opt) =
            // 1.740779 (Walling's "gamma" file), n_beta(opt) = 1.742730 (Walling's
            // "alpha" file, used as n_d above), n_gamma(opt) = 1.748360 (Walling's
            // "beta" file). True total birefringence = n_gamma(opt) - n_alpha(opt) =
            // 0.007581.
            Self {
                name: "Alexandrite".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.785_22, 1.212_02, 16.81],
                    c: [0.0, 0.012_62, 1000.0],
                },
                birefringence_delta: 0.0076,
                // TRICHROISM: figure-read from Farrell & Newnham's Table 1 and Figs.
                // 3-4 (same figure-read provenance tier the Sapphire entry's
                // E-parallel-c amplitude uses).
                //
                // Source: E.F. Farrell & R.E. Newnham, "Crystal-field spectra of
                // chrysoberyl, alexandrite, peridot, and sinhalite," American
                // Mineralogist 50, 1972 (1965). Polarized absorption at 77K, space
                // group Pnma (a=9.40, b=5.48, c=4.43 A -- same axis convention as this
                // entry's dispersion source, Walling 1980), pleochroic colours
                // yellow||a, green||b, red||c. Two Cr3+ band systems: 4A2->4T2 at
                // 560-595nm and 4A2->4T1 at 410-440nm, straddling Neuhaus's 580/415nm
                // red-green critical values -- that straddle is the illuminant-
                // dependent daylight-green/incandescent-red colour change (pinned by
                // `raytracer_tests::alexandrite_shifts_redder_under_incandescent_than_d65`);
                // the per-axis split below adds direction-dependence on top.
                //
                // AXIS MAPPING: `AbsorptionTensor::biaxial(alpha, beta, gamma)` slots
                // are the n_alpha/n_beta/n_gamma principal directions. Per this
                // entry's Walling-derived indicatrix sort above (n_alpha(opt) is
                // Walling's E||c curve, n_beta(opt) is E||a, n_gamma(opt) is E||b),
                // the crystallographic axes land as: alpha = crystal c (RED), beta =
                // crystal a (YELLOW), gamma = crystal b (GREEN, along `c_axis` below).
                //
                // Band positions (cited, F&N Table 1, natural alexandrite): E||c: 4T2
                // at 565nm, 4T1 the unresolved 410/430nm pair (420nm midpoint used);
                // E||a: 4T2 at 560nm, 4T1 at 422nm; E||b: 4T2 at 595nm, 4T1 the
                // 410/430/440nm group (430nm central peak used). Widths (40nm/32nm)
                // tuned (77K bands sharpen relative to room temperature, so published
                // low-temperature widths aren't used directly).
                //
                // Amplitude ratios (figure-read, F&N Figs. 3-4, net peak height above
                // baseline): 4T2 from Fig. 4 (natural, all three axes): E||a 6.5,
                // E||b 31, E||c 19 cm^-1 -> the 0.65:3.1:1.9 split below (x0.1 scale).
                // 4T1 E||a:E||b from Fig. 3 (synthetic, Cr-only, cleaner than Fig. 4's
                // Fe3+-contaminated region): 25:10.5 cm^-1 ~= 2.4:1. 4T1 E||c not
                // separately measurable (F&N: c-polarization "proved impracticable"
                // on synthetic platelets); estimated from Fig. 4's Fe-contaminated
                // natural c:b ratio (~1.24) applied to the synthetic E||b value -- the
                // one lower-confidence amplitude here. Overall per-band scales (x0.1
                // for 4T2, x0.14 for 4T1) tuned for plausible saturation.
                absorption: AbsorptionTensor::biaxial(
                    vec![
                        // alpha -- crystal c-axis, RED: 4T2 at 565nm sits below the
                        // 580nm critical value (Neuhaus), so the deep-red window past
                        // ~640nm stays open; moderate 4T1 blocks blue-violet.
                        AbsorptionBand::new(565.0, 40.0, 1.9),
                        AbsorptionBand::new(420.0, 32.0, 1.8),
                    ],
                    vec![
                        // beta -- crystal a-axis, YELLOW: dominated by the strongest
                        // 4T1 of the three directions ("an intense absorption in the
                        // dark blue (0.42u) dominates the a spectrum" -- F&N), while
                        // its 4T2 is the weakest, leaving yellow-red mostly open.
                        AbsorptionBand::new(560.0, 40.0, 0.65),
                        AbsorptionBand::new(422.0, 32.0, 3.5),
                    ],
                    vec![
                        // gamma -- crystal b-axis, GREEN (along `c_axis` below): the
                        // strongest 4T2, at 595nm above the critical value, absorbs
                        // "both red and yellow" (F&N) leaving green transmitted; the
                        // weakest 4T1 leaves the green window's blue edge open.
                        AbsorptionBand::new(595.0, 40.0, 3.1),
                        AbsorptionBand::new(430.0, 32.0, 1.5),
                    ],
                ),
                c_axis: Vec3::Y,
                // n_beta - n_alpha at the D line, TIGHT confidence -- from the same
                // Walling 1980 alpha/beta/gamma Sellmeier trio used for
                // birefringence_delta above, sorted into the true optical indicatrix:
                // n_alpha(opt) = 1.740779, n_beta(opt) = 1.742730 (== n_d above),
                // n_gamma(opt) = 1.748360. delta_beta_alpha = 1.742730 - 1.740779 =
                // 0.001951 (cross-checks: n_gamma - n_alpha = 0.007581 matches
                // birefringence_delta = 0.0076 within rounding).
                biaxial_delta_beta_alpha: Some(0.001_951),
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Topaz (Al2SiO4(F,OH)2)
            //
            // No primary Sellmeier/Cauchy fit for topaz exists (absent from the
            // refractiveindex.info database, and no dedicated visible-range topaz
            // dispersion paper found). LOWER-CONFIDENCE 2-parameter Cauchy fallback.
            //
            // n_d = 1.627178, within International Gem Society's cited topaz range
            // "1.61-1.638". Dispersion shape: IGS's cited "dispersion .014" is the
            // Fraunhofer B-G interval, not F-C. Converted via the same
            // physically-derived B-G->F-C ratio as Emerald's comment above (0.579):
            // Delta n(F-C) = 0.0141*0.579 = 0.00816. Two data points (n_d, converted
            // Delta n) exactly determine a 2-parameter Cauchy fit (same method as
            // Zircon's comment): A=1.614872, B=0.004273, verified n_d=1.627176, Delta
            // n(F-C)=0.008163, Abbe V_d=76.83. LOWER CONFIDENCE than a
            // primary-literature entry -- flagged for human cross-check.
            Self {
                name: "Topaz".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.614_872,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: 0.0080,
                // This entry models "London/Sky Blue" TOPAZ specifically
                // (the commercially dominant form: near-colourless natural topaz,
                // irradiated then heat-treated) -- its colour comes from an
                // irradiation-induced colour centre (a trapped-electron/hole defect),
                // not a transition-metal d-d transition, a broad band centred roughly
                // 610-620nm (red-orange), leaving blue transmitted. Source: K. Nassau,
                // "Gemstone Enhancement" (2nd ed., Butterworth-Heinemann, 1994), the
                // standard gemological reference for this mechanism. Width (70nm,
                // broader than the sharp transition-metal bands elsewhere in this
                // file -- colour centres are typically broad) and peak (1.6) tuned;
                // band centre (620nm) cited. Colourless natural (pre-treatment) topaz
                // is reachable with an empty band set; imperial topaz (orange-pink,
                // Cr3+ ~540nm) is a candidate for a follow-up built-in.
                absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                    620.0, 70.0, 1.6,
                )]),
                c_axis: Vec3::Y,
                // n_beta - n_alpha at the D line, LOWER CONFIDENCE (derived, not
                // directly measured for a single specimen). No primary 3-index topaz
                // measurement exists. Source: The Gemology Project's topaz entry,
                // citing ranges n_alpha=1.606-1.634, n_beta=1.609-1.637,
                // n_gamma=1.616-1.644 across topaz's fluorine/hydroxyl compositional
                // range. Midpoints (1.620, 1.623, 1.630) give the fractional position
                // of beta between alpha and gamma: (1.623-1.620)/(1.630-1.620) = 0.30.
                // Applied to this entry's own birefringence_delta (0.0080):
                // delta_beta_alpha = 0.30 * 0.0080 = 0.0024. Implied n_alpha=1.624778,
                // n_gamma=1.632778, both inside the cited ranges.
                biaxial_delta_beta_alpha: Some(0.0024),
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }
}
