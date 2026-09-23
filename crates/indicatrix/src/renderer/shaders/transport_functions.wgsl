// Phase 2, Tier 2: standalone per-function ULP checks for the small pieces
// `shaders/spectral_transport.wgsl`'s megakernel calls -- driven by
// `renderer::gpu::transport_check`. Every kernel below calls the SAME shared function
// the megakernel calls: both files consume `shaders/transport_physics.wgsl`,
// concatenated ahead of each of them by `build.rs` (see that file's header comment for
// the mechanism and why it exists). There is no manually-kept-in-sync copy here any
// more -- a dense-grid sweep mismatch found by a kernel below now localizes to exactly
// one named function in the ONE place it's defined, and -- because that place is also
// what the megakernel calls -- it is necessarily testing the shipped code path, not a
// duplicate of it.
//
// Every case bank is dispatched against the REAL CPU function it was translated from
// (`optics::polarization::MuellerMatrix::*`, `optics::raytracer::{tir_phase_delta,
// signed_frame_rotation_psi}`, `optics::dispersion::DispersionModel::evaluate`,
// `optics::raytracer::spectral_absorption`, `optics::birefringence::pleochroic_channel_alpha`)
// -- never a hand-written parallel reimplementation of the physics -- by
// `renderer::gpu::transport_check`.

// ---------------------------------------------------------------------------------
// MuellerMatrix::frame_rotation + StokesVector::apply_matrix
// ---------------------------------------------------------------------------------

struct FrameRotationCase {
    psi: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<storage, read> frame_rotation_cases: array<FrameRotationCase>;
@group(0) @binding(1) var<storage, read_write> frame_rotation_out: array<f32>;

@compute @workgroup_size(64)
fn frame_rotation_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&frame_rotation_cases)) {
        return;
    }
    let c = frame_rotation_cases[idx];
    let m = mueller_frame_rotation(c.psi);
    let out = m * vec4<f32>(c.si, c.sq, c.su, c.sv);
    frame_rotation_out[idx * 4u + 0u] = out.x;
    frame_rotation_out[idx * 4u + 1u] = out.y;
    frame_rotation_out[idx * 4u + 2u] = out.z;
    frame_rotation_out[idx * 4u + 3u] = out.w;
}

// ---------------------------------------------------------------------------------
// MuellerMatrix::fresnel_reflection + StokesVector::apply_matrix
// ---------------------------------------------------------------------------------

struct FresnelReflectionCase {
    r_s: f32,
    r_p: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(2) var<storage, read> fresnel_reflection_cases: array<FresnelReflectionCase>;
@group(0) @binding(3) var<storage, read_write> fresnel_reflection_out: array<f32>;

@compute @workgroup_size(64)
fn fresnel_reflection_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&fresnel_reflection_cases)) {
        return;
    }
    let c = fresnel_reflection_cases[idx];
    let m = mueller_fresnel_reflection(c.r_s, c.r_p);
    let out = m * vec4<f32>(c.si, c.sq, c.su, c.sv);
    fresnel_reflection_out[idx * 4u + 0u] = out.x;
    fresnel_reflection_out[idx * 4u + 1u] = out.y;
    fresnel_reflection_out[idx * 4u + 2u] = out.z;
    fresnel_reflection_out[idx * 4u + 3u] = out.w;
}

// ---------------------------------------------------------------------------------
// MuellerMatrix::fresnel_transmission + StokesVector::apply_matrix
// ---------------------------------------------------------------------------------

struct FresnelTransmissionCase {
    n1: f32,
    n2: f32,
    cos_i: f32,
    cos_t: f32,
    t_s: f32,
    t_p: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(4) var<storage, read> fresnel_transmission_cases: array<FresnelTransmissionCase>;
@group(0) @binding(5) var<storage, read_write> fresnel_transmission_out: array<f32>;

@compute @workgroup_size(64)
fn fresnel_transmission_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&fresnel_transmission_cases)) {
        return;
    }
    let c = fresnel_transmission_cases[idx];
    let m = mueller_fresnel_transmission(c.n1, c.n2, c.cos_i, c.cos_t, c.t_s, c.t_p);
    let out = m * vec4<f32>(c.si, c.sq, c.su, c.sv);
    fresnel_transmission_out[idx * 4u + 0u] = out.x;
    fresnel_transmission_out[idx * 4u + 1u] = out.y;
    fresnel_transmission_out[idx * 4u + 2u] = out.z;
    fresnel_transmission_out[idx * 4u + 3u] = out.w;
}

// ---------------------------------------------------------------------------------
// MuellerMatrix::tir_retardation + StokesVector::apply_matrix
// ---------------------------------------------------------------------------------

struct TirRetardationCase {
    delta: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(6) var<storage, read> tir_retardation_cases: array<TirRetardationCase>;
@group(0) @binding(7) var<storage, read_write> tir_retardation_out: array<f32>;

@compute @workgroup_size(64)
fn tir_retardation_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&tir_retardation_cases)) {
        return;
    }
    let c = tir_retardation_cases[idx];
    let m = mueller_tir_retardation(c.delta);
    let out = m * vec4<f32>(c.si, c.sq, c.su, c.sv);
    tir_retardation_out[idx * 4u + 0u] = out.x;
    tir_retardation_out[idx * 4u + 1u] = out.y;
    tir_retardation_out[idx * 4u + 2u] = out.z;
    tir_retardation_out[idx * 4u + 3u] = out.w;
}

// ---------------------------------------------------------------------------------
// optics::raytracer::signed_frame_rotation_psi
// ---------------------------------------------------------------------------------

struct SignedPsiCase {
    prev: vec3<f32>,
    _pad0: f32,
    curr: vec3<f32>,
    _pad1: f32,
    axis: vec3<f32>,
    _pad2: f32,
}

@group(0) @binding(8) var<storage, read> signed_psi_cases: array<SignedPsiCase>;
@group(0) @binding(9) var<storage, read_write> signed_psi_out: array<f32>;

@compute @workgroup_size(64)
fn signed_psi_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&signed_psi_cases)) {
        return;
    }
    let c = signed_psi_cases[idx];
    signed_psi_out[idx] = signed_frame_rotation_psi(c.prev, c.curr, c.axis);
}

// ---------------------------------------------------------------------------------
// optics::raytracer::tir_phase_delta
// ---------------------------------------------------------------------------------

struct TirPhaseDeltaCase {
    n1k: f32,
    cos_i: f32,
    sin_i: f32,
    _pad0: f32,
}

@group(0) @binding(10) var<storage, read> tir_phase_delta_cases: array<TirPhaseDeltaCase>;
@group(0) @binding(11) var<storage, read_write> tir_phase_delta_out: array<f32>;

