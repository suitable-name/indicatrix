// optics::raytracer::uniaxial_fresnel::EntryIncidenceFrame / entry_incidence_frame --
// performance (requirement 7): built once per bounce by the caller (`n1 == 1.0` for
// every channel at an air->crystal entry), shared across every channel's own
// `entry_solve_pair_with_incidence` call -- see the CPU type's own doc comment.
struct EntryIncidenceFrameW {
    incident_s: array<Cplx, 4>,
    incident_p: array<Cplx, 4>,
    reflected_s: array<Cplx, 4>,
    reflected_p: array<Cplx, 4>,
}

fn entry_incidence_frame(n1: f32, frame: UniaxialFrameW) -> EntryIncidenceFrameW {
    let k_tan = n1 * frame.sin_i;
    let k_fwd = cvec3_wavevector(frame.that, frame.zhat, k_tan, cplx_re(n1 * frame.cos_i));
    let k_bwd = cvec3_wavevector(frame.that, frame.zhat, k_tan, cplx_re(-n1 * frame.cos_i));
    let s_axis_c = cvec3_from_real(frame.s_axis);
    let es_fwd = s_axis_c;
    let hs_fwd = cvec3_cross(k_fwd, es_fwd);
    let ep_bwd = cvec3_scale_real(cvec3_cross(k_bwd, s_axis_c), -1.0 / (n1 * n1));
    let ep_fwd = cvec3_scale_real(cvec3_cross(k_fwd, s_axis_c), -1.0 / (n1 * n1));

    var inc_s: ModeFieldsW;
    inc_s.k = k_fwd;
    inc_s.e = es_fwd;
    inc_s.h = hs_fwd;

    var inc_p: ModeFieldsW;
    inc_p.k = k_fwd;
    inc_p.e = ep_fwd;
    inc_p.h = cvec3_cross(k_fwd, ep_fwd);

    var refl_s: ModeFieldsW;
    refl_s.k = k_bwd;
    refl_s.e = s_axis_c;
    refl_s.h = cvec3_cross(k_bwd, s_axis_c);

    var refl_p: ModeFieldsW;
    refl_p.k = k_bwd;
    refl_p.e = ep_bwd;
    refl_p.h = cvec3_cross(k_bwd, ep_bwd);

    var result: EntryIncidenceFrameW;
    result.incident_s = tangential_components(inc_s, frame.that, frame.s_axis);
    result.incident_p = tangential_components(inc_p, frame.that, frame.s_axis);
    result.reflected_s = tangential_components(refl_s, frame.that, frame.s_axis);
    result.reflected_p = tangential_components(refl_p, frame.that, frame.s_axis);
    return result;
}

// optics::raytracer::uniaxial_fresnel::EntryPolarizationSolution /
// entry_solve_pair_with_incidence
struct EntrySolW {
    r_s: Cplx,
    r_p: Cplx,
    t_o: Cplx,
    t_e: Cplx,
    flux_o: f32,
    flux_e: f32,
    o_hat: vec3<f32>,
    e_hat: vec3<f32>,
}

struct EntryPairResultW {
    s_sol: EntrySolW,
    p_sol: EntrySolW,
}

fn entry_solve_pair_with_incidence(
    inc: EntryIncidenceFrameW,
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: vec3<f32>,
    frame: UniaxialFrameW,
) -> EntryPairResultW {
    let k_tan = n1 * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);
    let fo = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus);
    let fe = extraordinary_mode_fields(n_o, n_e, c_axis, frame.that, frame.zhat, k_tan, roots.qe_plus);

    let eo_c = tangential_components(fo, frame.that, frame.s_axis);
    let ee_c = tangential_components(fe, frame.that, frame.s_axis);

    var a: array<array<Cplx, 4>, 4>;
    var b_s: array<Cplx, 4>;
    var b_p: array<Cplx, 4>;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        a[i][0] = inc.reflected_s[i];
        a[i][1] = inc.reflected_p[i];
        a[i][2] = cplx_sub(cplx_zero(), eo_c[i]);
        a[i][3] = cplx_sub(cplx_zero(), ee_c[i]);
        b_s[i] = cplx_sub(cplx_zero(), inc.incident_s[i]);
        b_p[i] = cplx_sub(cplx_zero(), inc.incident_p[i]);
    }
    let sol = solve4_two_rhs(a, b_s, b_p);

    let flux_o = abs(poynting_z(fo, cplx_re(1.0), frame.zhat));
    let flux_e = abs(poynting_z(fe, cplx_re(1.0), frame.zhat));
    let o_hat = normalize_or_zero(fo.e.re);
    let e_hat = normalize_or_zero(fe.e.re);

    var s_sol: EntrySolW;
    s_sol.r_s = sol.b1[0];
    s_sol.r_p = sol.b1[1];
    s_sol.t_o = sol.b1[2];
    s_sol.t_e = sol.b1[3];
    s_sol.flux_o = flux_o;
    s_sol.flux_e = flux_e;
    s_sol.o_hat = o_hat;
    s_sol.e_hat = e_hat;

    var p_sol: EntrySolW;
    p_sol.r_s = sol.b2[0];
    p_sol.r_p = sol.b2[1];
    p_sol.t_o = sol.b2[2];
    p_sol.t_e = sol.b2[3];
    p_sol.flux_o = flux_o;
    p_sol.flux_e = flux_e;
    p_sol.o_hat = o_hat;
    p_sol.e_hat = e_hat;

    var result: EntryPairResultW;
    result.s_sol = s_sol;
    result.p_sol = p_sol;
    return result;
}

