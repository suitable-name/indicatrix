// ---------------------------------------------------------------------------------
// P2 full uniaxial Fresnel (Lekner 1991) -- Tier 2 kernel-level equivalence checks for
// `optics::raytracer::uniaxial_fresnel::{entry_solve_pair, internal_solve}` against
// `transport_physics.wgsl`'s own `entry_solve_pair`/`internal_solve` mirror (see that
// file's own P2-full section header comment). Driven by
// `renderer::gpu::transport_check::p2_uniaxial_fresnel`.
// ---------------------------------------------------------------------------------

struct EntrySolvePairCase {
    k_hat: vec3<f32>,
    _pad0: f32,
    normal: vec3<f32>,
    _pad1: f32,
    c_axis: vec3<f32>,
    _pad2: f32,
    n1: f32,
    n_o: f32,
    n_e: f32,
    _pad3: f32,
}

@group(0) @binding(46) var<storage, read> entry_solve_pair_cases: array<EntrySolvePairCase>;
// Layout per case, 32 floats: [0..16) s_sol (r_s.re, r_s.im, r_p.re, r_p.im, t_o.re,
// t_o.im, t_e.re, t_e.im, flux_o, flux_e, o_hat.xyz, e_hat.xyz), [16..32) p_sol (same
// layout).
@group(0) @binding(47) var<storage, read_write> entry_solve_pair_out: array<f32>;

fn write_entry_sol(out_base: u32, sol: EntrySolW) {
    entry_solve_pair_out[out_base + 0u] = sol.r_s.re;
    entry_solve_pair_out[out_base + 1u] = sol.r_s.im;
    entry_solve_pair_out[out_base + 2u] = sol.r_p.re;
    entry_solve_pair_out[out_base + 3u] = sol.r_p.im;
    entry_solve_pair_out[out_base + 4u] = sol.t_o.re;
    entry_solve_pair_out[out_base + 5u] = sol.t_o.im;
    entry_solve_pair_out[out_base + 6u] = sol.t_e.re;
    entry_solve_pair_out[out_base + 7u] = sol.t_e.im;
    entry_solve_pair_out[out_base + 8u] = sol.flux_o;
    entry_solve_pair_out[out_base + 9u] = sol.flux_e;
    entry_solve_pair_out[out_base + 10u] = sol.o_hat.x;
    entry_solve_pair_out[out_base + 11u] = sol.o_hat.y;
    entry_solve_pair_out[out_base + 12u] = sol.o_hat.z;
    entry_solve_pair_out[out_base + 13u] = sol.e_hat.x;
    entry_solve_pair_out[out_base + 14u] = sol.e_hat.y;
    entry_solve_pair_out[out_base + 15u] = sol.e_hat.z;
}

@compute @workgroup_size(64)
fn entry_solve_pair_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&entry_solve_pair_cases)) {
        return;
    }
    let c = entry_solve_pair_cases[idx];
    let cos_i = clamp(dot(-c.k_hat, c.normal), 0.0, 1.0);
    let sin_i = sqrt(max(fma(-cos_i, cos_i, 1.0), 0.0));
    let frame = uniaxial_frame_build(c.k_hat, c.normal, c.c_axis, cos_i, sin_i);
    let inc = entry_incidence_frame(c.n1, frame);
    let pair = entry_solve_pair_with_incidence(inc, c.n1, c.n_o, c.n_e, c.c_axis, frame);
    write_entry_sol(idx * 32u, pair.s_sol);
    write_entry_sol(idx * 32u + 16u, pair.p_sol);
}

struct InternalSolveCase {
    k_hat: vec3<f32>,
    _pad0: f32,
    normal: vec3<f32>,
    _pad1: f32,
    c_axis: vec3<f32>,
    _pad2: f32,
    n_mode_inc: f32,
    n_o: f32,
    n_e: f32,
    incident_is_ordinary: u32,
}

@group(0) @binding(48) var<storage, read> internal_solve_cases: array<InternalSolveCase>;
// Layout per case, 19 floats: r_o.re, r_o.im, r_e.re, r_e.im, t_s.re, t_s.im, t_p.re,
// t_p.im, flux_ro, flux_re, flux_ts, flux_tp, flux_inc, o_hat.xyz, e_hat.xyz.
@group(0) @binding(49) var<storage, read_write> internal_solve_out: array<f32>;

