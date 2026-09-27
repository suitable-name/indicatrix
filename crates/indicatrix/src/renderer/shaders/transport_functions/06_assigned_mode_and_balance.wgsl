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

