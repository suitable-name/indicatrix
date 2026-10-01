//! The genuinely biaxial optical indicatrix [`BiaxialIndicatrix`] (Fresnel's
//! equation of the wave normals, eigenvector extraction, walk-off) and the
//! general (uniaxial-or-biaxial) Poynting-direction formula it builds on.

use super::{
    helpers::{canonicalize_eigenvector_sign, cross_fma, dot_fma, stable_orthonormal_basis},
    uniaxial::BirefringenceParams,
};
use glam::{Mat3, Vec3};

/// A biaxial crystal's optical indicatrix.
///
/// Three principal refractive indices `n_alpha <= n_beta <= n_gamma` and the
/// orthonormal frame (world/model space) they are defined along (`axes` columns 0, 1,
/// 2 correspond to alpha, beta, gamma respectively). Uniaxial crystals are the special
/// case `n_alpha == n_beta` (positive) or `n_beta == n_gamma` (negative); isotropic is
/// all three equal.
#[derive(Debug, Clone, Copy)]
pub struct BiaxialIndicatrix {
    /// Smallest principal refractive index.
    pub n_alpha: f32,
    /// Intermediate principal refractive index.
    pub n_beta: f32,
    /// Largest principal refractive index.
    pub n_gamma: f32,
    /// Columns are the principal axes for alpha, beta and gamma.
    pub axes: Mat3,
}

impl BiaxialIndicatrix {
    /// Builds an indicatrix from its three principal indices and axis frame directly,
    /// with no ordering or orthonormality check on `axes`.
    #[must_use]
    pub const fn new(n_alpha: f32, n_beta: f32, n_gamma: f32, axes: Mat3) -> Self {
        Self {
            n_alpha,
            n_beta,
            n_gamma,
            axes,
        }
    }

    /// Convenience constructor for the uniaxial special case (`n_alpha == n_beta ==
    /// n_o`, `n_gamma == n_e`), used by tests to pin the biaxial equation's reduction
    /// to the existing uniaxial formula (`BirefringenceParams::effective_extraordinary_index`).
    #[must_use]
    pub fn uniaxial(n_o: f32, n_e: f32, c_axis: Vec3) -> Self {
        let c = c_axis.normalize_or_zero();
        let (a1, a2) = stable_orthonormal_basis(c);
        Self::new(n_o, n_o, n_e, Mat3::from_cols(a1, a2, c))
    }

    /// Builds a genuinely biaxial indicatrix from the three principal indices plus a
    /// single reference axis (`gamma_axis`, the `n_gamma` principal direction): the
    /// other two principal axes are an arbitrary orthonormal completion via
    /// `stable_orthonormal_basis` -- the same placeholder-orientation convention used
    /// for every uniaxial material's `c_axis` (it does not model a gem's
    /// crystallographic orientation relative to its cut). See
    /// `materials::GemMaterial::biaxial_indicatrix` for the built-in Alexandrite, Topaz
    /// and Tanzanite entries that feed this.
    #[must_use]
    pub fn from_gamma_axis(n_alpha: f32, n_beta: f32, n_gamma: f32, gamma_axis: Vec3) -> Self {
        let g = gamma_axis.normalize_or_zero();
        let (a1, a2) = stable_orthonormal_basis(g);
        Self::new(n_alpha, n_beta, n_gamma, Mat3::from_cols(a1, a2, g))
    }

    /// `1/n_i^2` for each principal index, in axis order -- the natural variable
    /// Fresnel's equation of the wave normals is quadratic in.
    pub(super) fn b_coeffs(&self) -> [f32; 3] {
        [
            1.0 / (self.n_alpha * self.n_alpha),
            1.0 / (self.n_beta * self.n_beta),
            1.0 / (self.n_gamma * self.n_gamma),
        ]
    }

