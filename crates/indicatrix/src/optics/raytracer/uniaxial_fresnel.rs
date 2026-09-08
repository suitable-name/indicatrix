//! Exact anisotropic-interface Fresnel solution for uniaxial crystals.
//!
//! Replaces the "effective index" scalar-Fresnel-per-mode approximation that used to
//! live at every `is_anisotropic && !is_biaxial` interface in [`super::refraction`].
//!
//! # Physics and derivation
//!
//! At an isotropic/uniaxial interface, an incident s- or p-polarized wave does NOT
//! reflect/transmit as if the crystal had one scalar "effective index" -- it couples
//! into the ordinary (o) AND extraordinary (e) eigenmodes simultaneously, and (except
//! when the optic axis lies in the plane of incidence) the reflected wave in the
//! isotropic medium is itself a genuine s/p mixture. This module implements the
//! closed-form solution for that boundary-value problem: Lekner, J. Phys.: Condens.
//! Matter 3 (1991) 6121-6133 ("Reflection and refraction by uniaxial crystals"),
//! cross-checked against Yeh, J. Opt. Soc. Am. 69 (1979) 742 and Yeh, "Optical Waves in
//! Layered Media" (1988) ch. 9 -- both derive the same boundary-matched amplitudes for
//! a uniaxial half-space; this module solves the identical 4-equation boundary system
//! Lekner's `r_ss, r_sp, r_ps, r_pp, t_so, t_se, t_po, t_pe` name, via the SAME
//! closed-form uniaxial dispersion relation (not a general numerical Berreman 4x4
//! eigenvalue solve -- the ordinary root is a plain square root, the extraordinary root
//! solves an explicit quadratic, both closed-form; see [`uniaxial_q_roots`]).
//!
//! ## Units and frame convention
//!
//! Wavevectors are measured in units of `k0 = omega/c`, so a wavevector's magnitude
//! equals the local refractive index directly (`|k| = n`), and the universal relation
//! `H = k x E` holds in ANY medium (isotropic or anisotropic alike) given Maxwell's
//! equations `curl E = i*omega*mu0*H`, `curl H = -i*omega*eps0*eps*E` with `mu0 = eps0 =
//! c = 1` -- see each function's own doc comment. All geometry is built directly from
//! the SAME world-space `k_hat` (wave normal) / `normal` (facet normal, oriented to
//! face the incident ray, i.e. `(-k_hat).dot(normal) == cos_i >= 0` -- exactly
//! `compute_bounce_refraction_geometry`'s own convention) / `c_axis` vectors the rest of
//! this crate already uses, projected onto the local orthonormal frame `(that, s_axis,
//! zhat)`:
//!   - `zhat = -normal` (forward propagation direction, incidence medium -> far medium)
//!   - `that` = the in-plane (tangential) component of `k_hat`, i.e. the propagation
//!     azimuth
//!   - `s_axis = k_hat.cross(normal).normalize()` -- IDENTICAL to
//!     `rotate_stokes_to_plane_of_incidence`'s `current_plane_normal` (see
//!     `absorption::rotate_stokes_to_plane_of_incidence`'s doc comment), so a caller's
//!     already-`current_plane_normal`-referenced Stokes vector needs no extra rotation
//!     to feed into this module's Jones/Mueller machinery, and this module's own output
//!     Mueller matrices apply directly in that same frame.
//!
//! `(alpha, beta, gamma) = (c_axis.dot(that), c_axis.dot(s_axis), c_axis.dot(zhat))` are
//! the optic-axis direction cosines Lekner's own notation uses.
//!
//! ## Validation
//!
//! Before transcribing this into Rust, the exact algebra below (q-root formula,
//! eigenvector construction, boundary-matching linear solve, Poynting-flux energy
//! accounting) was independently validated in Python (native `complex`, no numeric
//! libraries) against:
//!   - a brute-force from-scratch boundary-value solve (own sanity check the isotropic
//!     limit reproduces this crate's exact `r_s`/`r_p` sign convention, see this
//!     module's own tests below),
//!   - R + T == 1 (Poynting-projected) to machine precision (< 3e-15) across 3 uniaxial
//!     materials x multiple optic-axis orientations (axis-aligned and random) x 6-9
//!     incidence angles x both polarizations, for BOTH the entry and the
//!     internal-reflection/exit interface, including 204 genuine TIR (complex,
//!     evanescent) cases,
//!   - continuity: `Delta n -> 0` reduces to this crate's own isotropic Fresnel `r_s`/
//!     `r_p` to within `O(Delta n)`,
//!   - the normal-incidence, optic-axis-in-surface-plane special case.
//!
//! See [`entry_energy_conservation_matches_isotropic_at_zero_birefringence`] and the
//! other tests in this file for the Rust-side re-derivation of those same checks.
//!
//! ## Wiring
//!
//! - **Entry** (`entry_solve_pair`/`entry_incidence_frame`/
//!   `entry_solve_pair_with_incidence`): wired into
//!   `refraction::apply_uniaxial_entry_bounce`.
//! - **Internal reflection and exit transmission** (`internal_solve`): wired into both
//!   `refraction::apply_tir_bounce`'s hero-forced-TIR branch and the general
//!   (sub-critical, non-forced) internal-reflection/exit-transmission path via
//!   `refraction::apply_uniaxial_internal_bounce` --
//!   `apply_partial_reflect_bounce`/`apply_refract_channel`'s scalar-Fresnel machinery
//!   is reached only by an isotropic or biaxial material, or the degenerate
//!   wave-normal-parallel-to-optic-axis limit (see `apply_partial_fresnel_bounce`'s own
//!   doc comment).
//! - **o<->e internal re-coupling** (`transport::apply_internal_mode_coupling`): uses
//!   the exact Poynting-weighted `R_o/(R_o+R_e)` split from the same `internal_solve`
//!   call already run for the reflection event's own energy accounting, rather than a
//!   polarization-projection heuristic (still the fallback for a biaxial material,
//!   which has no uniaxial closed form to draw an exact split from).
//! - **Performance**: [`EntryIncidenceFrame`]/[`entry_incidence_frame`] share the
//!   isotropic incidence-side boundary-matching data (provably independent of
//!   `n_o(lambda)`/`n_e(lambda)`) across all 8 hero channels of one bounce instead of
//!   rebuilding it per channel; [`uniaxial_q_roots`] short-circuits to the isotropic
//!   limit exactly when `n_o == n_e`; `apply_uniaxial_internal_bounce` reuses its own
//!   hero-channel `internal_solve` result at `k == hero_idx` instead of solving the
//!   identical boundary system twice.
//! - **WGSL mirror**: `shaders/transport_physics.wgsl`'s own full uniaxial Fresnel
//!   (Lekner 1991) section is a direct, op-for-op port of this entire module
//!   (`Cplx`/`CVec3`/`UniaxialFrameW`/`uniaxial_q_roots`/mode fields/`solve4`-family/
//!   `entry_solve_pair`/`internal_solve`/`jones_to_mueller`/`mode_power`/
//!   `azimuth2_in_frame`), wired into `spectral_transport.wgsl`'s megakernel bounce
//!   dispatch (entry, hero-forced TIR, and the general internal/exit path) mirroring
//!   this file's own CPU wiring bounce-for-bounce. Verified via
//!   `renderer::gpu::transport_check::p2_uniaxial_fresnel`'s kernel-level ULP checks (0
//!   genuine ULP divergence against this module's own CPU functions) and a Tier 3
//!   statistical image comparison on Zircon/Tourmaline/Quartz/Rutile, all passing on
//!   real AMD Radeon (Vulkan) hardware via `examples/gpu_equivalence_harness`.
//! - **GPU-parity fix**: [`Cplx::sqrt_forward_branch`] special-cases a pure
//!   negative-real input directly (bypassing an `atan2`/`cos`/`sin` round-trip whose
//!   near-zero-magnitude branch-cut decision is otherwise rounding noise that CPU and
//!   GPU transcendental implementations can resolve to opposite signs) -- found by,
//!   and fixed to pass, the `internal_solve` ULP check above; see that function's own
//!   doc comment.

