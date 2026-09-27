//! Symmetric second-rank absorption tensor [`AbsorptionTensor3`] (pleochroism)
//! and the assigned-mode/effective pleochroic absorption coefficient functions
//! built on it.

use super::{helpers::stable_orthonormal_basis, uniaxial::BirefringenceParams};
use crate::optics::polarization::{StokesVector, electric_field_direction};
use glam::{Mat3, Vec3};

/// Symmetric second-rank absorption tensor **A**, diagonal in the crystal's principal
/// axes.
///
/// Represented as three principal coefficients plus that orthonormal axis frame
/// (`axes`) rather than a dense 3x3 matrix -- cheaper to evaluate, and the natural form
/// gem chromophore data already takes (an ordinary/extraordinary pair via
/// [`Self::uniaxial`], or three independent coefficients for a fully biaxial tensor).
///
/// Absorption in an anisotropic crystal depends on the light's electric-field
/// polarization direction, not its propagation direction -- this is pleochroism (e.g.
/// tanzanite reading blue, violet or brownish by polarization). See
/// [`Self::quadratic_form`].
#[derive(Debug, Clone, Copy)]
pub struct AbsorptionTensor3 {
    /// Principal absorption coefficients, one per column of `axes`, same order.
    pub alpha: Vec3,
    /// Orthonormal principal-axis frame in world space; columns are the three
    /// principal directions the tensor is diagonal in.
    pub axes: Mat3,
}

impl AbsorptionTensor3 {
    /// Builds the uniaxial special case directly from a material's existing
    /// ordinary/extraordinary absorption coefficients and single optical c-axis: two
    /// degenerate principal coefficients (`alpha_o`) in the plane perpendicular to
    /// `c_axis`, and one (`alpha_e`) along it. The in-plane pair of principal axes is
    /// arbitrary -- the tensor is isotropic within that plane, so their specific
    /// directions don't affect `quadratic_form`'s result -- and filled in via
    /// `stable_orthonormal_basis`.
    #[must_use]
    pub fn uniaxial(alpha_o: f32, alpha_e: f32, c_axis: Vec3) -> Self {
        let c = c_axis.normalize_or_zero();
        let (a1, a2) = stable_orthonormal_basis(c);
        Self {
            alpha: Vec3::new(alpha_o, alpha_o, alpha_e),
            axes: Mat3::from_cols(a1, a2, c),
        }
    }

    /// Trichroism: the genuinely biaxial case -- three independent principal absorption
    /// coefficients rather than `uniaxial`'s degenerate `(alpha_o, alpha_o, alpha_e)`
    /// pair.
    ///
    /// Builds its axis frame via the exact same `stable_orthonormal_basis(gamma_axis)`
    /// call, on the exact same input, that `BiaxialIndicatrix::from_gamma_axis` uses
    /// for the material's index frame -- load-bearing, since `eigen_polarizations`'
    /// eigenmodes (which `quadratic_form` is evaluated against) live in that same
    /// index frame; any drift between the two would silently score absorption against
    /// the wrong axes. `gamma_axis` alone determines the frame: `alpha` lands on the
    /// first `stable_orthonormal_basis` output, `beta` on the second, `gamma` on
    /// `gamma_axis` itself (e.g. `gamma_axis = +Y` gives `alpha` -> `+X`, `beta` ->
    /// `-Z`, `gamma` -> `+Y`).
    ///
    /// Degenerate case: `alpha == beta` reproduces `uniaxial(alpha, gamma, gamma_axis)`
    /// bit-identically (same `Vec3::new` arguments, same `stable_orthonormal_basis`
    /// call).
    #[must_use]
    pub fn biaxial(alpha: f32, beta: f32, gamma: f32, gamma_axis: Vec3) -> Self {
        let g = gamma_axis.normalize_or_zero();
        let (a1, a2) = stable_orthonormal_basis(g);
        Self {
            alpha: Vec3::new(alpha, beta, gamma),
            axes: Mat3::from_cols(a1, a2, g),
        }
    }

    /// Evaluates the quadratic form `alpha = e_hat . A . e_hat` for a unit electric-
    /// field polarization direction `e_hat`. Since `A` is diagonal in `axes`, this is
    /// just the alpha-weighted sum of `e_hat`'s squared components along each
    /// principal axis -- no dense 3x3 matrix multiply needed.
    #[must_use]
    pub fn quadratic_form(&self, e_hat: Vec3) -> f32 {
        let local = self.axes.transpose() * e_hat;
        self.alpha.dot(local * local)
    }
}