    /// Fresnel's equation of the wave normals (Born & Wolf, *Principles of Optics*,
    /// Sec. 14.2; Hecht, *Optics*, the "normal ellipsoid" construction), giving the two
    /// refractive indices allowed for a wave whose unit wave-normal direction is
    /// `wave_normal` (world space). Closed-form: with `(a, b, g)` the direction
    /// cosines of `wave_normal` in the principal frame and `b_i = 1/n_i^2`, the
    /// equation
    ///   a^2 (x-b_beta)(x-b_gamma) + b^2 (x-b_alpha)(x-b_gamma) + g^2 (x-b_alpha)(x-b_beta) = 0
    /// (x = 1/n^2) expands to a plain quadratic `x^2 - B*x + C = 0` (the `x^2`
    /// coefficient is `a^2+b^2+g^2 = 1`), solved directly via the quadratic formula --
    /// no iteration. Returns `(n_slow, n_fast)` with `n_slow >= n_fast`.
    ///
    /// Reduces exactly to the uniaxial formula when `n_alpha == n_beta`: substituting
    /// `b_alpha = b_beta = b_o` collapses the equation to `x = b_o` for every direction
    /// and, via the roots' product `x_a * x_b = C`, the other root to
    /// `sin^2(theta)*b_e + cos^2(theta)*b_o` -- algebraically identical to
    /// `BirefringenceParams::effective_extraordinary_index`.
    ///
    /// Near/exact degeneracy between two (or three) principal indices is detected up
    /// front via `indices_are_degenerate` and short-circuited to the exact closed forms
    /// below rather than run through the general quadratic: as the discriminant
    /// `big_b^2 - 4*big_c` approaches zero, evaluating it becomes a subtraction of two
    /// nearly-equal `f32` values, and the resulting catastrophic cancellation (amplified
    /// by the subsequent `sqrt`) is what `indices_are_degenerate`'s ~3.5e-4 relative
    /// tolerance guards against. Isotropic materials skip the solve entirely; uniaxial
    /// ones (most gems in this crate's catalogue) reuse
    /// `BirefringenceParams::effective_extraordinary_index` rather than duplicating its
    /// algebra.
    #[must_use]
    pub fn wave_indices(&self, wave_normal: Vec3) -> (f32, f32) {
        let k = wave_normal.normalize_or_zero();

        let alpha_beta_degenerate = Self::indices_are_degenerate(self.n_alpha, self.n_beta);
        let beta_gamma_degenerate = Self::indices_are_degenerate(self.n_beta, self.n_gamma);

        if alpha_beta_degenerate && beta_gamma_degenerate {
            // Isotropic: all three principal indices agree, so every direction gives
            // exactly that index for both roots -- no quadratic (and no sqrt) needed.
            let n = (self.n_alpha + self.n_beta + self.n_gamma) / 3.0;
            return (n, n);
        }
        if alpha_beta_degenerate {
            // Positive-uniaxial degeneracy: n_alpha == n_beta == n_o, n_gamma == n_e,
            // optic axis along principal axis 2 (gamma).
            return Self::uniaxial_wave_indices(
                k,
                self.axes.z_axis,
                f32::midpoint(self.n_alpha, self.n_beta),
                self.n_gamma,
            );
        }
        if beta_gamma_degenerate {
            // Negative-uniaxial degeneracy: n_beta == n_gamma == n_o, n_alpha == n_e,
            // optic axis along principal axis 0 (alpha).
            return Self::uniaxial_wave_indices(
                k,
                self.axes.x_axis,
                f32::midpoint(self.n_beta, self.n_gamma),
                self.n_alpha,
            );
        }

        // Genuinely biaxial (no two principal indices numerically degenerate): the
        // general quadratic solve is safe here since the discriminant stays well
        // clear of zero.
        let local = self.axes.transpose() * k;
        let (a2, b2, g2) = (local.x * local.x, local.y * local.y, local.z * local.z);
        let [b1, b2c, b3] = self.b_coeffs();

        let big_b = a2.mul_add(b2c + b3, b2.mul_add(b1 + b3, g2 * (b1 + b2c)));
        let big_c = a2.mul_add(b2c * b3, b2.mul_add(b1 * b3, g2 * b1 * b2c));
        let disc = big_b.mul_add(big_b, -4.0 * big_c).max(0.0).sqrt();

        let x_lo = 0.5 * (big_b - disc); // smaller x -> larger n (slow ray)
        let x_hi = f32::midpoint(big_b, disc); // larger x -> smaller n (fast ray)

        let n_slow = 1.0 / x_lo.max(1e-12).sqrt();
        let n_fast = 1.0 / x_hi.max(1e-12).sqrt();
        (n_slow, n_fast)
    }

