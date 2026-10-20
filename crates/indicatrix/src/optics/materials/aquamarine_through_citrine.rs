//! Built-in material data: Aquamarine, Morganite, Chrysoberyl (Yellow).

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// First of four new-species quarters: Aquamarine, Morganite (both beryl, sharing
    /// Emerald's host-mineral optics), Chrysoberyl (Yellow) (sharing Alexandrite's
    /// host-mineral indicatrix), Amethyst and Citrine (both quartz, sharing the
    /// colorless Quartz entry's exact Ghosh 1999 o/e Sellmeier pair above).
    ///
    /// # Dispersion-figure convention note (applies to every entry in this file)
    ///
    /// The stored convention is the Fraunhofer F-C interval (486.1-656.3 nm). Every
    /// gemological "dispersion" target below is the standard Fraunhofer B-G
    /// table value (e.g. Andradite/demantoid's famous "0.057," higher than diamond's
    /// own 0.044) and is converted B-G -> F-C by multiplying with the physically-derived
    /// 0.579 ratio (range 0.569-0.587) the Emerald/Zircon/Topaz/Tourmaline/Tanzanite
    /// entries above use, rather than taken at face value as F-C. The exception is entries with a genuine named
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
    /// +0.2957) Fresnel treatment, compounded by rutile's very strong dispersion, so
    /// it gets its own dedicated entry -- see [`Self::built_in_material_rutile`].
    pub(super) fn built_in_materials_aquamarine_through_citrine() -> Vec<Self> {
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
                // Chromophore: Fe2+-Fe3+ intervalence band at ~600-680 nm polarised
                // along c (e-ray, the blue), Fe2+ at ~830 nm (strong in the o-ray,
                // where only its 650-780 nm tail is visible), weak Fe3+ at 428 nm and
                // a near-UV edge in both rays. FITTED 2026-10-19 by bounded
                // least-squares energy-Gaussian fits (log residual, 380-780 nm; the
                // NIR band centres sit on their bounds, only their tails matter) to
                // the two polarised Caltech Mineral Spectroscopy Server spectra of
                // aquamarine GRR 1488 (Lajes Pintada, Brazil; 5.531 mm; per-ray
                // baselines 0.683 / 0.866 removed), `data/reference_spectra/
                // caltech_aquamarine_grr1488_{eperpc,eparc}_5p531mm.csv`, at the
                // crystal's own iron content over the 8.8 mm face-up path of a 7 mm
                // stone. Swatch (7 mm, D65, (2o + e)/3): L*85 C*16 h=247 (o-ray
                // L*97 C*13: near-colorless; e-ray L*44 C*88 h 287: deep
                // violet-blue); p = 2: h 227. The face-up wave normal is the view
                // axis and `c_axis` is `Vec3::X` (c in the table plane, as for
                // Tourmaline, the way aquamarine is cut), so the face-up view mixes
                // the o- and e-ray and reads blue; with c along the view axis it
                // would see the near-colorless o-ray only.
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E perp c): near-colorless; UV/Fe3+ edge, Fe3+ 429 nm,
                        // weak IVCT leak, Fe2+ ~830 nm tail
                        AbsorptionBand::energy(363.0, 5_531.0, 1.19),
                        AbsorptionBand::energy(429.0, 1_500.0, 0.214),
                        AbsorptionBand::energy(645.0, 3_657.0, 0.198),
                        AbsorptionBand::energy(845.0, 2_797.0, 6.54),
                    ],
                    vec![
                        // e-ray (E par c): the blue ray; Fe2+-Fe3+ IVCT ~600-680 nm +
                        // Fe2+ ~830 nm
                        AbsorptionBand::energy(378.0, 3_102.0, 0.892),
                        AbsorptionBand::energy(428.0, 565.0, 0.317),
                        AbsorptionBand::energy(600.0, 4_405.0, 3.26),
                        AbsorptionBand::energy(810.0, 5_471.0, 8.82),
                    ],
                ),
                // c_axis set INTO the table plane (Vec3::X) rather than the Vec3::Y
                // every other built-in defaults to, exactly as Tourmaline does -- a
                // deliberate override, not an oversight. Aquamarine's color sits in
                // the e-ray (E parallel to c): with c_axis=Y the face-up wave normal
                // is parallel to c, where both eigenmodes are the o-ray (L*97, almost
                // colorless), backwards from how aquamarine is cut and worn (c in the
                // table plane, so the blue ray shows face-up). See the test
                // `aquamarine_face_up_sees_the_blue_ray_with_c_axis_in_table_plane`
                // in `tests/raytracer_tests/pleochroic_orientation.rs`.
                c_axis: Vec3::X,
                biaxial_delta_beta_alpha: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
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
                // color. Width (45nm) and peak (1.0, deliberately weak -- morganite
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
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
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
                // -- the standard attribution for ordinary (non-color-change)
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
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }
}
