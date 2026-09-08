use glam::{Mat4, Vec3, Vec4};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StokesVector {
    pub i: f32, // Total intensity
    pub q: f32, // Linear horizontal (+Q) vs vertical (-Q)
    pub u: f32, // Linear +45 deg (+U) vs -45 deg (-U)
    pub v: f32, // Circular right (+V) vs left (-V)
}

impl StokesVector {
    #[must_use]
    pub const fn new(i: f32, q: f32, u: f32, v: f32) -> Self {
        Self { i, q, u, v }
    }

    #[must_use]
    pub const fn unpolarized(intensity: f32) -> Self {
        Self::new(intensity, 0.0, 0.0, 0.0)
    }

    #[must_use]
    pub const fn to_vec4(self) -> Vec4 {
        Vec4::new(self.i, self.q, self.u, self.v)
    }

    /// Not `const fn`, and cannot be on this workspace's primary target -- see the
    /// `cfg_attr` below.
    #[must_use]
    // `missing_const_for_fn` fires ONLY under `target_arch = "wasm32"`, and taking its
    // advice would break every other target. glam picks its `Vec4` representation per
    // architecture: on `x86_64` it is a SIMD vector whose `.x`/`.y`/`.z`/`.w` are reached
    // through a `Deref` to a scalar struct, and `cannot perform non-const deref coercion
    // on Vec4 in constant functions` (E0015) is a hard compile error -- verified, 5
    // instances, one per field access plus the call. On `wasm32` glam falls back to a
    // plain scalar struct with real fields, no deref, so the same body IS const-eligible
    // and clippy notices.
    //
    // So this is a genuine target-dependent divergence in a dependency's type layout, not
    // a missed `const`. Marking it `const` for wasm only would mean `cfg`-duplicating the
    // whole function to satisfy a lint about a two-line constructor -- strictly worse than
    // one narrowly-scoped, explained suppression. Same situation `simd::detect_level`
    // already documents for its own `x86_64`-gated branches.
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            clippy::missing_const_for_fn,
            reason = "const-eligible only on wasm32's scalar glam backend; see above"
        )
    )]
    pub fn from_vec4(v: Vec4) -> Self {
        Self::new(v.x, v.y, v.z, v.w)
    }

    #[must_use]
    pub const fn intensity(&self) -> f32 {
        self.i.max(0.0)
    }

    #[must_use]
    pub fn degree_of_polarization(&self) -> f32 {
        if self.i <= 1e-7 {
            return 0.0;
        }
        (self
            .v
            .mul_add(self.v, self.u.mul_add(self.u, self.q * self.q))
            .sqrt()
            / self.i)
            .clamp(0.0, 1.0)
    }

    /// Linear polarization azimuth (radians), the orientation of the electric-field
    /// oscillation plane measured from this Stokes frame's Q axis. Standard Stokes
    /// convention: `Q = I*p*cos(2*psi)`, `U = I*p*sin(2*psi)`, so `psi` recovers as
    /// half the two-argument arctangent of `(U, Q)`. Meaningless (returns whatever
    /// `atan2` gives for near-zero inputs, typically 0) for fully unpolarized light,
    /// where the azimuth genuinely has no physical definition -- callers combine this
    /// with `degree_of_polarization` so that case is weighted out rather than trusted.
    #[must_use]
    pub fn polarization_azimuth(&self) -> f32 {
        0.5 * self.u.atan2(self.q)
    }

    #[must_use]
    pub fn apply_matrix(&self, m: &Mat4) -> Self {
        let v = m.mul_vec4(self.to_vec4());
        Self::from_vec4(v)
    }

    #[must_use]
    pub fn scale(&self, s: f32) -> Self {
        Self::new(self.i * s, self.q * s, self.u * s, self.v * s)
    }
}

pub struct MuellerMatrix;