    /// Closed-form `wave_indices` for a uniaxial degeneracy: `n_o` is the
    /// direction-independent ordinary index, `n_e` the extraordinary principal index,
    /// and `c_axis` the optic axis. Reuses
    /// `BirefringenceParams::effective_extraordinary_index` rather than duplicating its
    /// algebra, and returns `(n_slow, n_fast)` with `n_slow >= n_fast` regardless of
    /// whether this is a positive (`n_e > n_o`) or negative (`n_e < n_o`) material.
    fn uniaxial_wave_indices(k_hat: Vec3, c_axis: Vec3, n_o: f32, n_e: f32) -> (f32, f32) {
        let theta = k_hat.dot(c_axis).clamp(-1.0, 1.0).abs().acos();
        let n_eff = BirefringenceParams::effective_extraordinary_index(n_o, n_e, theta);
        (n_o.max(n_eff), n_o.min(n_eff))
    }

    /// True when two principal refractive indices are close enough that the general
    /// biaxial quadratic in `wave_indices` would lose numerical precision solving for
    /// them, and its exact closed-form short-circuit should be used instead.
    ///
    /// As the discriminant `big_b^2 - 4*big_c` approaches zero (exactly when two
    /// principal indices coincide), evaluating it becomes a subtraction of two
    /// nearly-equal `f32` values -- catastrophic cancellation, whose relative error
    /// scales as `f32::EPSILON` divided by how small the discriminant is, then halved
    /// in exponent by the subsequent `sqrt`. So the smallest relative discriminant
    /// error the general solve can resolve is on the order of `sqrt(f32::EPSILON)` ~=
    /// 3.45e-4; indices closer than that (scaled by their own magnitude) are treated as
    /// degenerate and routed to the closed forms instead.
    fn indices_are_degenerate(a: f32, b: f32) -> bool {
        let scale = a.abs().max(b.abs()).max(1.0);
        (a - b).abs() <= f32::EPSILON.sqrt() * scale
    }

    /// The two eigenmodes' electric-DISPLACEMENT (D) direction at `wave_normal`, in
    /// the same (slow, fast) order as `wave_indices`.
    ///
    /// For a root `x` of the Fresnel equation above, the D-eigenvector in the
    /// principal frame is proportional to `(a/(x-b_alpha), b/(x-b_beta),
    /// g/(x-b_gamma))` (Born & Wolf, same section); built via `eigenvector_world`'s
    /// numerically-conditioned construction (see that method's doc comment).
    ///
    /// Uses D (not E) as the pleochroism axis fed to `AbsorptionTensor3::quadratic_form`
    /// -- the two coincide along a principal axis and are close elsewhere, the same
    /// approximation this renderer makes treating gem dichroism as diagonal in a fixed
    /// crystal frame. `d_to_e_direction` below recovers E for the walk-off calculation.
    #[must_use]
    pub fn eigen_polarizations(&self, wave_normal: Vec3) -> (Vec3, Vec3) {
        let k = wave_normal.normalize_or_zero();
        let local = self.axes.transpose() * k;
        let [b1, b2, b3] = self.b_coeffs();
        let (n_slow, n_fast) = self.wave_indices(wave_normal);
        let x_slow = 1.0 / (n_slow * n_slow);
        let x_fast = 1.0 / (n_fast * n_fast);

        let d_slow = self.eigenvector_world(local, [b1, b2, b3], x_slow, k);
        let d_fast = self.eigenvector_world(local, [b1, b2, b3], x_fast, k);
        (d_slow, d_fast)
    }