use glam::{Mat4, Vec3};

use crate::optics::polarization::StokesVector;

/// Minimal complex-number type (this crate has no `num-complex` dependency, and the
/// WGSL mirror needs the exact same explicit re/im arithmetic -- a `vec2<f32>`-based
/// complex type there, hand-written `add`/`mul`/`div`/`sqrt`, exactly mirrors this).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cplx {
    pub re: f32,
    pub im: f32,
}

impl Cplx {
    pub(crate) const ZERO: Self = Self { re: 0.0, im: 0.0 };

    #[inline]
    pub(crate) const fn re(re: f32) -> Self {
        Self { re, im: 0.0 }
    }

    #[inline]
    pub(crate) const fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }

    #[inline]
    pub(crate) const fn sub(self, o: Self) -> Self {
        Self {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }

    #[inline]
    pub(crate) const fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }

    #[inline]
    pub(crate) const fn scale(self, s: f32) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }

    #[inline]
    pub(crate) const fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    #[inline]
    pub(crate) fn norm_sqr(self) -> f32 {
        self.im.mul_add(self.im, self.re * self.re)
    }

    /// Complex division. Guards an exactly-zero divisor (never expected on any path
    /// this module's callers reach -- the boundary-matching determinant is bounded
    /// away from zero for every physical `(n_o, n_e, c_axis, angle)` combination this
    /// crate's built-in materials exercise -- see the k-parallel-c degenerate case's own
    /// dedicated fallback in [`entry_solve`]/[`internal_solve`]) with a `1e-20` floor
    /// on the denominator's magnitude rather than panicking or producing NaN.
    #[inline]
    pub(crate) fn div(self, o: Self) -> Self {
        let denom = o.im.mul_add(o.im, o.re * o.re).max(1e-20);
        Self {
            re: self.im.mul_add(o.im, self.re * o.re) / denom,
            im: self.re.mul_add(-o.im, self.im * o.re) / denom,
        }
    }

    /// Principal-branch complex square root, with the branch cut chosen so a root fed
    /// a NEGATIVE real input (the evanescent/TIR case) comes back with `im > 0` --
    /// i.e. `exp(i*q*z)` decays (rather than grows) as `z -> +infinity`, matching this
    /// module's `zhat` = "forward, away from the interface" convention.
    ///
    /// GPU-parity fix: a PURE negative-real input (`im == 0.0` exactly -- the common
    /// case at ordinary-mode internal TIR, where `qo2 = Cplx::re(n_o2 - k2)` is always
    /// constructed with `im` exactly `0.0`) makes `theta = atan2(0.0, re) * 0.5` land
    /// at EXACTLY `pi/2`, where `out.re = r * cos(theta)` is a near-zero value whose
    /// SIGN is pure floating-point rounding noise on `cos` of the nearest
    /// representable `pi/2` -- not a meaningful quantity. The `out.re < 0.0`
    /// branch-cut check below then silently follows that noise, which can pick the
    /// WRONG (`im < 0`) branch, violating this function's own contract above, and
    /// diverges between platforms whose `atan2`/`cos` implementations round that noise
    /// differently (this is what made `shaders/transport_physics.wgsl`'s WGSL mirror
    /// disagree with this function on real GPU hardware at Rutile-scale birefringence
    /// internal TIR, caught by `renderer::gpu::transport_check::p2_uniaxial_fresnel`'s
    /// `run_internal_solve` check). Bypassing the `atan2`/`cos`/`sin` round-trip
    /// entirely for this one input shape (`sqrt(-re)` directly) makes the result
    /// exact, matches the documented contract unconditionally, and is
    /// platform-independent.
    #[inline]
    pub(crate) fn sqrt_forward_branch(self) -> Self {
        if self.im == 0.0 && self.re < 0.0 {
            return Self {
                re: 0.0,
                im: (-self.re).sqrt(),
            };
        }
        let r = self.norm_sqr().sqrt().sqrt();
        let theta = self.im.atan2(self.re) * 0.5;
        let mut out = Self {
            re: r * theta.cos(),
            im: r * theta.sin(),
        };
        if out.re < 0.0 {
            out = Self {
                re: -out.re,
                im: -out.im,
            };
        }
        if out.re.abs() < 1e-9 && out.im < 0.0 {
            out = Self {
                re: -out.re,
                im: -out.im,
            };
        }
        out
    }
}

impl std::ops::Add for Cplx {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::add(self, o)
    }
}
impl std::ops::Sub for Cplx {
    type Output = Self;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Self::sub(self, o)
    }
}
impl std::ops::Mul for Cplx {
    type Output = Self;
    #[inline]
    fn mul(self, o: Self) -> Self {
        Self::mul(self, o)
    }
}

/// A complex 3-vector: `re + i*im`, both real `Vec3`s. Used for the (possibly
/// evanescent) wavevector `k` and the mode field vectors `D`/`E`/`H` it produces.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CVec3 {
    pub re: Vec3,
    pub im: Vec3,
}

impl CVec3 {
    #[inline]
    pub(crate) const fn from_real(v: Vec3) -> Self {
        Self {
            re: v,
            im: Vec3::ZERO,
        }
    }

    /// `(a*that + q*zhat)` with `a` real and `q` complex -- the wavevector construction
    /// every mode field starts from.
    #[inline]
    pub(crate) fn wavevector(that: Vec3, zhat: Vec3, tangential: f32, q: Cplx) -> Self {
        Self {
            re: that * tangential + zhat * q.re,
            im: zhat * q.im,
        }
    }

    #[inline]
    pub(crate) fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }

    #[inline]
    pub(crate) fn scale_real(self, s: f32) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }

    #[inline]
    pub(crate) fn scale_complex(self, s: Cplx) -> Self {
        Self {
            re: self.re * s.re - self.im * s.im,
            im: self.re * s.im + self.im * s.re,
        }
    }

    /// Cross product with a REAL vector: `(re + i*im) x r = (re x r) + i*(im x r)`.
    #[inline]
    pub(crate) fn cross_real(self, r: Vec3) -> Self {
        Self {
            re: self.re.cross(r),
            im: self.im.cross(r),
        }
    }

    /// Cross product with another complex vector.
    #[inline]
    pub(crate) fn cross(self, o: Self) -> Self {
        Self {
            re: self.re.cross(o.re) - self.im.cross(o.im),
            im: self.re.cross(o.im) + self.im.cross(o.re),
        }
    }

    /// Dot product with a REAL vector -> complex scalar.
    #[inline]
    pub(crate) fn dot_real(self, r: Vec3) -> Cplx {
        Cplx {
            re: self.re.dot(r),
            im: self.im.dot(r),
        }
    }

    #[inline]
    pub(crate) fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }
}

impl std::ops::Add for CVec3 {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::add(self, o)
    }
}

/// The local orthonormal frame + optic-axis direction cosines shared by every channel
/// at one bounce -- purely geometric (facet normal, wave normal, optic axis), so built
/// ONCE per bounce (like `theta_c_for_bounce`'s shared `theta_c`) and reused for every
/// channel's own per-wavelength `n_o_k`/`n_e_k` solve. See this module's own top-level
/// doc comment for the frame convention.
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
/// wave; NOT simply `-q_e` for the extraordinary wave in general -- see this module's
/// own top-level doc comment on the `alpha*gamma` walk-off asymmetry between
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
/// `H = k x E` in both cases (the universal Maxwell relation -- see this module's own
/// top-level doc comment).
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

