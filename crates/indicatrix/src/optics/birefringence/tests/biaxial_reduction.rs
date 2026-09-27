//! [`BiaxialIndicatrix`] reduction/regression tests: the uniaxial and
//! isotropic degeneracies, eigenvector orthonormality across a dense sweep,
//! the discriminant reformulation, [`poynting_direction`], and the
//! absorption/index frame identity.

use crate::optics::birefringence::{
    AbsorptionTensor3, BiaxialIndicatrix, BirefringenceParams, poynting_direction,
};
use glam::{Mat3, Vec3};

#[cfg(test)]
mod biaxial_reduction_tests {
    use super::*;

    /// The decisive test: setting `n_alpha == n_beta` must reproduce the
    /// existing uniaxial code path's index -- `BirefringenceParams::effective_extraordinary_index`
    /// -- to within float tolerance, across many propagation directions and a
    /// deliberately off-axis (non-axis-aligned) c-axis. This pins the biaxial Fresnel
    /// equation's reduction and would catch most algebra errors in `wave_indices`.
    #[test]
    fn biaxial_wave_indices_reduce_to_uniaxial_formula() {
        let n_o = 1.65f32;
        let n_e = 1.72f32; // positive uniaxial
        let c_axis = Vec3::new(0.35, 0.82, -0.45).normalize(); // deliberately off-axis
        let indicatrix = BiaxialIndicatrix::uniaxial(n_o, n_e, c_axis);

        let directions = [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            c_axis, // exactly along the optic axis
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.2, 0.9, 0.1).normalize(),
            Vec3::new(-0.5, 0.3, 0.8).normalize(),
            Vec3::new(0.7, -0.6, 0.2).normalize(),
            Vec3::new(0.1, 0.1, 0.99).normalize(),
            Vec3::new(-0.9, -0.3, 0.2).normalize(),
        ];

