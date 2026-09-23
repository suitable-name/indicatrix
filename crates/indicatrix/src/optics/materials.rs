use super::{
    absorption::{AbsorptionBand, AbsorptionTensor, legacy_rgb_bands},
    birefringence::BiaxialIndicatrix,
    dispersion::DispersionModel,
};
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CrystalSystem {
    Cubic,
    Tetragonal,
    Hexagonal,
    Trigonal,
    Orthorhombic,
    Monoclinic,
    Triclinic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OpticalCharacter {
    Isotropic,
    UniaxialPositive,
    UniaxialNegative,
    BiaxialPositive,
    BiaxialNegative,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GemMaterial {
    pub name: String,
    pub crystal_system: CrystalSystem,
    pub optical_character: OpticalCharacter,
    pub dispersion: DispersionModel,
    pub birefringence_delta: f32, // n_e - n_o (or max-min)
    pub absorption: AbsorptionTensor,
    /// Optical (crystallographic) c-axis direction, in crystal/model space. Uniaxial
    /// birefringence (`effective_extraordinary_index`, `extraordinary_poynting_dir`)
    /// is evaluated against this axis. Defaults to `Vec3::Y`. For a biaxial material
    /// (`biaxial_delta_beta_alpha.is_some()`) this doubles as the `n_gamma` principal
    /// axis -- see `biaxial_indicatrix`.
    pub c_axis: Vec3,
    /// `n_beta - n_alpha` at the sodium D line, for the three biaxial (orthorhombic)
    /// built-ins. `None` for every isotropic/uniaxial material, which keep using the
    /// `c_axis` + `birefringence_delta` uniaxial machinery.
    ///
    /// Convention: `birefringence_delta` is `n_gamma - n_alpha` for a biaxial entry, and
    /// the base `dispersion` curve is `n_beta(lambda)`, the middle principal index (see
    /// `biaxial_indicatrix`). This one extra scalar then places all three principal
    /// indices `n_alpha <= n_beta <= n_gamma`, treating the SPREAD between them as
    /// wavelength-independent (only the base curve disperses) -- the same
    /// achromatic-delta approximation the uniaxial `n_e = n_o + birefringence_delta`
    /// already makes.
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// Inclusion/subsurface scattering: the homogeneous Henyey-Greenstein scattering
    /// coefficient (`sigma_s`, a physical, linear coefficient in inverse model units of
    /// path length -- no perceptual/logarithmic remapping) modeling silk, rutile
    /// needles, and clouds as a single averaged-out volumetric density.
    ///
    /// `0.0` (every built-in's own stored value, and `new_custom`'s) means no
    /// scattering medium: `raytracer::apply_absorption`'s deterministic Beer-Lambert
    /// path is taken unconditionally whenever this is `<= 0.0`. See
    /// `raytracer::maybe_scatter_or_extinguish` for the estimator this feeds once
    /// nonzero (extinction `sigma_t = sigma_a + sigma_s`, free-path distance sampling,
    /// single-scattering albedo `sigma_s / sigma_t`). Use [`Self::with_scattering`] (or
    /// [`Self::with_recommended_scattering`] for a per-species starting point) to opt a
    /// material into a nonzero value.
    ///
    /// # Useful range
    ///
    /// The built-in cuts (`geometry::StandardGemCuts`) have a girdle radius of order 1
    /// model unit, so a typical internal chord length is roughly 0.5-2 units and the
    /// mean free path between scattering events is `1/sigma_s`. `sigma_s` in roughly
    /// `0.05` (barely perceptible haze) to `3.0` (milky/heavily included) covers the
    /// visually meaningful range for this geometry scale.
    pub scattering_sigma_s: f32,
    /// The Henyey-Greenstein phase function's asymmetry parameter `g` in `(-1, 1)`: `0`
    /// is isotropic scattering, positive values forward-scatter (silk/rutile needle
    /// inclusions are usually forward-scattering), negative values back-scatter.
    /// Meaningless while `scattering_sigma_s <= 0.0`; defaults to `0.0` alongside it.
    /// See [`Self::DEFAULT_SCATTERING_G`] for a sensible default when a caller only
    /// wants to control the amount ([`Self::with_scattering_amount`]).
    pub scattering_g: f32,
    /// Facet edge rounding: the micron-scale rounding radius real meet-point edges
    /// have, in the same world units as `scattering_sigma_s` (girdle radius of order 1
    /// model unit) -- so `0.01` models an edge rounded over about 1% of the stone's
    /// scale, comfortably in the "throws a soft glint, does not visibly bevel the
    /// facet" range. `0.0` (every built-in) disables the effect entirely:
    /// `raytracer::shading_normal_near_edge` returns the flat facet normal unperturbed
    /// whenever this is `<= 0.0`. See [`Self::with_edge_rounding`] to opt in.
    pub edge_rounding_radius: f32,
    /// Model units to absorption-length units: every interior path length is
    /// multiplied by this before Beer-Lambert absorption and inclusion scattering.
    /// `1.0` (every built-in, and `new_custom`'s default) is a no-op. See
    /// [`Self::with_absorption_path_scale`] to opt a material into a different
    /// physical size (e.g. a larger or smaller real-world stone rendered at the same
    /// ~1-model-unit girdle radius as every built-in cut).
    pub absorption_path_scale: f32,
    /// An optional, genuinely wavelength-dependent extraordinary-ray dispersion curve
    /// for a uniaxial material, evaluated instead of the constant-offset approximation
    /// `n_e(lambda) = n_o(lambda) + birefringence_delta` (see
    /// [`Self::extraordinary_index_at`], the single place both are read). Real
    /// birefringence is not wavelength-flat, but modelling that needs the
    /// extraordinary ray's own independent dispersion curve; only Quartz (and the
    /// quartz-derived Amethyst/Citrine) carry a genuine primary o/e Sellmeier pair
    /// (G. Ghosh 1999 -- see that entry's comment).
    ///
    /// `None` (every other built-in, and [`Self::new_custom`]'s default) falls back to
    /// `n_o + birefringence_delta` in `extraordinary_index_at`. Meaningless for an
    /// isotropic or biaxial material -- only `optics::raytracer::refraction`'s uniaxial
    /// per-channel index lookups read it.
    ///
    /// Threaded through to the GPU backend: `renderer::buffers::GpuGemMaterial` carries
    /// this curve as its own `has_extraordinary_dispersion`/`extraordinary_model_type`/
    /// `extraordinary_param_a`/`extraordinary_param_b` fields (via
    /// `renderer::buffers::encode_dispersion_model`), and
    /// `shaders/spectral_transport.wgsl`'s `extraordinary_index_at` evaluates it with
    /// the identical formula (same `n >= 1.0` floor) and the same fallback.
    pub uniaxial_extraordinary_dispersion: Option<DispersionModel>,
}

impl GemMaterial {
    #[must_use]
    pub fn all_materials() -> Vec<Self> {
        let mut materials = Self::built_in_materials_diamond_through_emerald();
        materials.extend(Self::built_in_materials_zircon_through_topaz());
        materials.extend(Self::built_in_materials_spinel_through_tourmaline());
        materials.extend(Self::built_in_materials_tanzanite_through_cubic_zirconia());
        // Four more quarters of new species. See
        // [`Self::built_in_materials_aquamarine_through_citrine`]'s module-level note
        // for Sphene (left out) and [`Self::built_in_material_rutile`] for why Rutile
        // is included.
        materials.extend(Self::built_in_materials_aquamarine_through_citrine());
        materials.extend(Self::built_in_materials_amethyst_through_citrine());
        materials.extend(Self::built_in_materials_garnets_pyrope_through_spessartine());
        materials.extend(Self::built_in_materials_garnets_grossular_and_andradite());
        materials.extend(Self::built_in_materials_peridot_through_benitoite());
        materials.extend(Self::built_in_materials_andalusite_through_glass());
        materials.push(Self::built_in_material_rutile());
        materials
    }

    /// First quarter of the built-in material table (Diamond through Emerald). Split
    /// out of `all_materials` purely to keep each part under clippy's function-length
    /// lint -- this is plain data, not logic, so the split point carries no
    /// significance.
    fn built_in_materials_diamond_through_emerald() -> Vec<Self> {
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
                // Colourless (no chromophore): empty band set, zero absorption at every
                // wavelength -- must render identically to before.
                absorption: AbsorptionTensor::isotropic(vec![]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
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
                // Chromophore: blue sapphire's colour comes from a broad Fe2+-Ti4+
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
                absorption: AbsorptionTensor::uniaxial(
                    vec![AbsorptionBand::new(580.0, 90.0, 3.0)], // o-ray (E-perp-c)
                    vec![AbsorptionBand::new(700.0, 95.0, 2.1)], // e-ray (E-parallel-c), IVCT at 700nm
                ),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
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
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        AbsorptionBand::new(410.0, 30.0, 3.0), // o-ray (E-perp-c)
                        AbsorptionBand::new(556.0, 45.0, 2.5),
                    ],
                    vec![
                        AbsorptionBand::new(400.0, 30.0, 2.9), // e-ray (E-parallel-c)
                        AbsorptionBand::new(550.0, 45.0, 1.4),
                    ],
                ),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
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
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(430.0, 30.0, 2.8),
                    AbsorptionBand::new(600.0, 40.0, 2.6),
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

    /// Second quarter of the built-in material table (Zircon through Topaz). Split out
    /// of `built_in_materials_diamond_through_emerald` purely to keep each part under
    /// clippy's function-length lint -- this is plain data, not logic, so the split
    /// point carries no significance.
    fn built_in_materials_zircon_through_topaz() -> Vec<Self> {
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

    /// Third quarter of the built-in material table (Spinel through Tourmaline). See
    /// `built_in_materials_diamond_through_emerald` for why the table is split into
    /// several functions.
    fn built_in_materials_spinel_through_tourmaline() -> Vec<Self> {
        vec![
            // Spinel (MgAl2O4)
            //
            // Source: W.J. Tropf & M.E. Thomas, "Magnesium Aluminum Oxide (Spinel),"
            // Handbook of Optical Constants of Solids III (1991), as tabulated by
            // refractiveindex.info ("MgAl2O4: Tropf"), valid 0.35-5.5 um:
            //   n^2-1 = 1.8938*l^2/(l^2-0.09942^2) + 3.0755*l^2/(l^2-15.826^2)
            // Encoded via Sellmeier3 with the unused 3rd pole zeroed (b=0, c=1.0 um^2,
            // outside the visible-range l^2 domain). Verified: n_d = 1.71610, Delta
            // n(F-C) = 0.01181, Abbe V_d = 60.63. Cross-check: gemological (B-G)
            // dispersion table lists Spinel = 0.020; 0.020*0.591 (the B-G->F-C ratio,
            // see Emerald entry) = 0.0118, matching this Sellmeier-derived value
            // almost exactly.
            Self {
                name: "Spinel".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.8938, 3.0755, 0.0],
                    c: [0.009_884, 250.462, 1.0],
                },
                birefringence_delta: 0.0,
                // Models RED spinel: Cr3+ substituting for Al3+/Mg2+, absorbing green
                // light in a band centred ~540nm (the same Cr3+ d-d transition family
                // as Ruby/Alexandrite/Emerald, here a single dominant band rather than
                // corundum's two-band structure) -- standard gemological attribution,
                // e.g. K. Nassau, "The Physics and Chemistry of Color" (2nd ed.,
                // Wiley, 2001), Ch. 7. Width (55nm) and peak (2.4) tuned; band centre
                // (540nm) cited. BLUE spinel (Co2+, an optically distinct
                // chromophore) is not modelled here -- a candidate follow-up built-in.
                absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                    540.0, 55.0, 2.4,
                )]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Quartz (Rock Crystal / Amethyst / Citrine, alpha-SiO2)
            //
            // Source: G. Ghosh, "Dispersion-equation coefficients for the refractive
            // index and birefringence of calcite and quartz crystals," Opt. Commun.
            // 163, 95-102 (1999), ordinary-ray fit (refractiveindex.info "SiO2:
            // Ghosh-o"), valid 0.198-2.0531 um:
            //   n^2-1 = 0.28604141 + 1.07044083*l^2/(l^2-0.0100585997) + 1.10202242*l^2/(l^2-100)
            // Independently verified against a manufacturer (pmoptics.com) quartz
            // spec sheet: n_o(630nm)=1.54270 (spec: 1.542737), n_o(1550nm)=1.52770
            // (spec: 1.527606), both matching to <1e-4. Encoded via Sellmeier3: the
            // leading 0.28604141 constant uses pole 1 with c=0.0 (reduces to the bare
            // constant); the other two are the genuine poles. Verified: n_d = 1.54421,
            // Delta n(F-C) = 0.00781, Abbe V_d = 69.65. Note: this is lower than the
            // ~0.013 gemological "dispersion" figure often quoted for quartz -- that
            // figure is the standard B-G (not F-C) interval (0.013*0.591 = 0.00768,
            // matching this Ghosh-derived value to within 2%).
            Self {
                name: "Quartz".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // This entry is the colourless rock-crystal reference: empty band set
                // (zero absorption at every wavelength). Rock crystal quartz
                // genuinely has no visible-range chromophore; its tinted varieties
                // (Amethyst: a hole-colour-centre defect; Citrine: Fe3+) are their own
                // separate built-ins (see
                // [`Self::built_in_materials_aquamarine_through_citrine`]), sharing
                // this exact dispersion/birefringence data (same SiO2 host crystal)
                // but with their own cited absorption bands.
                absorption: AbsorptionTensor::isotropic(vec![]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                // Quartz is the one built-in with a genuine primary e-ray Sellmeier
                // fit alongside its o-ray one -- G. Ghosh, Opt. Commun. 163, 95-102
                // (1999), extraordinary-ray fit (refractiveindex.info "SiO2:
                // Ghosh-e"), valid 0.198-2.0531 um:
                //   n^2-1 = 0.28851804 + 1.09509924*l^2/(l^2-0.0102101864) + 1.15662475*l^2/(l^2-100)
                // Same encoding technique as the o-ray fit above. Verified: n_e(D) =
                // 1.55338 against n_o(D) = 1.54421, i.e. n_e - n_o = +0.00917 at the
                // sodium D line, matching `birefringence_delta` (0.0091) within
                // rounding, from the same primary paper as the o-ray fit. Wired
                // through [`GemMaterial::extraordinary_index_at`] so quartz's
                // extraordinary ray disperses with its own genuine curve rather than
                // the constant-offset approximation every other uniaxial built-in
                // uses.
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
            },
            // Tourmaline (Elbaite, complex borosilicate)
            //
            // No primary Sellmeier/Cauchy fit for tourmaline exists (absent from the
            // refractiveindex.info database; tourmaline's compositional variability,
            // a large solid-solution family rather than a fixed stoichiometric
            // crystal, makes a single universal fit unlikely to exist at all).
            // LOWER-CONFIDENCE 2-parameter Cauchy fallback.
            //
            // n_d = 1.639405, within International Gem Society's cited elbaite range
            // (n_o = 1.619-1.655, n_e = 1.603-1.634). Dispersion shape: IGS's cited
            // "dispersion .017" is the Fraunhofer B-G interval, not F-C. Converted via
            // the same physically-derived B-G->F-C ratio as Emerald's comment above
            // (0.579): Delta n(F-C) = 0.0171*0.579 = 0.00990. Two data points exactly
            // determine a 2-parameter Cauchy fit (same method as Zircon's comment):
            // A=1.624481, B=0.005183, verified n_d=1.639406, Delta n(F-C)=0.009902,
            // Abbe V_d=64.58. LOWER CONFIDENCE than a primary-literature entry --
            // flagged for human cross-check.
            //
            // birefringence_delta = -0.0210, matching the IGS representative pairing
            // (n_o~1.644, n_e~1.623) widely reproduced for "typical" (green) elbaite
            // -- IGS's own range only bounds it (n_e-n_o spans roughly -0.016 to
            // -0.021 across the elbaite range), so this is an estimate, not a
            // single-crystal measurement.
            Self {
                name: "Tourmaline".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.624_481,
                    b: 0.005_183,
                    c: 0.0,
                },
                birefringence_delta: -0.0210,
                // PLEOCHROISM: genuine uniaxial `o_ray`/`e_ray` split -- tourmaline
                // (elbaite) is famous for its strong dichroism ("the dark ray" along c
                // vs the lighter ray across it).
                //
                // Band positions: R.P. Mattson & G.R. Rossman, Phys. Chem. Minerals
                // 14, 163 (1987), Fe-bearing tourmaline polarized absorption spectra,
                // and the Caltech Mineral Spectroscopy Server's dravite entry (sample
                // GRR 787): intense E-perp-c ("the dark ray") absorptions at 730nm and
                // 1120nm (the 1120nm band is outside this renderer's 380-780nm
                // channel range), plus a Fe2+-Ti4+ IVCT band near 430nm present in
                // both rays. Centres here (720nm, 430nm) sit at the cited positions
                // rounded to the nearest 10nm; widths (55nm, 45nm) tuned.
                //
                // Dichroic ratio: Mattson & Rossman report that at high Fe content the
                // E-perp-c (o-ray) intensity is enhanced more than 10x over
                // E-parallel-c (e-ray) -- real stones span roughly 1.1x (pale) to
                // >10x (dark) depending on iron content and path length. The 3x ratio
                // used here (o-ray peaks 3.0/1.8 vs e-ray peaks 1.0/0.6) is a
                // mid-range, tuned choice sitting well inside that span, giving a
                // visibly dichroic stone without the e-ray direction rendering
                // near-black at short path lengths.
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        AbsorptionBand::new(720.0, 55.0, 3.0), // o-ray (E-perp-c), "the dark ray"
                        AbsorptionBand::new(430.0, 45.0, 1.8),
                    ],
                    vec![
                        AbsorptionBand::new(720.0, 55.0, 1.0), // e-ray (E-parallel-c)
                        AbsorptionBand::new(430.0, 45.0, 0.6),
                    ],
                ),
                // c_axis set INTO the table plane (Vec3::X) rather than the Vec3::Y
                // every other built-in defaults to -- a deliberate override, not an
                // oversight. Real tourmaline cutters orient the table perpendicular to
                // the c-axis because face-up down the closed (e-ray/dark-ray) axis is
                // tourmaline's worst viewing direction: with c_axis=Y, the face-up
                // hero shot would look straight down the dark ray, backwards from how
                // the stone is actually cut and worn. See
                // `raytracer_tests::tourmaline_face_up_is_brighter_with_c_axis_in_table_plane`.
                c_axis: Vec3::X,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }

    /// Fourth quarter of the built-in material table (Tanzanite through Cubic
    /// Zirconia). See `built_in_materials_diamond_through_emerald` for why the table is
    /// split into several functions.
    fn built_in_materials_tanzanite_through_cubic_zirconia() -> Vec<Self> {
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
                // follows directly from the three colours Hurlbut reports.
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
                // Colourless (no chromophore): empty band set, zero absorption at
                // every wavelength -- must render identically to before.
                absorption: AbsorptionTensor::isotropic(vec![]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
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
                // Colourless (no chromophore): empty band set, zero absorption at
                // every wavelength -- must render identically to before.
                absorption: AbsorptionTensor::isotropic(vec![]),
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

    /// First of four new-species quarters: Aquamarine, Morganite (both beryl, sharing
    /// Emerald's host-mineral optics), Chrysoberyl (Yellow) (sharing Alexandrite's
    /// host-mineral indicatrix), Amethyst and Citrine (both quartz, sharing the
    /// colourless Quartz entry's exact Ghosh 1999 o/e Sellmeier pair above).
    ///
    /// # Dispersion-figure convention note (applies to every entry in this file)
    ///
    /// Every gemological "dispersion" target below is the standard Fraunhofer B-G
    /// table value (e.g. Andradite/demantoid's famous "0.057," higher than diamond's
    /// own 0.044) and is converted B-G -> F-C via the same physically-derived 0.579
    /// ratio the Emerald/Zircon/Topaz/Tourmaline/Tanzanite entries above use, rather
    /// than taken at face value as F-C. The exception is entries with a genuine named
    /// primary dispersion source (Chrysoberyl/Aquamarine/Morganite, which reuse an
    /// already-F-C-fitted host-mineral curve directly; YAG, whose real Zelmon 1998
    /// Sellmeier is used as-is per this file's primary-source-wins rule; and the two
    /// optical glasses, whose Schott catalogue Abbe numbers are already true F-C).
    ///
    /// # Deliberately left out of this list: Sphene
    ///
    /// Sphene (titanite) needs `birefringence_delta` up to ~0.135, well beyond every
    /// existing Delta-n-range assumption this file's biaxial/uniaxial machinery has
    /// been verified against (the largest here, Zircon's +0.059, is under half that),
    /// so adding it without dedicated verification at that magnitude would ship an
    /// unverified extrapolation, not a measurement. Left out.
    ///
    /// Rutile needs the full anisotropic (uniaxial, extremely high birefringence
    /// +0.287) Fresnel treatment, compounded by rutile's very strong dispersion, so
    /// it gets its own dedicated entry -- see [`Self::built_in_material_rutile`].
    fn built_in_materials_aquamarine_through_citrine() -> Vec<Self> {
        vec![
            // Aquamarine (Beryl, Be3Al2(SiO3)6:Fe2+) -- same host mineral as Emerald
            // above, so it reuses Emerald's exact Cauchy dispersion shape (the `b`
            // coefficient, i.e. the same Delta n(F-C) curvature) with only the leading
            // constant `a` retargeted to this variety's own n_d -- beryl's dispersion
            // is a host-lattice property essentially independent of which trace ion
            // tints it. a = 1.577 - 0.004273/0.5893^2 = 1.564698. Verified: n_d =
            // 1.577000 (exact by construction), Delta n(F-C) = 0.00816 (identical to
            // Emerald's), Abbe V_d = 70.71.
            Self {
                name: "Aquamarine".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.564_698,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: -0.0060,
                // Chromophore: Fe2+ in the beryl channel structure, whose primary
                // absorption sits near 830nm (near-infrared), outside this renderer's
                // 380-780nm sampled band. Modelled with a wide Gaussian (width 110nm)
                // so its blue-side tail still reaches into the 700-780nm red edge of
                // the visible band (unlike Tourmaline's 1120nm band, too far out to
                // reach it at all), giving aquamarine its pale blue-green cast. Peak
                // (1.0) deliberately weak, matching aquamarine's reputation as one of
                // the palest common coloured gemstones.
                absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                    830.0, 110.0, 1.0,
                )]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Morganite (Beryl, Be3Al2(SiO3)6:Mn3+) -- same beryl host as Aquamarine
            // immediately above (both varieties share n_d=1.577), so this entry's
            // dispersion is bit-identical to Aquamarine's.
            Self {
                name: "Morganite".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.564_698,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: -0.0060,
                // Chromophore: Mn3+ in the beryl channel structure, a single band
                // near 540nm (green) -- the standard attribution for morganite's pink
                // colour. Width (45nm) and peak (1.0, deliberately weak -- morganite
                // is a characteristically pale pink stone) tuned; band centre cited.
                absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                    540.0, 45.0, 1.0,
                )]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Chrysoberyl (Yellow, BeAl2O4:Fe3+) -- same host mineral as Alexandrite
            // above (a Cr3+-free, Fe3+-bearing chrysoberyl), sharing its indicatrix.
            // Reuses Alexandrite's exact Sellmeier3 dispersion shape (both
            // non-constant poles unchanged) with only the leading constant pole
            // (`b[0]`, a c=0 pole contributing a wavelength-independent additive
            // constant to n^2-1) retargeted from Alexandrite's 0.78522 to 0.796473 so
            // n_d hits this variety's own 1.746 target, while Delta n(F-C) -- which a
            // purely additive constant cannot change -- stays identical to
            // Alexandrite's own primary-sourced 0.010051 (Abbe V_d = 74.22).
            Self {
                name: "Chrysoberyl (Yellow)".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.796_473, 1.212_02, 16.81],
                    c: [0.0, 0.012_62, 1000.0],
                },
                birefringence_delta: 0.0090,
                // Chromophore: Fe3+ substitution (rather than Alexandrite's Cr3+), a
                // single band near 440nm (blue-violet) leaving yellow-red transmitted
                // -- the standard attribution for ordinary (non-colour-change)
                // yellow/green chrysoberyl. Width (35nm) and peak (1.4) tuned; band
                // centre cited.
                absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                    440.0, 35.0, 1.4,
                )]),
                c_axis: Vec3::Y,
                // Same fractional-position method as Alexandrite/Topaz/Tanzanite
                // above (no primary 3-index measurement for the Fe3+ variety):
                // reuses Alexandrite's own beta-between-alpha-and-gamma fraction
                // (0.001951 / 0.0076 = 0.2567, sharing the same host-mineral
                // indicatrix) applied to this entry's own birefringence_delta:
                // 0.2567 * 0.0090 = 0.002310.
                biaxial_delta_beta_alpha: Some(0.002_310),
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }

    /// Continuation of [`Self::built_in_materials_aquamarine_through_citrine`] (split
    /// purely to keep each part under clippy's function-length lint): Amethyst and
    /// Citrine, both quartz.
    fn built_in_materials_amethyst_through_citrine() -> Vec<Self> {
        vec![
            // Amethyst (alpha-Quartz, SiO2, colour centre) -- physically the same
            // SiO2 crystal as the colourless "Quartz" entry above: reuses that
            // entry's exact Ghosh 1999 o-ray Sellmeier3 fit and its e-ray
            // `uniaxial_extraordinary_dispersion` and `birefringence_delta`
            // bit-for-bit, differing only in absorption. n_d = 1.54421, Delta n(F-C)
            // = 0.00781 (quartz's well-known "0.013" figure is the B-G interval, not
            // F-C -- see the Quartz entry above).
            Self {
                name: "Amethyst".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // Chromophore: an irradiation-induced Fe-related colour centre (not a
                // simple Fe3+/Fe4+ d-d transition -- broadly analogous to blue
                // topaz's colour centre above but a different defect), a broad band
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
                // leaving yellow-orange transmitted, citrine's characteristic colour.
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
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
            },
        ]
    }

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
    fn built_in_materials_garnets_pyrope_through_spessartine() -> Vec<Self> {
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
                // responsible for pyrope's deep red colour. Widths (35nm/45nm) and
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

    /// Continuation of [`Self::built_in_materials_garnets_pyrope_through_spessartine`]
    /// (split purely to keep each part under clippy's function-length lint):
    /// Grossular (Tsavorite) and Andradite (Demantoid). See that function's doc
    /// comment for the shared garnet-group dispersion-fit convention (2-parameter
    /// Cauchy, B-G->F-C converted).
    fn built_in_materials_garnets_grossular_and_andradite() -> Vec<Self> {
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
                // Chromophore: V3+/Cr3+ (the same chromophore family as Emerald
                // above, in a garnet host) absorbing violet-blue (430nm) and
                // red-orange (610nm), leaving tsavorite's vivid green transmitted --
                // the same two-window mechanism as Emerald's own entry above. Widths
                // (30nm/40nm) and peaks (1.6/1.8) TUNED for a strong green; band
                // CENTRES cited.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(430.0, 30.0, 1.6),
                    AbsorptionBand::new(610.0, 40.0, 1.8),
                ]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
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
                // Chromophore: Cr3+ 440/620nm pair, giving demantoid its vivid
                // saturated green -- the strongest of the five garnets modelled here.
                // Widths (30nm/45nm) and peaks (1.8/2.0) TUNED; band CENTRES cited.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(440.0, 30.0, 1.8),
                    AbsorptionBand::new(620.0, 45.0, 2.0),
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

    /// Third quarter: Peridot (the one biaxial addition in this quarter), YAG, GGG
    /// and Benitoite.
    fn built_in_materials_peridot_through_benitoite() -> Vec<Self> {
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
            // YAG (Yttrium Aluminium Garnet, Y3Al5O12, undoped/colourless laser host)
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
                // Colourless (undoped YAG, the laser-host reference composition):
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
            // GGG (Gadolinium Gallium Garnet, Gd3Ga5O12, colourless synthetic --
            // historically used as a diamond simulant before Cubic Zirconia).
            //
            // No primary Sellmeier fit for GGG is transcribed here; this is a
            // LOWER-CONFIDENCE 2-parameter Cauchy fit solved directly from n_d=1.970
            // and Delta n(F-C)=0.045 (A=1.970-0.023556/0.5893^2=1.902186,
            // B=0.023556 -- unlike the natural gemstone entries above, this figure is
            // not converted via the B-G->F-C ratio, since a synthetic laser-crystal
            // figure like this one is more likely to already be a true F-C/Abbe
            // figure, as YAG's and the two glasses' are). n_d's general magnitude
            // (~1.97-2.0) is corroborated by D.L. Wood & K. Nassau, "Optical
            // properties of gadolinium gallium garnet," Appl. Opt. 29, 3704-3707
            // (1990) -- the same author pair cited for this file's Cubic Zirconia
            // entry -- though this entry's Cauchy coefficients are not transcribed
            // from that paper. Flagged for human cross-check against a primary
            // Sellmeier fit if one becomes available.
            Self {
                name: "GGG".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.902_186,
                    b: 0.023_556,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Colourless (undoped GGG): empty band set, zero absorption at
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
                // sapphire-blue o-ray (E-perp-c) against a near-colourless e-ray
                // (E-parallel-c), the standard gemological description of this
                // species (Ti/Fe-related blue chromophore spanning roughly
                // 380-500nm). Modelled as one broad band (width 60nm, centred 440nm
                // to span that range) present strongly in the o-ray and only weakly
                // in the e-ray -- peaks (2.0 vs 0.4, a 5x dichroic ratio) TUNED for a
                // clearly, strongly dichroic stone; the qualitative "blue one
                // direction, near-colourless the other" pattern and the 380-500nm
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

    /// Fourth and final quarter: Andalusite (the third biaxial
    /// addition), Opal, and the two Schott optical glasses.
    fn built_in_materials_andalusite_through_glass() -> Vec<Self> {
        vec![
            // Andalusite (Al2SiO5): n_d=1.634, B-G 0.016 -> Delta n(F-C) = 0.00926,
            // A=1.634-0.004849/0.5893^2=1.620041, B=0.004849. Abbe V_d~68.45. No
            // primary Sellmeier fit is available; LOWER CONFIDENCE
            // 2-parameter Cauchy fallback. `birefringence_delta` stored as the
            // POSITIVE n_gamma-n_alpha magnitude (this field's own doc comment: "or
            // max-min") -- `BiaxialPositive`/`BiaxialNegative` is a genuinely separate
            // property (which principal axis the acute bisectrix sits closer to, NOT
            // the sign of the total birefringence, which is never negative by the
            // alpha<=beta<=gamma sort `biaxial_indicatrix` assumes).
            Self {
                name: "Andalusite".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.620_041,
                    b: 0.004_849,
                    c: 0.0,
                },
                birefringence_delta: 0.010,
                // PLEOCHROISM: andalusite is a classic TRICHROIC teaching example
                // (Fe/Mn-related), commonly described as red/reddish-brown along one
                // axis, yellow-green along a second, and olive/yellow-brown along the
                // third. No primary per-axis spectroscopic dataset is available
                // (unlike Alexandrite/Tanzanite's figure-read primary data
                // above) -- this is a QUALITATIVE, tuned trichroic pattern built from
                // the well-known pleochroic COLOUR description alone, the same
                // "known colours -> qualitative band pattern" approach Tanzanite's own
                // entry above used before its own primary source was found, flagged
                // here as LOWER CONFIDENCE than a figure-read or numeric source.
                absorption: AbsorptionTensor::biaxial(
                    vec![AbsorptionBand::new(500.0, 40.0, 1.0)], // alpha: red-brown
                    vec![AbsorptionBand::new(480.0, 40.0, 1.8)], // beta: yellow-green
                    vec![AbsorptionBand::new(580.0, 40.0, 1.4)], // gamma: olive/yellow-brown
                ),
                c_axis: Vec3::Y,
                // No primary 3-index andalusite measurement was available in this
                // pass. Commonly tabulated ranges (e.g. Mindat/Gemology Project)
                // place n_alpha~1.629-1.640, n_beta~1.633-1.644, n_gamma~1.638-1.650;
                // midpoints (1.6345, 1.6385, 1.644) give fraction
                // (1.6385-1.6345)/(1.644-1.6345) = 0.421 -- BUT andalusite's negative
                // optic sign means the acute bisectrix sits toward alpha (beta closer
                // to gamma), the opposite lean from the three existing (all positive-
                // sign) biaxial built-ins above, so this uses 1-0.421 = 0.579 (fraction
                // of birefringence_delta beta sits ABOVE alpha) applied here as
                // delta_beta_alpha = 0.579 * 0.010 = 0.00579, rounded to 0.0058.
                biaxial_delta_beta_alpha: Some(0.0058),
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Opal (amorphous hydrated SiO2, common/body-colour opal -- NOT precious
            // opal's structural play-of-colour, which this Gaussian-band absorption
            // model has no mechanism to represent at all: that effect is diffraction
            // off an ordered silica-sphere lattice, not a wavelength-selective
            // absorption coefficient, and is explicitly OUT OF SCOPE for this entry.
            // `crystal_system: Cubic` is a placeholder here,
            // not a mineralogical claim -- opal is a mineraloid with no true crystal
            // structure at all, and `CrystalSystem` has no "amorphous" variant; Cubic
            // is chosen only because it is this enum's existing isotropic-only
            // variant, matching this entry's `Isotropic`/`birefringence_delta: 0.0`
            // (a real, correct claim: amorphous silica has no birefringence).
            //
            // Dispersion: `n_d=1.45` is the standard reference value for common
            // opal's essentially glass-like silica; "Disp" chosen as a small but
            // non-zero Delta n(F-C) = 0.001 (not a cited figure -- deliberately a
            // small hand-picked value, "~0.0, use a tiny
            // positive value," since opal's classical dispersion is minor and
            // visually dominated by the unmodelled structural play-of-colour anyway).
            // A=1.45-0.000523/0.5893^2=1.448493, B=0.000523.
            Self {
                name: "Opal".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.448_493,
                    b: 0.000_523,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Body colour only, and even that varies far too widely (white,
                // black, fire/orange body opal) for one representative band set --
                // left colourless (empty band set) as the neutral reference; a
                // specific body-colour variant (e.g. fire opal's Fe3+ tint) is a
                // candidate follow-up built-in, same convention as un-added Topaz/
                // Tanzanite variants above. Play-of-colour is NOT modelled -- see this
                // entry's own comment above.
                absorption: AbsorptionTensor::isotropic(vec![]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Glass (Schott N-BK7) -- the most common optical crown glass, included
            // as a colourless isotropic reference/calibration material (e.g. for
            // comparing a gemstone's fire against ordinary glass).
            //
            // Source: SCHOTT optical glass data sheet (N-BK7), Sellmeier coefficients
            // as tabulated by refractiveindex.info ("SCHOTT-optical: N-BK7"):
            //   n^2-1 = 1.03961212*l^2/(l^2-0.00600069867) + 0.231792344*l^2/(l^2-0.0200179144)
            //           + 1.01046945*l^2/(l^2-103.560653)
            // Independently verified by hand computation: n_d = 1.51672
            // (catalogue nd = 1.5168, matching to <0.0001 -- the small residual is the
            // catalogue's helium d-line 587.56nm vs this crate's sodium-D convention
            // 589.3nm), Delta n(F-C) = 0.00808 (catalogue-derived 0.0081), Abbe
            // V_d = 63.98 (catalogue Vd = 64.17) -- all within the 0.002
            // tight tolerance.
            Self {
                name: "Glass (N-BK7)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.039_612_2, 0.231_792_35, 1.010_469_4],
                    c: [0.006_000_699, 0.020_017_914, 103.560_65],
                },
                birefringence_delta: 0.0,
                // Colourless glass: empty band set, zero absorption at every
                // wavelength.
                absorption: AbsorptionTensor::isotropic(vec![]),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                uniaxial_extraordinary_dispersion: None,
            },
            // Glass (Schott F2) -- a common dense flint glass, the classic
            // high-dispersion counterpart to N-BK7's crown glass above (an
            // achromatic-doublet pairing in real optics), included for the same
            // colourless-reference reason.
            //
            // Source: SCHOTT optical glass data sheet (F2), Sellmeier coefficients as
            // tabulated by refractiveindex.info ("SCHOTT-optical: F2"):
            //   n^2-1 = 1.34533359*l^2/(l^2-0.00997743871) + 0.209073176*l^2/(l^2-0.0470450767)
            //           + 0.937357162*l^2/(l^2-111.886764)
            // Independently verified by hand computation: n_d = 1.61980
            // (catalogue nd = 1.62004, matching to <0.0003 -- same helium-d/sodium-D
            // line offset as N-BK7 above), Delta n(F-C) = 0.01705 (catalogue-derived
            // 0.0170), matching the catalogue Vd = 36.37 directly.
            Self {
                name: "Glass (F2)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.345_333_6, 0.209_073_17, 0.937_357_2],
                    c: [0.009_977_439, 0.047_045_077, 111.886_76],
                },
                birefringence_delta: 0.0,
                // Colourless glass: empty band set, zero absorption at every
                // wavelength.
                absorption: AbsorptionTensor::isotropic(vec![]),
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

    /// Rutile (`TiO2`): a strongly uniaxial species, at a birefringence magnitude
    /// (`+0.287`) higher than any other built-in material.
    ///
    /// Previously deliberately excluded (see
    /// `built_in_materials_aquamarine_through_citrine`'s own "Deliberately NOT added:
    /// Sphene, Rutile" doc comment) because it needed the full anisotropic Fresnel
    /// treatment `optics::raytracer::uniaxial_fresnel` provides; added once that
    /// existed.
    ///
    /// Runs the EXACT closed-form uniaxial Fresnel solve on both CPU
    /// (`optics::raytracer::uniaxial_fresnel`) and GPU (`shaders/transport_physics.wgsl`'s
    /// own port of it, wired into `shaders/spectral_transport.wgsl`'s megakernel
    /// dispatch). Used as the Tier 3 image-comparison material for GPU-parity
    /// verification (`renderer::gpu::estimator_check::rutile_material`): its extreme
    /// birefringence makes it the built-in most likely to expose CPU/GPU divergence in
    /// the closed-form solve -- it did, catching the cross-platform branch-cut bug
    /// documented in `uniaxial_fresnel::Cplx::sqrt_forward_branch`'s own doc comment.
    ///
    /// Indices and dispersion: `DeVore`, J. Opt. Soc. Am. 41, 416 (1951) -- the standard
    /// reference measurement for rutile's principal indices and dispersion, as commonly
    /// tabulated (e.g. refractiveindex.info "`TiO2` (Titanium dioxide): Rutile phase").
    /// `n_o(589.3nm) = 2.616`, `n_e(589.3nm) = 2.903` (this entry's own
    /// `birefringence_delta = 0.287` matches this exactly). This file's Cauchy model
    /// (see `DispersionModel::Cauchy`'s own doc comment for why every non-primary-
    /// Sellmeier entry here uses a 2-parameter visible-range fit rather than `DeVore`'s
    /// own multi-pole Sellmeier form, which this crate's `DispersionModel` enum has no
    /// variant for) is fit independently per ray to `DeVore`'s own tabulated `n_d` AND
    /// `Delta n(F-C)` figures: `a = n_d - b/0.5893^2`, `b` solved from
    /// `Delta n(F-C) = b*(1/0.4861^2 - 1/0.6563^2)`. o-ray: `Delta n(F-C) ~ 0.300`
    /// (rutile's extreme dispersion, Abbe number `V_d` ~ 8.5, an order of magnitude more
    /// dispersive than diamond) gives `b_o = 0.1572`, `a_o = 2.1634`. e-ray: `Delta
    /// n(F-C) ~ 0.31` (the e-ray disperses slightly more strongly than the o-ray in the
    /// real material) gives `b_e = 0.1624`, `a_e = 2.4354`. Verified: `n_o(D) = 2.6160`,
    /// `n_e(D) = 2.9030` (both exact by construction).
    ///
    /// Near-UV absorption edge (~430nm): rutile's real absorption edge is a steep
    /// semiconductor band edge, not a
    /// molecular-transition Gaussian -- modelled here (this file's existing convention
    /// for every non-measured band, see e.g. Amethyst's colour-centre band above) as a
    /// single strong band centred at 350nm wide enough to tail audibly into the violet
    /// by ~430nm, giving rutile's characteristic yellow-to-brown body colour. TUNED
    /// (aesthetic, not a cited absorption coefficient), like every other inclusion/
    /// pleochroism band in this file that isn't a directly measured spectrum.
    fn built_in_material_rutile() -> Self {
        Self {
            name: "Rutile".to_string(),
            crystal_system: CrystalSystem::Tetragonal,
            optical_character: OpticalCharacter::UniaxialPositive,
            dispersion: DispersionModel::Cauchy {
                a: 2.1634,
                b: 0.1572,
                c: 0.0,
            },
            birefringence_delta: 0.287,
            absorption: AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::new(350.0, 40.0, 8.0)],
                vec![AbsorptionBand::new(350.0, 40.0, 8.0)],
            ),
            c_axis: Vec3::Y,
            biaxial_delta_beta_alpha: None,
            scattering_sigma_s: 0.0,
            scattering_g: 0.0,
            edge_rounding_radius: 0.0,
            absorption_path_scale: 1.0,
            uniaxial_extraordinary_dispersion: Some(DispersionModel::Cauchy {
                a: 2.4354,
                b: 0.1624,
                c: 0.0,
            }),
        }
    }

    /// Creates a custom gemstone material with specified physical optical properties.
    ///
    /// # `dispersion_delta` convention: Fraunhofer F-C, not B-G
    ///
    /// `dispersion_delta` is interpreted as the Fraunhofer **F-C** interval,
    /// `n(486.1nm) - n(656.3nm)`, matching every built-in material in
    /// [`Self::all_materials`] (whose Cauchy/Sellmeier fits were, per their own sourcing
    /// comments, deliberately normalised to F-C -- gemological tables usually publish
    /// the wider Fraunhofer **B-G** interval, `n(430.8nm) - n(686.7nm)`, instead, and
    /// every one of those comments records converting B-G -> F-C before fitting).
    /// `new_custom` picks the same convention so a caller mixing a custom material into
    /// a scene with built-ins gets a consistent, comparable dispersion figure -- a
    /// caller with a genuine B-G figure in hand should convert it first (multiply by
    /// the `k_bg` ratio derived below, or just use the physics: F-C and B-G differ by
    /// roughly a factor of 1.71 for typical gemstone Cauchy curves, per the worked
    /// conversions throughout `all_materials`' sourcing comments).
    ///
    /// Solved in closed form from the single-term Cauchy fit this constructor always
    /// builds (`n(lambda) = a + b / lambda_um^2`, `c = 0`): the F-C delta this produces
    /// is `b` times a fixed geometric factor (the difference of `1 / lambda^2` at F and
    /// C), so dividing the requested delta by that SAME factor for `b` reproduces the
    /// requested F-C delta exactly (up to floating-point rounding) rather than only
    /// approximately. Computed here from the wavelengths directly (not a hand-rounded
    /// literal) so the reciprocal-square arithmetic is exact regardless of how many
    /// digits get typed into a comment; evaluates to a factor of ~0.5235. A flat
    /// `0.347` multiplier, by contrast, has no derivation behind it and measurably
    /// under-delivers the requested F-C delta (only ~66% of it) while over-delivering
    /// a B-G-interpreted delta (~113% of it) -- i.e. it matches neither convention.
    ///
    /// See this module's own test
    /// `new_custom_dispersion_delta_measures_exactly_at_f_and_c` for the regression
    /// pinning this.
    #[must_use]
    pub fn new_custom(
        name: &str,
        mean_ri: f32,
        dispersion_delta: f32,
        birefringence_delta: f32,
        absorption_rgb: [f32; 3],
    ) -> Self {
        // Cauchy dispersion model fit: n(lambda) = A + B / lambda^2
        // where lambda is in um, lambda_D = 0.5893 um (sodium D line).
        const LAMBDA_D_UM: f32 = 0.5893;
        // Fraunhofer F (486.1nm) and C (656.3nm) lines, matching every other F-C
        // measurement in this crate (see `color::metrics::evaluate_gem_optical_metrics`
        // and `all_materials`' own verification comments, which evaluate at these exact
        // two wavelengths).
        const LAMBDA_F_UM: f32 = 0.4861;
        const LAMBDA_C_UM: f32 = 0.6563;
        let lambda_d_sq = LAMBDA_D_UM * LAMBDA_D_UM;
        let k_fc = 1.0 / (1.0 / (LAMBDA_F_UM * LAMBDA_F_UM) - 1.0 / (LAMBDA_C_UM * LAMBDA_C_UM));
        let b = (dispersion_delta * k_fc).max(0.0);
        let a = (mean_ri - b / lambda_d_sq).max(1.0);

        Self {
            name: name.to_string(),
            crystal_system: if birefringence_delta.abs() > 1e-4 {
                CrystalSystem::Trigonal
            } else {
                CrystalSystem::Cubic
            },
            optical_character: if birefringence_delta > 1e-4 {
                OpticalCharacter::UniaxialPositive
            } else if birefringence_delta < -1e-4 {
                OpticalCharacter::UniaxialNegative
            } else {
                OpticalCharacter::Isotropic
            },
            dispersion: DispersionModel::Cauchy { a, b, c: 0.0 },
            birefringence_delta,
            // `new_custom`'s public signature keeps accepting a plain
            // `[R, G, B]` triple (callers, including existing tests, pass one) --
            // internally converted to the band-set representation via
            // `legacy_rgb_bands` (see that function's doc comment for the three-lobe
            // shape it produces).
            absorption: AbsorptionTensor::isotropic(legacy_rgb_bands(absorption_rgb)),
            // No caller currently supplies a c-axis for a custom material, so
            // default to Vec3::Y, keeping `new_custom` behaviour unchanged rather
            // than adding a new parameter every call site would need to be updated
            // for.
            c_axis: Vec3::Y,
            // No caller currently supplies biaxial principal-index data
            // for a custom material -- `new_custom` remains a uniaxial/isotropic
            // constructor (see `optical_character`/`crystal_system` above, which never
            // produce Biaxial* for this constructor either).
            biaxial_delta_beta_alpha: None,
            scattering_sigma_s: 0.0,
            scattering_g: 0.0,
            edge_rounding_radius: 0.0,
            absorption_path_scale: 1.0,
            uniaxial_extraordinary_dispersion: None,
        }
    }

    /// Inclusion/subsurface scattering: opts an existing material into a
    /// homogeneous Henyey-Greenstein scattering medium (silk/rutile/cloud inclusions),
    /// leaving every other field -- crucially including `absorption` -- untouched. A
    /// pure builder-style setter (`GemMaterial::ruby().with_scattering(0.4, 0.3)`),
    /// added so scenes/tests can opt individual materials in without a breaking change
    /// to `new_custom`'s or any built-in constructor's signature. See
    /// `scattering_sigma_s`/`scattering_g`'s own doc comments for what each parameter
    /// means; `sigma_s <= 0.0` (the default every built-in keeps) disables the feature
    /// entirely, reproducing today's exact deterministic Beer-Lambert path.
    #[must_use]
    pub const fn with_scattering(mut self, sigma_s: f32, g: f32) -> Self {
        self.scattering_sigma_s = sigma_s;
        self.scattering_g = g;
        self
    }

    /// A sensible default Henyey-Greenstein asymmetry for a caller who only wants to
    /// dial the AMOUNT of scattering ([`Self::with_scattering_amount`]) without thinking
    /// about anisotropy separately: mild forward scattering, physically typical for
    /// small needle-like/particulate inclusions (silk, rutile) at visible wavelengths --
    /// not a measured value for any specific species, just a reasonable "character"
    /// default.
    pub const DEFAULT_SCATTERING_G: f32 = 0.4;

    /// [`Self::with_scattering`] with [`Self::DEFAULT_SCATTERING_G`] for `g`, for a
    /// caller who wants a single "how much inclusion haze" knob. Two independent
    /// physical parameters genuinely exist here -- `sigma_s` (amount) and `g`
    /// (character: forward-scattering silk reads very differently from near-isotropic
    /// cloud) -- so this is a convenience on top of [`Self::with_scattering`], not a
    /// replacement for it; a caller who cares about the distinction should call
    /// `with_scattering` directly with an explicit `g`.
    #[must_use]
    pub const fn with_scattering_amount(self, sigma_s: f32) -> Self {
        self.with_scattering(sigma_s, Self::DEFAULT_SCATTERING_G)
    }

    /// A plausible per-species `(sigma_s, g)` starting point for "this species is
    /// typically included/hazy" -- e.g. Emerald's proverbial *jardin* -- keyed by this
    /// material's own `name`.
    ///
    /// # These are aesthetic choices, not measurements
    ///
    /// Unlike this material's Sellmeier dispersion coefficients or its pleochroic
    /// absorption bands (both cited to specific spectroscopic sources in
    /// [`Self::all_materials`]), there is no published "typical `sigma_s`" for any gem
    /// species -- inclusion density varies enormously by individual specimen, locality,
    /// and treatment, and clarity is conventionally assessed by eye/loupe grading, not a
    /// volumetric scattering coefficient. These numbers were chosen to LOOK plausible
    /// (documented per species below in descriptive terms -- "typically included",
    /// "usually eye-clean" -- deliberately NOT any standardized clarity-grade vocabulary
    /// like GIA's VVS/VS/SI scale, which grades visible-inclusion appearance under 10x
    /// magnification, a different thing this coefficient does not claim to reproduce),
    /// not derived from any citable source. Every built-in material's OWN
    /// `scattering_sigma_s` still stays exactly `0.0` (see that field's doc comment) --
    /// this method has no effect until a caller explicitly opts in via
    /// `material.with_recommended_scattering()`.
    #[must_use]
    pub fn recommended_scattering(&self) -> (f32, f32) {
        Self::recommended_scattering_arm(&self.name).unwrap_or((0.0, Self::DEFAULT_SCATTERING_G))
    }

    /// The explicit per-species arm [`Self::recommended_scattering`] delegates to, or
    /// `None` for any name with no dedicated arm (that method then falls back to
    /// `(0.0, Self::DEFAULT_SCATTERING_G)`).
    ///
    /// Pulled out as its own `Option`-returning function, rather than inlining this
    /// `match` directly in [`Self::recommended_scattering`] with a `_ =>` catch-all, so
    /// `tests::every_builtin_has_an_explicit_scattering_arm` can distinguish "this name
    /// has a real entry" from "this name silently fell through to the default," which a
    /// catch-all's return value alone cannot do (a species genuinely tuned to the same
    /// numbers as the default would look identical to one that was simply forgotten).
    /// A `_ =>` catch-all is also fragile to name drift: matching the literal
    /// `"Moissanite"` instead of the built-in material's actual name,
    /// `"Synthetic Moissanite"` (see `Self::all_materials`), would let the intended
    /// `(0.0, 0.0)` arm silently never fire, so every Moissanite render would use the
    /// generic default instead -- hence the explicit-arm-plus-test structure to catch
    /// this class of bug.
    fn recommended_scattering_arm(name: &str) -> Option<(f32, f32)> {
        match name {
            // Typically visibly included (the proverbial "jardin", French for garden --
            // multi-phase fluid inclusions and growth-tube silk are part of how an
            // untreated natural emerald is expected to look, not a flaw to hide).
            "Emerald" => Some((0.6, 0.35)),
            // Silk (fine rutile needles) is common in natural corundum; heat treatment
            // (the overwhelming majority of the commercial supply) dissolves much of it,
            // so this is a moderate, not extreme, default.
            "Ruby" => Some((0.3, 0.5)),
            "Sapphire" => Some((0.25, 0.5)),
            // Natural rutilated quartz is famous for coarse, strongly forward-scattering
            // needles; ordinary rock crystal is usually clean. This default sits toward
            // the light-haze end since "Quartz" here is the generic rock-crystal entry.
            // Peridot shares this same light-haze tier: it commonly shows "lily pad"
            // (chromite + stress-fracture) inclusions.
            "Quartz" | "Peridot" => Some((0.15, 0.3)),
            // Elbaite tourmaline commonly carries visible needle/fingerprint inclusions.
            "Tourmaline" => Some((0.25, 0.4)),
            // Faceted gem-quality diamond is typically eye-clean; a small isotropic
            // haze (cloud inclusions) rather than a strong directional character.
            "Diamond" => Some((0.02, 0.0)),
            // Typically eye-clean once faceted (heat-treated zircon in particular).
            // Ordinary (non-colour-change) chrysoberyl, amethyst, citrine, pyrope,
            // spessartine, benitoite and andalusite share this same light-default tier:
            // they are likewise typically eye-clean.
            "Zircon"
            | "Alexandrite"
            | "Topaz"
            | "Spinel"
            | "Tanzanite"
            | "Chrysoberyl (Yellow)"
            | "Amethyst"
            | "Citrine"
            | "Pyrope Garnet"
            | "Spessartine Garnet"
            | "Benitoite"
            | "Andalusite" => Some((0.02, 0.2)),
            // Lab-grown, essentially inclusion-free by construction. "Synthetic
            // Moissanite" -- see this function's own doc comment -- is
            // [`Self::all_materials`]'s actual built-in name; matching the bare
            // "Moissanite" instead would silently miss it, since that name never
            // appears there. Lab-grown YAG/GGG and Schott catalogue glass share this
            // same "essentially inclusion-free" tier.
            "Synthetic Moissanite"
            | "Cubic Zirconia"
            | "YAG"
            | "GGG"
            | "Glass (N-BK7)"
            | "Glass (F2)" => Some((0.0, 0.0)),
            // Aquamarine/Morganite: typically eye-clean beryl varieties (unlike
            // Emerald's proverbial jardin above -- beryl's clarity expectation varies
            // sharply by variety, not just by species).
            "Aquamarine" => Some((0.1, 0.3)),
            // Rutile joins Morganite in this same light-default tier -- rutile's own
            // name is synonymous with the fine acicular inclusions it forms in OTHER
            // species (see e.g. Ruby/Sapphire's "silk" comment above), but as a species
            // in its own right it is typically faceted from clean synthetic boules.
            "Morganite" | "Rutile" => Some((0.05, 0.2)),
            // Almandine commonly carries needle/rutile inclusions.
            "Almandine Garnet" => Some((0.15, 0.4)),
            // Tsavorite is frequently included (fine growth-tube inclusions,
            // moderately forward-scattering).
            "Grossular Garnet (Tsavorite)" => Some((0.2, 0.3)),
            // Demantoid's "horsetail" byssolite inclusions are a famous, often
            // deliberately showcased identification feature -- strongly
            // forward-scattering, not a flaw to hide (the same "characteristic
            // inclusion, not a defect" reasoning as Emerald's jardin above).
            "Andradite Garnet (Demantoid)" => Some((0.35, 0.6)),
            // Common (body-colour) opal: some visible internal haze/crazing is
            // typical, though this entry deliberately does not model
            // play-of-colour -- see this entry's own comment in
            // `built_in_materials_andalusite_through_glass`.
            "Opal" => Some((0.1, 0.2)),
            _ => None,
        }
    }

    /// [`Self::with_scattering`] using [`Self::recommended_scattering`]'s per-species
    /// `(sigma_s, g)` pair -- see that method's doc comment for why these are aesthetic
    /// defaults, not measurements. `GemMaterial::emerald().with_recommended_scattering()`
    /// reads at the call site the way a caller who wants "this species' typical included
    /// look" would expect.
    #[must_use]
    pub fn with_recommended_scattering(self) -> Self {
        let (sigma_s, g) = self.recommended_scattering();
        self.with_scattering(sigma_s, g)
    }

    /// Facet edge rounding: opts a material into a nonzero
    /// meet-edge rounding radius -- see [`Self::edge_rounding_radius`]'s doc comment for
    /// units/range. `radius <= 0.0` (the default every built-in keeps) reproduces
    /// today's perfectly sharp, measure-zero edges exactly.
    #[must_use]
    pub const fn with_edge_rounding(mut self, radius: f32) -> Self {
        self.edge_rounding_radius = radius;
        self
    }

    /// Model-units-to-absorption-length-units scale: opts a material into a
    /// physical size other than the implicit "girdle radius ~1 model unit" every
    /// built-in cut renders at -- see [`Self::absorption_path_scale`]'s doc comment.
    /// `scale == 1.0` (the default every built-in keeps) reproduces today's
    /// behaviour exactly (a multiply by exactly `1.0` is an IEEE 754 no-op).
    #[must_use]
    pub const fn with_absorption_path_scale(mut self, scale: f32) -> Self {
        self.absorption_path_scale = scale;
        self
    }

    /// Looks a material up by name, tolerating extra surrounding words in the query
    /// (e.g. a diagram title like "Fine Blue Sapphire").
    ///
    /// An exact match always wins outright. The substring fallback then prefers the
    /// LONGEST matching material name: "Zircon" is a substring of "Cubic Zirconia" and
    /// is listed earlier, so a naive first-match search silently returned Zircon —
    /// a completely different stone (`n_d` 1.92 vs 2.15) — for `by_name("Cubic Zirconia")`.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        let all = Self::all_materials();
        if let Some(m) = all.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
            return Some(m.clone());
        }
        let needle = name.to_lowercase();
        all.into_iter()
            .filter(|m| needle.contains(&m.name.to_lowercase()))
            .max_by_key(|m| m.name.len())
    }

    /// Convenience accessor for the built-in Diamond material.
    ///
    /// # Panics
    ///
    /// Panics if `"Diamond"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- this is an internal consistency
    /// invariant of this module (every name used by a convenience accessor below must
    /// have a matching entry there), not something a caller can trigger.
    #[must_use]
    pub fn diamond() -> Self {
        Self::by_name("Diamond").expect("\"Diamond\" must be present in all_materials()")
    }

    /// Convenience accessor for the built-in Ruby material.
    ///
    /// # Panics
    ///
    /// Panics if `"Ruby"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- see [`diamond`](Self::diamond) for why.
    #[must_use]
    pub fn ruby() -> Self {
        Self::by_name("Ruby").expect("\"Ruby\" must be present in all_materials()")
    }

    /// Convenience accessor for the built-in Sapphire material.
    ///
    /// # Panics
    ///
    /// Panics if `"Sapphire"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- see [`diamond`](Self::diamond) for why.
    #[must_use]
    pub fn sapphire() -> Self {
        Self::by_name("Sapphire").expect("\"Sapphire\" must be present in all_materials()")
    }

    /// Convenience accessor for the built-in Emerald material.
    ///
    /// # Panics
    ///
    /// Panics if `"Emerald"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- see [`diamond`](Self::diamond) for why.
    #[must_use]
    pub fn emerald() -> Self {
        Self::by_name("Emerald").expect("\"Emerald\" must be present in all_materials()")
    }

    /// Builds this material's full [`BiaxialIndicatrix`] (three principal
    /// indices `n_alpha` <= `n_beta` <= `n_gamma` plus their orthonormal axis frame) at
    /// `lambda_nm`, for the three biaxial (orthorhombic) built-ins. `None` for every
    /// other material -- see `biaxial_delta_beta_alpha`'s doc comment for the field
    /// this reads and the convention it documents (base `dispersion` curve == `n_beta`,
    /// `birefringence_delta` == `n_gamma` - `n_alpha`, `c_axis` doubles as the `n_gamma`
    /// principal axis).
    #[must_use]
    pub fn biaxial_indicatrix(&self, lambda_nm: f32) -> Option<BiaxialIndicatrix> {
        let delta_beta_alpha = self.biaxial_delta_beta_alpha?;
        let n_beta = self.dispersion.evaluate(lambda_nm);
        let n_alpha = n_beta - delta_beta_alpha;
        let n_gamma = n_alpha + self.birefringence_delta;
        Some(BiaxialIndicatrix::from_gamma_axis(
            n_alpha,
            n_beta,
            n_gamma,
            self.c_axis,
        ))
    }

    /// This uniaxial material's extraordinary-ray index at `lambda_nm`, given the
    /// already-evaluated ordinary index `n_o` at that SAME wavelength (every caller
    /// already has `n_o` in hand from `self.dispersion.evaluate(lambda_nm)`, so this
    /// takes it rather than recomputing it).
    ///
    /// Uses [`Self::uniaxial_extraordinary_dispersion`] (a genuine independent e-ray
    /// curve) when present, falling back to the constant-offset `n_o +
    /// birefringence_delta` approximation every material used before that field
    /// existed -- see that field's own doc comment for which built-ins (Quartz,
    /// Amethyst, Citrine) carry a real curve, and for the GPU port
    /// (`shaders/spectral_transport.wgsl`'s own `extraordinary_index_at`) mirroring
    /// this exactly.
    #[must_use]
    pub fn extraordinary_index_at(&self, lambda_nm: f32, n_o: f32) -> f32 {
        self.uniaxial_extraordinary_dispersion
            .map_or(n_o + self.birefringence_delta, |e_dispersion| {
                e_dispersion.evaluate(lambda_nm)
            })
    }

    /// GPU routing predicate: whether this material's optics are supported by the GPU
    /// backend (`renderer::shaders::spectral_transport.wgsl`'s `transport_main`).
    ///
    /// A pure function of the material alone. Isotropic and uniaxial birefringent
    /// materials (the `theta_c` fixed-point iteration, the 50/50 ordinary/extraordinary
    /// eigenmode split, `extraordinary_poynting_dir` walk-off) have always been ported
    /// and GPU-capable. Genuinely biaxial materials (`biaxial_delta_beta_alpha.is_some()`
    /// -- Alexandrite, Topaz, Tanzanite among the built-ins) are supported too, now that
    /// `BiaxialIndicatrix::eigenvector_world` (the symmetric "transverse impermeability"
    /// matrix `Gamma = P.B.P`, its null vector extracted via a smooth, sign-aligned sum
    /// of row-pair cross products rather than a hard largest-of-three selection) and
    /// `wave_indices`'s discriminant (`BiaxialIndicatrix::precise_root_near`: an
    /// algebraically-exact reformulation replacing the `B^2-4C` cancellation with
    /// Sterbenz-exact differences of the principal `1/n^2` values) are both
    /// well-conditioned near mode degeneracy -- see both functions' doc comments in
    /// `optics::birefringence` for the full derivations, and
    /// `docs/history/indicatrix-core.md` for the CPU/GPU divergence the earlier
    /// formulation measured and the reverification after the fix.
    ///
    /// Deliberately checks only material data, never anything about the machine running
    /// the render (GPU availability, VRAM, load): the whole point of a routing predicate
    /// living in this crate's hashed source (`lib::BUILD_ID` covers `src/**/*.rs`) is
    /// that a viewer and a remote render worker -- each independently deciding whether to
    /// hand a given scene to the GPU backend -- can never disagree about what runs where,
    /// which requires the decision to depend on nothing but the scene (here: the
    /// material) itself. A caller that assembles a full scene (material + geometry +
    /// environment) should call this once per distinct material the scene uses and
    /// require all of them to return `true` before routing that scene's render to the
    /// GPU backend.
    #[must_use]
    pub const fn gpu_supported(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Zircon" is a substring of "Cubic Zirconia" and is listed earlier in
    /// `all_materials()`, so a first-match search silently returned the wrong stone.
    #[test]
    fn by_name_prefers_the_longest_match_not_the_first() {
        let cz = GemMaterial::by_name("Cubic Zirconia").expect("Cubic Zirconia must resolve");
        assert_eq!(cz.name, "Cubic Zirconia", "must not fall through to Zircon");

        let zircon = GemMaterial::by_name("Zircon").expect("Zircon must resolve");
        assert_eq!(
            zircon.name, "Zircon",
            "an exact name must still resolve to itself"
        );

        assert!(
            (cz.dispersion.evaluate(589.3) - zircon.dispersion.evaluate(589.3)).abs() > 0.1,
            "test premise: the two stones must have clearly different refractive indices"
        );
    }

    /// Every built-in material must resolve to itself by its own exact name.
    #[test]
    fn by_name_round_trips_every_builtin_material() {
        for m in GemMaterial::all_materials() {
            let found = GemMaterial::by_name(&m.name)
                .unwrap_or_else(|| panic!("{} must resolve by its own name", m.name));
            assert_eq!(
                found.name, m.name,
                "{} resolved to the wrong material",
                m.name
            );
        }
    }

    /// Every built-in material must have its OWN explicit `recommended_scattering` arm,
    /// keyed by its exact `all_materials()` name -- falling through to the generic
    /// default silently, as `"Moissanite"` (rather than the real name `"Synthetic
    /// Moissanite"`) used to, is a bug this test now catches for every current AND
    /// future built-in. See
    /// `GemMaterial::recommended_scattering_arm`'s own doc comment for why this needed a
    /// separate `Option`-returning helper to detect at all: a species whose intended
    /// numbers happen to equal the default would look identical to a truly-forgotten one
    /// if this only checked `recommended_scattering`'s return value.
    #[test]
    fn every_builtin_has_an_explicit_scattering_arm() {
        for material in GemMaterial::all_materials() {
            assert!(
                GemMaterial::recommended_scattering_arm(&material.name).is_some(),
                "{} has no explicit recommended_scattering arm -- it silently falls \
                 through to the generic default",
                material.name
            );
        }
    }

    /// The substring fallback is what lets a diagram title carry extra words.
    #[test]
    fn by_name_still_matches_a_name_embedded_in_a_longer_string() {
        let m = GemMaterial::by_name("Fine Blue Sapphire, Ceylon").expect("should match Sapphire");
        assert_eq!(m.name, "Sapphire");
    }

    /// Verifies every built-in material's computed Abbe number
    /// `V_d = (n_d - 1) / (n_F - n_C)` against a trusted-source reference value, so a
    /// future mistyped coefficient (Sellmeier or Cauchy) gets caught instead of
    /// silently shipping -- see each material's own doc comment in `all_materials` for
    /// the primary/reputable source and the derivation of its target Abbe number.
    ///
    /// Source priority for these targets (re-verified against trusted online sources,
    /// which now OUTRANK `GEMSTONE_RENDERING_BLUEPRINT.md` -- the document is used only
    /// as a tiebreaker/corroboration, never as the primary authority, because it is a
    /// research compilation whose own "Fire Delta n(F-C)" column demonstrably mixes
    /// Fraunhofer B-G and F-C conventions row-to-row, e.g. Quartz lists 0.013, the
    /// well-known B-G figure, in a column labelled F-C, contradicting that same row's
    /// own `V_d`):
    ///   1. Primary optical measurements (Sellmeier/Cauchy fits from refractiveindex.info
    ///      and the papers it cites).
    ///   2. Reputable gemological references (GIA, Mindat, Handbook of Mineralogy,
    ///      International Gem Society), corroborated by 2+ independent sources where
    ///      possible, with their "dispersion" figures explicitly treated as Fraunhofer
    ///      B-G (never plugged directly into an F-C slot).
    ///   3. The document, as a tiebreaker only.
    ///
    /// Two confidence tiers, reflected in the tolerance:
    ///   - TIGHT (0.5): Diamond, Sapphire, Ruby, Quartz, Spinel, Cubic Zirconia,
    ///     Alexandrite, Synthetic Moissanite -- dispersion coefficients transcribed
    ///     directly from a primary optical-constants paper (Peter 1923; Malitson &
    ///     Dodge 1972; Ghosh 1999; Tropf & Thomas 1991; Wood & Nassau 1982; Walling
    ///     1980; Wang 2013 corroborated by Singh 1971), target computed from that exact
    ///     published formula. Alexandrite and Moissanite CONTRADICT the document here
    ///     (68.0 -> 73.90 and 21.5 -> 25.94 respectively) -- per the source priority
    ///     above the primary measurement wins; see each entry's comment in
    ///     `all_materials` for the two-source corroboration.
    ///   - LOWER CONFIDENCE (2.5-4.0, roughly 5-12% relative): Zircon, Topaz,
    ///     Tourmaline, Tanzanite, Emerald -- no primary Sellmeier/Cauchy fit exists in
    ///     the optics literature for these species (confirmed absent from
    ///     refractiveindex.info's database), so `n_d` is taken from 2+ corroborating
    ///     reputable gemological sources and the dispersion shape is a 2-parameter
    ///     Cauchy fit solved from that `n_d` and a Delta n(F-C) obtained by converting
    ///     each species' well-corroborated gemological B-G "dispersion" figure via a
    ///     B-G->F-C ratio (0.579, range 0.569-0.587) computed directly from the 8
    ///     genuine primary Sellmeier curves above -- not from cross-comparing
    ///     gemological tables against each other, which is how the earlier 0.591
    ///     factor was derived. All five CONTRADICT the document (Zircon 28.0 -> 40.96, Topaz 64.0 ->
    ///     76.83, Tourmaline 55.0 -> 64.58, Tanzanite 45.0 -> 40.21, Emerald's estimate
    ///     shifts modestly from 69.99 to 70.93 with the refined ratio). Zircon's `n_d`
    ///     also carried an unrelated, unsourced error, corrected to 1.925 from
    ///     1.956878 (see its entry's comment) -- critical-angle-relevant since `n_d`
    ///     drives it directly.
    #[test]
    fn builtin_material_abbe_numbers_match_published_values() {
        // (name, sourced Abbe number V_d, tolerance)
        let expected: &[(&str, f32, f32)] = &[
            // -- tight tier: primary Sellmeier source for both n_d and V_d --
            ("Diamond", 55.27, 0.5),
            ("Sapphire", 72.27, 0.5),
            ("Ruby", 72.27, 0.5),
            ("Quartz", 69.65, 0.5),
            ("Spinel", 60.63, 0.5),
            ("Cubic Zirconia", 33.52, 0.5),
            ("Alexandrite", 73.90, 0.5),
            ("Synthetic Moissanite", 25.94, 0.5),
            // -- lower-confidence tier: no primary fit exists; n_d from corroborated
            // gemological sources, dispersion shape from a gemological B-G figure
            // converted via the physically-derived 0.579 ratio -- see each entry's
            // comment in `all_materials` --
            ("Zircon", 40.96, 3.0),
            ("Topaz", 76.83, 4.0),
            ("Tourmaline", 64.58, 3.5),
            ("Tanzanite", 40.21, 3.0),
            ("Emerald", 70.93, 3.5),
        ];

        for &(name, published_abbe, tol) in expected {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must be a built-in material"));
            let n_f = material.dispersion.evaluate(486.1);
            let n_d = material.dispersion.evaluate(589.3);
            let n_c = material.dispersion.evaluate(656.3);
            let dn = n_f - n_c;
            assert!(
                dn > 0.0,
                "{name}: Delta n(F-C) must be positive (normal dispersion), got {dn}"
            );
            let computed_abbe = (n_d - 1.0) / dn;
            assert!(
                (computed_abbe - published_abbe).abs() <= tol,
                "{name}: computed Abbe V_d={computed_abbe:.2} (n_d={n_d:.5}, n_F={n_f:.5}, n_C={n_c:.5}) \
                 does not match published V_d={published_abbe:.2} within tolerance {tol} -- check the \
                 dispersion coefficients against the source cited in all_materials()"
            );
        }
    }

    /// Companion to `builtin_material_abbe_numbers_match_published_values`: pins down
    /// `n_d` at the sodium D line (589.3nm) for every re-sourced material below, so a
    /// future coefficient edit that silently shifts `n_d` gets caught. `n_d` drives
    /// the critical angle and therefore brilliance/windowing/extinction, so any
    /// intentional change here is significant downstream and must be deliberate, not
    /// an accident of refitting the dispersion shape.
    ///
    /// For five of the six re-sourced materials (Synthetic Moissanite, Topaz,
    /// Tourmaline, Tanzanite, Alexandrite), `n_d` is unchanged from before -- only the
    /// dispersion shape (`V_d`) moved, per
    /// `builtin_material_abbe_numbers_match_published_values`.
    ///
    /// Zircon is the exception, and the one material in this file whose `n_d`
    /// DID change materially: from 1.956878 to 1.925, a -0.0319 shift, far above
    /// the ~0.002 threshold at which critical-angle-driven behaviour (brilliance,
    /// windowing, extinction) is expected to move. The old value was never actually
    /// sourced from anywhere (not the document, which gives `n_o=1.9250`, nor any
    /// external reference found); 1.925 is corroborated by the Handbook of Mineralogy,
    /// International Gem Society, and the document itself. See Zircon's comment in
    /// `all_materials` for the full derivation. Any test elsewhere in the workspace
    /// that hardcodes zircon's brilliance/windowing/pose behaviour will need
    /// re-stabilising against this new `n_d`.
    #[test]
    fn builtin_material_n_d_matches_sourced_values() {
        // (name, sourced n_d, tolerance)
        let expected_n_d: &[(&str, f32, f32)] = &[
            ("Zircon", 1.925, 1e-4),
            ("Synthetic Moissanite", 2.647_434, 1e-4),
            ("Topaz", 1.627_178, 1e-4),
            ("Tourmaline", 1.639_405, 1e-4),
            ("Tanzanite", 1.700_858, 1e-4),
            ("Alexandrite", 1.742_73, 1e-4),
        ];

        for &(name, expected, tol) in expected_n_d {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must be a built-in material"));
            let n_d = material.dispersion.evaluate(589.3);
            assert!(
                (n_d - expected).abs() < tol,
                "{name}: n_d={n_d:.6} does not match the sourced value {expected:.6} -- check the \
                 dispersion coefficients against the source cited in all_materials(); if this is a \
                 deliberate re-sourcing, update this test's expected value and flag the n_d change \
                 in the task report if it moves by more than ~0.002 (critical-angle significance)"
            );
        }
    }

    /// Only the six orthorhombic (biaxial) built-ins -- Alexandrite, Topaz,
    /// Tanzanite, and Chrysoberyl (Yellow), Peridot, Andalusite
    /// -- should carry biaxial principal-index data; every other material (isotropic
    /// or uniaxial) must resolve to `None`, keeping them on the existing uniaxial code
    /// path unchanged.
    #[test]
    fn only_biaxial_materials_expose_a_biaxial_indicatrix() {
        let biaxial_names = [
            "Alexandrite",
            "Topaz",
            "Tanzanite",
            "Chrysoberyl (Yellow)",
            "Peridot",
            "Andalusite",
        ];
        for material in GemMaterial::all_materials() {
            let indicatrix = material.biaxial_indicatrix(589.3);
            if biaxial_names.contains(&material.name.as_str()) {
                assert!(
                    indicatrix.is_some(),
                    "{} should expose a biaxial indicatrix",
                    material.name
                );
            } else {
                assert!(
                    indicatrix.is_none(),
                    "{} should NOT expose a biaxial indicatrix (uniaxial/isotropic)",
                    material.name
                );
            }
        }
    }

    /// GPU routing test: `gpu_supported` must report `true` for EVERY built-in
    /// material, biaxial ones (Alexandrite, Topaz, Tanzanite -- the same set
    /// `only_biaxial_materials_expose_a_biaxial_indicatrix` above pins for
    /// `biaxial_indicatrix`) included -- see `gpu_supported`'s own doc comment for the
    /// eigenvector-conditioning fix and full re-verification that made this safe.
    /// Keeping a single test that tracks the routing predicate's CURRENT contract,
    /// rather than layering an exception list on top, means a future regression
    /// (e.g. reintroducing a biaxial-only gate) shows up here directly.
    #[test]
    fn gpu_supported_is_true_for_every_built_in_material() {
        let biaxial_names = [
            "Alexandrite",
            "Topaz",
            "Tanzanite",
            "Chrysoberyl (Yellow)",
            "Peridot",
            "Andalusite",
        ];
        for material in GemMaterial::all_materials() {
            assert!(
                material.gpu_supported(),
                "{} must be reported gpu_supported() == true",
                material.name
            );
            if biaxial_names.contains(&material.name.as_str()) {
                assert!(
                    material.biaxial_delta_beta_alpha.is_some(),
                    "{} must actually carry biaxial data for this test to be meaningful",
                    material.name
                );
            }
        }
    }

    /// `gpu_supported` does not depend on `biaxial_delta_beta_alpha` at all (nor on
    /// anything else) -- it is `true` unconditionally: the biaxial eigenvector
    /// conditioning lets Alexandrite/Topaz/Tanzanite pass the same GPU-equivalence
    /// bar every other material already has to. Kept as a dedicated test (rather than
    /// folded into the one above) because this predicate is itself a documented,
    /// load-bearing contract point -- pinning it here is the useful signal a future
    /// regression (e.g. reintroducing a biaxial-only gate) should trip.
    #[test]
    fn gpu_supported_no_longer_depends_on_biaxial_delta_beta_alpha() {
        let mut zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in");
        assert!(
            zircon.gpu_supported(),
            "Zircon (uniaxial) must be GPU-supported"
        );

        zircon.birefringence_delta *= -3.0;
        zircon.c_axis = Vec3::X;
        assert!(
            zircon.gpu_supported(),
            "changing unrelated uniaxial fields must not affect gpu_supported()"
        );

        zircon.biaxial_delta_beta_alpha = Some(0.0);
        assert!(
            zircon.gpu_supported(),
            "biaxial_delta_beta_alpha no longer gates GPU support"
        );

        zircon.biaxial_delta_beta_alpha = None;
        assert!(zircon.gpu_supported(), "and stays supported once cleared");
    }

    /// Each biaxial built-in's indicatrix must order its three principal indices
    /// `n_alpha` <= `n_beta` <= `n_gamma`; must place `n_beta` exactly at the
    /// material's own base dispersion curve (the documented convention); and must
    /// place `n_gamma` minus `n_alpha` exactly at the material's own
    /// `birefringence_delta` (its pre-existing "or max-min" meaning, unchanged) --
    /// pinning the `biaxial_indicatrix` wiring itself, independent of whether the
    /// underlying `biaxial_delta_beta_alpha` numbers are later refined.
    #[test]
    fn biaxial_indicatrix_is_internally_consistent_with_existing_fields() {
        for name in ["Alexandrite", "Topaz", "Tanzanite"] {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must be a built-in material"));
            let n_d = material.dispersion.evaluate(589.3);
            let indicatrix = material
                .biaxial_indicatrix(589.3)
                .unwrap_or_else(|| panic!("{name} must expose a biaxial indicatrix"));

            assert!(
                indicatrix.n_alpha <= indicatrix.n_beta && indicatrix.n_beta <= indicatrix.n_gamma,
                "{name}: principal indices must be ordered n_alpha<=n_beta<=n_gamma, got ({}, {}, {})",
                indicatrix.n_alpha,
                indicatrix.n_beta,
                indicatrix.n_gamma
            );
            assert!(
                (indicatrix.n_beta - n_d).abs() < 1e-5,
                "{name}: n_beta ({}) must equal the base dispersion curve's n_d ({n_d})",
                indicatrix.n_beta
            );
            assert!(
                (indicatrix.n_gamma - indicatrix.n_alpha - material.birefringence_delta).abs()
                    < 1e-5,
                "{name}: n_gamma - n_alpha ({}) must equal birefringence_delta ({})",
                indicatrix.n_gamma - indicatrix.n_alpha,
                material.birefringence_delta
            );
        }
    }

    /// `optical_character`'s `BiaxialPositive`/`BiaxialNegative`
    /// sign must actually match the three principal indices `biaxial_indicatrix`
    /// builds from `dispersion` (equal to `n_beta`), `biaxial_delta_beta_alpha` (equal
    /// to `n_beta` minus `n_alpha`) and `birefringence_delta` (equal to `n_gamma`
    /// minus `n_alpha`) -- optic sign is determined by where `n_beta` sits between
    /// `n_alpha` and `n_gamma`: positive when beta sits closer to alpha (`n_beta`
    /// minus `n_alpha` is less than `n_gamma` minus `n_beta`), negative when it sits
    /// closer to gamma (`n_beta` minus `n_alpha` is greater than `n_gamma` minus
    /// `n_beta`). This caught the Tanzanite entry shipping
    /// `biaxial_delta_beta_alpha: Some(0.0070)` against `birefringence_delta: 0.0130`
    /// under a declared `BiaxialPositive`: 0.0070 is greater than 0.0130 minus 0.0070
    /// equals 0.0060, which is actually NEGATIVE. Fixed by re-deriving
    /// `biaxial_delta_beta_alpha` from Hurlbut's per-specimen tanzanite indices
    /// (American Mineralogist 54, 702 (1969): `n_alpha` = 1.6915, `n_beta` = 1.6935,
    /// `n_gamma` = 1.7020) rather than the independent-range midpoints the old comment
    /// used -- see the field's own comment on the Tanzanite entry above. Runs over
    /// every built-in with `biaxial_delta_beta_alpha: Some(_)` (not just Tanzanite) so
    /// this stays a live guard against the same mistake recurring on Alexandrite,
    /// Topaz, Chrysoberyl (Yellow), Peridot or Andalusite, or on any future biaxial
    /// built-in.
    #[test]
    fn biaxial_sign_matches_optical_character() {
        for material in GemMaterial::all_materials() {
            let Some(delta_beta_alpha) = material.biaxial_delta_beta_alpha else {
                continue;
            };
            let n_beta = material.dispersion.evaluate(589.3);
            let n_alpha = n_beta - delta_beta_alpha;
            let n_gamma = n_alpha + material.birefringence_delta;

            let beta_minus_alpha = n_beta - n_alpha;
            let gamma_minus_beta = n_gamma - n_beta;

            match material.optical_character {
                OpticalCharacter::BiaxialPositive => assert!(
                    beta_minus_alpha < gamma_minus_beta,
                    "{}: declared BiaxialPositive requires (n_beta - n_alpha) < \
                     (n_gamma - n_beta), got {beta_minus_alpha} >= {gamma_minus_beta} \
                     (n_alpha={n_alpha}, n_beta={n_beta}, n_gamma={n_gamma})",
                    material.name
                ),
                OpticalCharacter::BiaxialNegative => assert!(
                    beta_minus_alpha > gamma_minus_beta,
                    "{}: declared BiaxialNegative requires (n_beta - n_alpha) > \
                     (n_gamma - n_beta), got {beta_minus_alpha} <= {gamma_minus_beta} \
                     (n_alpha={n_alpha}, n_beta={n_beta}, n_gamma={n_gamma})",
                    material.name
                ),
                other => panic!(
                    "{}: carries biaxial_delta_beta_alpha but optical_character is {other:?}, \
                     not BiaxialPositive/BiaxialNegative",
                    material.name
                ),
            }
        }
    }

    /// `new_custom`'s `dispersion_delta` must be interpreted as the Fraunhofer
    /// F-C interval -- i.e. `n(486.1nm) - n(656.3nm)` on the constructed material must
    /// equal the requested `dispersion_delta`, not 66% or 113% of it (the old flat
    /// `0.347` multiplier's actual behaviour). Checked across several representative
    /// deltas, including one large enough
    /// that a wrong conversion factor would be very obvious.
    #[test]
    fn new_custom_dispersion_delta_measures_exactly_at_f_and_c() {
        for dispersion_delta in [0.005f32, 0.010, 0.02564, 0.05] {
            let material =
                GemMaterial::new_custom("F-C probe", 1.6, dispersion_delta, 0.0, [0.0, 0.0, 0.0]);
            let n_f = material.dispersion.evaluate(486.1);
            let n_c = material.dispersion.evaluate(656.3);
            let measured = n_f - n_c;
            assert!(
                (measured - dispersion_delta).abs() < 1e-4,
                "requested F-C delta {dispersion_delta} but measured {measured} \
                 (n_F={n_f}, n_C={n_c})"
            );
        }
    }

    /// Trichroism convention pin for Alexandrite's own shipped data (the same
    /// discipline `birefringence::biaxial_reduction_tests` applies with synthetic
    /// coefficients, applied here to the real Farrell & Newnham-derived entry):
    /// each principal direction's band set must carry the qualitative amplitude
    /// pattern the cited pleochroic colours dictate, and feeding the shipped band
    /// sums through the REAL `AbsorptionTensor3::biaxial` constructor must land each
    /// one on its own world axis (alpha -> +X, beta -> Z, gamma -> +Y for
    /// `c_axis = Vec3::Y`, per `stable_orthonormal_basis`'s pinned construction) --
    /// so a future swap of the alpha/beta/gamma argument order, or of the axis
    /// convention underneath, fails loudly instead of silently recolouring the stone.
    #[test]
    fn alexandrite_trichroic_band_sets_follow_the_cited_pleochroic_pattern() {
        use super::super::birefringence::AbsorptionTensor3;

        let alex =
            GemMaterial::by_name("Alexandrite").expect("Alexandrite must be a built-in material");
        let bands_alpha = &alex.absorption.o_ray;
        let bands_gamma = &alex.absorption.e_ray;
        let bands_beta = alex
            .absorption
            .beta_ray
            .as_deref()
            .expect("Alexandrite must carry a third (beta) trichroic band set");
        assert!(
            alex.absorption.is_pleochroic,
            "Alexandrite's trichroic tensor must be flagged pleochroic"
        );

        let sum_at = |bands: &[AbsorptionBand], nm: f32| -> f32 {
            bands.iter().map(|b| b.evaluate(nm)).sum()
        };

        // 4T2 system (yellow-to-red region, evaluated at gamma's cited 595nm centre):
        // gamma (crystal b, GREEN -- absorbs "both red and yellow") strongest, alpha
        // (crystal c, RED) intermediate, beta (crystal a, YELLOW -- red/yellow left
        // open) weakest, per the figure-read Fig. 4 amplitudes in the entry's comment.
        let (t2_alpha, t2_beta, t2_gamma) = (
            sum_at(bands_alpha, 595.0),
            sum_at(bands_beta, 595.0),
            sum_at(bands_gamma, 595.0),
        );
        assert!(
            t2_gamma > t2_alpha && t2_alpha > t2_beta,
            "4T2 amplitude order must be gamma(green) > alpha(red) > beta(yellow), got \
             gamma={t2_gamma:.3}, alpha={t2_alpha:.3}, beta={t2_beta:.3}"
        );

        // 4T1 system (blue-violet, evaluated at beta's cited 422nm centre): beta
        // (crystal a) strongest -- F&N: the 0.42u absorption "dominates the a
        // spectrum" -- with alpha and gamma both clearly weaker.
        let (t1_alpha, t1_beta, t1_gamma) = (
            sum_at(bands_alpha, 422.0),
            sum_at(bands_beta, 422.0),
            sum_at(bands_gamma, 422.0),
        );
        assert!(
            t1_beta > t1_alpha && t1_beta > t1_gamma,
            "4T1 amplitude must peak on beta(yellow, crystal a), got alpha={t1_alpha:.3}, \
             beta={t1_beta:.3}, gamma={t1_gamma:.3}"
        );

        // Convention pin through the real constructor: with c_axis = +Y, the alpha
        // set's sum must appear along +X, the beta set's along Z, and the gamma set's
        // along +Y (the `c_axis`/n_gamma direction) -- see
        // `AbsorptionTensor3::biaxial`'s doc comment.
        assert_eq!(
            alex.c_axis,
            Vec3::Y,
            "test premise: Alexandrite c_axis is +Y"
        );
        let tensor = AbsorptionTensor3::biaxial(t2_alpha, t2_beta, t2_gamma, alex.c_axis);
        for (axis, expected, label) in [
            (Vec3::X, t2_alpha, "alpha on +X"),
            (Vec3::Z, t2_beta, "beta on Z"),
            (Vec3::Y, t2_gamma, "gamma on +Y (c_axis)"),
        ] {
            let measured = tensor.quadratic_form(axis);
            assert!(
                (measured - expected).abs() < 1e-5,
                "{label}: quadratic_form along {axis:?} must return that principal set's \
                 band sum ({expected:.4}), got {measured:.4}"
            );
        }
    }

    /// The mean refractive index (`mean_ri`) must be preserved exactly at the sodium D
    /// line regardless of `dispersion_delta` -- the F-C conversion factor changes how
    /// steeply `n(lambda)` varies away from D, not its value AT D (the Cauchy `a` term
    /// is solved to compensate exactly, per `new_custom`'s own formula).
    #[test]
    fn new_custom_preserves_mean_ri_at_sodium_d_line_regardless_of_dispersion_delta() {
        for dispersion_delta in [0.0f32, 0.01, 0.03] {
            let material = GemMaterial::new_custom(
                "D-line probe",
                1.72,
                dispersion_delta,
                0.0,
                [0.0, 0.0, 0.0],
            );
            let n_d = material.dispersion.evaluate(589.3);
            assert!(
                (n_d - 1.72).abs() < 1e-4,
                "dispersion_delta={dispersion_delta}: n_d should stay 1.72, got {n_d}"
            );
        }
    }

    /// Every built-in material's dispersion curve (Sellmeier or Cauchy) must
    /// evaluate to a finite, physically-sane (`n >= 1.0`) index at BOTH ends of this
    /// renderer's actual sampled visible band (380nm violet, 780nm red) -- not just
    /// near the sodium D line every other dispersion test in this file checks. Most
    /// Cauchy fits here are only solved/verified near D and the F/C Fraunhofer lines
    /// (486-656nm); this pins the `.max(1.0)` floor added to
    /// `DispersionModel::Cauchy::evaluate` (see that variant's own doc comment) so an
    /// out-of-fit-range extrapolation can never silently produce `n < 1` or a NaN.
    #[test]
    fn every_builtin_dispersion_curve_stays_physical_at_the_sampled_band_edges() {
        for material in GemMaterial::all_materials() {
            for lambda_nm in [380.0f32, 780.0] {
                let n = material.dispersion.evaluate(lambda_nm);
                assert!(
                    n.is_finite(),
                    "{}: dispersion.evaluate({lambda_nm}) must be finite, got {n}",
                    material.name
                );
                assert!(
                    n >= 1.0,
                    "{}: dispersion.evaluate({lambda_nm}) = {n} must be >= 1.0 \
                     (physically-impossible index)",
                    material.name
                );
                // The extraordinary ray (when a genuine independent curve is
                // present, or the constant-offset fallback otherwise) must be
                // equally well-behaved.
                if material.optical_character == OpticalCharacter::UniaxialPositive
                    || material.optical_character == OpticalCharacter::UniaxialNegative
                {
                    let n_e = material.extraordinary_index_at(lambda_nm, n);
                    assert!(
                        n_e.is_finite() && n_e >= 1.0,
                        "{}: extraordinary_index_at({lambda_nm}, {n}) = {n_e} must be \
                         finite and >= 1.0",
                        material.name
                    );
                }
            }
        }
    }

    /// Every new built-in must resolve by its own exact name
    /// (the same `by_name` round-trip `by_name_round_trips_every_builtin_material`
    /// already covers exhaustively -- this test's real job is the tolerance check
    /// below) and its `n(589.3nm)` must match the target table below to within
    /// 0.002.
    #[test]
    fn m4_new_species_match_their_target_n_d_within_tolerance() {
        // (name, target n_D, tolerance)
        let expected: &[(&str, f32, f32)] = &[
            ("Aquamarine", 1.577, 0.002),
            ("Morganite", 1.577, 0.002),
            ("Chrysoberyl (Yellow)", 1.746, 0.002),
            ("Amethyst", 1.544, 0.001),
            ("Citrine", 1.544, 0.001),
            ("Pyrope Garnet", 1.714, 0.002),
            ("Almandine Garnet", 1.790, 0.002),
            ("Spessartine Garnet", 1.800, 0.002),
            ("Grossular Garnet (Tsavorite)", 1.734, 0.002),
            ("Andradite Garnet (Demantoid)", 1.887, 0.002),
            ("Peridot", 1.654, 0.002),
            ("YAG", 1.833, 0.002),
            ("Benitoite", 1.757, 0.002),
            ("Andalusite", 1.634, 0.002),
            ("Opal", 1.45, 0.002),
            ("Glass (N-BK7)", 1.5168, 0.002),
            ("Glass (F2)", 1.620, 0.002),
        ];

        for &(name, target_n_d, tol) in expected {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must resolve via by_name"));
            assert_eq!(material.name, name, "{name} must resolve to itself exactly");
            let n_d = material.dispersion.evaluate(589.3);
            assert!(
                (n_d - target_n_d).abs() <= tol,
                "{name}: n_d={n_d:.5} does not match target {target_n_d} within {tol}"
            );
        }

        // GGG is checked separately: its target n_D (1.970) is only reproduced to
        // within ~0.002 by construction from this entry's own Cauchy fit (see that
        // entry's comment for why no primary Sellmeier fit was available), so this
        // uses the same tolerance but is called out on its own for clarity.
        let ggg = GemMaterial::by_name("GGG").expect("GGG must resolve via by_name");
        let ggg_n_d = ggg.dispersion.evaluate(589.3);
        assert!(
            (ggg_n_d - 1.970).abs() <= 0.002,
            "GGG: n_d={ggg_n_d:.5} does not match target 1.970 within 0.002"
        );
    }

    /// Every new built-in must have an explicit birefringence sign/magnitude
    /// matching the target table below (a cheap, high-signal regression against
    /// a mistyped `birefringence_delta` literal).
    #[test]
    fn m4_new_species_match_their_target_birefringence() {
        let expected: &[(&str, f32, f32)] = &[
            ("Aquamarine", -0.006, 1e-4),
            ("Morganite", -0.006, 1e-4),
            ("Chrysoberyl (Yellow)", 0.009, 1e-4),
            ("Amethyst", 0.0091, 1e-4),
            ("Citrine", 0.0091, 1e-4),
            ("Peridot", 0.036, 1e-4),
            ("Benitoite", 0.047, 1e-4),
            ("Andalusite", 0.010, 1e-4),
        ];
        for &(name, target, tol) in expected {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must resolve via by_name"));
            assert!(
                (material.birefringence_delta - target).abs() <= tol,
                "{name}: birefringence_delta={} does not match target {target}",
                material.birefringence_delta
            );
        }
        // The five garnets, YAG, GGG, Opal and the two glasses are isotropic --
        // birefringence_delta must be exactly 0.0.
        for name in [
            "Pyrope Garnet",
            "Almandine Garnet",
            "Spessartine Garnet",
            "Grossular Garnet (Tsavorite)",
            "Andradite Garnet (Demantoid)",
            "YAG",
            "GGG",
            "Opal",
            "Glass (N-BK7)",
            "Glass (F2)",
        ] {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must resolve via by_name"));
            assert_eq!(
                material.birefringence_delta, 0.0,
                "{name}: isotropic material must have birefringence_delta == 0.0"
            );
        }
    }

    /// Quartz's genuine per-axis (Ghosh o/e) dispersion must make its
    /// extraordinary-ray index actually VARY with wavelength in a way the old
    /// constant-offset approximation could not (the offset `n_o(lambda) +
    /// birefringence_delta` tracks `n_o`'s own curvature exactly, so `n_e(lambda) -
    /// n_o(lambda)` is constant under the old model but need not be under a genuine
    /// independent curve); a material with no `uniaxial_extraordinary_dispersion` set
    /// (e.g. Sapphire) must keep the exact old constant-delta behaviour, bit-identical.
    #[test]
    fn quartz_extraordinary_index_has_wavelength_dependent_birefringence_while_legacy_entries_stay_constant()
     {
        let quartz = GemMaterial::by_name("Quartz").expect("Quartz must resolve");
        assert!(
            quartz.uniaxial_extraordinary_dispersion.is_some(),
            "test premise: Quartz must carry a genuine per-axis e-ray dispersion curve"
        );

        let delta_at = |lambda_nm: f32| {
            let n_o = quartz.dispersion.evaluate(lambda_nm);
            let n_e = quartz.extraordinary_index_at(lambda_nm, n_o);
            n_e - n_o
        };
        let delta_380 = delta_at(380.0);
        let delta_780 = delta_at(780.0);
        assert!(
            (delta_380 - delta_780).abs() > 1e-4,
            "Quartz's n_e - n_o must genuinely vary across the visible band: \
             delta(380nm)={delta_380:.6}, delta(780nm)={delta_780:.6}"
        );

        // A legacy entry (no per-axis curve) must keep the OLD constant-delta
        // behaviour exactly: n_e - n_o == birefringence_delta at every wavelength.
        let sapphire = GemMaterial::by_name("Sapphire").expect("Sapphire must resolve");
        assert!(
            sapphire.uniaxial_extraordinary_dispersion.is_none(),
            "test premise: Sapphire must NOT carry a per-axis e-ray dispersion curve"
        );
        for lambda_nm in [380.0f32, 589.3, 780.0] {
            let n_o = sapphire.dispersion.evaluate(lambda_nm);
            let n_e = sapphire.extraordinary_index_at(lambda_nm, n_o);
            assert!(
                (n_e - n_o - sapphire.birefringence_delta).abs() < 1e-6,
                "Sapphire (legacy entry) at {lambda_nm}nm: n_e - n_o must equal the \
                 constant birefringence_delta exactly"
            );
        }
    }
}