impl MuellerMatrix {
    /// Frame rotation operator R(psi) to align reference plane of incidence between consecutive facet bounces.
    ///
    /// `glam::Mat4::from_cols_array` reads its flat argument as COLUMN-major
    /// (four consecutive elements = one column), but the array below was written out
    /// textbook ROW-major (`[[1,0,0,0],[0,c2,s2,0],[0,-s2,c2,0],[0,0,0,1]]`, read row by
    /// row). Feeding a row-major layout to a column-major constructor builds the
    /// TRANSPOSE of the intended matrix -- i.e. `R(-psi)` instead of `R(psi)`, since a
    /// 2D rotation's transpose is its inverse. This was invisible to every existing
    /// test because the affected 2x2 rotation block only ever appears sandwiched
    /// between Fresnel matrices (symmetric, so transpose-invariant) and the TIR
    /// retarder (sign-symmetric in this same way, so the chain's net effect on
    /// intensity is unchanged either way) -- so `I` transport was exactly unaffected.
    /// It is NOT invisible to `polarization_azimuth`/`electric_field_direction`, which
    /// read the rotated Q/U components directly to recover which way the electric
    /// field itself points: with the transpose, that reconstructed field is mirrored
    /// about the propagation axis relative to the physically correct one. See
    /// `polarization_tests::frame_rotation_round_trip_recovers_the_original_field_direction`.
    #[must_use]
    pub fn frame_rotation(psi: f32) -> Mat4 {
        let c2 = (2.0 * psi).cos();
        let s2 = (2.0 * psi).sin();
        Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, 0.0, c2, -s2, 0.0, 0.0, s2, c2, 0.0, 0.0, 0.0, 0.0, 1.0,
        ])
    }

    /// Dielectric Fresnel reflection Mueller matrix.
    #[must_use]
    pub fn fresnel_reflection(r_s: f32, r_p: f32) -> Mat4 {
        let rs2 = r_s * r_s;
        let rp2 = r_p * r_p;
        let a = f32::midpoint(rs2, rp2);
        let b = 0.5 * (rs2 - rp2);
        let c = r_s * r_p;

        Mat4::from_cols_array(&[
            a, b, 0.0, 0.0, b, a, 0.0, 0.0, 0.0, 0.0, c, 0.0, 0.0, 0.0, 0.0, c,
        ])
    }

    /// Dielectric Fresnel transmission Mueller matrix with flux conservation.
    #[must_use]
    pub fn fresnel_transmission(
        n1: f32,
        n2: f32,
        cos_i: f32,
        cos_t: f32,
        t_s: f32,
        t_p: f32,
    ) -> Mat4 {
        let factor = (n2 * cos_t) / ((n1 * cos_i).max(1e-6));
        let ts2 = t_s * t_s * factor;
        let tp2 = t_p * t_p * factor;
        let a = f32::midpoint(ts2, tp2);
        let b = 0.5 * (ts2 - tp2);
        let c = t_s * t_p * factor;

        Mat4::from_cols_array(&[
            a, b, 0.0, 0.0, b, a, 0.0, 0.0, 0.0, 0.0, c, 0.0, 0.0, 0.0, 0.0, c,
        ])
    }

    /// Total Internal Reflection (TIR) phase retardation matrix delta = `delta_p` - `delta_s`.
    #[must_use]
    pub fn tir_retardation(delta: f32) -> Mat4 {
        let cos_d = delta.cos();
        let sin_d = delta.sin();
        Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, cos_d, -sin_d, 0.0, 0.0, sin_d, cos_d,
        ])
    }
}