@compute @workgroup_size(64)
fn tir_phase_delta_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&tir_phase_delta_cases)) {
        return;
    }
    let c = tir_phase_delta_cases[idx];
    tir_phase_delta_out[idx] = tir_phase_delta(c.n1k, c.cos_i, c.sin_i);
}

// ---------------------------------------------------------------------------------
// optics::dispersion::DispersionModel::evaluate
// ---------------------------------------------------------------------------------

struct DispersionCase {
    model_type: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    lambda_nm: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

@group(0) @binding(12) var<storage, read> dispersion_cases: array<DispersionCase>;
@group(0) @binding(13) var<storage, read_write> dispersion_out: array<f32>;

@compute @workgroup_size(64)
fn dispersion_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dispersion_cases)) {
        return;
    }
    let c = dispersion_cases[idx];
    dispersion_out[idx] = dispersion_evaluate(c.model_type, c.param_a, c.param_b, c.lambda_nm);
}

// ---------------------------------------------------------------------------------
// optics::raytracer::spectral_absorption
// ---------------------------------------------------------------------------------

struct AbsorptionCase {
    band_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    bands: array<AbsorptionBand, 8>,
    lambda_nm: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

@group(0) @binding(14) var<storage, read> absorption_cases: array<AbsorptionCase>;
@group(0) @binding(15) var<storage, read_write> absorption_out: array<f32>;

@compute @workgroup_size(64)
fn absorption_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&absorption_cases)) {
        return;
    }
    let c = absorption_cases[idx];
    absorption_out[idx] = spectral_absorption(c.bands, c.band_count, c.lambda_nm);
}

// ---------------------------------------------------------------------------------
// optics::birefringence::pleochroic_channel_alpha (end to end: electric_field_direction,
// ordinary/extraordinary eigen polarization inputs supplied directly, AbsorptionTensor3
// quadratic form, effective_pleochroic_alpha combination).
// ---------------------------------------------------------------------------------

struct PleochroicCase {
    alpha_o: f32,
    alpha_e: f32,
    _pad0: f32,
    _pad1: f32,
    c_axis: vec3<f32>,
    _pad2: f32,
    s_axis: vec3<f32>,
    _pad3: f32,
    propagation_dir: vec3<f32>,
    _pad4: f32,
    eigen_a: vec3<f32>,
    _pad5: f32,
    eigen_b: vec3<f32>,
    _pad6: f32,
    stokes: vec4<f32>,
}

@group(0) @binding(16) var<storage, read> pleochroic_cases: array<PleochroicCase>;
@group(0) @binding(17) var<storage, read_write> pleochroic_out: array<f32>;

@compute @workgroup_size(64)
fn pleochroic_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&pleochroic_cases)) {
        return;
    }
    let cs = pleochroic_cases[idx];
    pleochroic_out[idx] = pleochroic_channel_alpha(
        cs.alpha_o, cs.alpha_e, cs.c_axis, cs.s_axis, cs.propagation_dir, cs.eigen_a, cs.eigen_b, cs.stokes,
    );
}

// ---------------------------------------------------------------------------------
// optics::birefringence::{BirefringenceParams::ordinary_eigen_polarization,
// BirefringenceParams::extraordinary_eigen_polarization}
// ---------------------------------------------------------------------------------

struct EigenPolarizationCase {
    wave_normal: vec3<f32>,
    _pad0: f32,
    c_axis: vec3<f32>,
    _pad1: f32,
}

@group(0) @binding(18) var<storage, read> eigen_polarization_cases: array<EigenPolarizationCase>;
@group(0) @binding(19) var<storage, read_write> eigen_polarization_out: array<f32>;

@compute @workgroup_size(64)
fn eigen_polarization_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&eigen_polarization_cases)) {
        return;
    }
    let c = eigen_polarization_cases[idx];
    let o_hat = ordinary_eigen_polarization(c.wave_normal, c.c_axis);
    let e_hat = extraordinary_eigen_polarization(c.wave_normal, c.c_axis);
    eigen_polarization_out[idx * 6u + 0u] = o_hat.x;
    eigen_polarization_out[idx * 6u + 1u] = o_hat.y;
    eigen_polarization_out[idx * 6u + 2u] = o_hat.z;
    eigen_polarization_out[idx * 6u + 3u] = e_hat.x;
    eigen_polarization_out[idx * 6u + 4u] = e_hat.y;
    eigen_polarization_out[idx * 6u + 5u] = e_hat.z;
}

// ---------------------------------------------------------------------------------
// Phase 3: optics::raytracer::theta_c_for_bounce (the theta_c fixed-point iteration --
// see `transport_physics.wgsl`'s Phase 3 section for why `is_biaxial` is omitted).
// ---------------------------------------------------------------------------------

struct ThetaCCase {
    normal: vec3<f32>,
    _pad0: f32,
    ray_dir: vec3<f32>,
    _pad1: f32,
    c_axis: vec3<f32>,
    _pad2: f32,
    cos_i: f32,
    inside_gem: u32,
    is_anisotropic: u32,
    n_o_hero_seed: f32,
    birefringence_delta: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

@group(0) @binding(20) var<storage, read> theta_c_cases: array<ThetaCCase>;
@group(0) @binding(21) var<storage, read_write> theta_c_out: array<f32>;

@compute @workgroup_size(64)
fn theta_c_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&theta_c_cases)) {
        return;
    }
    let c = theta_c_cases[idx];
    // P5: `theta_c_for_bounce`'s WGSL signature takes the precomputed hero e-index
    // directly (see that function's own doc comment in transport_physics.wgsl). This
    // case bank's `GemMaterial` (built in `cpu_theta_c` in
    // `renderer::gpu::transport_check::eigenmodes_uniaxial`) never carries a genuine
    // independent extraordinary-ray curve, so `extraordinary_index_at`'s CPU-side
    // fallback reduces to exactly this constant-offset form -- computing it here keeps
    // this kernel testing the SAME iteration algorithm `theta_c_for_bounce` runs on the
    // CPU side, without needing a dispersion-curve-carrying case bank of its own (that
    // is covered separately by the `per_mode_index`/`dispersion` checks).
    let n_e_hero_seed_case = c.n_o_hero_seed + c.birefringence_delta;
    theta_c_out[idx] = theta_c_for_bounce(
        c.normal, c.ray_dir, c.cos_i, c.inside_gem != 0u, c.is_anisotropic != 0u, c.c_axis,
        c.n_o_hero_seed, n_e_hero_seed_case,
    );
}

// ---------------------------------------------------------------------------------
// Phase 3: optics::birefringence::BirefringenceParams::extraordinary_poynting_dir (the
// extraordinary ray's walk-off direction).
// ---------------------------------------------------------------------------------