// optics::raytracer::uniaxial_fresnel::entry_solve_pair -- thin wrapper, kept for the
// Tier 2 check's single-call convenience (bit-identical to calling
// `entry_incidence_frame` + `entry_solve_pair_with_incidence` directly, which is what
// every production megakernel call site does instead, sharing one `EntryIncidenceFrameW`
// across all 8 channels of a bounce -- see this section's own header comment).
fn entry_solve_pair(n1: f32, n_o: f32, n_e: f32, c_axis: vec3<f32>, frame: UniaxialFrameW) -> EntryPairResultW {
    let inc = entry_incidence_frame(n1, frame);
    return entry_solve_pair_with_incidence(inc, n1, n_o, n_e, c_axis, frame);
}

// optics::raytracer::uniaxial_fresnel::InternalPolarizationSolution / internal_solve
struct InternalSolW {
    r_o: Cplx,
    r_e: Cplx,
    t_s: Cplx,
    t_p: Cplx,
    flux_ro: f32,
    flux_re: f32,
    flux_ts: f32,
    flux_tp: f32,
    flux_inc: f32,
    o_hat: vec3<f32>,
    e_hat: vec3<f32>,
}

fn internal_solve(
    n_mode_inc: f32,
    n_o: f32,
    n_e: f32,
    c_axis: vec3<f32>,
    frame: UniaxialFrameW,
    incident_is_ordinary: bool,
) -> InternalSolW {
    let k_tan = n_mode_inc * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);

    var f_inc: ModeFieldsW;
    if (incident_is_ordinary) {
        f_inc = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus);
    } else {
        f_inc = extraordinary_mode_fields(n_o, n_e, c_axis, frame.that, frame.zhat, k_tan, roots.qe_plus);
    }
    let f_o_bwd = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_minus);
    let f_e_bwd = extraordinary_mode_fields(n_o, n_e, c_axis, frame.that, frame.zhat, k_tan, roots.qe_minus);

    let n2 = 1.0;
    let zeta = k_tan / n2;
    var cos_t: Cplx;
    if (abs(zeta) <= 1.0) {
        cos_t = cplx_re(sqrt(max(fma(zeta, -zeta, 1.0), 0.0)));
    } else {
        var ct: Cplx;
        ct.re = 0.0;
        ct.im = sqrt(fma(zeta, zeta, -1.0));
        cos_t = ct;
    }
    let k_iso = cvec3_wavevector(frame.that, frame.zhat, k_tan, cplx_scale(cos_t, n2));
    let s_axis_c = cvec3_from_real(frame.s_axis);
    let es_t = s_axis_c;
    let hs_t = cvec3_cross(k_iso, es_t);
    let ep_t = cvec3_scale_real(cvec3_cross(k_iso, s_axis_c), -1.0 / (n2 * n2));
    let hp_t = cvec3_cross(k_iso, ep_t);

    var f_s_t: ModeFieldsW;
    f_s_t.k = k_iso;
    f_s_t.e = es_t;
    f_s_t.h = hs_t;

    var f_p_t: ModeFieldsW;
    f_p_t.k = k_iso;
    f_p_t.e = ep_t;
    f_p_t.h = hp_t;

    let inc_c = tangential_components(f_inc, frame.that, frame.s_axis);
    let eo_c = tangential_components(f_o_bwd, frame.that, frame.s_axis);
    let ee_c = tangential_components(f_e_bwd, frame.that, frame.s_axis);
    let es_c = tangential_components(f_s_t, frame.that, frame.s_axis);
    let ep_c = tangential_components(f_p_t, frame.that, frame.s_axis);

    var a: array<array<Cplx, 4>, 4>;
    var b: array<Cplx, 4>;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        a[i][0] = eo_c[i];
        a[i][1] = ee_c[i];
        a[i][2] = cplx_sub(cplx_zero(), es_c[i]);
        a[i][3] = cplx_sub(cplx_zero(), ep_c[i]);
        b[i] = cplx_sub(cplx_zero(), inc_c[i]);
    }
    let sol = solve4_single(a, b);

    var result: InternalSolW;
    result.r_o = sol[0];
    result.r_e = sol[1];
    result.t_s = sol[2];
    result.t_p = sol[3];
    result.flux_ro = abs(poynting_z(f_o_bwd, cplx_re(1.0), frame.zhat));
    result.flux_re = abs(poynting_z(f_e_bwd, cplx_re(1.0), frame.zhat));
    result.flux_ts = abs(poynting_z(f_s_t, cplx_re(1.0), frame.zhat));
    result.flux_tp = abs(poynting_z(f_p_t, cplx_re(1.0), frame.zhat));
    result.flux_inc = abs(poynting_z(f_inc, cplx_re(1.0), frame.zhat));
    result.o_hat = normalize_or_zero(f_o_bwd.e.re);
    result.e_hat = normalize_or_zero(f_e_bwd.e.re);
    return result;
}