/// Assigned-mode pleochroic absorption coefficient: `alpha = e_mode_hat . A . e_mode_hat`.
///
/// For light KNOWN to occupy exactly one eigenmode (`e_mode_hat`), with no
/// degree-of-polarization blending against a Stokes vector at all.
///
/// # Why this replaces [`effective_pleochroic_alpha`] for the interior transport path
///
/// Inside a birefringent crystal, `trace_spectral_ray_inner` assigns each traced path to
/// ONE eigenmode (`is_extraordinary`) at its air->crystal entry, and that mode's
/// polarization is known EXACTLY from the geometry (wave normal, c-axis/indicatrix) --
/// it never needs to be read off the light's own Stokes vector. Using the Stokes-derived
/// `electric_field_direction` instead (as [`effective_pleochroic_alpha`] does) has two
/// problems: (a) on a camera-traced path the Stokes vector represents IMPORTANCE, not
/// the light's physical state, and the old function's degree-of-polarization blend is
/// non-linear, so the adjoint identity that justifies camera-side Mueller products does
/// not hold through it; (b) after internal reflections the Stokes azimuth drifts away
/// from the assigned mode's own axis (`apply_tir_bounce`'s uniaxial branch only scales
/// magnitude by `r_total_k`, it does not re-align the azimuth), so the old function's
/// `e_hat` stops matching the mode `is_extraordinary` actually names. Calling this
/// function with the mode's own geometric E-field direction (see
/// `assigned_mode_e_field_uniaxial`/`BiaxialIndicatrix::assigned_mode_e_field`) sidesteps
/// both: no Stokes vector is read at all.
///
/// See `raytracer::absorption::channel_absorption_alphas_assigned` for the per-channel
/// driver that calls this once per spectral channel, and that function's own doc comment
/// for the isotropic-by-symmetry special case (no single eigenmode assignment is
/// meaningful for an isotropic material, so that case is handled separately, without this
/// function).
#[must_use]
pub fn assigned_mode_alpha(tensor: &AbsorptionTensor3, e_mode_hat: Vec3) -> f32 {
    tensor.quadratic_form(e_mode_hat)
}

/// The assigned uniaxial eigenmode's world-space electric-field direction at the CURRENT
/// wave normal `k_hat`.
///
/// - Ordinary mode (`!is_extraordinary`): identical to
///   [`BirefringenceParams::ordinary_eigen_polarization`] -- perpendicular to the plane
///   containing `k_hat` and `c_axis`.
/// - Extraordinary mode: the e-mode's E-field does NOT lie perpendicular to `k_hat` (that
///   would be D, not E) -- it lies IN the `(k_hat, c_axis)` plane, perpendicular to the
///   Poynting vector `S` (`S` from [`BirefringenceParams::extraordinary_poynting_dir`],
///   evaluated at the hero channel's own `n_o_hero`/`n_e_hero` -- the walk-off direction
///   is a per-ray geometric quantity, not per-channel). Computed as
///   `normalize((k_hat x c_axis) x S)`: `k_hat x c_axis` is the ordinary axis direction
///   (unnormalized), so this is perpendicular to both the ordinary axis and `S`, which
///   places it exactly in the `(k_hat, c_axis)` plane and perpendicular to `S` as
///   required.
///
/// Degenerates to [`BirefringenceParams::extraordinary_eigen_polarization`] when `k_hat`
/// is parallel to `c_axis` (`|k_hat x c_axis|^2 < 1e-8`): both eigenmodes collapse to
/// ordinary there (no birefringence along the optic axis), and the `(k_hat, c_axis)`
/// plane itself is undefined.
#[must_use]
pub fn assigned_mode_e_field_uniaxial(
    k_hat: Vec3,
    c_axis: Vec3,
    is_extraordinary: bool,
    n_o_hero: f32,
    n_e_hero: f32,
) -> Vec3 {
    if !is_extraordinary {
        return BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis);
    }
    let k_cross_c = k_hat.cross(c_axis);
    if k_cross_c.length_squared() < 1e-8 {
        return BirefringenceParams::extraordinary_eigen_polarization(k_hat, c_axis);
    }
    let s = BirefringenceParams::extraordinary_poynting_dir(k_hat, c_axis, n_o_hero, n_e_hero);
    k_cross_c.cross(s).normalize()
}