struct WalkOffCase {
    wave_normal: vec3<f32>,
    _pad0: f32,
    c_axis: vec3<f32>,
    _pad1: f32,
    n_o: f32,
    n_e: f32,
    _pad2: f32,
    _pad3: f32,
}

@group(0) @binding(22) var<storage, read> walk_off_cases: array<WalkOffCase>;
@group(0) @binding(23) var<storage, read_write> walk_off_out: array<f32>;

@compute @workgroup_size(64)
fn walk_off_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&walk_off_cases)) {
        return;
    }
    let c = walk_off_cases[idx];
    let dir = extraordinary_poynting_dir(c.wave_normal, c.c_axis, c.n_o, c.n_e);
    walk_off_out[idx * 3u + 0u] = dir.x;
    walk_off_out[idx * 3u + 1u] = dir.y;
    walk_off_out[idx * 3u + 2u] = dir.z;
}

// ---------------------------------------------------------------------------------
// Phase 3: optics::raytracer::per_channel_uniaxial_indices (one channel's per-mode
// (n_o, n_eff) index pair, via `per_channel_uniaxial_index` -- see
// `transport_physics.wgsl`'s Phase 3 section for why the CPU's NUM_CHANNELS loop is the
// caller's responsibility here).
// ---------------------------------------------------------------------------------

struct PerModeIndexCase {
    model_type: u32,
    is_anisotropic: u32,
    _pad0: u32,
    _pad1: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    lambda_nm: f32,
    birefringence_delta: f32,
    theta_c: f32,
    _pad2: f32,
}

@group(0) @binding(24) var<storage, read> per_mode_index_cases: array<PerModeIndexCase>;
@group(0) @binding(25) var<storage, read_write> per_mode_index_out: array<f32>;

@compute @workgroup_size(64)
fn per_mode_index_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&per_mode_index_cases)) {
        return;
    }
    let c = per_mode_index_cases[idx];
    let pair = per_channel_uniaxial_index(
        c.model_type, c.param_a, c.param_b, c.lambda_nm, c.birefringence_delta, c.is_anisotropic != 0u, c.theta_c,
    );
    per_mode_index_out[idx * 2u + 0u] = pair.x;
    per_mode_index_out[idx * 2u + 1u] = pair.y;
}

// ---------------------------------------------------------------------------------
// Frosted-facet GPU port: optics::raytracer::cosine_weighted_hemisphere -- the
// frosted-bounce direction sampler (Malley's method).
// ---------------------------------------------------------------------------------

// Field order matters here: `n` (a vec3, needing 16-byte WGSL alignment) is placed
// FIRST so it lands at offset 0 with no implicit leading padding, then `u1`/`u2` pack
// into its trailing 4 bytes plus one more (the same "vec3 + scalar(s)" pattern this
// crate's struct-layout doc comments describe elsewhere) -- `renderer::gpu::
// transport_check::CosineHemisphereCase` mirrors this EXACT field order for that
// reason; reordering either side without the other would silently misalign every case.
struct CosineHemisphereCase {
    n: vec3<f32>,
    u1: f32,
    u2: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(26) var<storage, read> cosine_hemisphere_cases: array<CosineHemisphereCase>;
@group(0) @binding(27) var<storage, read_write> cosine_hemisphere_out: array<f32>;

@compute @workgroup_size(64)
fn cosine_hemisphere_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&cosine_hemisphere_cases)) {
        return;
    }
    let c = cosine_hemisphere_cases[idx];
    let dir = cosine_weighted_hemisphere(c.u1, c.u2, c.n);
    cosine_hemisphere_out[idx * 3u + 0u] = dir.x;
    cosine_hemisphere_out[idx * 3u + 1u] = dir.y;
    cosine_hemisphere_out[idx * 3u + 2u] = dir.z;
}

// ---------------------------------------------------------------------------------
// Frosted-facet GPU port: optics::raytracer::apply_frosted_bounce -- the full
// frosted-facet bounce dispatch (TIR-forced / reflect / transmit branch selection, the broadband
// hero-only r_unpol split, Stokes depolarization, path_pdf scaling). Calls the SAME
// `transport_physics.wgsl` function `spectral_transport.wgsl`'s megakernel calls for a
// `FacetFinish::Frosted` facet -- see that shared function's own doc comment.
// ---------------------------------------------------------------------------------

struct FrostedBounceCase {
    is_anisotropic: u32,
    sin2_t: f32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    inside_gem: u32,
    is_extraordinary: u32,
    rng_seed: u32,
    bounce: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    normal: vec3<f32>,
    _pad3: f32,
    stokes_in: array<vec4<f32>, 8>,
    path_pdf_in: array<f32, 8>,
}

@group(0) @binding(28) var<storage, read> frosted_bounce_cases: array<FrostedBounceCase>;
// Layout per case, 46 floats: [0..3) new_dir, [3] new_inside_gem (0.0/1.0),
// [4] has_extraordinary_update (0.0/1.0), [5] extraordinary_update (0.0/1.0),
// [6..38) stokes_out (8 vec4s), [38..46) path_pdf_out.
@group(0) @binding(29) var<storage, read_write> frosted_bounce_out: array<f32>;

@compute @workgroup_size(64)
fn frosted_bounce_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&frosted_bounce_cases)) {
        return;
    }
    let c = frosted_bounce_cases[idx];
    var stokes: array<vec4<f32>, 8> = c.stokes_in;
    var path_pdf: array<f32, 8> = c.path_pdf_in;
    let result = apply_frosted_bounce(
        c.is_anisotropic != 0u, c.sin2_t, c.n1, c.n2, c.cos_i, c.normal,
        c.inside_gem != 0u, c.is_extraordinary != 0u, c.rng_seed, c.bounce,
        &stokes, &path_pdf,
    );
    let base = idx * 46u;
    frosted_bounce_out[base + 0u] = result.new_dir.x;
    frosted_bounce_out[base + 1u] = result.new_dir.y;
    frosted_bounce_out[base + 2u] = result.new_dir.z;
    frosted_bounce_out[base + 3u] = f32(result.new_inside_gem);
    frosted_bounce_out[base + 4u] = f32(result.has_extraordinary_update);
    frosted_bounce_out[base + 5u] = f32(result.extraordinary_update);
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        frosted_bounce_out[base + 6u + k * 4u + 0u] = stokes[k].x;
        frosted_bounce_out[base + 6u + k * 4u + 1u] = stokes[k].y;
        frosted_bounce_out[base + 6u + k * 4u + 2u] = stokes[k].z;
        frosted_bounce_out[base + 6u + k * 4u + 3u] = stokes[k].w;
    }
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        frosted_bounce_out[base + 38u + k] = path_pdf[k];
    }
}