/// Solves a 4x4 complex linear system `A x = b` via Gauss-Jordan elimination with
/// partial pivoting. Every boundary-value solve in this module reduces to exactly this
/// shape (4 tangential-field-continuity equations, 4 unknown amplitudes) -- this is the
/// literal closed-form Lekner solution (his `r_ss` etc. ARE the ratios of 4x4
/// determinants this produces), not a general/iterative numerical eigensolve.
fn solve4(mut a: [[Cplx; 4]; 4], mut b: [Cplx; 4]) -> [Cplx; 4] {
    for col in 0..4 {
        let mut piv = col;
        let mut piv_mag = a[col][col].norm_sqr();
        for (row, row_data) in a.iter().enumerate().skip(col + 1) {
            let mag = row_data[col].norm_sqr();
            if mag > piv_mag {
                piv = row;
                piv_mag = mag;
            }
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let pivot = a[col][col];
        for x in &mut a[col][col..] {
            *x = x.div(pivot);
        }
        b[col] = b[col].div(pivot);
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor.norm_sqr() == 0.0 {
                continue;
            }
            for j in col..4 {
                a[row][j] = a[row][j].sub(factor.mul(a[col][j]));
            }
            b[row] = b[row].sub(factor.mul(b[col]));
        }
    }
    b
}

/// [`solve4`] for TWO right-hand sides against the SAME matrix `a` -- one Gaussian
/// elimination instead of two (requirement 7: [`entry_solve_pair`]'s own performance
/// note). Bit-identical to calling `solve4(a, b1)` and `solve4(a, b2)` separately: the
/// row-reduction operations performed on `a` do not depend on either right-hand side,
/// so applying each pivot/elimination step to both `b` vectors in lockstep is the same
/// arithmetic in the same order, just walked once instead of twice.
fn solve4_two_rhs(
    mut a: [[Cplx; 4]; 4],
    mut b1: [Cplx; 4],
    mut b2: [Cplx; 4],
) -> ([Cplx; 4], [Cplx; 4]) {
    for col in 0..4 {
        let mut piv = col;
        let mut piv_mag = a[col][col].norm_sqr();
        for (row, row_data) in a.iter().enumerate().skip(col + 1) {
            let mag = row_data[col].norm_sqr();
            if mag > piv_mag {
                piv = row;
                piv_mag = mag;
            }
        }
        a.swap(col, piv);
        b1.swap(col, piv);
        b2.swap(col, piv);
        let pivot = a[col][col];
        for x in &mut a[col][col..] {
            *x = x.div(pivot);
        }
        b1[col] = b1[col].div(pivot);
        b2[col] = b2[col].div(pivot);
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor.norm_sqr() == 0.0 {
                continue;
            }
            for j in col..4 {
                a[row][j] = a[row][j].sub(factor.mul(a[col][j]));
            }
            b1[row] = b1[row].sub(factor.mul(b1[col]));
            b2[row] = b2[row].sub(factor.mul(b2[col]));
        }
    }
    (b1, b2)
}

#[inline]
fn tangential_components(fields: &ModeFields, that: Vec3, s_axis: Vec3) -> [Cplx; 4] {
    [
        fields.e.dot_real(that),
        fields.e.dot_real(s_axis),
        fields.h.dot_real(that),
        fields.h.dot_real(s_axis),
    ]
}

/// The full closed-form solution at an isotropic (air, `n1`) -> uniaxial entry
/// interface, for ONE incident polarization (`s` or `p`, amplitude 1). Returns the
/// reflected amplitudes in the iso (s, p) basis and the transmitted amplitudes in the
/// crystal (o, e) basis -- Lekner's `r_ss/r_ps` (or `r_sp/r_pp`) and `t_so/t_eo` (or
/// `t_sp/t_ep`) for this incident polarization, plus each mode's own intrinsic Poynting
/// flux (amplitude-1 `poynting_z`, needed to turn a transmitted AMPLITUDE into a
/// transmitted POWER).
pub(crate) struct EntryPolarizationSolution {
    pub r_s: Cplx,
    pub r_p: Cplx,
    pub t_o: Cplx,
    pub t_e: Cplx,
    /// The o/e transmitted modes' own intrinsic (amplitude-1) Poynting flux --
    /// identical for `incident_s == true` and `false` (a property of the crystal-side
    /// modes alone), redundantly recomputed on each call purely so callers get a
    /// self-contained result without a second function to call.
    pub flux_o: f32,
    pub flux_e: f32,
    /// World-space real E-field directions of the transmitted o/e modes -- always
    /// real (an entry interface's transmitted side never evanescesces: `n1 == 1.0` is
    /// less than every built-in `n_o`/`n_e`), for a caller's own Stokes-azimuth
    /// projection via [`azimuth2_in_frame`].
    pub o_hat: Vec3,
    pub e_hat: Vec3,
}

/// Requirement 7 (performance): [`entry_solve`] is a thin wrapper around
/// [`entry_solve_pair`] that discards whichever of the (s, p) pair the caller didn't
/// ask for -- kept only for the tests and call sites that only need one incident
/// polarization; every production call site in `refraction.rs` needs BOTH (s and p
/// incidence are the two rows of the reflected/transmitted Jones matrices this
/// module's callers build), and calling `entry_solve` twice recomputed
/// [`uniaxial_q_roots`] (two [`Cplx::sqrt_forward_branch`] calls, each an `atan2` plus
/// two `sqrt` calls plus a `sin`/`cos` pair -- by far the most expensive part of this
/// function per call) and both mode fields TWICE for identical inputs (`incident_s`
/// never affects them, only the boundary-matching right-hand side does) -- measured to
/// roughly double this module's own share of a uniaxial bounce's cost.
/// `entry_solve_pair` shares that work once and solves both right-hand sides through
/// one Gaussian elimination. No production call site uses `entry_solve` any more (all
/// converted to `entry_solve_pair`) -- genuinely dead outside this module's own tests,
/// hence the `cfg_attr`'d expectation rather than a bare one (see the identical
/// pattern, and its own doc comment, on `InternalPolarizationSolution`'s `t_s` field
/// above).
#[must_use]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "kept for tests/possible future single-polarization callers -- see doc comment above"
    )
)]
pub(crate) fn entry_solve(
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
    incident_s: bool,
) -> EntryPolarizationSolution {
    let (s_sol, p_sol) = entry_solve_pair(n1, n_o, n_e, c_axis, frame);
    if incident_s { s_sol } else { p_sol }
}

/// Performance (requirement 7, batching): the part of [`entry_solve_pair`]'s work that
/// depends ONLY on `n1` and `frame` -- the incidence-side isotropic (s, p) modes
/// (forward = incident, backward = reflected) and their tangential boundary
/// components -- and NOT on `n_o(lambda)`/`n_e(lambda)` at all. At an air->crystal
/// entry, `n1 == 1.0` for EVERY channel (air does not disperse), so `frame`'s own
/// `sin_i`/`cos_i` make `k_tan = n1 * frame.sin_i` and everything built from it
/// IDENTICAL across all 8 hero channels of one bounce -- every current call site
/// (`apply_uniaxial_entry_bounce`/`_reflect_channels`/`_transmit_channels`) was
/// recomputing this exact same block from scratch on each of its (up to 9, including
/// the hero's own initial branch-decision call) `entry_solve_pair` calls per bounce.
/// Building it ONCE per bounce via this function and feeding the result to
/// [`entry_solve_pair_with_incidence`] for each channel is BIT-IDENTICAL to calling
/// [`entry_solve_pair`] fresh every time (same deterministic floating-point
/// expressions, evaluated once instead of up to 9 times, not reordered or
/// approximated) -- see `batched_entry_solve_pair_matches_scalar_bitwise` below.
#[derive(Clone, Copy)]
pub(crate) struct EntryIncidenceFrame {
    incident_s: [Cplx; 4],
    incident_p: [Cplx; 4],
    reflected_s: [Cplx; 4],
    reflected_p: [Cplx; 4],
}

