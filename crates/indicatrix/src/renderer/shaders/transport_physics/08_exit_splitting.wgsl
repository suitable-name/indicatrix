// ---------------------------------------------------------------------------------
// P6 exit-event spectral splitting: the three pure (no-binding)
// per-channel helpers optics::raytracer::refraction's own top-of-file "Exit-event
// spectral splitting" doc comment names -- `compute_channel_transmission`,
// `compute_uniaxial_exit_transmission`, and `narrow_compat`. Every operation, and its
// order, is a direct transcription of the CPU function it mirrors (`f32::mul_add` ->
// `fma`, same clamp bounds, same intermediate names where WGSL's lack of tuple returns
// allows it) -- see each function's own comment for the exact CPU counterpart.
//
// `compute_channel_transmission`/`compute_uniaxial_exit_transmission` are private
// (module-private, not even `pub(super)`) inside `refraction.rs`, and `narrow_compat`
// is `pub(super)` (visible only within `optics::raytracer`, not from this crate's
// `renderer` tree) -- `refraction.rs` is on the protected/coordinator-owned
// list, so its visibility cannot be widened to import these directly the way
// `p2_uniaxial_fresnel.rs` imports the (already `pub(crate)`) `entry_solve_pair`/
// `internal_solve`. `renderer::gpu::transport_check::p6_exit_splitting`'s own CPU
// reference functions are therefore verbatim transcriptions of the real CPU source
// (cross-referenced by file/line in that module's doc comment) rather than a call
// through the module boundary -- the one deliberate exception to this file tree's
// usual "always call the real CPU function" rule, forced by that protection boundary,
// not a design choice.
//
// Constants duplicated locally (`8u` in place of `NUM_CHANNELS`, an inlined `1.0 -
// 1e-6` in place of `DIRECTION_MATCH_COS_TOL`) rather than referencing
// `spectral_transport.wgsl`'s own copies: `build.rs` concatenates THIS file ahead of
// both `spectral_transport.wgsl` and `transport_functions.wgsl` (see this file's own
// header comment), so a name defined only in one of those two files is not yet in
// scope here -- exactly the same reason `R_UNPOL_MIN`/`R_UNPOL_MAX` above are this
// file's own copies rather than a reference to `spectral_transport.wgsl`'s
// `R_UNPOL_SELECT_MIN`/`MAX`.
// ---------------------------------------------------------------------------------

// optics::raytracer::refraction::compute_channel_transmission. WGSL has no `Option`, so
// `azimuth_valid == false` stands in for the Rust `None` case (same convention as
// `entry_eigenmode_selection`'s own `valid` field above) -- callers pass
// `false`/`0.0`/`0.0` for `azimuth_valid`/`cos_2psi_x`/`sin_2psi_x` whenever
// `entering_anisotropic` is false or the entry mode-selection draw was invalid.
struct ChannelTransmissionW {
    transmitted: vec4<f32>,
    r_unpol_k: f32,
}

fn compute_channel_transmission(
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    azimuth_valid: bool,
    cos_2psi_x: f32,
    sin_2psi_x: f32,
    incident_stokes_k: vec4<f32>,
) -> ChannelTransmissionW {
    let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k = mueller_fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    var incident_k = incident_stokes_k;
    if (entering_anisotropic && azimuth_valid) {
        let i_k = incident_stokes_k.x;
        incident_k = vec4<f32>(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0);
    }
    let transmitted = (trans_matrix_k * incident_k) * (1.0 / (1.0 - r_unpol));
    let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), 1e-4, 1.0 - 1e-4);
    var result: ChannelTransmissionW;
    result.transmitted = transmitted;
    result.r_unpol_k = r_unpol_k;
    return result;
}

// optics::raytracer::refraction::compute_uniaxial_exit_transmission.
struct UniaxialExitTransmissionW {
    transmitted: vec4<f32>,
    i_unit: f32,
}

fn compute_uniaxial_exit_transmission(
    sol: InternalSolW,
    r_branch: f32,
    incident_i: f32,
) -> UniaxialExitTransmissionW {
    let flux_inc = max(sol.flux_inc, 1e-12);
    let ts_n = cplx_scale(sol.t_s, sqrt(sol.flux_ts / flux_inc));
    let tp_n = cplx_scale(sol.t_p, sqrt(sol.flux_tp / flux_inc));
    let i_unit = cplx_norm_sqr(ts_n) + cplx_norm_sqr(tp_n);
    let q_unit = cplx_norm_sqr(ts_n) - cplx_norm_sqr(tp_n);
    let cross_st = cplx_mul(ts_n, cplx_conj(tp_n));
    let u_unit = 2.0 * cross_st.re;
    let v_unit = -2.0 * cross_st.im;
    let transmitted = vec4<f32>(
        incident_i * i_unit, incident_i * q_unit, incident_i * u_unit, incident_i * v_unit,
    ) * (1.0 / (1.0 - r_branch));
    var result: UniaxialExitTransmissionW;
    result.transmitted = transmitted;
    result.i_unit = i_unit;
    return result;
}

// optics::raytracer::refraction::narrow_compat. `compat[c]` bit `j` set means channels
// `c`/`j` have refracted within the direction-match tolerance of each other at every
// interior dispersive event so far -- see `ExitSplitCtx::compat`'s own CPU-side doc
// comment. Hero is always channel 0 in this kernel's convention (`path_pdf[0]`
// throughout `spectral_transport.wgsl`), so this is specialized to that fixed hero
// index rather than taking one as a parameter, unlike the CPU function's `hero: usize`.
// `dirs_valid[k] == false` stands in for the CPU `dirs[k]: Option<Vec3> == None` case.
fn narrow_compat(
    compat: ptr<function, array<u32, 8>>,
    dirs: array<vec3<f32>, 8>,
    dirs_valid: array<bool, 8>,
    hero_match: array<bool, 8>,
) {
    let direction_match_cos_tol = 1.0 - 1e-6;
    for (var a: u32 = 0u; a < 8u; a = a + 1u) {
        for (var b: u32 = a + 1u; b < 8u; b = b + 1u) {
            var matches_ab: bool;
            if (a == 0u) {
                matches_ab = hero_match[b];
            } else if (b == 0u) {
                matches_ab = hero_match[a];
            } else if (dirs_valid[a] && dirs_valid[b]) {
                matches_ab = dot(dirs[a], dirs[b]) >= direction_match_cos_tol;
            } else {
                matches_ab = true;
            }
            if (!matches_ab) {
                (*compat)[a] = (*compat)[a] & ~(1u << b);
                (*compat)[b] = (*compat)[b] & ~(1u << a);
            }
        }
    }
}