// ---------------------------------------------------------------------------------
// Inclusion/subsurface scattering GPU port: optics::raytracer::{henyey_greenstein_phase,
// sample_henyey_greenstein_direction, maybe_scatter_or_extinguish}. Calls the SAME
// `transport_physics.wgsl` functions `spectral_transport.wgsl`'s megakernel calls for a
// scattering-active material -- see that shared file's own doc comment.
// ---------------------------------------------------------------------------------

struct HgPhaseCase {
    cos_theta: f32,
    g: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(30) var<storage, read> hg_phase_cases: array<HgPhaseCase>;
@group(0) @binding(31) var<storage, read_write> hg_phase_out: array<f32>;

@compute @workgroup_size(64)
fn hg_phase_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&hg_phase_cases)) {
        return;
    }
    let c = hg_phase_cases[idx];
    hg_phase_out[idx] = henyey_greenstein_phase(c.cos_theta, c.g);
}

struct HgSampleCase {
    u1: f32,
    u2: f32,
    g: f32,
    _pad0: f32,
    forward: vec3<f32>,
    _pad1: f32,
}

@group(0) @binding(32) var<storage, read> hg_sample_cases: array<HgSampleCase>;
@group(0) @binding(33) var<storage, read_write> hg_sample_out: array<f32>;

@compute @workgroup_size(64)
fn hg_sample_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&hg_sample_cases)) {
        return;
    }
    let c = hg_sample_cases[idx];
    let dir = sample_henyey_greenstein_direction(c.u1, c.u2, c.g, c.forward);
    hg_sample_out[idx * 3u + 0u] = dir.x;
    hg_sample_out[idx * 3u + 1u] = dir.y;
    hg_sample_out[idx * 3u + 2u] = dir.z;
}

// ---------------------------------------------------------------------------------
// Phase 4 GPU port: optics::birefringence::BiaxialIndicatrix -- standalone per-function
// checks for the genuinely biaxial machinery, mirroring the uniaxial Phase 3 checks
// above. Every case carries the indicatrix's three principal indices plus its
// `gamma_axis` (not the derived `axes` frame directly -- `biaxial_axes_from_gamma` is
// itself part of what's being checked, exactly as the CPU side's
// `BiaxialIndicatrix::from_gamma_axis` derives `axes` from `gamma_axis` fresh).
// ---------------------------------------------------------------------------------

struct BiaxialWaveIndicesCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: vec3<f32>,
    _pad1: f32,
    wave_normal: vec3<f32>,
    _pad2: f32,
}

@group(0) @binding(36) var<storage, read> biaxial_wave_indices_cases: array<BiaxialWaveIndicesCase>;
@group(0) @binding(37) var<storage, read_write> biaxial_wave_indices_out: array<f32>;

@compute @workgroup_size(64)
fn biaxial_wave_indices_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&biaxial_wave_indices_cases)) {
        return;
    }
    let c = biaxial_wave_indices_cases[idx];
    let ax = biaxial_axes_from_gamma(c.gamma_axis);
    let ni = biaxial_wave_indices(c.n_alpha, c.n_beta, c.n_gamma, ax.ax0, ax.ax1, ax.ax2, c.wave_normal);
    biaxial_wave_indices_out[idx * 2u + 0u] = ni.x;
    biaxial_wave_indices_out[idx * 2u + 1u] = ni.y;
}

struct BiaxialEigenPolarizationCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: vec3<f32>,
    _pad1: f32,
    wave_normal: vec3<f32>,
    _pad2: f32,
}

@group(0) @binding(38) var<storage, read> biaxial_eigen_polarization_cases: array<BiaxialEigenPolarizationCase>;
@group(0) @binding(39) var<storage, read_write> biaxial_eigen_polarization_out: array<f32>;

@compute @workgroup_size(64)
fn biaxial_eigen_polarization_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&biaxial_eigen_polarization_cases)) {
        return;
    }
    let c = biaxial_eigen_polarization_cases[idx];
    let ax = biaxial_axes_from_gamma(c.gamma_axis);
    let eig = biaxial_eigen_polarizations(c.n_alpha, c.n_beta, c.n_gamma, ax.ax0, ax.ax1, ax.ax2, c.wave_normal);
    biaxial_eigen_polarization_out[idx * 6u + 0u] = eig.d_slow.x;
    biaxial_eigen_polarization_out[idx * 6u + 1u] = eig.d_slow.y;
    biaxial_eigen_polarization_out[idx * 6u + 2u] = eig.d_slow.z;
    biaxial_eigen_polarization_out[idx * 6u + 3u] = eig.d_fast.x;
    biaxial_eigen_polarization_out[idx * 6u + 4u] = eig.d_fast.y;
    biaxial_eigen_polarization_out[idx * 6u + 5u] = eig.d_fast.z;
}

struct BiaxialModePoyntingCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: vec3<f32>,
    _pad1: f32,
    wave_normal: vec3<f32>,
    want_slow: u32,
}

@group(0) @binding(40) var<storage, read> biaxial_mode_poynting_cases: array<BiaxialModePoyntingCase>;
@group(0) @binding(41) var<storage, read_write> biaxial_mode_poynting_out: array<f32>;

@compute @workgroup_size(64)
fn biaxial_mode_poynting_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&biaxial_mode_poynting_cases)) {
        return;
    }
    let c = biaxial_mode_poynting_cases[idx];
    let ax = biaxial_axes_from_gamma(c.gamma_axis);
    let dir = biaxial_mode_poynting_dir(c.n_alpha, c.n_beta, c.n_gamma, ax.ax0, ax.ax1, ax.ax2, c.wave_normal, c.want_slow != 0u);
    biaxial_mode_poynting_out[idx * 3u + 0u] = dir.x;
    biaxial_mode_poynting_out[idx * 3u + 1u] = dir.y;
    biaxial_mode_poynting_out[idx * 3u + 2u] = dir.z;
}

struct BiaxialResolveEntryModeCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: vec3<f32>,
    _pad1: f32,
    incident_dir: vec3<f32>,
    _pad2: f32,
    normal: vec3<f32>,
    _pad3: f32,
    cos_i: f32,
    n_seed: f32,
    want_slow: u32,
    _pad4: f32,
}

@group(0) @binding(42) var<storage, read> biaxial_resolve_entry_mode_cases: array<BiaxialResolveEntryModeCase>;
@group(0) @binding(43) var<storage, read_write> biaxial_resolve_entry_mode_out: array<f32>;