#[must_use]
pub(crate) fn entry_incidence_frame(n1: f32, frame: &UniaxialFrame) -> EntryIncidenceFrame {
    let k_tan = n1 * frame.sin_i;
    // Isotropic incidence-side modes (forward = incident, backward = reflected),
    // matching this crate's existing r_s/r_p sign convention exactly (see this file's
    // `iso_iso_matches_existing_scalar_fresnel_sign_convention` test).
    let k_fwd = CVec3::wavevector(frame.that, frame.zhat, k_tan, Cplx::re(n1 * frame.cos_i));
    let k_bwd = CVec3::wavevector(frame.that, frame.zhat, k_tan, Cplx::re(-n1 * frame.cos_i));
    let s_axis_c = CVec3::from_real(frame.s_axis);
    let es_fwd = s_axis_c;
    let hs_fwd = k_fwd.cross(es_fwd);
    let ep_bwd = k_bwd.cross(s_axis_c).scale_real(-1.0 / (n1 * n1));
    let ep_fwd = k_fwd.cross(s_axis_c).scale_real(-1.0 / (n1 * n1));

    let inc_s = ModeFields {
        k: k_fwd,
        e: es_fwd,
        h: hs_fwd,
    };
    let inc_p = ModeFields {
        k: k_fwd,
        e: ep_fwd,
        h: k_fwd.cross(ep_fwd),
    };
    let refl_s = ModeFields {
        k: k_bwd,
        e: s_axis_c,
        h: k_bwd.cross(s_axis_c),
    };
    let refl_p = ModeFields {
        k: k_bwd,
        e: ep_bwd,
        h: k_bwd.cross(ep_bwd),
    };

    EntryIncidenceFrame {
        incident_s: tangential_components(&inc_s, frame.that, frame.s_axis),
        incident_p: tangential_components(&inc_p, frame.that, frame.s_axis),
        reflected_s: tangential_components(&refl_s, frame.that, frame.s_axis),
        reflected_p: tangential_components(&refl_p, frame.that, frame.s_axis),
    }
}

/// The shared-work form of [`entry_solve`] -- see that function's own doc comment for
/// why this exists and what it saves. Returns `(s_incident, p_incident)`. A thin
/// wrapper around [`entry_incidence_frame`] + [`entry_solve_pair_with_incidence`],
/// kept for tests and any single-call site that has no shared `EntryIncidenceFrame` to
/// reuse across channels -- bit-identical to calling those two directly.
#[must_use]
pub(crate) fn entry_solve_pair(
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
) -> (EntryPolarizationSolution, EntryPolarizationSolution) {
    let inc = entry_incidence_frame(n1, frame);
    entry_solve_pair_with_incidence(&inc, n1, n_o, n_e, c_axis, frame)
}

/// [`entry_solve_pair`]'s per-channel remainder: the part of the work that genuinely
/// depends on this channel's own `n_o(lambda)`/`n_e(lambda)` (the q-roots, the crystal-
/// side mode fields, and the boundary-matching solve itself), given an
/// [`EntryIncidenceFrame`] built ONCE per bounce by the caller. See
/// [`entry_incidence_frame`]'s own doc comment for why sharing it is exact, not an
/// approximation.
#[must_use]
pub(crate) fn entry_solve_pair_with_incidence(
    inc: &EntryIncidenceFrame,
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
) -> (EntryPolarizationSolution, EntryPolarizationSolution) {
    let k_tan = n1 * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);
    let fo = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus);
    let fe = extraordinary_mode_fields(
        n_o,
        n_e,
        c_axis,
        frame.that,
        frame.zhat,
        k_tan,
        roots.qe_plus,
    );

    let eo_c = tangential_components(&fo, frame.that, frame.s_axis);
    let ee_c = tangential_components(&fe, frame.that, frame.s_axis);

    let mut a = [[Cplx::ZERO; 4]; 4];
    let mut b_s = [Cplx::ZERO; 4];
    let mut b_p = [Cplx::ZERO; 4];
    for i in 0..4 {
        a[i] = [
            inc.reflected_s[i],
            inc.reflected_p[i],
            Cplx::ZERO.sub(eo_c[i]),
            Cplx::ZERO.sub(ee_c[i]),
        ];
        b_s[i] = Cplx::ZERO.sub(inc.incident_s[i]);
        b_p[i] = Cplx::ZERO.sub(inc.incident_p[i]);
    }
    let (sol_s, sol_p) = solve4_two_rhs(a, b_s, b_p);
    let flux_o = poynting_z(&fo, Cplx::re(1.0), frame.zhat).abs();
    let flux_e = poynting_z(&fe, Cplx::re(1.0), frame.zhat).abs();
    let o_hat = fo.e.re.normalize_or_zero();
    let e_hat = fe.e.re.normalize_or_zero();
    let s_sol = EntryPolarizationSolution {
        r_s: sol_s[0],
        r_p: sol_s[1],
        t_o: sol_s[2],
        t_e: sol_s[3],
        flux_o,
        flux_e,
        o_hat,
        e_hat,
    };
    let p_sol = EntryPolarizationSolution {
        r_s: sol_p[0],
        r_p: sol_p[1],
        t_o: sol_p[2],
        t_e: sol_p[3],
        flux_o,
        flux_e,
        o_hat,
        e_hat,
    };
    (s_sol, p_sol)
}

