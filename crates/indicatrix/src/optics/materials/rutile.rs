//! Built-in material data: Rutile.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
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
    pub(super) fn built_in_material_rutile() -> Self {
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
}