@compute @workgroup_size(64)
fn biaxial_resolve_entry_mode_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&biaxial_resolve_entry_mode_cases)) {
        return;
    }
    let c = biaxial_resolve_entry_mode_cases[idx];
    let ax = biaxial_axes_from_gamma(c.gamma_axis);
    let result = biaxial_resolve_entry_mode(
        c.n_alpha, c.n_beta, c.n_gamma, ax.ax0, ax.ax1, ax.ax2,
        c.incident_dir, c.normal, c.cos_i, c.n_seed, c.want_slow != 0u,
    );
    biaxial_resolve_entry_mode_out[idx * 4u + 0u] = result.n;
    biaxial_resolve_entry_mode_out[idx * 4u + 1u] = result.wave_dir.x;
    biaxial_resolve_entry_mode_out[idx * 4u + 2u] = result.wave_dir.y;
    biaxial_resolve_entry_mode_out[idx * 4u + 3u] = result.wave_dir.z;
}

// optics::birefringence::pleochroic_channel_alpha with `alpha_beta = Some(alpha_beta)`
// -- the genuinely biaxial (trichroic) three-coefficient absorption path.
struct BiaxialPleochroicCase {
    alpha_o: f32,
    alpha_beta: f32,
    alpha_e: f32,
    _pad0: f32,
    c_axis: vec3<f32>,
    _pad1: f32,
    s_axis: vec3<f32>,
    _pad2: f32,
    propagation_dir: vec3<f32>,
    _pad3: f32,
    eigen_a: vec3<f32>,
    _pad4: f32,
    eigen_b: vec3<f32>,
    _pad5: f32,
    stokes: vec4<f32>,
}

@group(0) @binding(44) var<storage, read> biaxial_pleochroic_cases: array<BiaxialPleochroicCase>;
@group(0) @binding(45) var<storage, read_write> biaxial_pleochroic_out: array<f32>;

@compute @workgroup_size(64)
fn biaxial_pleochroic_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&biaxial_pleochroic_cases)) {
        return;
    }
    let cs = biaxial_pleochroic_cases[idx];
    biaxial_pleochroic_out[idx] = pleochroic_channel_alpha_biaxial(
        cs.alpha_o, cs.alpha_beta, cs.alpha_e, cs.c_axis, cs.s_axis, cs.propagation_dir, cs.eigen_a, cs.eigen_b, cs.stokes,
    );
}

struct ScatterOrExtinguishCase {
    sigma_s: f32,
    g: f32,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    // P1 (absorption path scale): reuses what was `_pad0` -- see the Rust-side
    // `ScatterOrExtinguishCase`'s own doc comment.
    path_scale: f32,
    _pad1: u32,
    _pad2: u32,
    ray_dir: vec3<f32>,
    _pad3: f32,
    alphas: array<f32, 8>,
    stokes_in: array<vec4<f32>, 8>,
    path_pdf_in: array<f32, 8>,
}

@group(0) @binding(34) var<storage, read> scatter_cases: array<ScatterOrExtinguishCase>;
// Layout per case, 45 floats: [0] scattered (0.0/1.0), [1] t_free, [2..5) new_dir,
// [5..37) stokes_out (8 vec4s), [37..45) path_pdf_out.
@group(0) @binding(35) var<storage, read_write> scatter_out: array<f32>;

@compute @workgroup_size(64)
fn scatter_or_extinguish_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&scatter_cases)) {
        return;
    }
    let c = scatter_cases[idx];
    var stokes: array<vec4<f32>, 8> = c.stokes_in;
    var path_pdf: array<f32, 8> = c.path_pdf_in;
    let result = maybe_scatter_or_extinguish(
        c.alphas, c.sigma_s, c.g, c.ray_dir, c.hit_t, c.path_scale, c.rng_seed, c.bounce, &stokes, &path_pdf,
    );
    let base = idx * 45u;
    scatter_out[base + 0u] = f32(result.scattered);
    scatter_out[base + 1u] = result.t_free;
    scatter_out[base + 2u] = result.new_dir.x;
    scatter_out[base + 3u] = result.new_dir.y;
    scatter_out[base + 4u] = result.new_dir.z;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        scatter_out[base + 5u + k * 4u + 0u] = stokes[k].x;
        scatter_out[base + 5u + k * 4u + 1u] = stokes[k].y;
        scatter_out[base + 5u + k * 4u + 2u] = stokes[k].z;
        scatter_out[base + 5u + k * 4u + 3u] = stokes[k].w;
    }
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        scatter_out[base + 37u + k] = path_pdf[k];
    }
}

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

// ---------------------------------------------------------------------------------
// P1 (assigned-mode absorption): optics::birefringence::assigned_mode_alpha, combined
// with assigned_mode_e_field_uniaxial / BiaxialIndicatrix::assigned_mode_e_field via
// transport_physics.wgsl's assigned_mode_alpha_uniaxial / assigned_mode_alpha_biaxial --
// see renderer::gpu::transport_check::absorption_pleochroism / eigenmodes_biaxial for the
// CPU-side runners these compare against.
// ---------------------------------------------------------------------------------

struct AssignedModeAlphaUniaxialCase {
    alpha_o: f32,
    alpha_e: f32,
    is_extraordinary: u32,
    _pad0: f32,
    c_axis: vec3<f32>,
    _pad1: f32,
    k: vec3<f32>,
    _pad2: f32,
    n_o_hero: f32,
    n_e_hero: f32,
    _pad3: f32,
    _pad4: f32,
}

@group(0) @binding(56) var<storage, read> assigned_mode_alpha_uniaxial_cases: array<AssignedModeAlphaUniaxialCase>;
@group(0) @binding(57) var<storage, read_write> assigned_mode_alpha_uniaxial_out: array<f32>;

@compute @workgroup_size(64)
fn assigned_mode_alpha_uniaxial_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&assigned_mode_alpha_uniaxial_cases)) {
        return;
    }
    let c = assigned_mode_alpha_uniaxial_cases[idx];
    assigned_mode_alpha_uniaxial_out[idx] = assigned_mode_alpha_uniaxial(
        c.alpha_o, c.alpha_e, c.c_axis, c.k, c.is_extraordinary != 0u, c.n_o_hero, c.n_e_hero,
    );
}

struct AssignedModeAlphaBiaxialCase {
    alpha_o: f32,
    alpha_beta: f32,
    alpha_e: f32,
    is_extraordinary: u32,
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    ax0: vec3<f32>,
    _pad1: f32,
    ax1: vec3<f32>,
    _pad2: f32,
    ax2: vec3<f32>,
    _pad3: f32,
    c_axis: vec3<f32>,
    _pad4: f32,
    k: vec3<f32>,
    _pad5: f32,
}

@group(0) @binding(58) var<storage, read> assigned_mode_alpha_biaxial_cases: array<AssignedModeAlphaBiaxialCase>;
@group(0) @binding(59) var<storage, read_write> assigned_mode_alpha_biaxial_out: array<f32>;