    /// Shared body of `eigen_polarizations` for a single root `x`.
    ///
    /// Builds the eigenvector via the symmetric "transverse impermeability" matrix
    /// `Gamma = P . B . P` (`P = I - k k^T` projects onto the plane perpendicular to
    /// the wave normal; `B = diag(b_alpha, b_beta, b_gamma)`), in the principal-axis
    /// frame (`local` is `k_hat`'s direction cosines there). `k` is always exactly
    /// `Gamma`'s zero-eigenvalue eigenvector, and its other two eigenvalues are exactly
    /// `x_slow`/`x_fast` (Yariv & Yeh, *Optical Waves in Crystals*) -- algebraically
    /// equivalent to the `D_i (x-b_i) = -k_i (k . E)` relation a "cleared-denominator"
    /// form (`v_i = local_i * prod_{j!=i}(x-b_j)`) also satisfies, via `E_j = b_j D_j`.
    ///
    /// Reformulated for numerical conditioning: the cleared-denominator form's
    /// pre-normalization magnitude collapses toward zero whenever `x` sits close to a
    /// principal `b_i`, which happens across a wide swath of directions for any weakly
    /// birefringent gem (the common case -- real gemstone birefringence is often only a
    /// few thousandths). A near-zero pre-normalization vector means a 1-ULP difference
    /// in `x` (CPU vs. GPU evaluation order) is amplified through `normalize` into a
    /// materially different unit vector -- the root cause of the GPU port's Tier-2
    /// mismatches (up to ~3.5M ULP before this fix).
    ///
    /// Given `M = Gamma - x*I` (singular by construction), a robust null space for a
    /// rank-<=2 symmetric 3x3 matrix is the cross product of whichever pair of `M`'s
    /// rows has the largest cross-product magnitude. Tried: a hard `argmax` over the
    /// three pairs -- still failed GPU parity by up to ~640K ULP, since a few-ULP
    /// disagreement in `x` can flip which pair "wins" between two candidate directions
    /// that are not close in this near-degenerate regime; rejected in favor of the
    /// smooth combination below.
    ///
    /// For a true rank-2 `M`, all three row-pair cross products are (anti)parallel, so
    /// rather than selecting the largest, `c02`/`c12` are sign-aligned to `c01` via the
    /// robust sign of their dot products and all three summed (`c01 +
    /// sign(c01.c02)*c02 + sign(c01.c12)*c12`) -- always a positive multiple of the
    /// true null direction, continuous under a few-ULP perturbation of `x`. Falls back
    /// to `stable_orthonormal_basis` on the wave normal only at true degeneracy (`M`
    /// rank <=1, the uniaxial ordinary-ray case, where any perpendicular direction is
    /// an equally valid answer).
    ///
    /// Sign convention (an eigenvector is only defined up to an overall sign): the
    /// returned vector's largest-magnitude component is made positive, deterministically
    /// -- see `canonicalize_eigenvector_sign`. The WGSL port (`biaxial_eigenvector_world`
    /// in `transport_physics.wgsl`) mirrors every operation here op-for-op, including
    /// this sign policy, so CPU and GPU agree on which of the two valid signs to return.
    ///
    /// Even with branch-free null-vector extraction, near-degenerate directions still
    /// amplify upstream noise: eigenvector perturbation theory says sensitivity to the
    /// eigenvalue scales as `1/gap`, so `precise_root_near` below recomputes the same
    /// quadratic's discriminant via an algebraically-exact reformulation (avoiding the
    /// `B^2-4C` cancellation) to recover the precision that matters for the
    /// smallest-gap directions -- see that function's own doc comment for the
    /// derivation.
    fn eigenvector_world(&self, local: Vec3, b: [f32; 3], x: f32, k_hat: Vec3) -> Vec3 {
        let x = Self::precise_root_near(local, b, x);

        let s = b[0].mul_add(
            local.x * local.x,
            b[1].mul_add(local.y * local.y, b[2] * local.z * local.z),
        );

        let m00 = (local.x * local.x).mul_add(2.0f32.mul_add(-b[0], s), b[0] - x);
        let m11 = (local.y * local.y).mul_add(2.0f32.mul_add(-b[1], s), b[1] - x);
        let m22 = (local.z * local.z).mul_add(2.0f32.mul_add(-b[2], s), b[2] - x);
        let m01 = local.x * local.y * (s - b[0] - b[1]);
        let m02 = local.x * local.z * (s - b[0] - b[2]);
        let m12 = local.y * local.z * (s - b[1] - b[2]);

        let row0 = Vec3::new(m00, m01, m02);
        let row1 = Vec3::new(m01, m11, m12);
        let row2 = Vec3::new(m02, m12, m22);

        // Explicit `mul_add` cross products (rather than `Vec3::cross`) for the same
        // reason `wave_indices`' own discriminant uses `mul_add` rather than plain
        // `*`/`-`: a plain multiply-then-subtract chain is free to be auto-contracted
        // into a fused multiply-add by one compiler (CPU LLVM or the GPU shader
        // compiler) and not the other, which would round differently and silently
        // break CPU/GPU bit-parity.
        let c01 = cross_fma(row0, row1);
        let c02 = cross_fma(row0, row2);
        let c12 = cross_fma(row1, row2);

        // Sign-align c02/c12 to c01 and sum -- see this method's doc comment for why
        // this replaced a hard "largest of three" selection.
        let sign02 = if dot_fma(c01, c02) < 0.0 { -1.0 } else { 1.0 };
        let sign12 = if dot_fma(c01, c12) < 0.0 { -1.0 } else { 1.0 };
        let v_local = c01 + c02 * sign02 + c12 * sign12;

        let v_world = self.axes * v_local;
        if v_world.length_squared() > 1e-12 {
            canonicalize_eigenvector_sign(v_world).normalize()
        } else {
            stable_orthonormal_basis(k_hat).0
        }
    }

