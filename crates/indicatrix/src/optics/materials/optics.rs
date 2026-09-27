//! Per-material optics queries: the biaxial indicatrix, the extraordinary-ray
//! index lookup, and the GPU-routing predicate.

use super::GemMaterial;
use crate::optics::birefringence::BiaxialIndicatrix;

impl GemMaterial {
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
    /// `optics::birefringence` for the full derivations.
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