/// Effective pleochroic (polarization-dependent) absorption coefficient.
///
/// Combines the light's actual electric-field direction (for the polarized fraction)
/// with an unweighted average over the two eigenmodes (for the unpolarized fraction)
/// -- unpolarized light couples equally into both eigenmodes, so its absorption is
/// their average regardless of the crystal's own principal axes.
///
/// `degree_of_polarization` in `[0, 1]` (see `StokesVector::degree_of_polarization`)
/// linearly interpolates between the two limits. An isotropic tensor (all principal
/// coefficients equal) returns the same value regardless of direction, so isotropic
/// materials are automatically azimuth-independent with no special case needed here.
///
/// No longer used by the interior transport path (`apply_absorption`/
/// `maybe_scatter_or_extinguish` now call [`assigned_mode_alpha`] instead, via
/// `raytracer::absorption::channel_absorption_alphas_assigned`) -- kept for its own unit
/// tests below and the Tier 2 GPU harness (`renderer::gpu::transport_check::
/// absorption_pleochroism`/`eigenmodes_biaxial`), which still pin `pleochroic_channel_alpha`
/// (built on this) against its WGSL translation.
#[must_use]
pub fn effective_pleochroic_alpha(
    tensor: &AbsorptionTensor3,
    e_hat: Vec3,
    eigenmode_a: Vec3,
    eigenmode_b: Vec3,
    degree_of_polarization: f32,
) -> f32 {
    let p = degree_of_polarization.clamp(0.0, 1.0);
    let alpha_polarized = tensor.quadratic_form(e_hat);
    let alpha_unpolarized = f32::midpoint(
        tensor.quadratic_form(eigenmode_a),
        tensor.quadratic_form(eigenmode_b),
    );
    p.mul_add(alpha_polarized - alpha_unpolarized, alpha_unpolarized)
}

/// All-in-one pleochroic Beer-Lambert coefficient for one spectral channel.
///
/// Derives the channel's own electric-field direction from its own `stokes` vector
/// (see `electric_field_direction`), builds this channel's `AbsorptionTensor3` from
/// `alpha_o`/`alpha_e` (and, for a genuinely biaxial material with a third band set,
/// `alpha_beta`), and combines with the two eigenmode directions via
/// `effective_pleochroic_alpha`. The single call site `trace_spectral_ray`'s
/// per-channel absorption loop needs.
///
/// `alpha_beta` is `Some` only when the caller has confirmed both that the material is
/// genuinely biaxial (`eigenmode_a`/`eigenmode_b` came from
/// `BiaxialIndicatrix::eigen_polarizations`, not the uniaxial approximation) and that
/// it carries a third band set (`AbsorptionTensor::beta_ray.is_some()`); see
/// `raytracer::apply_absorption`. `None` takes the `AbsorptionTensor3::uniaxial` path.
///
/// No longer used by the interior transport path for an anisotropic material -- see
/// [`effective_pleochroic_alpha`]'s doc comment. Still used by the transport path's
/// ISOTROPIC branch (`raytracer::absorption::channel_absorption_alphas_assigned`'s own
/// doc comment explains why that branch alone keeps the Stokes-based call, on the GPU
/// side only), by this file's own unit tests, and by the Tier 2 GPU harness.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "argument order deliberately mirrors transport_physics.wgsl's \
              pleochroic_channel_alpha/pleochroic_channel_alpha_biaxial (alpha_o, \
              alpha_e, [alpha_beta,] c_axis, s_axis, propagation_dir, eigen_a, eigen_b, \
              stokes) one-for-one; bundling these into a context struct here would break \
              that correspondence and make it harder to check the CPU and GPU paths \
              against each other, which is the whole point of this function existing \
              as a direct WGSL mirror"
)]
pub fn pleochroic_channel_alpha(
    alpha_o: f32,
    alpha_e: f32,
    alpha_beta: Option<f32>,
    c_axis: Vec3,
    s_axis: Vec3,
    propagation_dir: Vec3,
    eigenmode_a: Vec3,
    eigenmode_b: Vec3,
    stokes: &StokesVector,
) -> f32 {
    let tensor = alpha_beta.map_or_else(
        || AbsorptionTensor3::uniaxial(alpha_o, alpha_e, c_axis),
        |beta| AbsorptionTensor3::biaxial(alpha_o, beta, alpha_e, c_axis),
    );
    let e_hat = electric_field_direction(stokes, s_axis, propagation_dir);
    effective_pleochroic_alpha(
        &tensor,
        e_hat,
        eigenmode_a,
        eigenmode_b,
        stokes.degree_of_polarization(),
    )
}
