//! The local incidence frame and the closed-form mode fields built from it.
//!
//! The ordinary/extraordinary wavevector normal-component roots, the per-mode field
//! vectors built from them, and the Poynting flux those fields carry through the
//! interface.

use super::complex::{CVec3, Cplx};
use glam::Vec3;

/// The local orthonormal frame + optic-axis direction cosines shared by every channel
/// at one bounce -- purely geometric (facet normal, wave normal, optic axis), so built
/// ONCE per bounce (like `theta_c_for_bounce`'s shared `theta_c`) and reused for every
/// channel's own per-wavelength `n_o_k`/`n_e_k` solve. See this module tree's own
/// top-level (`uniaxial_fresnel`) doc comment for the frame convention.
#[derive(Clone, Copy)]
pub(crate) struct UniaxialFrame {
    pub that: Vec3,
    pub zhat: Vec3,
    pub s_axis: Vec3,
    /// `k_hat x s_axis` -- perpendicular to the INCIDENT wave normal specifically, NOT
    /// generally usable as the transverse partner axis for a TRANSMITTED/reflected
    /// mode direction (those are only guaranteed perpendicular to their OWN
    /// propagation direction, which differs from `k_hat` by Snell's law) -- see
    /// `apply_uniaxial_entry_bounce`'s own `p_axis_transmitted`, which every current
    /// caller uses instead for exactly that reason. Kept on this struct for API
    /// completeness/symmetry with `that`/`s_axis`/`zhat`.
    #[expect(
        dead_code,
        reason = "kept for API completeness -- see doc comment above"
    )]
    pub p_axis: Vec3,
    pub cos_i: f32,
    pub sin_i: f32,
    pub alpha: f32,
    pub beta: f32,
    pub gamma: f32,
}

impl UniaxialFrame {
    /// Builds the shared frame from the SAME `k_hat`/`normal`/`cos_i`/`sin_i` every
    /// other per-bounce geometry function in this module tree already computes --
    /// `normal` is the ray-facing shading normal (`(-k_hat).dot(normal) == cos_i`,
    /// [`compute_bounce_refraction_geometry`]'s convention), `c_axis` the material's
    /// optic axis. Degenerate at `k_hat` (anti)parallel to `normal` (normal incidence,
    /// `sin_i ~ 0`) -- `that`/`s_axis` are then only defined up to an arbitrary
    /// rotation about `normal`; callers at that limit get a well-defined but ARBITRARY
    /// azimuth, physically correct since the interface itself has no other
    /// direction to break the tie UNLESS `c_axis` has its own in-plane component, in
    /// which case a caller wanting the exact non-degenerate normal-incidence physics
    /// should build the frame from `c_axis`'s own tangential projection instead (this
    /// crate's tests exercise that case directly against the closed-form `(n-1)/(n+1)`
    /// identity; production bounce dispatch never needs it since `sin_i ~ 0` already
    /// makes the mode split itself vanish in every other code path).
    #[must_use]
    pub(crate) fn build(k_hat: Vec3, normal: Vec3, c_axis: Vec3, cos_i: f32, sin_i: f32) -> Self {
        let zhat = -normal;
        let that = if sin_i > 1e-5 {
            ((k_hat + normal * cos_i) / sin_i).normalize_or_zero()
        } else {
            // Arbitrary but stable in-plane axis at normal incidence.
            let fallback = if zhat.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            (fallback - zhat * zhat.dot(fallback)).normalize_or_zero()
        };
        let s_axis = k_hat.cross(normal).normalize_or_zero();
        let s_axis = if s_axis.length_squared() > 1e-8 {
            s_axis
        } else {
            zhat.cross(that).normalize_or_zero()
        };
        let p_axis = k_hat.cross(s_axis);
        Self {
            that,
            zhat,
            s_axis,
            p_axis,
            cos_i,
            sin_i,
            alpha: c_axis.dot(that),
            beta: c_axis.dot(s_axis),
            gamma: c_axis.dot(zhat),
        }
    }
}

/// Closed-form ordinary/extraordinary wavevector normal-component (`q`, along `zhat`)
/// roots -- Lekner (1991) eqs. for `q_o`, `q_e` (cross-checked against the same
/// closed-form quadratic in Yeh's uniaxial treatment). `_plus` is the root with `re >
/// 0` (or, if evanescent, `im > 0`) -- i.e. propagating/decaying in `+zhat` (the
/// "forward into this medium" root); `_minus` is the other (`-q_o` for the ordinary
/// wave; NOT simply `-q_e` for the extraordinary wave in general -- see this module
/// tree's own top-level doc comment on the `alpha*gamma` walk-off asymmetry between
/// forward/backward extraordinary propagation).
///
/// `tangential` is the shared tangential wavevector magnitude `K = n1 * sin_i` (`n1`:
/// the CURRENT medium's index at this channel -- `1.0` for air, or the incident mode's
/// own index for an internal/exit interface).
#[derive(Clone, Copy)]
pub(crate) struct QRoots {
    pub qo_plus: Cplx,
    pub qo_minus: Cplx,
    pub qe_plus: Cplx,
    pub qe_minus: Cplx,
}