    /// Recomputes the general biaxial Fresnel quadratic's discriminant via an
    /// algebraically-exact reformulation, then returns whichever of its two roots
    /// (`x_lo`, `x_hi`) is nearest the `x` this was handed (see `eigenvector_world`'s
    /// doc comment for why: the input `x`, from `wave_indices`, is already accurate
    /// enough -- gap-relative error a few percent at worst, always far less than 100%
    /// -- to unambiguously identify WHICH of the two roots it was meant to be, even
    /// though it is not accurate enough for the eigenvector built from it).
    ///
    /// Derivation: writing `a = local.x^2`, `bb = local.y^2`, `cc = local.z^2` (so
    /// `a+bb+cc = 1`) and `p,q,r = b[0],b[1],b[2]`, the naive discriminant is
    /// `disc^2 = B^2 - 4C` with `B = a(q+r)+bb(p+r)+cc(p+q)`, `C = aqr+bb*pr+cc*pq` --
    /// exactly what `wave_indices` computes. Expanding and eliminating `cc = 1-a-bb`
    /// gives, EXACTLY (verified symbolically and pinned by
    /// `discriminant_reformulation_matches_naive_b2_minus_4c` below):
    ///   `disc^2 = (a+cc)^2 * X^2  +  2*(a - bb*cc) * X*Y  +  (a+bb)^2 * Y^2`
    /// where `X = p-q` (`b_alpha - b_beta`) and `Y = r-p` (`b_gamma - b_alpha`) are
    /// DIRECT differences of the principal `1/n^2` values rather than the original
    /// formula's difference of two ~1-magnitude sums `B^2` and `4C`. Since any two
    /// principal indices of a realistic gem material sit within a factor of 2 of each
    /// other (indices cluster in roughly the 1.4-2.5 range, so `1/n^2` does too), `X`
    /// and `Y` are each computed via a Sterbenz-exact subtraction -- no rounding error
    /// at all beyond what `p`, `q`, `r` already carried from their own `1/n^2`. That
    /// makes this reformulation's cancellation benign (the three terms above are each
    /// already `O(disc^2)`, not `O(B^2)` collapsing down to `O(disc^2)`), where the
    /// naive `B^2-4C` loses essentially all its precision resolving a `disc` far
    /// smaller than either operand.
    fn precise_root_near(local: Vec3, b: [f32; 3], x: f32) -> f32 {
        let a = local.x * local.x;
        let bb = local.y * local.y;
        let cc = local.z * local.z;
        let big_b = a.mul_add(b[1] + b[2], bb.mul_add(b[0] + b[2], cc * (b[0] + b[1])));

        let xdiff = b[0] - b[1]; // p - q, Sterbenz-exact
        let ydiff = b[2] - b[0]; // r - p, Sterbenz-exact

        let a_plus_c = a + cc;
        let a_plus_bb = a + bb;
        let two_a_minus_bc = 2.0 * bb.mul_add(-cc, a);

        let disc_sq = (a_plus_c * a_plus_c).mul_add(
            xdiff * xdiff,
            (a_plus_bb * a_plus_bb).mul_add(ydiff * ydiff, two_a_minus_bc * xdiff * ydiff),
        );
        let disc = disc_sq.max(0.0).sqrt();

        let x_lo = 0.5 * (big_b - disc);
        let x_hi = f32::midpoint(big_b, disc);

        if (x - x_lo).abs() <= (x - x_hi).abs() {
            x_lo
        } else {
            x_hi
        }
    }

