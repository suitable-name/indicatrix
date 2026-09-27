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

