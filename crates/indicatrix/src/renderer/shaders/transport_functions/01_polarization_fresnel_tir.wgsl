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