    /// Converts a D-field eigen-direction to the corresponding E-field direction via
    /// `E = eps^-1 . D`, diagonal in the principal frame (`eps_i = n_i^2`). Needed
    /// because the Poynting/walk-off direction depends on E, not D: D is always
    /// exactly perpendicular to the wave normal by construction; E is what tilts away
    /// from that perpendicular by the walk-off angle (see `poynting_direction`).
    #[must_use]
    pub fn d_to_e_direction(&self, d_hat: Vec3) -> Vec3 {
        let d_local = self.axes.transpose() * d_hat;
        let e_local = Vec3::new(
            d_local.x / (self.n_alpha * self.n_alpha),
            d_local.y / (self.n_beta * self.n_beta),
            d_local.z / (self.n_gamma * self.n_gamma),
        );
        (self.axes * e_local).normalize_or_zero()
    }

    /// Fixed-point resolution of the refracted WAVE-NORMAL direction for
    /// one eigenmode (`want_slow`: the `n_slow` root if true, `n_fast` if false) at an
    /// air->crystal entry into a biaxial material.
    ///
    /// Generalizes `trace_spectral_ray`'s existing uniaxial `theta_c` iteration (see
    /// that call site's doc comment) from a single angle-to-c-axis to a full 3D
    /// direction: a biaxial mode's index depends on the FULL wave-normal direction (not
    /// just the angle to one fixed axis, since there is no single optic axis), which
    /// itself depends on the index via Snell's law -- the same circularity, resolved
    /// with the same two-iteration fixed point seeded from an isotropic first guess
    /// `n_seed` (in practice the material's own base dispersion curve value, i.e.
    /// `n_beta` -- exactly correct for neither root individually, but a reasonable
    /// starting point, mirroring how the uniaxial iteration seeds from `n_o` even
    /// though that is only exactly correct for the ordinary root).
    ///
    /// Unlike the uniaxial case, NEITHER root has a direction-independent index to fall
    /// back on without iterating -- there is no "ordinary ray" in a biaxial crystal --
    /// so this is called once per mode, not just for the extraordinary-like one.
    ///
    /// Returns `(n, wave_normal)`: the converged index for the requested root and the
    /// wave-normal direction it was evaluated at.
    #[must_use]
    pub fn resolve_entry_mode(
        &self,
        incident_dir: Vec3,
        normal: Vec3,
        cos_i: f32,
        n_seed: f32,
        want_slow: bool,
    ) -> (f32, Vec3) {
        let mut n_guess = n_seed;
        let mut wave_dir = incident_dir;
        for _ in 0..2 {
            let eta_guess = 1.0 / n_guess;
            let sin2_t_guess = eta_guess * eta_guess * cos_i.mul_add(-cos_i, 1.0);
            if sin2_t_guess > 1.0 {
                break;
            }
            let cos_t_guess = (1.0 - sin2_t_guess).max(0.0).sqrt();
            wave_dir = (eta_guess * incident_dir + eta_guess.mul_add(cos_i, -cos_t_guess) * normal)
                .normalize();
            let (n_slow, n_fast) = self.wave_indices(wave_dir);
            n_guess = if want_slow { n_slow } else { n_fast };
        }
        (n_guess, wave_dir)
    }

