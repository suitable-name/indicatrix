//! Built-in material data: Spinel, Quartz, Tourmaline.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Third quarter of the built-in material table (Spinel through Tourmaline). See
    /// `built_in_materials_diamond_through_emerald` for why the table is split into
    /// several functions.
    pub(super) fn built_in_materials_spinel_through_tourmaline() -> Vec<Self> {
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
            // dispersion table lists Spinel = 0.020; 0.020*0.579 (the B-G->F-C ratio
            // used throughout, see Emerald entry) = 0.01158, within 2% of this
            // Sellmeier-derived value. The stored convention is F-C (486.1-656.3 nm).
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
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
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
            // figure is the standard B-G (not F-C) interval (0.013*0.579 = 0.00753,
            // within 4% of this Ghosh-derived value; the stored curve is the Ghosh
            // fit itself, in the F-C convention, not the converted figure).
            Self {
                name: "Quartz".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // This entry is the colorless rock-crystal reference: empty band set
                // (zero absorption at every wavelength). Rock crystal quartz
                // genuinely has no visible-range chromophore; its tinted varieties
                // (Amethyst: a hole-color-centre defect; Citrine: Fe3+) are their own
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
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
                // Quartz (with Amethyst and Citrine, which reuse this exact curve) and
                // Rutile are the built-ins with a genuine primary e-ray Sellmeier
                // fit alongside the o-ray one -- for Quartz, G. Ghosh, Opt. Commun. 163, 95-102
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
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                #[cfg(feature = "zoning")]
                zoning: None,
                uniaxial_extraordinary_dispersion: None,
            },
        ]
    }
}