@compute @workgroup_size(64)
fn internal_solve_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&internal_solve_cases)) {
        return;
    }
    let c = internal_solve_cases[idx];
    let cos_i = clamp(dot(-c.k_hat, c.normal), 0.0, 1.0);
    let sin_i = sqrt(max(fma(-cos_i, cos_i, 1.0), 0.0));
    let frame = uniaxial_frame_build(c.k_hat, c.normal, c.c_axis, cos_i, sin_i);
    let sol = internal_solve(c.n_mode_inc, c.n_o, c.n_e, c.c_axis, frame, c.incident_is_ordinary != 0u);
    let base = idx * 19u;
    internal_solve_out[base + 0u] = sol.r_o.re;
    internal_solve_out[base + 1u] = sol.r_o.im;
    internal_solve_out[base + 2u] = sol.r_e.re;
    internal_solve_out[base + 3u] = sol.r_e.im;
    internal_solve_out[base + 4u] = sol.t_s.re;
    internal_solve_out[base + 5u] = sol.t_s.im;
    internal_solve_out[base + 6u] = sol.t_p.re;
    internal_solve_out[base + 7u] = sol.t_p.im;
    internal_solve_out[base + 8u] = sol.flux_ro;
    internal_solve_out[base + 9u] = sol.flux_re;
    internal_solve_out[base + 10u] = sol.flux_ts;
    internal_solve_out[base + 11u] = sol.flux_tp;
    internal_solve_out[base + 12u] = sol.flux_inc;
    internal_solve_out[base + 13u] = sol.o_hat.x;
    internal_solve_out[base + 14u] = sol.o_hat.y;
    internal_solve_out[base + 15u] = sol.o_hat.z;
    internal_solve_out[base + 16u] = sol.e_hat.x;
    internal_solve_out[base + 17u] = sol.e_hat.y;
    internal_solve_out[base + 18u] = sol.e_hat.z;
}

// ---------------------------------------------------------------------------------
// P6 exit-event spectral splitting: kernel-level equivalence for the
// three pure per-channel helpers `transport_physics.wgsl`'s own "P6 exit-event
// spectral splitting" section defines -- `compute_channel_transmission`,
// `compute_uniaxial_exit_transmission`, `narrow_compat` -- driven by
// `renderer::gpu::transport_check::p6_exit_splitting`. See that module's own doc
// comment for why its CPU reference functions are verbatim transcriptions of the real
// CPU source rather than a direct call (both `compute_channel_transmission`/
// `compute_uniaxial_exit_transmission` are module-private in `refraction.rs`, and
// `narrow_compat` is `pub(super)` there -- `refraction.rs` is a
// coordinator-owned/protected file, so none of the three can be imported cross-module).
// ---------------------------------------------------------------------------------

struct ChannelTransmissionCase {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    cos_2psi_x: f32,
    sin_2psi_x: f32,
    entering_anisotropic: u32,
    azimuth_valid: u32,
    stokes_i: f32,
    stokes_q: f32,
    stokes_u: f32,
    stokes_v: f32,
}

@group(0) @binding(50) var<storage, read> channel_transmission_cases: array<ChannelTransmissionCase>;
// Layout per case, 5 floats: transmitted.i/q/u/v, r_unpol_k.
@group(0) @binding(51) var<storage, read_write> channel_transmission_out: array<f32>;

@compute @workgroup_size(64)
fn channel_transmission_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&channel_transmission_cases)) {
        return;
    }
    let c = channel_transmission_cases[idx];
    let incident = vec4<f32>(c.stokes_i, c.stokes_q, c.stokes_u, c.stokes_v);
    let result = compute_channel_transmission(
        c.n1k, c.n2k, c.cos_i, c.cos_t_k, c.r_unpol,
        c.entering_anisotropic != 0u, c.azimuth_valid != 0u, c.cos_2psi_x, c.sin_2psi_x, incident,
    );
    let base = idx * 5u;
    channel_transmission_out[base + 0u] = result.transmitted.x;
    channel_transmission_out[base + 1u] = result.transmitted.y;
    channel_transmission_out[base + 2u] = result.transmitted.z;
    channel_transmission_out[base + 3u] = result.transmitted.w;
    channel_transmission_out[base + 4u] = result.r_unpol_k;
}