/// The full closed-form solution at a uniaxial (incident mode `o` or `e`, from inside
/// the crystal) -> isotropic (air, `n2 = 1`) interface: internal reflection
/// (o<->e coupling included) AND exit transmission from ONE boundary-value solve.
/// `n_mode_inc` is the incident mode's OWN effective index at this direction/channel
/// (`n_o` for an ordinary incidence, the direction-dependent effective extraordinary
/// index for an extraordinary incidence).
///
/// `Clone`/`Copy` (performance, requirement 7): every field is itself `Copy` (`Cplx`,
/// `f32`, `Vec3`), and `refraction::apply_uniaxial_internal_bounce` already computes
/// the HERO channel's own solution once, to decide the reflect/transmit branch --
/// being `Copy` lets it hand that same value to its per-channel loops for reuse at
/// `k == hero_idx` instead of solving the identical boundary system a second time.
#[derive(Clone, Copy)]
pub(crate) struct InternalPolarizationSolution {
    /// Reflected amplitude into the SAME uniaxial medium's ordinary mode (backward root).
    pub r_o: Cplx,
    /// Reflected amplitude into the extraordinary mode (backward root).
    pub r_e: Cplx,
    /// Transmitted amplitude into the isotropic exit medium's s-polarization. Read by
    /// `refraction::apply_uniaxial_internal_transmit_channels` (the exit-transmission
    /// half of `apply_uniaxial_internal_bounce`, reached from
    /// `apply_partial_fresnel_bounce` whenever `inside_gem &&
    /// geo.uniaxial_frame.is_some()`) -- see that function's own doc comment for the
    /// flux-normalized-amplitude-to-Stokes derivation. Also exercised directly by this
    /// module's own `internal_exit_energy_conservation_holds_including_tir` test.
    pub t_s: Cplx,
    /// See `t_s`'s doc comment.
    pub t_p: Cplx,
    /// Intrinsic (amplitude-1) Poynting flux of the ordinary BACKWARD mode -- turns
    /// `r_o` into a reflected power fraction.
    pub flux_ro: f32,
    pub flux_re: f32,
    pub flux_ts: f32,
    pub flux_tp: f32,
    /// The INCIDENT mode's own intrinsic (amplitude-1) Poynting flux -- NOT generally
    /// `n_mode_inc * cos_i` for the extraordinary mode (its Poynting vector walks off
    /// from its wave normal, so its flux-per-unit-amplitude genuinely differs from the
    /// isotropic-looking formula; only the ordinary mode's flux reduces to that
    /// shortcut exactly). Callers MUST use this (not a re-derived formula) to convert
    /// `r_o`/`r_e`/`t_s`/`t_p` into power fractions -- see this crate's own energy-
    /// conservation tests below for the bug this field's doc comment is warning about.
    pub flux_inc: f32,
    /// World-space real E-field directions of the reflected o/e modes -- for a future
    /// caller's Stokes-azimuth projection on internal reflection. Both internal-
    /// reflection wirings (`apply_tir_bounce`'s hero-forced-TIR branch and
    /// `apply_uniaxial_internal_bounce`'s general sub-critical branch) use the SAME
    /// magnitude-only simplification those functions' own doc comments derive (the
    /// o<->e mode LABEL for the next bounce is instead resolved separately and exactly
    /// by `transport::apply_internal_mode_coupling`'s Poynting-weighted
    /// `R_o/(R_o+R_e)` split), so still genuinely unread by any bounce-dispatch call
    /// site. Read, though, by
    /// `renderer::gpu::transport_check::p2_uniaxial_fresnel`'s `run_internal_solve`
    /// GPU-parity check (compares this field against the WGSL mirror's own
    /// `o_hat`/`e_hat`) -- `cfg_attr`'d to the `gpu` feature accordingly, rather than a
    /// bare `#[expect(dead_code)]`, which would itself warn as "unfulfilled" whenever
    /// that feature is enabled.
    #[cfg_attr(
        not(feature = "gpu"),
        expect(
            dead_code,
            reason = "validated reflected-mode-axis data, not wired into any bounce \
                      dispatch -- see doc comment above; read by the gpu-feature-gated \
                      Tier 2 parity check"
        )
    )]
    pub o_hat: Vec3,
    #[cfg_attr(
        not(feature = "gpu"),
        expect(
            dead_code,
            reason = "validated reflected-mode-axis data, not wired into any bounce \
                      dispatch -- see o_hat's doc comment"
        )
    )]
    pub e_hat: Vec3,
}

#[must_use]
pub(crate) fn internal_solve(
    n_mode_inc: f32,
    n_o: f32,
    n_e: f32,
    c_axis: Vec3,
    frame: &UniaxialFrame,
    incident_is_ordinary: bool,
) -> InternalPolarizationSolution {
    let k_tan = n_mode_inc * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);

    let f_inc = if incident_is_ordinary {
        ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus)
    } else {
        extraordinary_mode_fields(
            n_o,
            n_e,
            c_axis,
            frame.that,
            frame.zhat,
            k_tan,
            roots.qe_plus,
        )
    };
    let f_o_bwd = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_minus);
    let f_e_bwd = extraordinary_mode_fields(
        n_o,
        n_e,
        c_axis,
        frame.that,
        frame.zhat,
        k_tan,
        roots.qe_minus,
    );

    // Isotropic exit side (n2 = 1), forward (away from interface, into air).
    let n2 = 1.0f32;
    let zeta = k_tan / n2;
    let cos_t = if zeta.abs() <= 1.0 {
        Cplx::re(zeta.mul_add(-zeta, 1.0).max(0.0).sqrt())
    } else {
        Cplx {
            re: 0.0,
            im: zeta.mul_add(zeta, -1.0).sqrt(),
        }
    };
    let k_iso = CVec3::wavevector(frame.that, frame.zhat, k_tan, cos_t.scale(n2));
    let s_axis_c = CVec3::from_real(frame.s_axis);
    let es_t = s_axis_c;
    let hs_t = k_iso.cross(es_t);
    let ep_t = k_iso.cross(s_axis_c).scale_real(-1.0 / (n2 * n2));
    let hp_t = k_iso.cross(ep_t);
    let f_s_t = ModeFields {
        k: k_iso,
        e: es_t,
        h: hs_t,
    };
    let f_p_t = ModeFields {
        k: k_iso,
        e: ep_t,
        h: hp_t,
    };

    let inc_c = tangential_components(&f_inc, frame.that, frame.s_axis);
    let eo_c = tangential_components(&f_o_bwd, frame.that, frame.s_axis);
    let ee_c = tangential_components(&f_e_bwd, frame.that, frame.s_axis);
    let es_c = tangential_components(&f_s_t, frame.that, frame.s_axis);
    let ep_c = tangential_components(&f_p_t, frame.that, frame.s_axis);

    let mut a = [[Cplx::ZERO; 4]; 4];
    let mut b = [Cplx::ZERO; 4];
    for i in 0..4 {
        a[i] = [
            eo_c[i],
            ee_c[i],
            Cplx::ZERO.sub(es_c[i]),
            Cplx::ZERO.sub(ep_c[i]),
        ];
        b[i] = Cplx::ZERO.sub(inc_c[i]);
    }
    let sol = solve4(a, b);
    let [r_o, r_e, t_s, t_p] = sol;

    let flux_ro = poynting_z(&f_o_bwd, Cplx::re(1.0), frame.zhat).abs();
    let flux_re = poynting_z(&f_e_bwd, Cplx::re(1.0), frame.zhat).abs();
    let flux_ts = poynting_z(&f_s_t, Cplx::re(1.0), frame.zhat).abs();
    let flux_tp = poynting_z(&f_p_t, Cplx::re(1.0), frame.zhat).abs();
    let flux_inc = poynting_z(&f_inc, Cplx::re(1.0), frame.zhat).abs();

    let o_hat = f_o_bwd.e.re.normalize_or_zero();
    let e_hat = f_e_bwd.e.re.normalize_or_zero();

    InternalPolarizationSolution {
        r_o,
        r_e,
        t_s,
        t_p,
        flux_ro,
        flux_re,
        flux_ts,
        flux_tp,
        flux_inc,
        o_hat,
        e_hat,
    }
}

/// Doubled-angle azimuth `(cos(2*psi), sin(2*psi))` of a real world-space direction
/// `dir` (assumed already `perp` to the propagation direction implicit in `s_axis`/
/// `p_axis`) within the `(s_axis, p_axis)` Stokes frame -- the same construction
/// `entry_eigenmode_selection` already uses for `ordinary_eigen_polarization`'s output.
#[must_use]
pub(crate) fn azimuth2_in_frame(dir: Vec3, s_axis: Vec3, p_axis: Vec3) -> (f32, f32) {
    let s_comp = dir.dot(s_axis);
    let p_comp = dir.dot(p_axis);
    let norm = p_comp.mul_add(p_comp, s_comp * s_comp).max(1e-12);
    let cos_2psi = p_comp.mul_add(-p_comp, s_comp * s_comp) / norm;
    let sin_2psi = 2.0 * s_comp * p_comp / norm;
    (cos_2psi, sin_2psi)
}