// optics::raytracer::uniaxial_fresnel::azimuth2_in_frame
fn azimuth2_in_frame(dir: vec3<f32>, s_axis: vec3<f32>, p_axis: vec3<f32>) -> vec2<f32> {
    let s_comp = dot(dir, s_axis);
    let p_comp = dot(dir, p_axis);
    let norm = max(fma(p_comp, p_comp, s_comp * s_comp), 1e-12);
    let cos_2psi = fma(p_comp, -p_comp, s_comp * s_comp) / norm;
    let sin_2psi = 2.0 * s_comp * p_comp / norm;
    return vec2<f32>(cos_2psi, sin_2psi);
}

// optics::raytracer::uniaxial_fresnel::jones_to_mueller
fn jones_to_mueller(j_ss: Cplx, j_sp: Cplx, j_ps: Cplx, j_pp: Cplx) -> mat4x4<f32> {
    let m_ss = cplx_norm_sqr(j_ss);
    let m_sp = cplx_norm_sqr(j_sp);
    let m_ps = cplx_norm_sqr(j_ps);
    let m_pp = cplx_norm_sqr(j_pp);

    let a_ssp = cplx_mul(j_ss, cplx_conj(j_sp));
    let a_pep = cplx_mul(j_ps, cplx_conj(j_pp));
    let a_ssp_plus = cplx_add(a_ssp, a_pep);
    let a_ssp_minus = cplx_sub(a_ssp, a_pep);

    let b_sp = cplx_mul(j_ss, cplx_conj(j_ps));
    let b_pp = cplx_mul(j_sp, cplx_conj(j_pp));

    let c_sspp = cplx_mul(j_ss, cplx_conj(j_pp));
    let c_spps = cplx_mul(j_sp, cplx_conj(j_ps));

    let i_i = 0.5 * (m_ss + m_ps + m_sp + m_pp);
    let i_q = 0.5 * (m_ss + m_ps - m_sp - m_pp);
    let i_u = a_ssp_plus.re;
    let i_v = a_ssp_plus.im;

    let q_i = 0.5 * (m_ss - m_ps + m_sp - m_pp);
    let q_q = 0.5 * ((m_ss - m_ps - m_sp) + m_pp);
    let q_u = a_ssp_minus.re;
    let q_v = a_ssp_minus.im;

    let u_i = b_sp.re + b_pp.re;
    let u_q = b_sp.re - b_pp.re;
    let u_u = c_sspp.re + c_spps.re;
    let u_v = c_sspp.im - c_spps.im;

    let v_i = -(b_sp.im + b_pp.im);
    let v_q = -(b_sp.im) + b_pp.im;
    let v_u = -(c_sspp.im + c_spps.im);
    let v_v = c_sspp.re - c_spps.re;

    return mat4x4<f32>(
        vec4<f32>(i_i, q_i, u_i, v_i),
        vec4<f32>(i_q, q_q, u_q, v_q),
        vec4<f32>(i_u, q_u, u_u, v_u),
        vec4<f32>(i_v, q_v, u_v, v_v),
    );
}

// optics::raytracer::uniaxial_fresnel::mode_power. `stokes` is this shader tree's usual
// `vec4<f32>(I, Q, U, V)` convention (`.x`/`.y`/`.z`/`.w`).
fn mode_power(t_s: Cplx, t_p: Cplx, mode_flux: f32, inc_flux: f32, stokes: vec4<f32>) -> f32 {
    let m_s = cplx_norm_sqr(t_s);
    let m_p = cplx_norm_sqr(t_p);
    let cross_term = cplx_mul(t_s, cplx_conj(t_p));
    let inner = 0.5 * fma(m_s, stokes.x + stokes.y, m_p * (stokes.x - stokes.y));
    let raw = fma(cross_term.im, stokes.w, fma(cross_term.re, stokes.z, inner));
    return (mode_flux / max(inc_flux, 1e-12)) * raw;
}