struct UniaxialExitTransmissionCase {
    t_s_re: f32,
    t_s_im: f32,
    t_p_re: f32,
    t_p_im: f32,
    flux_ts: f32,
    flux_tp: f32,
    flux_inc: f32,
    r_branch: f32,
    incident_i: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(52) var<storage, read> uniaxial_exit_transmission_cases: array<UniaxialExitTransmissionCase>;
// Layout per case, 5 floats: transmitted.i/q/u/v, i_unit.
@group(0) @binding(53) var<storage, read_write> uniaxial_exit_transmission_out: array<f32>;

@compute @workgroup_size(64)
fn uniaxial_exit_transmission_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&uniaxial_exit_transmission_cases)) {
        return;
    }
    let c = uniaxial_exit_transmission_cases[idx];
    // Only the fields `compute_uniaxial_exit_transmission` actually reads
    // (`t_s`/`t_p`/`flux_ts`/`flux_tp`/`flux_inc`) are populated from the case -- the
    // rest of `InternalSolW` (`r_o`/`r_e`/`flux_ro`/`flux_re`/`o_hat`/`e_hat`) is never
    // touched by that function, so left zero, exactly like this Tier 2 kernel's own
    // Rust-side case bank never derives them either.
    var sol: InternalSolW;
    sol.r_o = cplx_zero();
    sol.r_e = cplx_zero();
    sol.t_s = Cplx(c.t_s_re, c.t_s_im);
    sol.t_p = Cplx(c.t_p_re, c.t_p_im);
    sol.flux_ro = 0.0;
    sol.flux_re = 0.0;
    sol.flux_ts = c.flux_ts;
    sol.flux_tp = c.flux_tp;
    sol.flux_inc = c.flux_inc;
    sol.o_hat = vec3<f32>(0.0, 0.0, 0.0);
    sol.e_hat = vec3<f32>(0.0, 0.0, 0.0);
    let result = compute_uniaxial_exit_transmission(sol, c.r_branch, c.incident_i);
    let base = idx * 5u;
    uniaxial_exit_transmission_out[base + 0u] = result.transmitted.x;
    uniaxial_exit_transmission_out[base + 1u] = result.transmitted.y;
    uniaxial_exit_transmission_out[base + 2u] = result.transmitted.z;
    uniaxial_exit_transmission_out[base + 3u] = result.transmitted.w;
    uniaxial_exit_transmission_out[base + 4u] = result.i_unit;
}

struct NarrowCompatCase {
    dir0: vec4<f32>,
    dir1: vec4<f32>,
    dir2: vec4<f32>,
    dir3: vec4<f32>,
    dir4: vec4<f32>,
    dir5: vec4<f32>,
    dir6: vec4<f32>,
    dir7: vec4<f32>,
    hero_match_mask: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(54) var<storage, read> narrow_compat_cases: array<NarrowCompatCase>;
// Layout per case, 8 u32: the narrowed compat[] mask, one entry per channel.
@group(0) @binding(55) var<storage, read_write> narrow_compat_out: array<u32>;

@compute @workgroup_size(64)
fn narrow_compat_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&narrow_compat_cases)) {
        return;
    }
    let c = narrow_compat_cases[idx];
    let raw_dirs = array<vec4<f32>, 8>(c.dir0, c.dir1, c.dir2, c.dir3, c.dir4, c.dir5, c.dir6, c.dir7);
    var dirs: array<vec3<f32>, 8>;
    var dirs_valid: array<bool, 8>;
    var hero_match: array<bool, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        dirs[k] = raw_dirs[k].xyz;
        dirs_valid[k] = raw_dirs[k].w > 0.5;
        hero_match[k] = ((c.hero_match_mask >> k) & 1u) != 0u;
    }
    var compat: array<u32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        compat[k] = 0xFFu;
    }
    narrow_compat(&compat, dirs, dirs_valid, hero_match);
    let base = idx * 8u;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        narrow_compat_out[base + k] = compat[k];
    }
}