    /// The Poynting (walk-off) energy-propagation direction for one
    /// eigenmode at `wave_normal`, generalizing
    /// `BirefringenceParams::extraordinary_poynting_dir` (uniaxial-only, and only ever
    /// applied to the extraordinary mode -- the ordinary ray never walks off) to the
    /// biaxial case where BOTH eigenmodes walk off, since neither is ever exactly
    /// perpendicular to the D-field in general.
    ///
    /// Builds the requested mode's D-field eigenvector via `eigen_polarizations`,
    /// converts it to the corresponding E-field direction via `d_to_e_direction`, and
    /// feeds both into the general `poynting_direction` (S = E x H) formula above.
    #[must_use]
    pub fn mode_poynting_dir(&self, wave_normal: Vec3, want_slow: bool) -> Vec3 {
        let (d_slow, d_fast) = self.eigen_polarizations(wave_normal);
        let d_hat = if want_slow { d_slow } else { d_fast };
        let e_hat = self.d_to_e_direction(d_hat);
        poynting_direction(wave_normal, e_hat)
    }

    /// The assigned biaxial eigenmode's world-space electric-field direction at
    /// `wave_normal`: mode B (`is_extraordinary`, matching the uniaxial
    /// extraordinary-mode slot in `raytracer::absorption::
    /// channel_absorption_alphas_assigned`'s two-valued convention) selects the SLOW
    /// D-eigenvector, mode A the FAST one -- the same `want_slow` convention
    /// [`Self::mode_poynting_dir`] uses -- converted D -> E via [`Self::d_to_e_direction`].
    ///
    /// See [`assigned_mode_e_field_uniaxial`]'s doc comment for why this is evaluated
    /// fresh from the CURRENT wave normal rather than derived from a Stokes vector.
    #[must_use]
    pub fn assigned_mode_e_field(&self, wave_normal: Vec3, is_extraordinary: bool) -> Vec3 {
        let (d_slow, d_fast) = self.eigen_polarizations(wave_normal);
        let d_hat = if is_extraordinary { d_slow } else { d_fast };
        self.d_to_e_direction(d_hat)
    }
}

/// The Poynting (ray/walk-off) energy-propagation direction for an eigenmode.
///
/// Takes the wave normal `wave_normal` and the world-space electric-FIELD direction
/// `e_field_hat` (see `BiaxialIndicatrix::d_to_e_direction`) -- the general
/// (uniaxial-or-biaxial) counterpart of `extraordinary_poynting_dir` above, "the ray
/// direction is the gradient of the index surface" in its equivalent Poynting-vector
/// form: `S = E x H`, and for a plane wave `H` is parallel to `k_hat x E` (from
/// Maxwell's `curl E = -dB/dt`), so
///   S ~ E x (`k_hat` x E) = `k_hat`*(E.E) - E*(E.`k_hat`)
/// which for unit `E` reduces to the component of `k_hat` perpendicular to
/// `e_field_hat` -- i.e. `k_hat` tilted just enough to become exactly perpendicular to
/// E (S is always perpendicular to E, the same way D is always perpendicular to
/// `k_hat`). Falls back to `k_hat` itself (no walk-off) in the degenerate case where
/// `e_field_hat` is parallel to `k_hat` (should not occur for a genuine eigenmode, but
/// guarded rather than risking a zero-length normalize).
#[must_use]
pub fn poynting_direction(wave_normal: Vec3, e_field_hat: Vec3) -> Vec3 {
    let k_hat = wave_normal.normalize_or_zero();
    let e_hat = e_field_hat.normalize_or_zero();
    let perp = k_hat - e_hat * e_hat.dot(k_hat);
    let len2 = perp.length_squared();
    if len2 > 1e-10 {
        perp / len2.sqrt()
    } else {
        k_hat
    }
}