/// World-space electric-field polarization direction for `stokes`.
///
/// Given the current reference-frame axes: `s_axis` (this frame's Q=+1 basis vector,
/// perpendicular to the plane of incidence -- e.g. `trace_spectral_ray`'s
/// `current_plane_normal`) and `propagation_dir` (the wave's direction of travel,
/// `k_hat`).
///
/// Builds the in-plane axis `p_axis = k_hat x s_axis` so `(s_axis, p_axis, k_hat)` is
/// right-handed, matching the Q/U sign convention `MuellerMatrix::fresnel_reflection`
/// already commits to (its Q output, `0.5*(r_s^2 - r_p^2)`, is positive when
/// s-polarized reflectance dominates -- i.e. Q=+1 IS the `s_axis` direction), then
/// rotates within that plane by the azimuth `polarization_azimuth()`:
/// `e = cos(psi)*s_axis + sin(psi)*p_axis`.
///
/// Falls back to an arbitrary axis pair perpendicular to `propagation_dir` when
/// `s_axis` is degenerate (near-zero length, e.g. at normal incidence where the plane
/// of incidence itself is undefined) so the result stays a well-defined unit vector.
///
/// The returned vector's sign is arbitrary (linear-polarization azimuth is only
/// defined mod pi) -- callers evaluating a quadratic form (`e . A . e`, see
/// `birefringence::AbsorptionTensor3`) are sign-invariant anyway.
#[must_use]
pub fn electric_field_direction(
    stokes: &StokesVector,
    s_axis: Vec3,
    propagation_dir: Vec3,
) -> Vec3 {
    let k_hat = propagation_dir.normalize_or_zero();
    let s_raw = s_axis - k_hat * k_hat.dot(s_axis);
    let s_hat = if s_raw.length_squared() > 1e-8 {
        s_raw.normalize()
    } else {
        arbitrary_perpendicular(k_hat)
    };
    let p_hat = k_hat.cross(s_hat);
    let psi = stokes.polarization_azimuth();
    let e = psi.cos() * s_hat + psi.sin() * p_hat;
    if e.length_squared() > 1e-8 {
        e.normalize()
    } else {
        s_hat
    }
}

/// An arbitrary unit vector perpendicular to unit vector `n`, used as a stable
/// fallback reference axis when no better one (e.g. a plane-of-incidence normal) is
/// available.
fn arbitrary_perpendicular(n: Vec3) -> Vec3 {
    let a = if n.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    (a - n * n.dot(a)).normalize_or_zero()
}

#[cfg(test)]
mod electric_field_direction_tests {
    use super::*;

    /// At azimuth 0 (all polarization along Q, U=0), the field direction must be
    /// exactly `s_axis` (up to sign) regardless of propagation direction.
    #[test]
    fn zero_azimuth_gives_s_axis() {
        let stokes = StokesVector::new(1.0, 1.0, 0.0, 0.0);
        let s_axis = Vec3::new(1.0, 0.0, 0.0);
        let k_hat = Vec3::new(0.0, 0.0, 1.0);
        let e = electric_field_direction(&stokes, s_axis, k_hat);
        assert!(
            (e.dot(s_axis).abs() - 1.0).abs() < 1e-5,
            "expected e parallel to s_axis, got {e:?}"
        );
    }

    /// At azimuth 45 degrees (Q=0, U=1), the azimuth is measured from `s_axis`, so the
    /// field direction must be the diagonal `(s_axis + p_axis)/sqrt(2)` (up to sign) --
    /// not `p_axis` itself, which only occurs at azimuth 90 degrees.
    #[test]
    fn forty_five_degree_azimuth_gives_diagonal() {
        let stokes = StokesVector::new(1.0, 0.0, 1.0, 0.0);
        let s_axis = Vec3::new(1.0, 0.0, 0.0);
        let k_hat = Vec3::new(0.0, 0.0, 1.0);
        let p_axis = k_hat.cross(s_axis);
        let diagonal = (s_axis + p_axis).normalize();
        let e = electric_field_direction(&stokes, s_axis, k_hat);
        assert!(
            (e.dot(diagonal).abs() - 1.0).abs() < 1e-5,
            "expected e parallel to (s_axis + p_axis) diagonal, got {e:?}"
        );
    }

    /// At azimuth 90 degrees (Q=-1, U=0), the field direction must be exactly `p_axis =
    /// k_hat x s_axis` (up to sign).
    #[test]
    fn ninety_degree_azimuth_gives_p_axis() {
        let stokes = StokesVector::new(1.0, -1.0, 0.0, 0.0);
        let s_axis = Vec3::new(1.0, 0.0, 0.0);
        let k_hat = Vec3::new(0.0, 0.0, 1.0);
        let p_axis = k_hat.cross(s_axis);
        let e = electric_field_direction(&stokes, s_axis, k_hat);
        assert!(
            (e.dot(p_axis).abs() - 1.0).abs() < 1e-5,
            "expected e parallel to p_axis, got {e:?}"
        );
    }