        for wave_normal in directions {
            let (n_slow, n_fast) = indicatrix.wave_indices(wave_normal);
            let cos_theta = wave_normal.normalize().dot(c_axis).clamp(-1.0, 1.0).abs();
            let theta = cos_theta.acos();
            let n_eff = BirefringenceParams::effective_extraordinary_index(n_o, n_e, theta);

            let expected_lo = n_o.min(n_eff);
            let expected_hi = n_o.max(n_eff);

            assert!(
                (n_fast - expected_lo).abs() < 1e-3,
                "n_fast should match min(n_o, n_eff) at direction {wave_normal:?} (theta={theta}): got {n_fast}, expected {expected_lo}"
            );
            assert!(
                (n_slow - expected_hi).abs() < 1e-3,
                "n_slow should match max(n_o, n_eff) at direction {wave_normal:?} (theta={theta}): got {n_slow}, expected {expected_hi}"
            );
        }
    }

    /// Isotropic special case (all three principal indices equal): both roots must
    /// equal that single index for every direction.
    #[test]
    fn biaxial_wave_indices_reduce_to_isotropic_when_all_equal() {
        let n = 1.9f32;
        let indicatrix = BiaxialIndicatrix::new(n, n, n, Mat3::IDENTITY);
        for wave_normal in [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.3, 0.5, 0.8).normalize(),
        ] {
            let (n_slow, n_fast) = indicatrix.wave_indices(wave_normal);
            assert!(
                (n_slow - n).abs() < 1e-4,
                "isotropic n_slow should equal {n}, got {n_slow}"
            );
            assert!(
                (n_fast - n).abs() < 1e-4,
                "isotropic n_fast should equal {n}, got {n_fast}"
            );
        }
    }

    /// For a genuinely biaxial indicatrix (three distinct principal indices), the two
    /// eigen D-directions returned by `eigen_polarizations` must (a) be finite, (b) be
    /// unit length, (c) be mutually perpendicular, and (d) each be perpendicular to
    /// the wave normal -- D is always transverse, that's the defining property the
    /// Fresnel equation is built from.
    #[test]
    fn biaxial_eigen_polarizations_are_orthonormal_and_transverse() {
        let axes = Mat3::from_cols(Vec3::X, Vec3::Y, Vec3::Z);
        let indicatrix = BiaxialIndicatrix::new(1.60, 1.65, 1.75, axes);

        for wave_normal in [
            Vec3::new(0.3, 0.5, 0.8).normalize(),
            Vec3::new(0.9, 0.1, -0.2).normalize(),
            Vec3::new(-0.4, 0.6, 0.5).normalize(),
        ] {
            let (d_slow, d_fast) = indicatrix.eigen_polarizations(wave_normal);
            assert!(
                d_slow.is_finite() && d_fast.is_finite(),
                "eigenvectors must be finite (d_slow={d_slow:?}, d_fast={d_fast:?})"
            );
            assert!(
                (d_slow.length() - 1.0).abs() < 1e-3,
                "d_slow must be unit length, got {}",
                d_slow.length()
            );
            assert!(
                (d_fast.length() - 1.0).abs() < 1e-3,
                "d_fast must be unit length, got {}",
                d_fast.length()
            );
            assert!(
                d_slow.dot(d_fast).abs() < 1e-2,
                "eigenvectors must be mutually perpendicular, got dot={}",
                d_slow.dot(d_fast)
            );
            assert!(
                d_slow.dot(wave_normal).abs() < 1e-2,
                "d_slow must be transverse to the wave normal, got dot={}",
                d_slow.dot(wave_normal)
            );
            assert!(
                d_fast.dot(wave_normal).abs() < 1e-2,
                "d_fast must be transverse to the wave normal, got dot={}",
                d_fast.dot(wave_normal)
            );
        }
    }

    /// The well-conditioned `eigenvector_world` (cross-product-of-rows of the
    /// "transverse impermeability" matrix `Gamma - x*I`) must satisfy the
    /// eigen-equation tightly across a DENSE direction sweep (not just the handful of
    /// spot-checks above) for all three real biaxial built-ins (Alexandrite, Topaz,
    /// Tanzanite -- exactly the materials whose small real-world birefringence makes a
    /// cleared-denominator construction ill-conditioned).
    ///
    /// The residual checked is the textbook relation the eigenvector must satisfy
    /// independent of ANY particular matrix construction (so this is a genuine
    /// correctness check, not a tautological re-derivation of `eigenvector_world`'s own
    /// algebra): in the principal-axis frame, `D_i (x - b_i) = -k_i (k . E)` with
    /// `E_j = b_j D_j` (see `eigenvector_world`'s doc comment). Also pins unit length
    /// and continuous-sign (adjacent samples along the sweep must have `|dot| ~= 1`,
    /// i.e. the eigen-LINE direction -- which is only defined up to sign -- varies
    /// smoothly; an isolated sign flip at the deterministic sign convention's own
    /// crossover is fine and is exactly why this checks `abs(dot)`, not `dot`, against
    /// its floor).
    #[test]
    fn biaxial_eigenvectors_satisfy_eigen_equation_across_dense_sweep() {
        use crate::optics::materials::GemMaterial;

        const RESIDUAL_BOUND: f32 = 2e-4;

        for name in ["Alexandrite", "Topaz", "Tanzanite"] {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must be a built-in material"));
            let indicatrix = material
                .biaxial_indicatrix(589.3)
                .unwrap_or_else(|| panic!("{name} must expose a biaxial indicatrix"));
            let b = indicatrix.b_coeffs();

            let samples = 181;
            let mut prev_slow: Option<Vec3> = None;
            let mut prev_fast: Option<Vec3> = None;
            for i in 0..samples {
                // Dense sweep covering a full great-circle arc, deliberately not
                // aligned to any principal axis, so it crosses many directions where
                // the two mode indices sit close together (the common case for these
                // materials' small birefringence).
                let theta = (i as f32) / (samples as f32 - 1.0) * std::f32::consts::PI;
                let wave_normal = (theta.cos() * Vec3::new(0.4, 0.6, 0.69).normalize()
                    + theta.sin() * Vec3::new(0.8, -0.3, 0.2).normalize())
                .normalize();
                let k = wave_normal; // already unit
                let local = indicatrix.axes.transpose() * k;

                let (n_slow, n_fast) = indicatrix.wave_indices(wave_normal);
                let (d_slow, d_fast) = indicatrix.eigen_polarizations(wave_normal);

                for (n, d_hat, prev) in [
                    (n_slow, d_slow, &mut prev_slow),
                    (n_fast, d_fast, &mut prev_fast),
                ] {
                    assert!(
                        (d_hat.length() - 1.0).abs() < 1e-4,
                        "{name}: eigenvector must be unit length at theta={theta}, got {}",
                        d_hat.length()
                    );

                    let x = 1.0 / (n * n);
                    let d_local = indicatrix.axes.transpose() * d_hat;
                    let e_dot_k = (local.z * b[2]).mul_add(
                        d_local.z,
                        (local.y * b[1]).mul_add(d_local.y, local.x * b[0] * d_local.x),
                    );
                    let residual = Vec3::new(
                        d_local.x.mul_add(x - b[0], local.x * e_dot_k),
                        d_local.y.mul_add(x - b[1], local.y * e_dot_k),
                        d_local.z.mul_add(x - b[2], local.z * e_dot_k),
                    );
                    assert!(
                        residual.length() < RESIDUAL_BOUND,
                        "{name}: eigen-equation residual too large at theta={theta} \
                         (wave_normal={wave_normal:?}): |residual|={} >= {RESIDUAL_BOUND}",
                        residual.length()
                    );

                    if let Some(p) = *prev {
                        let cos = p.dot(d_hat).clamp(-1.0, 1.0).abs();
                        assert!(
                            cos > 0.98,
                            "{name}: eigenvector direction should vary continuously \
                             (up to sign) between adjacent sweep samples at theta={theta}, \
                             got |dot|={cos}"
                        );
                    }
                    *prev = Some(d_hat);
                }
            }
        }
    }

    /// Pins `precise_root_near`'s discriminant reformulation (see its doc comment for
    /// the derivation) against the NAIVE `B^2 - 4C` formula `wave_indices` itself uses,
    /// in `f64` (so this test's own arithmetic doesn't share whatever `f32` rounding
    /// is under scrutiny) -- a dense sweep of direction-cosine triples and principal
    /// `1/n^2` triples, asserting the two formulas' discriminants agree to within a
    /// tight `f64` tolerance. This is an ALGEBRAIC identity (verified symbolically
    /// during development, not just spot-checked), so any real disagreement here would
    /// mean a transcription bug in the reformulation, not mere floating-point noise.
    #[test]
    fn discriminant_reformulation_matches_naive_b2_minus_4c() {
        fn naive_disc_sq(a: f64, bb: f64, cc: f64, p: f64, q: f64, r: f64) -> f64 {
            let big_b = cc.mul_add(p + q, bb.mul_add(p + r, a * (q + r)));
            let big_c = (cc * p).mul_add(q, (bb * p).mul_add(r, a * q * r));
            4.0f64.mul_add(-big_c, big_b * big_b)
        }

        fn reformulated_disc_sq(a: f64, bb: f64, cc: f64, p: f64, q: f64, r: f64) -> f64 {
            let xdiff = p - q;
            let ydiff = r - p;
            let a_plus_c = a + cc;
            let a_plus_bb = a + bb;
            let two_a_minus_bc = 2.0 * bb.mul_add(-cc, a);
            (a_plus_bb * a_plus_bb).mul_add(
                ydiff * ydiff,
                (two_a_minus_bc * xdiff).mul_add(ydiff, (a_plus_c * a_plus_c) * (xdiff * xdiff)),
            )
        }

        let cosine_triples: [(f64, f64, f64); 6] = [
            (0.046_512, 0.011_628, 0.941_860),
            (1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0),
            (0.7, 0.2, 0.1),
            (0.02, 0.9, 0.08),
            (0.5, 0.0, 0.5),
            (0.9999, 0.00005, 0.00005),
        ];
        let index_triples = [
            (
                1.0 / 1.60_f64.powi(2),
                1.0 / 1.65_f64.powi(2),
                1.0 / 1.75_f64.powi(2),
            ),
            (
                1.0 / 1.740_778_4_f64.powi(2),
                1.0 / 1.742_729_4_f64.powi(2),
                1.0 / 1.748_378_4_f64.powi(2),
            ),
            (
                1.0 / 1.4_f64.powi(2),
                1.0 / 2.4_f64.powi(2),
                1.0 / 2.49_f64.powi(2),
            ),
        ];

        for &(a, bb, cc) in &cosine_triples {
            assert!(
                (a + bb + cc - 1.0).abs() < 1e-9,
                "test premise: direction cosines squared must sum to 1"
            );
            for &(p, q, r) in &index_triples {
                let naive = naive_disc_sq(a, bb, cc, p, q, r);
                let reformulated = reformulated_disc_sq(a, bb, cc, p, q, r);
                let scale = naive.abs().max(reformulated.abs()).max(1e-12);
                assert!(
                    (naive - reformulated).abs() / scale < 1e-9,
                    "discriminant reformulation disagrees with naive B^2-4C: \
                     cosines=({a},{bb},{cc}) indices=({p},{q},{r}) naive={naive:e} \
                     reformulated={reformulated:e}"
                );
            }
        }
    }

    /// `poynting_direction` must reduce to zero walk-off (S == `k_hat`) whenever the
    /// electric field is already perpendicular to the wave normal (the isotropic /
    /// ordinary-ray case), and must be finite and unit length in general.
    #[test]
    fn poynting_direction_has_no_walk_off_when_e_is_transverse() {
        let k_hat = Vec3::new(0.3, 0.6, 0.74).normalize();
        let e_hat = k_hat.cross(Vec3::Y).normalize(); // exactly transverse to k_hat
        let s = poynting_direction(k_hat, e_hat);
        assert!(
            (s - k_hat).length() < 1e-4,
            "S should equal k_hat when E is exactly transverse, got {s:?}"
        );
    }

    /// `poynting_direction` must always return a finite unit vector, including for an
    /// E direction with a genuine (non-degenerate) component along `k_hat` (the walk-off
    /// case).
    #[test]
    fn poynting_direction_is_finite_unit_vector_with_walk_off() {
        let k_hat = Vec3::new(0.0, 0.0, 1.0);
        let e_hat = Vec3::new(0.1, 0.0, 0.99).normalize(); // tilted toward k_hat
        let s = poynting_direction(k_hat, e_hat);
        assert!(s.is_finite(), "S must be finite, got {s:?}");
        assert!(
            (s.length() - 1.0).abs() < 1e-4,
            "S must be unit length, got {}",
            s.length()
        );
        assert!(
            s.dot(e_hat).abs() < 1e-3,
            "S must be perpendicular to E, got dot={}",
            s.dot(e_hat)
        );
    }

    /// Trichroism -- THE decisive frame-identity test. `AbsorptionTensor3::
    /// biaxial`'s doc comment claims its axis frame is bit-identical to
    /// `BiaxialIndicatrix::from_gamma_axis`'s, not merely close: both call the exact
    /// same `stable_orthonormal_basis` free function on the exact same `gamma_axis`
    /// input. This pins that claim directly by comparing the two `Mat3`s column by
    /// column, bit for bit (`to_bits()`), across several non-axis-aligned `gamma_axis`
    /// directions -- getting this subtly wrong (e.g. re-deriving the completion via a
    /// different but numerically-similar construction) would silently score absorption
    /// against axes that drift from the ones `eigen_polarizations` computed its
    /// eigenmodes in.
    #[test]
    fn absorption_frame_is_bit_identical_to_index_frame() {
        let gamma_axes = [
            Vec3::Y,
            Vec3::X,
            Vec3::Z,
            Vec3::new(0.35, 0.82, -0.45).normalize(),
            Vec3::new(-0.6, 0.1, 0.79).normalize(),
        ];
        for gamma_axis in gamma_axes {
            let tensor = AbsorptionTensor3::biaxial(1.0, 2.0, 3.0, gamma_axis);
            let indicatrix = BiaxialIndicatrix::from_gamma_axis(1.6, 1.65, 1.75, gamma_axis);

            let pairs = [
                ("x_axis", tensor.axes.x_axis, indicatrix.axes.x_axis),
                ("y_axis", tensor.axes.y_axis, indicatrix.axes.y_axis),
                ("z_axis", tensor.axes.z_axis, indicatrix.axes.z_axis),
            ];
            for (label, t_col, i_col) in pairs {
                assert_eq!(
                    t_col.x.to_bits(),
                    i_col.x.to_bits(),
                    "gamma_axis={gamma_axis:?} {label}: x component bits differ (tensor={t_col:?}, indicatrix={i_col:?})"
                );
                assert_eq!(
                    t_col.y.to_bits(),
                    i_col.y.to_bits(),
                    "gamma_axis={gamma_axis:?} {label}: y component bits differ (tensor={t_col:?}, indicatrix={i_col:?})"
                );
                assert_eq!(
                    t_col.z.to_bits(),
                    i_col.z.to_bits(),
                    "gamma_axis={gamma_axis:?} {label}: z component bits differ (tensor={t_col:?}, indicatrix={i_col:?})"
                );
            }
        }
    }

    /// Pins the worked example: with `gamma_axis = +Y`, the deterministic
    /// `stable_orthonormal_basis` completion must place `alpha` on `+X`, `beta` on
    /// `-Z`, and `gamma` on `+Y` itself.
    #[test]
    fn absorption_frame_matches_worked_example_for_plus_y_gamma_axis() {
        let tensor = AbsorptionTensor3::biaxial(1.0, 2.0, 3.0, Vec3::Y);
        assert!(
            (tensor.axes.x_axis - Vec3::X).length() < 1e-6,
            "alpha axis should be +X, got {:?}",
            tensor.axes.x_axis
        );
        assert!(
            (tensor.axes.y_axis - (-Vec3::Z)).length() < 1e-6,
            "beta axis should be -Z, got {:?}",
            tensor.axes.y_axis
        );
        assert!(
            (tensor.axes.z_axis - Vec3::Y).length() < 1e-6,
            "gamma axis should be +Y, got {:?}",
            tensor.axes.z_axis
        );
    }

    /// Three-coefficient test: for a tensor with three genuinely distinct principal
    /// coefficients, `quadratic_form` evaluated exactly along each principal axis must
    /// return exactly that axis's own coefficient (not some blend with the other two).
    #[test]
    fn biaxial_tensor_quadratic_form_matches_each_principal_coefficient_on_axis() {
        let gamma_axis = Vec3::new(0.2, 0.6, 0.77).normalize();
        let (alpha, beta, gamma) = (1.5f32, 4.0f32, 9.0f32);
        let tensor = AbsorptionTensor3::biaxial(alpha, beta, gamma, gamma_axis);

        let alpha_axis = tensor.axes.x_axis;
        let beta_axis = tensor.axes.y_axis;

        assert!(
            (tensor.quadratic_form(alpha_axis) - alpha).abs() < 1e-4,
            "E along the alpha axis should give alpha ({alpha}), got {}",
            tensor.quadratic_form(alpha_axis)
        );
        assert!(
            (tensor.quadratic_form(beta_axis) - beta).abs() < 1e-4,
            "E along the beta axis should give beta ({beta}), got {}",
            tensor.quadratic_form(beta_axis)
        );
        assert!(
            (tensor.quadratic_form(gamma_axis) - gamma).abs() < 1e-4,
            "E along the gamma axis should give gamma ({gamma}), got {}",
            tensor.quadratic_form(gamma_axis)
        );
    }

    /// Degeneracy test: `AbsorptionTensor3::biaxial(a, a, b, axis)` (alpha == beta) must
    /// reduce to `AbsorptionTensor3::uniaxial(a, b, axis)` BIT-IDENTICALLY -- not just
    /// numerically close -- since both share the exact same `Vec3::new`/`Mat3::from_cols`
    /// construction over the exact same `stable_orthonormal_basis(axis)` call. Checked
    /// via `to_bits()` on every component of both `alpha` and `axes`.
    #[test]
    fn biaxial_tensor_alpha_beta_degenerate_matches_uniaxial_bit_exact() {
        let axis = Vec3::new(0.35, 0.82, -0.45).normalize();
        let (a, b) = (2.5f32, 7.25f32);

        let biaxial = AbsorptionTensor3::biaxial(a, a, b, axis);
        let uniaxial = AbsorptionTensor3::uniaxial(a, b, axis);

        assert_eq!(biaxial.alpha.x.to_bits(), uniaxial.alpha.x.to_bits());
        assert_eq!(biaxial.alpha.y.to_bits(), uniaxial.alpha.y.to_bits());
        assert_eq!(biaxial.alpha.z.to_bits(), uniaxial.alpha.z.to_bits());
        let axis_pairs = [
            ("x_axis", biaxial.axes.x_axis, uniaxial.axes.x_axis),
            ("y_axis", biaxial.axes.y_axis, uniaxial.axes.y_axis),
            ("z_axis", biaxial.axes.z_axis, uniaxial.axes.z_axis),
        ];
        for (label, b_col, u_col) in axis_pairs {
            assert_eq!(b_col.x.to_bits(), u_col.x.to_bits(), "{label} x");
            assert_eq!(b_col.y.to_bits(), u_col.y.to_bits(), "{label} y");
            assert_eq!(b_col.z.to_bits(), u_col.z.to_bits(), "{label} z");
        }

        // And, as a corollary, quadratic_form itself must agree bit-for-bit for any
        // direction (not just special ones), since it's computed purely from `alpha`
        // and `axes`, which are now proven identical above.
        for d in [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.4, -0.5, 0.77).normalize(),
        ] {
            assert_eq!(
                biaxial.quadratic_form(d).to_bits(),
                uniaxial.quadratic_form(d).to_bits(),
                "quadratic_form must agree bit-exactly at d={d:?}"
            );
        }
    }
}