@compute @workgroup_size(64)
fn assigned_mode_alpha_biaxial_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&assigned_mode_alpha_biaxial_cases)) {
        return;
    }
    let c = assigned_mode_alpha_biaxial_cases[idx];
    assigned_mode_alpha_biaxial_out[idx] = assigned_mode_alpha_biaxial(
        c.alpha_o, c.alpha_beta, c.alpha_e, c.n_alpha, c.n_beta, c.n_gamma,
        c.ax0, c.ax1, c.ax2, c.c_axis, c.k, c.is_extraordinary != 0u,
    );
}

// ---------------------------------------------------------------------------------
// Next-event estimation, balance-heuristic MIS, and 1D/2D distribution
// importance sampling.
// ---------------------------------------------------------------------------------

struct BalanceHeuristicCase {
    pdf_a: f32,
    pdf_b: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(60) var<storage, read> balance_heuristic_cases: array<BalanceHeuristicCase>;
@group(0) @binding(61) var<storage, read_write> balance_heuristic_out: array<f32>;

@compute @workgroup_size(64)
fn balance_heuristic_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&balance_heuristic_cases)) {
        return;
    }
    let c = balance_heuristic_cases[idx];
    balance_heuristic_out[idx] = balance_heuristic(c.pdf_a, c.pdf_b);
}

// ---------------------------------------------------------------------------------
// Test environment & distribution buffers for Tier 2 NEE kernels
// ---------------------------------------------------------------------------------

@group(0) @binding(62) var<storage, read> dist_test_func: array<f32>;
@group(0) @binding(63) var<storage, read> dist_test_cdf: array<f32>;
@group(0) @binding(64) var<uniform> dist_test_dims: GpuDistDims;
@group(0) @binding(65) var<storage, read> dist_test_hdr_texels: array<vec4<f32>>;
@group(0) @binding(66) var<uniform> dist_test_hdr_dims: HdrEnvDims;

fn tf_dist1d_find_bucket(cdf_start: u32, count: u32, u: f32) -> u32 {
    var first: u32 = 0u;
    var len: u32 = count + 1u;
    while (len > 0u) {
        let half = len >> 1u;
        let middle = first + half;
        if (dist_test_cdf[cdf_start + middle] <= u) {
            first = middle + 1u;
            len = len - (half + 1u);
        } else {
            len = half;
        }
    }
    return clamp(first - 1u, 0u, count - 1u);
}

fn tf_dist1d_bucket_pdf(func_start: u32, func_int: f32, offset: u32) -> f32 {
    if (func_int > 0.0) {
        return max(dist_test_func[func_start + offset], 0.0) / func_int;
    }
    return 1.0;
}

fn tf_dist1d_sample_continuous(
    func_start: u32,
    cdf_start: u32,
    count: u32,
    func_int: f32,
    u_in: f32,
) -> Dist1dSample {
    let u = clamp(u_in, 0.0, 0.99999994);
    let offset = tf_dist1d_find_bucket(cdf_start, count, u);
    let span = dist_test_cdf[cdf_start + offset + 1u] - dist_test_cdf[cdf_start + offset];
    var du: f32 = 0.0;
    if (span > 0.0) {
        du = (u - dist_test_cdf[cdf_start + offset]) / span;
    }
    let sample = clamp((f32(offset) + du) / f32(count), 0.0, 0.99999994);
    let pdf = tf_dist1d_bucket_pdf(func_start, func_int, offset);

    var res: Dist1dSample;
    res.sample = sample;
    res.pdf = pdf;
    res.offset = offset;
    return res;
}

fn tf_dist1d_pdf(func_start: u32, count: u32, func_int: f32, x_in: f32) -> f32 {
    let x = clamp(x_in, 0.0, 0.99999994);
    let offset = min(u32(x * f32(count)), count - 1u);
    return tf_dist1d_bucket_pdf(func_start, func_int, offset);
}

fn tf_hdr_env_sample_bilinear(u_in: f32, v_in: f32) -> vec3<f32> {
    let width = dist_test_hdr_dims.width;
    let height = dist_test_hdr_dims.height;
    let width_i = i32(width);
    let height_i = i32(height);

    let u_wrapped = fract(u_in);
    let v_clamped = clamp(v_in, 0.0, 1.0);
    let fx = fma(u_wrapped, f32(width), -0.5);
    let fy = fma(v_clamped, f32(height), -0.5);

    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;

    let x0i = hdr_wrap_x(i32(x0), width_i);
    let x1i = hdr_wrap_x(i32(x0) + 1, width_i);
    let y0i = hdr_clamp_y(i32(y0), height_i);
    let y1i = hdr_clamp_y(i32(y0) + 1, height_i);

    let p00 = dist_test_hdr_texels[u32(y0i) * width + u32(x0i)].xyz;
    let p10 = dist_test_hdr_texels[u32(y0i) * width + u32(x1i)].xyz;
    let p01 = dist_test_hdr_texels[u32(y1i) * width + u32(x0i)].xyz;
    let p11 = dist_test_hdr_texels[u32(y1i) * width + u32(x1i)].xyz;

    // Same blend order as `hdr_env_sample_bilinear` in transport_bounce.wgsl and
    // `EnvironmentMap::radiance_at`; a different formula here would hide a real
    // production mismatch behind an unrelated harness one.
    let top = fma(p10, vec3<f32>(tx), p00 * (1.0 - tx));
    let bottom = fma(p11, vec3<f32>(tx), p01 * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}

fn tf_dist2d_sample(u0: f32, u1: f32) -> Dist2dSample {
    let width = dist_test_dims.width;
    let height = dist_test_dims.height;
    let marginal_func_start = width * height;
    let marginal_cdf_start = height * (width + 1u);
    let marginal_func_int = dist_test_dims.marginal_func_int;

    let v_sample = tf_dist1d_sample_continuous(marginal_func_start, marginal_cdf_start, height, marginal_func_int, u1);
    let row = v_sample.offset;

    let cond_func_start = row * width;
    let cond_cdf_start = row * (width + 1u);
    let cond_func_int = dist_test_func[marginal_func_start + row];
    let u_sample = tf_dist1d_sample_continuous(cond_func_start, cond_cdf_start, width, cond_func_int, u0);

    let dir = hdr_uv_to_direction(u_sample.sample, v_sample.sample);
    let rgb = tf_hdr_env_sample_bilinear(u_sample.sample, v_sample.sample);
    let pdf_uv = u_sample.pdf * v_sample.pdf;
    let pdf = pdf_uv_to_solid_angle(pdf_uv, v_sample.sample);

    var res: Dist2dSample;
    res.dir = dir;
    res.rgb = rgb;
    res.pdf = pdf;
    return res;
}

fn tf_dist2d_pdf_uv(u: f32, v: f32) -> f32 {
    let width = dist_test_dims.width;
    let height = dist_test_dims.height;
    let marginal_func_start = width * height;
    let marginal_func_int = dist_test_dims.marginal_func_int;

    let row = min(u32(clamp(v, 0.0, 0.99999994) * f32(height)), height - 1u);
    let pdf_v = tf_dist1d_pdf(marginal_func_start, height, marginal_func_int, v);

    let cond_func_start = row * width;
    let cond_func_int = dist_test_func[marginal_func_start + row];
    let pdf_u = tf_dist1d_pdf(cond_func_start, width, cond_func_int, u);

    return pdf_u * pdf_v;
}

// Same `sin(theta)`-off-the-direction form as transport_bounce.wgsl's `dist2d_pdf` and
// `EnvironmentMap::pdf` -- see the latter's own comment.
fn tf_dist2d_pdf(dir: vec3<f32>) -> f32 {
    let uv = hdr_direction_to_uv(dir);
    let pdf_uv = tf_dist2d_pdf_uv(uv.x, uv.y);
    let d = normalize(dir);
    let sin_theta = length(vec2<f32>(d.x, d.z));
    return pdf_uv_to_solid_angle_from_sin(pdf_uv, sin_theta);
}

// ---------------------------------------------------------------------------------
// dist1d_find_bucket_main
// ---------------------------------------------------------------------------------

struct Dist1dFindBucketCase {
    cdf_start: u32,
    count: u32,
    u: f32,
    _pad0: f32,
}

@group(0) @binding(67) var<storage, read> dist1d_find_bucket_cases: array<Dist1dFindBucketCase>;
@group(0) @binding(68) var<storage, read_write> dist1d_find_bucket_out: array<u32>;

@compute @workgroup_size(64)
fn dist1d_find_bucket_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dist1d_find_bucket_cases)) {
        return;
    }
    let c = dist1d_find_bucket_cases[idx];
    dist1d_find_bucket_out[idx] = tf_dist1d_find_bucket(c.cdf_start, c.count, c.u);
}