    /// The returned direction must always be perpendicular to the propagation
    /// direction, for a spread of azimuths and a non-axis-aligned `k_hat`/`s_axis`.
    #[test]
    fn field_direction_is_always_perpendicular_to_propagation() {
        let k_hat = Vec3::new(0.4, -0.5, 0.767).normalize();
        let s_axis = k_hat.cross(Vec3::Y).normalize();
        for i in 0..16 {
            let psi = (i as f32) * std::f32::consts::PI / 8.0;
            let stokes = StokesVector::new(1.0, (2.0 * psi).cos(), (2.0 * psi).sin(), 0.0);
            let e = electric_field_direction(&stokes, s_axis, k_hat);
            assert!(
                e.dot(k_hat).abs() < 1e-4,
                "e must stay perpendicular to k_hat (psi={psi}, got e.k={})",
                e.dot(k_hat)
            );
        }
    }

    /// Degenerate `s_axis` (zero length -- undefined plane of incidence) must not
    /// panic or produce NaN, and the result must still be a unit vector perpendicular
    /// to propagation.
    #[test]
    fn degenerate_s_axis_falls_back_gracefully() {
        let stokes = StokesVector::new(1.0, 0.3, 0.4, 0.0);
        let k_hat = Vec3::new(0.0, 1.0, 0.0);
        let e = electric_field_direction(&stokes, Vec3::ZERO, k_hat);
        assert!(
            e.is_finite(),
            "result must be finite for degenerate s_axis, got {e:?}"
        );
        assert!(
            (e.length() - 1.0).abs() < 1e-4,
            "result must be a unit vector, got {e:?}"
        );
        assert!(
            e.dot(k_hat).abs() < 1e-4,
            "result must stay perpendicular to k_hat, got {e:?}"
        );
    }
}

#[cfg(test)]
mod frame_rotation_tests {
    use super::*;

    /// `MuellerMatrix::frame_rotation(psi)` transforms a Stokes vector out of a
    /// reference frame into a SECOND frame whose axis is the first rotated by `+psi`
    /// about the propagation direction -- so decoding the rotated Stokes vector against
    /// that second (equally rotated) axis must recover exactly the ORIGINAL, physically
    /// unchanged electric-field direction. This is precisely the round trip the
    /// pre-fix transposed matrix broke: light polarized along the original `s_axis`,
    /// with the frame then rotated +30 degrees, reconstructed the MIRROR field
    /// (effectively `2*psi` away) instead of the true, unrotated field direction.
    #[test]
    fn frame_rotation_round_trip_recovers_the_original_field_direction() {
        let k_hat = Vec3::new(0.3, 0.5, 0.81).normalize();
        let s_axis0 = arbitrary_perpendicular(k_hat);
        let p_axis0 = k_hat.cross(s_axis0);

        for psi_deg in [-120.0f32, -45.0, -10.0, 10.0, 30.0, 60.0, 90.0, 150.0] {
            let psi = psi_deg.to_radians();
            // The reference frame's own axis, rotated by the SAME angle the Mueller
            // matrix is about to apply to the Stokes vector.
            let s_axis1 = (psi.cos() * s_axis0 + psi.sin() * p_axis0).normalize();

            // Light fully polarized along the ORIGINAL s_axis0 (azimuth 0 in frame 0).
            let stokes0 = StokesVector::new(1.0, 1.0, 0.0, 0.0);
            let stokes1 = stokes0.apply_matrix(&MuellerMatrix::frame_rotation(psi));

            let e_reconstructed = electric_field_direction(&stokes1, s_axis1, k_hat);

            assert!(
                e_reconstructed.dot(s_axis0).abs() > 0.999,
                "psi={psi_deg} deg: round trip should recover the original field \
                 direction s_axis0={s_axis0:?}, got e={e_reconstructed:?} \
                 (dot={})",
                e_reconstructed.dot(s_axis0)
            );
        }
    }
}