#[must_use]
pub(crate) fn uniaxial_q_roots(
    n_o: f32,
    n_e: f32,
    frame: &UniaxialFrame,
    tangential: f32,
) -> QRoots {
    let n_o2 = n_o * n_o;
    let k2 = tangential * tangential;

    let qo2 = Cplx::re(n_o2 - k2);
    let qo_plus = qo2.sqrt_forward_branch();
    let qo_minus = Cplx::ZERO.sub(qo_plus);

    // Performance (requirement 7): exact isotropic limit (`n_o == n_e` -- some
    // built-ins are formally uniaxial by crystal system but carry zero measured
    // birefringence, e.g. a legacy `birefringence_delta: 0.0` entry with no separate
    // extraordinary dispersion curve). At this exact limit the extraordinary
    // quadratic's two roots algebraically coincide with the ordinary root already
    // computed above (`deleps == 0` collapses `d_val` to `n_o^4 * (n_o2 - k2)` and
    // `denom` to `n_o2`, so `qe_plus == sqrt(n_o^4*(n_o2-k2))/n_o2 == sqrt(n_o2-k2) ==
    // qo_plus` exactly, up to the branch-cut convention `sqrt_forward_branch` already
    // shares between the two calls) -- so skip the second `sqrt_forward_branch` (an
    // `atan2` plus two `sqrt` calls plus a `sin`/`cos` pair, this function's most
    // expensive part per call, see `entry_solve`'s own doc comment) and the two
    // divisions entirely, rather than spend them computing a value already known.
    if n_o == n_e {
        return QRoots {
            qo_plus,
            qo_minus,
            qe_plus: qo_plus,
            qe_minus: qo_minus,
        };
    }

    let n_e2 = n_e * n_e;
    let deleps = n_e2 - n_o2;
    let denom = (frame.gamma * frame.gamma).mul_add(deleps, n_o2);
    let d_val = n_o2
        * (frame.beta * frame.beta).mul_add(-deleps, n_e2).mul_add(
            -k2,
            n_e2 * (frame.gamma * frame.gamma).mul_add(deleps, n_o2),
        );
    let sqrt_d = Cplx::re(d_val).sqrt_forward_branch();
    let shift = Cplx::re(frame.alpha * frame.gamma * tangential * deleps);
    let denom_c = Cplx::re(denom.max(1e-12));
    let qe_plus = sqrt_d.sub(shift).div(denom_c);
    let qe_minus = Cplx::ZERO.sub(sqrt_d).sub(shift).div(denom_c);

    QRoots {
        qo_plus,
        qo_minus,
        qe_plus,
        qe_minus,
    }
}

/// One uniaxial eigenmode's `(k, E, H)` fields at wavevector normal-component `q`,
/// amplitude 1. `D_o = k x c_axis` (always `perp c_axis` by construction -- the
/// ordinary eigenmode's defining property), `E_o = D_o / n_o^2` (the ordinary
/// permittivity is scalar `n_o^2` in the plane `perp c_axis`, which `D_o` lies in
/// entirely). `D_e = k x D_o` (the unique direction `perp k` AND `perp D_o`, the
/// extraordinary eigenmode's defining property), `E_e = eps^-1 D_e` via the uniaxial
/// tensor inverse `eps^-1 = I/n_o^2 - ((n_e^2-n_o^2)/(n_o^2*n_e^2)) * c_axis c_axis^T`.
/// `H = k x E` in both cases (the universal Maxwell relation -- see this module tree's
/// own top-level doc comment).
pub(crate) struct ModeFields {
    /// Kept alongside `e`/`h` for completeness and future callers (e.g. a direct
    /// wavevector-based chromatic-termination check mirroring `refraction.rs`'s
    /// existing direction comparisons) -- not read by any current caller, which all
    /// derive directions from `e`'s real part instead.
    #[expect(
        dead_code,
        reason = "public field kept for API completeness -- see doc comment above"
    )]
    pub k: CVec3,
    pub e: CVec3,
    pub h: CVec3,
}

#[must_use]
pub(crate) fn ordinary_mode_fields(
    n_o: f32,
    c_axis: Vec3,
    that: Vec3,
    zhat: Vec3,
    tangential: f32,
    q: Cplx,
) -> ModeFields {
    let k = CVec3::wavevector(that, zhat, tangential, q);
    let d_o = k.cross_real(c_axis);
    let e = d_o.scale_real(1.0 / (n_o * n_o));
    let h = k.cross(e);
    ModeFields { k, e, h }
}

#[must_use]
pub(crate) fn extraordinary_mode_fields(
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    that: Vec3,
    zhat: Vec3,
    tangential: f32,
    q: Cplx,
) -> ModeFields {
    let n_o2 = n_o * n_o;
    let n_e2 = n_e * n_e;
    let deleps = n_e2 - n_o2;
    let k = CVec3::wavevector(that, zhat, tangential, q);
    let d_o = k.cross_real(c_axis);
    let d_e = k.cross(d_o);
    let c_dot_de = d_e.dot_real(c_axis);
    let e = d_e
        .scale_real(1.0 / n_o2)
        .add(CVec3::from_real(c_axis).scale_complex(c_dot_de.scale(-(deleps / (n_o2 * n_e2)))));
    let h = k.cross(e);
    ModeFields { k, e, h }
}

/// `0.5 * Re(E x H*) . zhat` -- the time-averaged Poynting flux (in `k0`-normalized
/// units) through the interface, for `ModeFields` scaled by complex amplitude `amp`.
#[must_use]
pub(crate) fn poynting_z(fields: &ModeFields, amp: Cplx, zhat: Vec3) -> f32 {
    let e = fields.e.scale_complex(amp);
    let h = fields.h.scale_complex(amp);
    let hc = h.conj();
    // (E x H*).zhat, complex; the physical flux is 0.5*Re(...).
    let s = e.cross(hc);
    0.5 * s.dot_real(zhat).re
}
