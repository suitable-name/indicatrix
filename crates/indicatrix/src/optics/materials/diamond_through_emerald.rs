//! Built-in material data: Diamond, Sapphire, Ruby, Emerald.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// First quarter of the built-in material table (Diamond through Emerald). Split
    /// out of `all_materials` purely to keep each part under clippy's function-length
    /// lint -- this is plain data, not logic, so the split point carries no
    /// significance.
    pub(super) fn built_in_materials_diamond_through_emerald() -> Vec<Self> {
        let mut materials = Self::built_in_materials_diamond_and_sapphire();
        materials.extend(Self::built_in_materials_ruby_and_emerald());
        materials
    }

    /// First half of the Diamond-through-Emerald part (Diamond, Sapphire).
    fn built_in_materials_diamond_and_sapphire() -> Vec<Self> {
        vec![
            // Diamond (C)
            //
            // Source: R. Peter, Z. Phys. 15, 358 (1923), the standard 2-term Sellmeier
            // fit for diamond (refractiveindex.info "Diamond: n (Peter 1923)"), valid
            // 0.226-0.760 um:
            //   n^2 - 1 = 4.3356*lambda^2/(lambda^2-0.1060^2) + 0.3306*lambda^2/(lambda^2-0.1750^2)
            // Represented via Sellmeier3's 3-pole form with the 3rd pole zeroed (b=0,
            // c=1.0 um^2, outside the visible-range domain) since DispersionModel has
            // no native 2-term Sellmeier variant. Verified: n_d = 2.41726, Delta
            // n(F-C) = 0.02564, Abbe V_d = 55.27 -- matches literature (n_D ~ 2.417,
            // V_d ~ 55.3). See `tests::builtin_material_abbe_numbers_match_published_values`.
            Self {
                name: "Diamond".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [4.3356, 0.3306, 0.0],
                    c: [0.011_236, 0.030_625, 1.0],
                },
                birefringence_delta: 0.0,
                // colorless (no chromophore): empty band set, zero absorption at every
                // wavelength -- must render identically to before.
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
            // Sapphire (Al2O3)
            //
            // Source: I.H. Malitson & M.J. Dodge, J. Opt. Soc. Am. 62, 1405A (1972),
            // ordinary-ray Sellmeier fit (refractiveindex.info "Al2O3: Malitson-o"),
            // valid 0.2-5.0 um:
            //   n^2-1 = 1.4313493 l^2/(l^2-0.0726631^2) + 0.65054713 l^2/(l^2-0.1193242^2)
            //           + 5.3414021 l^2/(l^2-18.028251^2)
            // Verified: n_d = 1.76808, Delta n(F-C) = 0.01063, Abbe V_d = 72.27,
            // matching the standard literature figure V_d(sapphire) ~ 72.
            //
            // birefringence_delta derived from Malitson & Dodge's own companion e-ray
            // Sellmeier fit (same 1972 paper, "Al2O3: Malitson-e"):
            //   n_e^2 = 1 + 1.5039759*l^2/(l^2-0.0740288^2) + 0.55069141*l^2/(l^2-0.1216529^2)
            //           + 6.5927379*l^2/(l^2-20.072248^2)
            // giving n_e(D) = 1.760002 against n_o(D) = 1.768106, i.e. n_e - n_o =
            // -0.008104 at the sodium D line, both indices from the same primary paper.
            // See `tests::builtin_material_abbe_numbers_match_published_values`.
            Self {
                name: "Sapphire".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.431_349, 0.650_547, 5.341_402],
                    c: [0.00528, 0.01424, 325.015],
                },
                birefringence_delta: -0.0081,
                // Chromophore: blue sapphire's color comes from a broad Fe2+-Ti4+
                // intervalence charge-transfer (IVCT) band centred ~580nm (yellow),
                // absorbing yellow-orange-red and transmitting blue (see e.g. "Fe-Ti
                // Charge Transfer: The Mechanism Behind Sapphire's Blue," skyjems.ca).
                // Width (90nm) and amplitude (peak=3.0) tuned for plausible saturation
                // at a typical internal path length.
                //
                // PLEOCHROISM: genuine uniaxial `o_ray`/`e_ray` split with a real
                // ~70nm band-centre shift between the two rays. Source: A.J. Emmett, M.
                // Dubinsky, R. Hughes & M. Scarratt, "The Colors of Sapphires," Gems &
                // Gemology 56(1), Spring 2020: "For E-perp-c the [Fe2+-Ti4+ IVCT] band
                // peaks at 580 nm, while for E-parallel-c the peak is at 700 nm"
                // (E-perp-c = o-ray, E-parallel-c = e-ray). o-ray amplitude (peak=3.0)
                // is consistent with the paper's cited E-perp-c cross-section
                // (1.94e-18 cm^2 +/-25%). e_ray band centre (700nm) is cited; width
                // (95nm) and amplitude (peak=2.1, figure-read off Emmett et al. Fig.
                // 10's relative peak heights, the lowest-confidence number here) are
                // tuned/estimated.
                //
                // 2026-10-19 color check (OWNER DECISION: take the measured-shape
                // refit) REPLACED the single
                // 580/700 nm wavelength-Gaussians by bounded 3-band energy-Gaussian
                // fits of the GIA Fe2+-Ti4+ pair cross section (Dubinsky,
                // Stone-Sundberg & Emmett, Gems & Gemology Spring 2020 [S1];
                // `data/reference_spectra/gia_v3plus_fe2plus_ti4plus_sigma_5nm.csv`:
                // 1.98e-18 cm^2 at 580 nm E-perp-c, 1.25e-18 at 700 nm E-parallel-c)
                // at 10 ppma*cm. Fitted, not tuned. Swatch (7 mm, D65): L*57 C*40
                // h=283 (x1.2 for L*51); the old data gave L*54 h=275.
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E-perp-c)
                        AbsorptionBand::energy(585.0, 4_878.0, 2.01),
                        AbsorptionBand::energy(744.0, 3_142.0, 0.887),
                        AbsorptionBand::energy(457.0, 12_000.0, 0.433),
                    ],
                    vec![
                        // e-ray (E-parallel-c)
                        AbsorptionBand::energy(612.0, 7_466.0, 0.371),
                        AbsorptionBand::energy(729.0, 4_955.0, 1.03),
                        AbsorptionBand::energy(467.0, 11_169.0, 0.524),
                    ],
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

    /// Second half of the Diamond-through-Emerald part (Ruby, Emerald), split off
    /// `built_in_materials_diamond_through_emerald` for the same function-length reason.
    fn built_in_materials_ruby_and_emerald() -> Vec<Self> {
        vec![
            // Ruby (Al2O3:Cr) -- same host lattice as Sapphire above (trace Cr3+ at
            // sub-1% does not measurably shift the host Al2O3 dispersion), so it
            // shares the identical Malitson & Dodge (1972) Sellmeier fit. See the
            // Sapphire entry's comment for the source, the verified n_d/Delta
            // n(F-C)/Abbe numbers, and the birefringence_delta derivation.
            Self {
                name: "Ruby".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.431_349, 0.650_547, 5.341_402],
                    c: [0.00528, 0.01424, 325.015],
                },
                birefringence_delta: -0.0081,
                // Chromophore: ruby's Cr3+ absorbs in two bands -- violet near 410nm
                // and yellow-green near 550nm (the two spin-allowed Cr3+ d-d
                // transitions, ~4A2->4T1 and ~4A2->4T2) -- leaving a wide red
                // transmission window (>~620nm) and a narrower blue window
                // (~470-490nm) between the peaks. Sources: GIA "Application of
                // UV-Vis-NIR Spectroscopy to Gemology" (Winter 2024 Gems & Gemology)
                // and Cr:Al2O3 spectroscopy papers citing peaks at ~410-413nm/~550nm.
                // This double-window structure is what makes ruby shift redder under
                // incandescent light than daylight (see
                // `raytracer_tests::ruby_shifts_redder_under_incandescent_than_d65`).
                // Widths (30nm/45nm) and peaks (3.0/2.5) tuned; band positions cited.
                //
                // PLEOCHROISM: uniaxial `o_ray`/`e_ray` split. Source: J.A. Mandarino,
                // American Mineralogist 44, 961 (1959), Table 5: k_omega (o-ray) maxes
                // near 560nm, k_epsilon (e-ray) near 550nm; raw peak ratios
                // omega:epsilon range 1.37 (pink) to 2.25 (deep red), with the two
                // rays near-equal around 440nm. o-ray yellow-green centre set to 556nm
                // (close to Mandarino's ~560nm); e_ray centres sit blueward of the
                // o-ray's (400nm/550nm), matching k_epsilon's blueward shift; e_ray
                // widths reuse the o-ray's own (30nm/45nm, no separate figure given).
                // Amplitude ratio: 1.8x per band (violet band kept near-equal between
                // rays per Mandarino's "near-equal at 440nm"; yellow-green carries the
                // bulk of the dichroic ratio) -- a mid-range, tuned simplification of
                // Mandarino's cited 1.37-2.9 span.
                //
                // 2026-10-19 color check REPLACED
                // the hand-tuned 410/556 nm (o) and 400/550 nm (e) Gaussians (density
                // of a 0.8 mm slice, window ratio A(472)/A(556) = 0.32 vs 0.075
                // measured: pink-mauve, L*57 C*30 h=340) by bounded 3-band
                // energy-Gaussian fits to the GIA sample 1110 Cr3+ cross sections
                // (Dubinsky, Stone-Sundberg & Emmett, Gems & Gemology Spring 2020
                // [S1]; `data/reference_spectra/gia_cr3plus_sample1110_sigma.csv`),
                // scaled to 800 ppma*cm Cr (e.g. ~900 ppma over the 8.8 mm face-up
                // path; OWNER DECISION 2026-10-19; 1 ppma = 1.178e17 cm^-3 [S1]).
                // Fitted, not tuned; the DENSITY (800 ppma*cm) is the owner's choice
                // (600 ppma*cm would be these peaks x 0.75). The o-ray is the
                // stronger absorber (U band 10.9 vs 3.4); the e-ray's 397 nm band
                // carries the measured E-parallel-c near-UV rise. Swatch (7 mm,
                // D65): L*41 C*67 h=356 (p=1.5: h 4; p=2: h 11), GIA Appendix 2
                // "both" 38.7/70.8/352 at 750 ppma*cm.
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E-perp-c)
                        AbsorptionBand::energy(562.0, 1_493.0, 10.9),
                        AbsorptionBand::energy(410.0, 2_767.0, 15.8),
                        AbsorptionBand::energy(537.0, 2_688.0, 7.06),
                    ],
                    vec![
                        // e-ray (E-parallel-c)
                        AbsorptionBand::energy(553.0, 1_721.0, 3.38),
                        AbsorptionBand::energy(397.0, 3_445.0, 27.1),
                        AbsorptionBand::energy(525.0, 2_303.0, 2.56),
                    ],
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
            // Emerald (Beryl, Be3Al2(SiO3)6:Cr/V)
            //
            // No primary-literature Sellmeier fit for beryl exists (absent from
            // refractiveindex.info's database), so this is a Cauchy (visible-range-
            // only) fit, LOWER CONFIDENCE than the Sellmeier-sourced entries above.
            //
            // n_d = 1.5791, within International Gem Society's cited emerald range
            // 1.57-1.60. Dispersion shape: IGS's cited "dispersion .014" is the
            // Fraunhofer B-G interval, not F-C; converted via a B-G -> F-C ratio
            // computed directly from physics (evaluating this file's 8 genuine primary
            // Sellmeier fits -- Diamond, Sapphire/Ruby, Quartz, Spinel, Cubic
            // Zirconia, Alexandrite, Moissanite -- at the actual Fraunhofer
            // wavelengths: ratio range 0.569-0.587, mean 0.579, used throughout this
            // file for every gemological, non-primary-sourced species). Delta n(F-C)
            // = 0.0141*0.579 = 0.00816, giving Abbe V_d = 70.93. LOWER CONFIDENCE than
            // the Sellmeier-sourced entries -- flagged for human cross-check.
            Self {
                name: "Emerald".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.566_794,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: -0.0060,
                // Chromophore: emerald's Cr3+ (some localities: V3+ too) absorbs in a
                // blue-violet band near 430nm and a red-orange band near 600nm,
                // leaving a narrow green transmission window near 510nm (alpha(lambda)
                // minimized at ~506nm) -- this two-window structure is what makes
                // emerald green. Sources: GIA gemological references reporting
                // emerald's "main absorption bands at approximately 620 and 430nm"
                // (the red-side centre is reported ~600-620nm across localities;
                // 600nm used here). Widths (30nm/40nm) and peaks (2.8/2.6) tuned; band
                // positions cited.
                //
                // 2026-10-19 color check (OWNER DECISION: take the measured-shape
                // refit) REPLACED the isotropic
                // 430/600 nm wavelength-Gaussians by uniaxial bounded 3-band
                // energy-Gaussian fits of the Caltech GRR 3570 emerald spectrum
                // (G. R. Rossman, Caltech Mineral Spectroscopy Server;
                // `data/reference_spectra/caltech_emerald_grr3570_5nm.csv`) scaled to
                // 0.3 cm. Fitted, not tuned. This adds the measured o/e dichroism
                // (Wood & Nassau 1968 [S3]: o-ray yellowish green, e-ray bluish
                // green). Swatch (7 mm, D65): L*65 C*101 h=168 (old L*64 h=166).
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E-perp-c)
                        AbsorptionBand::energy(430.0, 3_579.0, 4.5),
                        AbsorptionBand::energy(591.0, 2_410.0, 2.7),
                        AbsorptionBand::energy(633.0, 1_860.0, 1.6),
                    ],
                    vec![
                        // e-ray (E-parallel-c)
                        AbsorptionBand::energy(422.0, 2_681.0, 3.7),
                        AbsorptionBand::energy(584.0, 1_656.0, 2.2),
                        AbsorptionBand::energy(645.0, 1_595.0, 5.4),
                    ],
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