/// Pins the Fresnel/TIR sign conventions in this file against pleochroism
/// (`birefringence::AbsorptionTensor3`), rather than only checking each subsystem is
/// internally self-consistent in isolation.
#[cfg(test)]
mod fresnel_tir_pleochroism_sign_convention_tests {
    use super::*;
    use crate::optics::birefringence::AbsorptionTensor3;

    /// `MuellerMatrix::fresnel_reflection`'s own doc comment states its sign convention
    /// directly: "Q=+1 IS the `s_axis` direction". This test pins that claim against BOTH
    /// halves of the pipeline that convention has to survive intact through: reflecting a
    /// purely s- (then p-) polarized wave off a dielectric interface and decoding the
    /// result back to a world-space field direction via `electric_field_direction` must
    /// reconstruct `s_axis` (resp. `p_axis`) -- AND, since a strongly dichroic material's
    /// absorption depends on that exact same reconstructed direction
    /// (`birefringence::AbsorptionTensor3::quadratic_form`), setting the tensor's c-axis
    /// to `s_axis` itself must make the s-polarized reflection read the EXTRAORDINARY
    /// coefficient and the p-polarized reflection read the ORDINARY one. A convention
    /// mismatch between the Fresnel matrices and the pleochroism tensor -- even if each
    /// were self-consistent on its own -- would show up here as reading the wrong
    /// principal coefficient, not merely the wrong axis label.
    #[test]
    fn fresnel_reflection_sign_convention_agrees_with_pleochroic_axis_convention() {
        let s_axis = Vec3::X;
        let k_hat = Vec3::Z;
        let p_axis = k_hat.cross(s_axis);

        // Strongly dichroic uniaxial tensor, c-axis pinned to s_axis itself.
        let alpha_o = 0.4f32;
        let alpha_e = 6.0f32;
        let tensor = AbsorptionTensor3::uniaxial(alpha_o, alpha_e, s_axis);

        // Arbitrary distinct reflection coefficients -- only their inequality (so s vs.
        // p reflectance genuinely differ) and magnitude (< 1, a physical reflectance)
        // matter here, not any particular angle of incidence.
        let r_s = 0.6f32;
        let r_p = 0.2f32;
        let refl = MuellerMatrix::fresnel_reflection(r_s, r_p);

        // s-polarized in (Q=+1).
        let s_in = StokesVector::new(1.0, 1.0, 0.0, 0.0);
        let s_out = s_in.apply_matrix(&refl);
        assert!(
            s_out.degree_of_polarization() > 0.999,
            "reflecting pure s-polarized light off an isotropic interface must stay \
             fully polarized (s/p are its eigenpolarizations), got dop={}",
            s_out.degree_of_polarization()
        );
        let e_s = electric_field_direction(&s_out, s_axis, k_hat);
        assert!(
            e_s.dot(s_axis).abs() > 0.999,
            "reflected s-polarized field must reconstruct along s_axis (up to sign), got e={e_s:?}"
        );
        let alpha_along_s = tensor.quadratic_form(e_s);
        assert!(
            (alpha_along_s - alpha_e).abs() < 1e-4,
            "s-polarized field direction should read the EXTRAORDINARY coefficient \
             (c_axis pinned to s_axis), got {alpha_along_s} vs alpha_e={alpha_e}"
        );

        // p-polarized in (Q=-1).
        let p_in = StokesVector::new(1.0, -1.0, 0.0, 0.0);
        let p_out = p_in.apply_matrix(&refl);
        assert!(
            p_out.degree_of_polarization() > 0.999,
            "reflecting pure p-polarized light off an isotropic interface must stay \
             fully polarized, got dop={}",
            p_out.degree_of_polarization()
        );
        let e_p = electric_field_direction(&p_out, s_axis, k_hat);
        assert!(
            e_p.dot(p_axis).abs() > 0.999,
            "reflected p-polarized field must reconstruct along p_axis (up to sign), got e={e_p:?}"
        );
        let alpha_along_p = tensor.quadratic_form(e_p);
        assert!(
            (alpha_along_p - alpha_o).abs() < 1e-4,
            "p-polarized field direction should read the ORDINARY coefficient \
             (perpendicular to c_axis == s_axis), got {alpha_along_p} vs alpha_o={alpha_o}"
        );
    }

