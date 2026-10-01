//! Tests for the uniaxial Fresnel closed-form solution: the isotropic limit against
//! this crate's existing scalar Fresnel formula, energy conservation (including TIR)
//! at both interfaces, the normal-incidence special case, Rutile's divergence from the
//! old effective-index approximation, the Mueller-matrix conversion, and the batched
//! entry-solve path's bitwise equivalence to the unbatched one.

use super::{
    complex::Cplx,
    entry::entry_solve,
    entry_incidence_frame, entry_solve_pair, entry_solve_pair_with_incidence,
    frame::{
        UniaxialFrame, extraordinary_mode_fields, ordinary_mode_fields, poynting_z,
        uniaxial_q_roots,
    },
    internal::internal_solve,
    mueller::jones_to_mueller,
};
use glam::Vec3;

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
/// including its sign convention -- this is the same check the isotropic-to-isotropic
/// check run against the reference prototype before the Rust implementation existed.
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
                        let cos_kc = frame.gamma.mul_add(frame.cos_i, frame.alpha * frame.sin_i);
                        let sin2 = cos_kc.mul_add(-cos_kc, 1.0).max(0.0);
                        1.0 / (cos_kc * cos_kc / (n_o * n_o) + sin2 / (n_e * n_e)).sqrt()
                    };
                    let sol = internal_solve(n_inc, n_o, n_e, c_axis, &frame, incident_is_ordinary);
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

/// Requirement 9: Rutile's extreme birefringence (`+0.2957`, the highest of any
/// built-in) should make the new closed-form entry Fresnel reflectance visibly
/// diverge from the OLD "effective index" scalar approximation (a single
/// isotropic-style Fresnel evaluated at `BirefringenceParams::
/// effective_extraordinary_index`) at an oblique incidence with the optic axis NOT
/// confined to the
/// plane of incidence (the general case where s/p<->o/e coupling is genuinely
/// present, per this module tree's own top-level doc comment). Proves the new
/// closed-form path is materially different physics, not a no-op refactor.
///
/// Compares the p-INCIDENT reflectance specifically (`|r_pp|^2` vs. the old
/// isotropic `r_p^2` at the same effective index) rather than the unpolarized
/// average: the coupling correction partially cancels between s- and p-incidence
/// in the unpolarized average (each channel's own `r_sp`/`r_ps` cross-leakage adds
/// intensity the old scalar formula didn't have, but the DIAGONAL `r_ss`/`r_pp`
/// terms drop correspondingly, since energy is conserved) -- the single-
/// polarization reflectance is where the ~15% deviation is
/// actually visible; this geometry (20 degrees incidence, optic axis close to the
/// surface plane but with a genuine out-of-plane `beta` component) was found, by a
/// small sweep over angle/axis combinations, to land closest to that figure.
///
/// Bound derivation: the geometry (20 degrees, `c_axis`) is fixed, so the deviation
/// depends on the indices only through the relative anisotropy `(n_e - n_o) / n_o`.
/// That is `0.2957 / 2.6129 = 0.1132` for the Sellmeier-3 Rutile fit (`n_o = 2.6129`,
/// `n_e = 2.9086` at 589.3 nm) against `0.1097` for the earlier Cauchy pair
/// (`0.287 / 2.616`), a 3% increase, which moves a ~15% deviation by well under one
/// percentage point even if it scaled quadratically. The `0.08..=0.25` window is
/// therefore unchanged.
#[test]
fn rutile_fresnel_diverges_from_isotropic_effective_index_approximation_by_about_15_percent() {
    use crate::optics::{birefringence::BirefringenceParams, materials::GemMaterial};

    let rutile = GemMaterial::by_name("Rutile").expect("Rutile must be a built-in material");
    let n_o = rutile.dispersion.evaluate(589.3);
    let n_e = rutile.extraordinary_index_at(589.3, n_o);
    assert!(
        (n_e - n_o - rutile.birefringence_delta).abs() < 1e-3
            && (rutile.birefringence_delta - 0.2957).abs() < 1e-4,
        "test premise: Rutile's birefringence should be +0.2957, got n_o={n_o} n_e={n_e}"
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
    let r_p_old = n_eff.mul_add(frame.cos_i, -cos_t_old) / n_eff.mul_add(frame.cos_i, cos_t_old);
    let r_pp_old_sq = r_p_old * r_p_old;

    // NEW closed-form: the true p-incident, p-reflected power `|r_pp|^2`.
    let sol_p = entry_solve(1.0, n_o, n_e, c_axis, &frame, false);
    let r_pp_new_sq = sol_p.r_p.norm_sqr();

    let relative_deviation = (r_pp_new_sq - r_pp_old_sq).abs() / r_pp_old_sq;
    assert!(
        (0.08..=0.25).contains(&relative_deviation),
        "Rutile's new closed-form |r_pp|^2 (={r_pp_new_sq}) should diverge from the \
         old effective-index approximation r_p^2 (={r_pp_old_sq}) by roughly the \
         ~15% expected -- got {:.2}%",
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
/// shared [`super::EntryIncidenceFrame`] (the "batched" path every uniaxial entry call
/// site uses -- see `refraction::apply_uniaxial_entry_bounce`) must be
/// BIT-IDENTICAL, field for field, to [`entry_solve_pair`] called completely fresh
/// (the "scalar" path, computing its own `EntryIncidenceFrame` from scratch every
/// time) -- across random angles, optic-axis
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

        let cplx_bits_eq =
            |a: Cplx, b: Cplx| a.re.to_bits() == b.re.to_bits() && a.im.to_bits() == b.im.to_bits();
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
