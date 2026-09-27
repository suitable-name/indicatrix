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