    /// A 45-degree linear input run through `frame_rotation` (a rotation in the Q-U
    /// plane) then `tir_retardation` (a rotation in the U-V plane) must (a) have its
    /// degree of polarization preserved exactly -- both are orthogonal (unitary)
    /// transforms on the `(Q, U, V)` 3-vector, so their composition cannot change its
    /// norm -- and (b) have `frame_rotation` alone rotate `(Q, U)` exactly as its own
    /// `Mat4::from_cols_array` layout implies: column-major storage means row `i`,
    /// column `j` of the matrix is `array[4*j + i]`, so the Q row (row 1) reads
    /// `[0, c2, s2, 0]` across columns 0-3 and the U row (row 2) reads `[0, -s2, c2, 0]`
    /// -- i.e. `Q' = cos(2psi)*Q + sin(2psi)*U`, `U' = -sin(2psi)*Q + cos(2psi)*U`.
    #[test]
    fn tir_retardation_and_frame_rotation_preserve_dop_and_rotate_q_u_as_r_2psi() {
        let stokes0 = StokesVector::new(1.0, 0.0, 1.0, 0.0);
        let dop0 = stokes0.degree_of_polarization();
        assert!(
            (dop0 - 1.0).abs() < 1e-5,
            "test premise: 45-degree linear input must be fully polarized, got {dop0}"
        );

        for psi_deg in [-70.0f32, -20.0, 15.0, 55.0, 100.0] {
            let psi = psi_deg.to_radians();
            let rotated = stokes0.apply_matrix(&MuellerMatrix::frame_rotation(psi));

            let c2 = (2.0 * psi).cos();
            let s2 = (2.0 * psi).sin();
            let expected_q = c2.mul_add(stokes0.q, s2 * stokes0.u);
            let expected_u = (-s2).mul_add(stokes0.q, c2 * stokes0.u);
            assert!(
                (rotated.q - expected_q).abs() < 1e-4 && (rotated.u - expected_u).abs() < 1e-4,
                "psi={psi_deg} deg: frame_rotation should rotate (Q,U) exactly as R(2*psi) \
                 predicts, got (Q,U)=({}, {}) vs expected ({expected_q}, {expected_u})",
                rotated.q,
                rotated.u
            );

            for delta_deg in [-150.0f32, -40.0, 30.0, 120.0] {
                let delta = delta_deg.to_radians();
                let retarded = rotated.apply_matrix(&MuellerMatrix::tir_retardation(delta));
                let dop_final = retarded.degree_of_polarization();
                assert!(
                    (dop_final - dop0).abs() < 1e-4,
                    "psi={psi_deg} delta={delta_deg}: degree of polarization must be \
                     preserved by frame_rotation + tir_retardation (both unitary \
                     rotations of (Q,U,V)), got {dop_final} vs {dop0}"
                );
            }
        }
    }