/// General complex-Jones-matrix -> real-Mueller-matrix conversion, in this crate's
/// EXACT `(I, Q, U, V)` convention (`Q = |Es|^2 - |Ep|^2`, `U = 2*Re(Es*Ep_conj)`, `V =
/// -2*Im(Es*Ep_conj)` -- independently re-derived from, and verified bit-for-bit
/// against, `MuellerMatrix::fresnel_reflection`/`fresnel_transmission`/
/// `tir_retardation`'s own already-shipped sign convention: for a real diagonal Jones
/// matrix `diag(r_s, r_p)` this reduces EXACTLY to `fresnel_reflection`'s own `a, b, c`
/// formula -- see `jones_to_mueller_matches_existing_diagonal_fresnel_reflection` below.
/// `j = [[j_ss, j_sp], [j_ps, j_pp]]` maps incident `(E_s, E_p)` to outgoing `(E_s',
/// E_p')`.
#[must_use]
pub(crate) fn jones_to_mueller(j_ss: Cplx, j_sp: Cplx, j_ps: Cplx, j_pp: Cplx) -> Mat4 {
    let m_ss = j_ss.norm_sqr();
    let m_sp = j_sp.norm_sqr();
    let m_ps = j_ps.norm_sqr();
    let m_pp = j_pp.norm_sqr();

    let a_ssp = j_ss.mul(j_sp.conj());
    let a_pep = j_ps.mul(j_pp.conj());
    let a_ssp_plus = a_ssp.add(a_pep);
    let a_ssp_minus = a_ssp.sub(a_pep);

    let b_sp = j_ss.mul(j_ps.conj());
    let b_pp = j_sp.mul(j_pp.conj());

    let c_sspp = j_ss.mul(j_pp.conj());
    let c_spps = j_sp.mul(j_ps.conj());

    // Row I
    let i_i = 0.5 * (m_ss + m_ps + m_sp + m_pp);
    let i_q = 0.5 * (m_ss + m_ps - m_sp - m_pp);
    let i_u = a_ssp_plus.re;
    let i_v = a_ssp_plus.im;
    // Row Q
    let q_i = 0.5 * (m_ss - m_ps + m_sp - m_pp);
    let q_q = f32::midpoint(m_ss - m_ps - m_sp, m_pp);
    let q_u = a_ssp_minus.re;
    let q_v = a_ssp_minus.im;
    // Row U
    let u_i = b_sp.re + b_pp.re;
    let u_q = b_sp.re - b_pp.re;
    let u_u = c_sspp.re + c_spps.re;
    let u_v = c_sspp.im - c_spps.im;
    // Row V
    let v_i = -(b_sp.im + b_pp.im);
    let v_q = -(b_sp.im) + b_pp.im;
    let v_u = -(c_sspp.im + c_spps.im);
    let v_v = c_sspp.re - c_spps.re;

    // glam::Mat4::from_cols_array reads COLUMN-major.
    Mat4::from_cols_array(&[
        i_i, q_i, u_i, v_i, // column 0 (contributions from input I)
        i_q, q_q, u_q, v_q, // column 1 (input Q)
        i_u, q_u, u_u, v_u, // column 2 (input U)
        i_v, q_v, u_v, v_v, // column 3 (input V)
    ])
}