// ---------------------------------------------------------------------------------
// dist2d_sample_main
// ---------------------------------------------------------------------------------

struct Dist2dSampleCase {
    u0: f32,
    u1: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(69) var<storage, read> dist2d_sample_cases: array<Dist2dSampleCase>;
@group(0) @binding(70) var<storage, read_write> dist2d_sample_out: array<f32>;

@compute @workgroup_size(64)
fn dist2d_sample_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dist2d_sample_cases)) {
        return;
    }
    let c = dist2d_sample_cases[idx];
    let sample = tf_dist2d_sample(c.u0, c.u1);
    let base = idx * 7u;
    dist2d_sample_out[base + 0u] = sample.dir.x;
    dist2d_sample_out[base + 1u] = sample.dir.y;
    dist2d_sample_out[base + 2u] = sample.dir.z;
    dist2d_sample_out[base + 3u] = sample.rgb.x;
    dist2d_sample_out[base + 4u] = sample.rgb.y;
    dist2d_sample_out[base + 5u] = sample.rgb.z;
    dist2d_sample_out[base + 6u] = sample.pdf;
}

// ---------------------------------------------------------------------------------
// dist2d_pdf_main
// ---------------------------------------------------------------------------------

struct Dist2dPdfCase {
    dir: vec3<f32>,
    _pad0: f32,
}

@group(0) @binding(71) var<storage, read> dist2d_pdf_cases: array<Dist2dPdfCase>;
@group(0) @binding(72) var<storage, read_write> dist2d_pdf_out: array<f32>;

@compute @workgroup_size(64)
fn dist2d_pdf_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dist2d_pdf_cases)) {
        return;
    }
    let c = dist2d_pdf_cases[idx];
    dist2d_pdf_out[idx] = tf_dist2d_pdf(c.dir);
}

// ---------------------------------------------------------------------------------
// nee_frosted_exterior_main
// ---------------------------------------------------------------------------------

struct NeeFrostedExteriorCase {
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    lambdas: array<f32, 8>,
    stokes: array<vec4<f32>, 8>,
    radiance_in: array<f32, 8>,
}

@group(0) @binding(73) var<storage, read> nee_frosted_cases: array<NeeFrostedExteriorCase>;
@group(0) @binding(74) var<storage, read_write> nee_frosted_out: array<f32>;

fn tf_nee_contribution_frosted_exterior(
    lambdas: ptr<function, array<f32, 8>>,
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = tf_dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let cos_light = dot(sample.dir, ext_normal);
    if (cos_light <= 0.0) {
        return;
    }

    let brdf_pdf = cos_light / PI;
    let mis_weight = balance_heuristic(sample.pdf, brdf_pdf);
    if (mis_weight <= 0.0) {
        return;
    }

    let nee_common = brdf_pdf * mis_weight / sample.pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let env_k = rgb_to_spectral_radiance(sample.rgb.x, sample.rgb.y, sample.rgb.z, (*lambdas)[k]);
        (*radiance)[k] = fma((*stokes)[k].x * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

@compute @workgroup_size(64)
fn nee_frosted_exterior_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&nee_frosted_cases)) {
        return;
    }
    let c = nee_frosted_cases[idx];
    var lambdas = c.lambdas;
    var stokes = c.stokes;
    var radiance = c.radiance_in;
    tf_nee_contribution_frosted_exterior(
        &lambdas, c.ext_normal, c.rng_seed, c.bounce, &stokes, &radiance,
    );
    let base = idx * 8u;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        nee_frosted_out[base + k] = radiance[k];
    }
}

// ---------------------------------------------------------------------------------
// nee_hg_scatter_main
// ---------------------------------------------------------------------------------

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
}

struct HitInfo {
    hit: bool,
    t: f32,
    normal: vec3<f32>,
    facet_idx: u32,
}

@group(0) @binding(75) var<storage, read> tf_planes: array<FacetPlane>;