    /// `tir_retardation`'s U-V handedness was pinned only by the
    /// degree-of-polarization test above, which is invariant under the SIGN of
    /// `delta` (and so cannot catch a future convention flip, e.g. accidentally
    /// negating `delta` or swapping the `sin_d`/`-sin_d` placement in the matrix
    /// literal). Pins it instead with a physical Fresnel rhomb: two total-internal-
    /// reflection bounces at `n=1.51`, `i=54.62deg` -- the classic BK7-glass rhomb
    /// design -- each contribute a retardation `delta` from the crate's own
    /// `tir_phase_delta` (`tan(delta/2) = cos_i*sqrt(n^2*sin_i^2 - 1)/(n*sin_i^2)`,
    /// called directly here rather than reimplemented, so this test tracks the real
    /// production formula) that comes out to ~45 degrees, so two bounces sum to a
    /// 90-degree quarter-wave retardation and must turn +45-degree linear light
    /// circular.
    ///
    /// `tir_retardation(delta)`'s matrix (see its own doc comment / the
    /// `from_cols_array` column-major layout worked out there) applies
    /// `U' = cos(delta)*U + sin(delta)*V`, `V' = -sin(delta)*U + cos(delta)*V` to an
    /// incoming `(Q=0, U=1, V=0)` state. Working that through two applications with
    /// `delta ~= 45deg` (so `2*delta ~= 90deg`) gives `U'' = cos(2*delta) ~= 0`,
    /// `V'' = -sin(2*delta) ~= -1` -- i.e. under this crate's convention a +45-degree
    /// linear Fresnel rhomb comes out LEFT circular (`V = -1`), not right circular.
    /// That sign (verified numerically below, not just algebraically) is the pin: a
    /// future change to the retarder convention that flips it should fail this test.
    ///
    /// A second rhomb (four TIR bounces total, `4*delta ~= 180deg`, a half-wave
    /// retardation) must then rotate the linear axis by 90 degrees, returning -45-
    /// degree linear light (`U ~= -1`, `V ~= 0`) -- the standard double-Fresnel-rhomb
    /// polarization rotator.
    #[test]
    fn tir_retardation_fresnel_rhomb_pins_circular_handedness() {
        let n = 1.51_f32;
        let i = 54.62_f32.to_radians();
        let cos_i = i.cos();
        let sin_i = i.sin();
        let delta = crate::optics::raytracer::refraction::tir_phase_delta(n, cos_i, sin_i);
        assert!(
            (delta.to_degrees() - 45.0).abs() < 0.5,
            "test premise: n=1.51 at i=54.62deg must be the classic ~45-degree-per-bounce \
             Fresnel rhomb design, got delta={} deg",
            delta.to_degrees()
        );

        // +45-degree linear input.
        let linear_plus_45 = StokesVector::new(1.0, 0.0, 1.0, 0.0);

        // One rhomb == two TIR bounces.
        let after_one_bounce = linear_plus_45.apply_matrix(&MuellerMatrix::tir_retardation(delta));
        let after_rhomb = after_one_bounce.apply_matrix(&MuellerMatrix::tir_retardation(delta));

        assert!(
            after_rhomb.q.abs() < 1e-3 && after_rhomb.u.abs() < 1e-3,
            "one Fresnel rhomb (two TIR bounces, delta={} deg each) applied to +45-degree \
             linear light must produce (near-)pure circular polarization (Q,U ~= 0), got \
             Q={} U={}",
            delta.to_degrees(),
            after_rhomb.q,
            after_rhomb.u
        );
        assert!(
            (after_rhomb.v - (-1.0)).abs() < 1e-3,
            "sign pin: with the crate's retarder convention U' = cos(delta)*U + sin(delta)*V, \
             V' = -sin(delta)*U + cos(delta)*V, a +45-degree linear input through two 45-degree \
             TIR retardations (one Fresnel rhomb) yields V = -1 (left circular), got V={} \
             (Q={}, U={})",
            after_rhomb.v,
            after_rhomb.q,
            after_rhomb.u
        );

        // A second rhomb (four TIR bounces total) must return LINEAR light rotated
        // 90 degrees, i.e. -45-degree linear.
        let after_third_bounce = after_rhomb.apply_matrix(&MuellerMatrix::tir_retardation(delta));
        let after_two_rhombs =
            after_third_bounce.apply_matrix(&MuellerMatrix::tir_retardation(delta));

        assert!(
            after_two_rhombs.q.abs() < 1e-3 && after_two_rhombs.v.abs() < 1e-3,
            "two Fresnel rhombs (four TIR bounces total, a half-wave retardation) must \
             return light to a linear state (Q,V ~= 0), got Q={} V={}",
            after_two_rhombs.q,
            after_two_rhombs.v
        );
        assert!(
            (after_two_rhombs.u - (-1.0)).abs() < 1e-3,
            "two Fresnel rhombs must rotate +45-degree linear light to -45-degree linear \
             (U ~= -1), got U={} (Q={}, V={})",
            after_two_rhombs.u,
            after_two_rhombs.q,
            after_two_rhombs.v
        );
    }
}