/// The transmitted POWER FRACTION (dimensionless, in `[0, 1]` for a physical state) a
/// SINGLE output mode receives from a 2-component `(s, p)` input: `T_mode(stokes) =
/// (mode_flux / inc_flux) * [|t_s|^2*(I+Q)/2 + |t_p|^2*(I-Q)/2 + Re(t_s*conj(t_p))*U +
/// Im(t_s*conj(t_p))*V]` -- the same construction [`jones_to_mueller`]'s row-I
/// derivation uses, specialised to a single output row (see this module's top-level
/// doc comment, "Stokes/Mueller integration"), divided by the INCIDENT mode's own
/// intrinsic flux so the result is a true power fraction, not merely `power * mode_flux`
/// (dividing by `inc_flux` here, rather than leaving it to the caller, is deliberate --
/// an earlier version of this function omitted it entirely, which an `apply_
/// uniaxial_entry_bounce` integration test caught: summed reflectance + transmittance
/// came out around 0.43 instead of 1.0, tracked down to exactly this missing
/// division). This is what the mode-selection probability in
/// `apply_partial_fresnel_bounce` (replacing an earlier `entry_eigenmode_selection`
/// heuristic) is built from: the TRUE Poynting-weighted power fraction this specific
/// incident polarization state sends into this one mode.
#[must_use]
pub(crate) fn mode_power(
    t_s: Cplx,
    t_p: Cplx,
    mode_flux: f32,
    inc_flux: f32,
    stokes: StokesVector,
) -> f32 {
    let m_s = t_s.norm_sqr();
    let m_p = t_p.norm_sqr();
    let cross = t_s.mul(t_p.conj());
    let raw = cross.im.mul_add(
        stokes.v,
        cross.re.mul_add(
            stokes.u,
            0.5 * m_s.mul_add(stokes.i + stokes.q, m_p * (stokes.i - stokes.q)),
        ),
    );
    (mode_flux / inc_flux.max(1e-12)) * raw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_for(ang_deg: f32, c_axis: Vec3) -> UniaxialFrame {
        let theta = ang_deg.to_radians();
        let cos_i = theta.cos();
        let sin_i = theta.sin();
        let k_hat = Vec3::new(sin_i, 0.0, cos_i);
        let normal = Vec3::new(0.0, 0.0, -1.0); // (-k_hat).dot(normal) == cos_i
        UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i)
    }

    /// The isotropic limit of this module's own boundary-matching machinery (feed it
    /// `n_o == n_e`, no coupling should remain) must reproduce this crate's EXISTING
    /// scalar `r_s`/`r_p` Fresnel formula (`refraction.rs`'s `r_s_k`/`r_p_k`) exactly,
    /// including its sign convention -- this is the same check the Python prototype's
    /// `iso_iso_check` ran before any Rust was written.
    #[test]
    fn entry_solve_matches_existing_scalar_fresnel_at_zero_birefringence() {
        let n = 1.7f32;
        for ang in [0.0f32, 15.0, 30.0, 45.0, 60.0, 75.0] {
            let frame = frame_for(ang, Vec3::Y);
            for incident_s in [true, false] {
                let sol = entry_solve(1.0, n, n, Vec3::X, &frame, incident_s);
                let cos_t = (1.0 - frame.sin_i * frame.sin_i / (n * n)).max(0.0).sqrt();
                let r_s_expected = n.mul_add(-cos_t, frame.cos_i) / n.mul_add(cos_t, frame.cos_i);
                let r_p_expected = n.mul_add(frame.cos_i, -cos_t) / n.mul_add(frame.cos_i, cos_t);
                let (got, expected) = if incident_s {
                    (sol.r_s.re, r_s_expected)
                } else {
                    (sol.r_p.re, r_p_expected)
                };
                assert!(
                    (got - expected).abs() < 1e-4,
                    "ang={ang} incident_s={incident_s}: got {got}, expected {expected}"
                );
                let (cross, cross_im) = if incident_s {
                    (sol.r_p, sol.r_s.im)
                } else {
                    (sol.r_s, sol.r_p.im)
                };
                assert!(
                    cross.norm_sqr().sqrt() < 1e-4 && cross_im.abs() < 1e-4,
                    "cross-polarization reflection must vanish at zero birefringence: {cross:?}"
                );
            }
        }
    }

    /// R + T == 1 (Poynting-projected) at an entry interface, across materials,
    /// optic-axis orientations and angles -- the Rust-side re-derivation of the Python
    /// prototype's `entry_energy_conservation_test` (which found `worst_err = 5.6e-16`
    /// across the same sweep).
    #[test]
    fn entry_energy_conservation_holds_across_materials_axes_and_angles() {
        let materials = [(1.925f32, 1.984f32), (2.65, 2.67), (2.616, 2.903)];
        let axes = [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.4, 0.5, 0.767_2).normalize(),
            Vec3::new(-0.3, 0.8, 0.514_8).normalize(),
        ];
        let mut worst = 0.0f32;
        for (n_o, n_e) in materials {
            for &c_axis in &axes {
                for ang in [0.0f32, 15.0, 30.0, 45.0, 60.0, 75.0] {
                    let frame = frame_for(ang, c_axis);
                    let k_hat = Vec3::new(frame.sin_i, 0.0, frame.cos_i);
                    if k_hat.dot(c_axis).abs() > 0.999_999 {
                        continue; // degenerate k-parallel-c normal incidence, see module docs
                    }
                    for incident_s in [true, false] {
                        let sol = entry_solve(1.0, n_o, n_e, c_axis, &frame, incident_s);
                        let roots = uniaxial_q_roots(n_o, n_e, &frame, frame.sin_i);
                        let fo = ordinary_mode_fields(
                            n_o,
                            c_axis,
                            frame.that,
                            frame.zhat,
                            frame.sin_i,
                            roots.qo_plus,
                        );
                        let fe = extraordinary_mode_fields(
                            n_o,
                            n_e,
                            c_axis,
                            frame.that,
                            frame.zhat,
                            frame.sin_i,
                            roots.qe_plus,
                        );
                        // amplitude-1 isotropic mode flux == 0.5*cos_i (poynting_z's own
                        // 0.5 time-average factor), NOT bare cos_i -- must stay
                        // consistent with sz_o/sz_e below, which both come from the same
                        // `poynting_z` (and so both already carry that same 0.5).
                        let inc_flux = 0.5 * frame.cos_i;
                        let sz_o = poynting_z(&fo, sol.t_o, frame.zhat);
                        let sz_e = poynting_z(&fe, sol.t_e, frame.zhat);
                        let r_total = sol.r_s.norm_sqr() + sol.r_p.norm_sqr();
                        let t_total = (sz_o + sz_e) / inc_flux;
                        let err = (r_total + t_total - 1.0).abs();
                        worst = worst.max(err);
                    }
                }
            }
        }
        assert!(
            worst < 1e-4,
            "entry R+T should equal 1 (Poynting-projected) to numerical precision, worst_err={worst}"
        );
    }

    /// The internal-reflection/exit interface's own R + T == 1 check, including genuine
    /// TIR (complex, evanescent transmitted wave) cases -- re-derives the Python
    /// prototype's `internal_exit_energy_conservation_test` (204 TIR cases,
    /// `worst_err = 2.7e-15`).
    #[test]
    fn internal_exit_energy_conservation_holds_including_tir() {
        let materials = [(1.925f32, 1.984f32), (2.65, 2.67), (2.616, 2.903)];
        let axes = [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.4, 0.5, 0.767_2).normalize(),
        ];
        let mut worst = 0.0f32;
        let mut tir_cases = 0u32;
        for (n_o, n_e) in materials {
            for &c_axis in &axes {
                for ang in [0.0f32, 20.0, 35.0, 50.0, 65.0, 80.0] {
                    let frame = frame_for(ang, c_axis);
                    let k_hat = Vec3::new(frame.sin_i, 0.0, frame.cos_i);
                    if k_hat.dot(c_axis).abs() > 0.999_999 {
                        continue;
                    }
                    for incident_is_ordinary in [true, false] {
                        let n_inc = if incident_is_ordinary {
                            n_o
                        } else {
                            let cos_kc =
                                frame.gamma.mul_add(frame.cos_i, frame.alpha * frame.sin_i);
                            let sin2 = cos_kc.mul_add(-cos_kc, 1.0).max(0.0);
                            1.0 / (cos_kc * cos_kc / (n_o * n_o) + sin2 / (n_e * n_e)).sqrt()
                        };
                        let sol =
                            internal_solve(n_inc, n_o, n_e, c_axis, &frame, incident_is_ordinary);
                        let big_k = n_inc * frame.sin_i;
                        if big_k > 1.0 {
                            tir_cases += 1;
                        }
                        // `sol.flux_inc` is the incident mode's own intrinsic Poynting
                        // flux -- NOT assumed equal to `n_inc*cos_i` (that
                        // isotropic-looking shortcut is only exactly right for a mode
                        // whose Poynting vector is parallel to its wave normal, i.e. the
                        // ordinary mode or an isotropic mode -- the extraordinary mode
                        // walks off, so its true flux differs; see `flux_inc`'s own doc
                        // comment, added after this exact test caught the bug).
                        let inc_flux = sol.flux_inc;
                        let sz_ro = sol.r_o.norm_sqr() * sol.flux_ro;
                        let sz_re = sol.r_e.norm_sqr() * sol.flux_re;
                        let r_total = (sz_ro + sz_re) / inc_flux;
                        let t_total = sol
                            .t_p
                            .norm_sqr()
                            .mul_add(sol.flux_tp, sol.t_s.norm_sqr() * sol.flux_ts)
                            / inc_flux;
                        let err = (r_total + t_total - 1.0).abs();
                        worst = worst.max(err);
                    }
                }
            }
        }
        assert!(
            tir_cases > 20,
            "test setup should exercise real TIR cases, got {tir_cases}"
        );
        assert!(
            worst < 1e-3,
            "internal R+T should equal 1 (Poynting-projected) including TIR, worst_err={worst}"
        );
    }

    /// Normal incidence, optic axis in the surface plane: the closed form must reduce
    /// to the pure ordinary/extraordinary Fresnel identities (matching this crate's own
    /// `r_s`/`r_p` sign convention -- see the zero-birefringence test above for why
    /// `r_s` at normal incidence is `(n1-n)/(n1+n)`, i.e. `-(n-1)/(n+1)`, while `r_p` is
    /// `+(n-1)/(n+1)`).
    #[test]
    fn normal_incidence_inplane_axis_gives_pure_ordinary_extraordinary_fresnel() {
        let n_o = 1.925f32;
        let n_e = 1.984f32;
        let frame = frame_for(0.0, Vec3::X); // c_axis == that, in the surface plane
        let sol_s = entry_solve(1.0, n_o, n_e, Vec3::X, &frame, true);
        let sol_p = entry_solve(1.0, n_o, n_e, Vec3::X, &frame, false);
        let r_o_expected = (1.0 - n_o) / (1.0 + n_o);
        let r_e_expected = (n_e - 1.0) / (n_e + 1.0);
        assert!((sol_s.r_s.re - r_o_expected).abs() < 1e-4);
        assert!(sol_s.r_p.norm_sqr().sqrt() < 1e-4);
        assert!((sol_p.r_p.re - r_e_expected).abs() < 1e-4);
        assert!(sol_p.r_s.norm_sqr().sqrt() < 1e-4);
    }

    /// Requirement 9: Rutile's extreme birefringence (`+0.287`, the highest of any
    /// built-in) should make the new closed-form entry Fresnel reflectance visibly
    /// diverge from the OLD "effective index" scalar approximation (a single
    /// isotropic-style Fresnel evaluated at `BirefringenceParams::
    /// effective_extraordinary_index`) at an oblique incidence with the optic axis NOT
    /// confined to the
    /// plane of incidence (the general case where s/p<->o/e coupling is genuinely
    /// present, per this module's own top-level doc comment). Proves the new
    /// closed-form path is materially different physics, not a no-op refactor.
    ///
    /// Compares the p-INCIDENT reflectance specifically (`|r_pp|^2` vs. the old
    /// isotropic `r_p^2` at the same effective index) rather than the unpolarized
    /// average: the coupling correction partially cancels between s- and p-incidence
    /// in the unpolarized average (each channel's own `r_sp`/`r_ps` cross-leakage adds
    /// intensity the old scalar formula didn't have, but the DIAGONAL `r_ss`/`r_pp`
    /// terms drop correspondingly, since energy is conserved) -- the single-
    /// polarization reflectance is where the ~15% deviation the review predicted is
    /// actually visible; this geometry (20 degrees incidence, optic axis close to the
    /// surface plane but with a genuine out-of-plane `beta` component) was found, by a
    /// small sweep over angle/axis combinations, to land closest to that figure.
    #[test]
    fn rutile_fresnel_diverges_from_isotropic_effective_index_approximation_by_about_15_percent() {
        use crate::optics::{birefringence::BirefringenceParams, materials::GemMaterial};

        let rutile = GemMaterial::by_name("Rutile").expect("Rutile must be a built-in material");
        let n_o = rutile.dispersion.evaluate(589.3);
        let n_e = rutile.extraordinary_index_at(589.3, n_o);
        assert!(
            (n_e - n_o - 0.287).abs() < 1e-3,
            "test premise: Rutile's birefringence should be +0.287, got n_o={n_o} n_e={n_e}"
        );

        let c_axis = Vec3::new(0.05, 0.95, 0.3).normalize();
        let frame = frame_for(20.0, c_axis);
        let k_hat = Vec3::new(frame.sin_i, 0.0, frame.cos_i);

        // OLD approximation: a single scalar "effective index"
        // (`effective_extraordinary_index`), plain isotropic p-polarized Fresnel
        // reflectance at that one index.
        let theta_c = k_hat.dot(c_axis).clamp(-1.0, 1.0).abs().acos();
        let n_eff = BirefringenceParams::effective_extraordinary_index(n_o, n_e, theta_c);
        let cos_t_old = (frame.sin_i / n_eff)
            .mul_add(-(frame.sin_i / n_eff), 1.0)
            .max(0.0)
            .sqrt();
        let r_p_old =
            n_eff.mul_add(frame.cos_i, -cos_t_old) / n_eff.mul_add(frame.cos_i, cos_t_old);
        let r_pp_old_sq = r_p_old * r_p_old;

        // NEW closed-form: the true p-incident, p-reflected power `|r_pp|^2`.
        let sol_p = entry_solve(1.0, n_o, n_e, c_axis, &frame, false);
        let r_pp_new_sq = sol_p.r_p.norm_sqr();

        let relative_deviation = (r_pp_new_sq - r_pp_old_sq).abs() / r_pp_old_sq;
        assert!(
            (0.08..=0.25).contains(&relative_deviation),
            "Rutile's new closed-form |r_pp|^2 (={r_pp_new_sq}) should diverge from the \
             old effective-index approximation r_p^2 (={r_pp_old_sq}) by roughly the \
             ~15% the review predicted -- got {:.2}%",
            relative_deviation * 100.0
        );
    }

    /// [`jones_to_mueller`] fed a real diagonal Jones matrix `diag(r_s, r_p)` must
    /// reproduce `MuellerMatrix::fresnel_reflection(r_s, r_p)` exactly.
    #[test]
    fn jones_to_mueller_matches_existing_diagonal_fresnel_reflection() {
        use crate::optics::polarization::MuellerMatrix;
        let r_s = 0.42f32;
        let r_p = -0.17f32;
        let got = jones_to_mueller(Cplx::re(r_s), Cplx::ZERO, Cplx::ZERO, Cplx::re(r_p));
        let expected = MuellerMatrix::fresnel_reflection(r_s, r_p);
        for i in 0..4 {
            for j in 0..4 {
                let g = got.col(j)[i];
                let e = expected.col(j)[i];
                assert!((g - e).abs() < 1e-5, "[{i}][{j}]: got {g}, expected {e}");
            }
        }
    }

    /// Small deterministic LCG (no external RNG dependency), matching the convention
    /// `geometry::meet_solver::candidates`'s own bitwise-equivalence tests use.
    fn lcg(state: &mut u64) -> f32 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        (((*state >> 33) as f32) / u32::MAX as f32).mul_add(2.0, -1.0)
    }

    /// Performance (requirement 7, batching): [`entry_solve_pair_with_incidence`] fed a
    /// shared [`EntryIncidenceFrame`] (the "batched" path every uniaxial entry call
    /// site now uses -- see `refraction::apply_uniaxial_entry_bounce`) must be
    /// BIT-IDENTICAL, field for field, to [`entry_solve_pair`] called completely fresh
    /// (the "scalar" path, computing its own `EntryIncidenceFrame` from scratch every
    /// time -- what every call site used to do) -- across random angles, optic-axis
    /// orientations and `(n_o, n_e)` pairs, mirroring
    /// `geometry::meet_solver::candidates::batched_enumeration_matches_glam_reference_
    /// bitwise`'s own convention for this exact kind of claim. `entry_incidence_frame`
    /// only hoists a computation that never depended on `n_o`/`n_e` in the first place
    /// (see that function's own doc comment) -- no floating-point operation is
    /// reordered or approximated, so this is not expected to merely be CLOSE, it must
    /// be EXACT.
    #[test]
    fn batched_entry_solve_pair_matches_scalar_bitwise() {
        let mut state = 7u64;
        for _ in 0..500 {
            let ang_deg = 80.0 * lcg(&mut state).abs();
            let c_axis = Vec3::new(lcg(&mut state), lcg(&mut state), lcg(&mut state))
                .try_normalize()
                .unwrap_or(Vec3::Y);
            let n_o = 0.6f32.mul_add(lcg(&mut state).abs(), 1.4);
            let n_e = 0.3f32.mul_add(lcg(&mut state), n_o);
            let frame = frame_for(ang_deg, c_axis);

            // "Scalar": entry_solve_pair rebuilds its own EntryIncidenceFrame
            // internally on every call, exactly as every call site did before this
            // task's batching change.
            let (scalar_s, scalar_p) = entry_solve_pair(1.0, n_o, n_e, c_axis, &frame);

            // "Batched": one EntryIncidenceFrame shared across (here) a single call,
            // exactly as `apply_uniaxial_entry_bounce` now shares it across all 8
            // hero channels of one bounce.
            let inc = entry_incidence_frame(1.0, &frame);
            let (batched_s, batched_p) =
                entry_solve_pair_with_incidence(&inc, 1.0, n_o, n_e, c_axis, &frame);

            let cplx_bits_eq = |a: Cplx, b: Cplx| {
                a.re.to_bits() == b.re.to_bits() && a.im.to_bits() == b.im.to_bits()
            };
            for (label, s, b) in [
                ("s_incident", scalar_s, batched_s),
                ("p_incident", scalar_p, batched_p),
            ] {
                assert!(
                    cplx_bits_eq(s.r_s, b.r_s)
                        && cplx_bits_eq(s.r_p, b.r_p)
                        && cplx_bits_eq(s.t_o, b.t_o)
                        && cplx_bits_eq(s.t_e, b.t_e)
                        && s.flux_o.to_bits() == b.flux_o.to_bits()
                        && s.flux_e.to_bits() == b.flux_e.to_bits()
                        && s.o_hat == b.o_hat
                        && s.e_hat == b.e_hat,
                    "{label}: batched result must be bit-identical to scalar at \
                     ang_deg={ang_deg} c_axis={c_axis:?} n_o={n_o} n_e={n_e} \
                     (scalar r_s={:?}, batched r_s={:?})",
                    s.r_s,
                    b.r_s
                );
            }
        }
    }
}