fn tf_intersect_ray(origin: vec3<f32>, dir: vec3<f32>) -> HitInfo {
    var t_near: f32 = -1e30;
    var t_far: f32 = 1e30;
    var near_normal = vec3<f32>(0.0, 0.0, 0.0);
    var far_normal = vec3<f32>(0.0, 0.0, 0.0);
    var near_idx: u32 = 0u;
    var far_idx: u32 = 0u;
    var result: HitInfo;
    let num_planes = arrayLength(&tf_planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        let p = tf_planes[i];
        let n = p.normal;
        let denom = dot(n, dir);
        let side = p.d + dot(n, origin);
        let numer = -side;
        if (abs(denom) > 1e-7) {
            let t = numer / denom;
            if (denom < 0.0) {
                if (t > t_near) {
                    t_near = t;
                    near_normal = n;
                    near_idx = i;
                }
            } else if (t < t_far) {
                t_far = t;
                far_normal = n;
                far_idx = i;
            }
        } else if (side > 0.0) {
            result.hit = false;
            result.t = 0.0;
            result.normal = vec3<f32>(0.0, 0.0, 0.0);
            result.facet_idx = 0u;
            return result;
        }
    }
    if (t_near > t_far) {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
    } else if (t_near > 1e-4) {
        result.hit = true;
        result.t = t_near;
        result.normal = near_normal;
        result.facet_idx = near_idx;
    } else if (t_far > 1e-4) {
        result.hit = true;
        result.t = t_far;
        result.normal = far_normal;
        result.facet_idx = far_idx;
    } else {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
    }
    return result;
}


// Findings 2b/2d: `sigma_s`/`absorption_path_scale`/`alphas` and `frosted_exit` mirror
// `optics::raytracer::scattering::nee_contribution_hg_scatter`'s own extra parameters
// -- the standalone harness has no scene-wide `material`/`facet_finishes` bindings the
// megakernel (`transport_bounce.wgsl`) reads those from, so each case carries its own
// copy instead (see the CPU-side `NeeHgScatterCase` struct's doc comments for how
// `run_nee_hg_scatter` drives them).
struct NeeHgScatterCase {
    scatter_point: vec3<f32>,
    n_inside_hero: f32,
    scatter_dir_in: vec3<f32>,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    sigma_s: f32,
    absorption_path_scale: f32,
    frosted_exit: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    alphas: array<f32, 8>,
    lambdas: array<f32, 8>,
    stokes: array<vec4<f32>, 8>,
    radiance_in: array<f32, 8>,
}

@group(0) @binding(76) var<storage, read> nee_hg_cases: array<NeeHgScatterCase>;
@group(0) @binding(77) var<storage, read_write> nee_hg_out: array<f32>;

// Operation-for-operation copy of `transport_bounce.wgsl`'s `nee_contribution_hg_scatter`
// (itself the WGSL translation of `optics::raytracer::scattering::nee_contribution_hg_scatter`,
// findings 2a-2d), adapted only in how it reaches the medium/finish inputs the megakernel
// reads off the shared `material`/`facet_finishes` bindings: this standalone twin takes
// them as explicit per-case parameters instead (`alphas`, `sigma_s`,
// `absorption_path_scale`, `frosted_exit`), and samples the HDR environment through this
// file's own `tf_hdr_env_sample_bilinear`/`tf_dist2d_sample` (bindings 62-66) rather than
// the megakernel's scene-wide texture bindings.
fn tf_nee_contribution_hg_scatter(
    lambdas: ptr<function, array<f32, 8>>,
    n_inside_hero: f32,
    scatter_point: vec3<f32>,
    scatter_dir_in: vec3<f32>,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    alphas: array<f32, 8>,
    sigma_s: f32,
    absorption_path_scale: f32,
    frosted_exit: u32,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = tf_dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let probe_origin = scatter_point + sample.dir * 1e-4;
    let hit = tf_intersect_ray(probe_origin, sample.dir);
    if (!hit.hit) {
        return;
    }
    // Mirrors `nee_contribution_hg_scatter`'s `facet_finishes` check -- the
    // standalone harness has no scene-wide facet-finish buffer to index by `hit.facet_idx`,
    // so the CPU driver marks the whole test cube Frosted or Polished up front and passes
    // that verdict directly (see `NeeHgScatterCase::frosted_exit`'s doc comment).
    if (frosted_exit != 0u) {
        return;
    }

    let cos_i = clamp(dot(sample.dir, hit.normal), 0.0, 1.0);
    let sin2_t = min(n_inside_hero * n_inside_hero * fma(-cos_i, cos_i, 1.0), 1.0);
    if (sin2_t >= 1.0) {
        return;
    }
    let cos_t = sqrt(max(1.0 - sin2_t, 0.0));
    let r_s = fma(n_inside_hero, cos_i, -cos_t) / fma(n_inside_hero, cos_i, cos_t);
    let r_p = fma(n_inside_hero, -cos_t, cos_i) / fma(n_inside_hero, cos_t, cos_i);
    let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), 0.0, 1.0);
    let t_unpol = 1.0 - r_unpol;

    let phase_cos = dot(sample.dir, scatter_dir_in);
    let phase_val = henyey_greenstein_phase(phase_cos, g);
    let mis_weight = balance_heuristic(sample.pdf, phase_val);
    if (mis_weight <= 0.0) {
        return;
    }

    // The exterior direction this light sample actually leaves along --
    // same Snell's-law form as the megakernel twin; `sample.pdf` stays in the INTERIOR
    // (pre-refraction) measure `tf_dist2d_sample` sampled in, only the radiance LOOKUP
    // moves to the refracted direction.
    let refracted_dir = normalize(n_inside_hero * sample.dir - fma(n_inside_hero, cos_i, -cos_t) * hit.normal);
    let refracted_uv = hdr_direction_to_uv(refracted_dir);
    let env_rgb = tf_hdr_env_sample_bilinear(refracted_uv.x, refracted_uv.y);

    // The medium transmittance a phase-sampled continuation reaching this
    // same boundary would have paid -- the same per-channel
    // `exp_poly(-(alphas[k]+sigma_s)*hit.t*path_scale)` the megakernel twin and the CPU
    // function both apply (`exp_poly`, not the `exp()` builtin -- see
    // that function's own doc comment, `transport_physics.wgsl`).
    let hit_t_scaled = hit.t * absorption_path_scale;

    let nee_common = t_unpol * phase_val * mis_weight / sample.pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let transmittance_k = exp_poly(-(alphas[k] + sigma_s) * hit_t_scaled);
        let env_k = rgb_to_spectral_radiance(env_rgb.x, env_rgb.y, env_rgb.z, (*lambdas)[k]);
        (*radiance)[k] = fma((*stokes)[k].x * transmittance_k * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

@compute @workgroup_size(64)
fn nee_hg_scatter_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&nee_hg_cases)) {
        return;
    }
    let c = nee_hg_cases[idx];
    var lambdas = c.lambdas;
    var stokes = c.stokes;
    var radiance = c.radiance_in;
    tf_nee_contribution_hg_scatter(
        &lambdas, c.n_inside_hero, c.scatter_point, c.scatter_dir_in,
        c.g, c.rng_seed, c.bounce, &stokes, &radiance,
        c.alphas, c.sigma_s, c.absorption_path_scale, c.frosted_exit,
    );
    let base = idx * 8u;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        nee_hg_out[base + k] = radiance[k];
    }
}
