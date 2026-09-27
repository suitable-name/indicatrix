//! The isotropic (air, `n1`) -> uniaxial crystal entry interface's closed-form solve.
//!
//! Batched to share the incidence-side isotropic mode data across every hero channel
//! of one bounce.

use super::{
    complex::{CVec3, Cplx},
    frame::{
        ModeFields, UniaxialFrame, extraordinary_mode_fields, ordinary_mode_fields, poynting_z,
        uniaxial_q_roots,
    },
    linalg::{solve4_two_rhs, tangential_components},
};
use glam::Vec3;

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
    /// projection via [`azimuth2_in_frame`](super::azimuth2_in_frame).
    pub o_hat: Vec3,
    pub e_hat: Vec3,
}

/// Requirement 7 (performance): [`entry_solve`] is a thin wrapper around
/// [`entry_solve_pair`] that discards whichever of the (s, p) pair the caller didn't
/// ask for -- kept only for the tests and call sites that only need one incident
/// polarization; every production call site in `refraction.rs` needs BOTH (s and p
/// incidence are the two rows of the reflected/transmitted Jones matrices this
/// module's callers build), and calling `entry_solve` twice recomputed
/// `uniaxial_q_roots` (two [`Cplx::sqrt_forward_branch`] calls, each an `atan2` plus
/// two `sqrt` calls plus a `sin`/`cos` pair -- by far the most expensive part of this
/// function per call) and both mode fields TWICE for identical inputs (`incident_s`
/// never affects them, only the boundary-matching right-hand side does) -- measured to
/// roughly double this module's own share of a uniaxial bounce's cost.
/// `entry_solve_pair` shares that work once and solves both right-hand sides through
/// one Gaussian elimination. No production call site uses `entry_solve` any more (all
/// converted to `entry_solve_pair`) -- genuinely dead outside this module's own tests,
/// hence the `cfg_attr`'d expectation rather than a bare one (see the identical
/// pattern, and its own doc comment, on `InternalPolarizationSolution`'s `t_s` field
/// in `internal.rs`).
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
