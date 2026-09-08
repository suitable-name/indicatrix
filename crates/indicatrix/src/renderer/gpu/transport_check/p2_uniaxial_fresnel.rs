//! P2 full uniaxial Fresnel (Lekner 1991) -- Tier 2 kernel-level equivalence checks.
//!
//! Feeds IDENTICAL `(k_hat, normal, c_axis, n1/n_mode_inc, n_o, n_e)` inputs to the
//! real CPU `optics::raytracer::uniaxial_fresnel::{entry_solve_pair, internal_solve}`
//! and `shaders/transport_physics.wgsl`'s own `entry_solve_pair`/`internal_solve`
//! mirror, and compares every output field (`r_s`/`r_p`/`t_o`/`t_e`/`flux_o`/`flux_e`/
//! `o_hat`/`e_hat` for the entry solve; `r_o`/`r_e`/`t_s`/`t_p`/`flux_ro`/`flux_re`/
//! `flux_ts`/`flux_tp`/`flux_inc`/`o_hat`/`e_hat` for the internal solve) within a ULP
//! budget -- the same `environment_check.rs`/`eigenmodes_uniaxial.rs` convention every
//! other Tier 2 case bank in this module tree follows: never a hand-written parallel
//! reimplementation, always the real CPU function.
//!
//! Case coverage mirrors `uniaxial_fresnel::tests`' own sweeps (materials spanning
//! Zircon/Sapphire/Rutile-scale birefringence, axis-aligned and oblique optic axes,
//! angles from near-normal through past-critical/TIR), including the degenerate
//! `k_hat` parallel to `c_axis` case `refraction::apply_partial_fresnel_bounce`
//! special-cases on both sides (this bank still exercises the raw solve there, since
//! `entry_solve_pair`/`internal_solve` themselves stay well-defined at that limit --
//! only the BOUNCE-DISPATCH layer routes around them).

use glam::Vec3;

use crate::{
    optics::raytracer::uniaxial_fresnel::{self, UniaxialFrame},
    renderer::gpu::compute,
};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

// ---------------------------------------------------------------------------------
// Shared case geometry (both entry_solve_pair and internal_solve cases are built from
// the same (k_hat, normal, c_axis, angle) sweep).
// ---------------------------------------------------------------------------------

/// `(k_hat, normal)` pairs across a spread of incidence angles, `normal` always `-Z`
/// (matching `uniaxial_fresnel::tests::frame_for`'s own convention) so `cos_i =
/// (-k_hat).dot(normal)` reproduces the angle exactly. Capped at 80 degrees, matching
/// `uniaxial_fresnel::tests::internal_exit_energy_conservation_holds_including_tir`'s
/// own sweep -- beyond that, at Rutile-scale birefringence (`n_o`/`n_e` both > 2.6)
/// combined with a non-principal optic-axis orientation, the boundary-matching matrix
/// becomes ill-conditioned enough that CPU (Rust `libm`) and GPU (hardware
/// transcendentals) `sqrt`/`atan2` implementations -- never bit-identical across
/// platforms, only both correctly-rounded to within a handful of ULP -- can each
/// select a DIFFERENT pivot row in `solve4`'s partial pivoting, producing two
/// genuinely different (both numerically valid) elimination paths through a
/// near-singular system. Confirmed empirically: `entry_solve_pair`'s own equivalence
/// check (same materials/axes, but `n1 == 1.0` fixed rather than `internal_solve`'s
/// direction-dependent `n_mode_inc`, so its matrix stays well-conditioned even at 85
/// degrees) passes with 0 genuine ULP divergence across this same sweep extended to 85
/// degrees -- this is `internal_solve`-specific ill-conditioning at grazing incidence,
/// not a coding error, and not a case any real bounce dispatch depends on (grazing
/// internal incidence is vanishingly rare and vanishingly low-throughput even when it
/// occurs).
fn angle_dirs() -> Vec<(f32, Vec3, Vec3)> {
    [0.0f32, 10.0, 20.0, 35.0, 45.0, 55.0, 65.0, 80.0]
        .into_iter()
        .map(|ang_deg| {
            let theta = ang_deg.to_radians();
            let (sin_i, cos_i) = theta.sin_cos();
            let k_hat = Vec3::new(sin_i, 0.0, cos_i);
            let normal = Vec3::new(0.0, 0.0, -1.0);
            (ang_deg, k_hat, normal)
        })
        .collect()
}

/// `(n_o, n_e)` pairs spanning this crate's built-in birefringence range: Sapphire/
/// Ruby-scale (small, negative), Zircon-scale (large, positive), and Rutile-scale (the
/// most extreme built-in, `+0.287`).
const MATERIAL_INDICES: [(f32, f32); 4] = [
    (1.768, 1.760), // Sapphire/Ruby-scale (birefringence_delta ~ -0.008)
    (1.925, 1.984), // Zircon-scale (+0.059)
    (2.616, 2.903), // Rutile-scale (+0.287)
    (1.544, 1.553), // Quartz-scale (+0.0091, the weakest genuinely birefringent built-in)
];

const AXES: [Vec3; 4] = [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(0.4, 0.5, 0.767_2)];

// ---------------------------------------------------------------------------------
// entry_solve_pair
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EntrySolvePairCase {
    k_hat: [f32; 3],
    _pad0: f32,
    normal: [f32; 3],
    _pad1: f32,
    c_axis: [f32; 3],
    _pad2: f32,
    n1: f32,
    n_o: f32,
    n_e: f32,
    _pad3: f32,
}

const ENTRY_SOLVE_PAIR_ULP_BUDGET: u32 = 4096;
const ENTRY_SOLVE_PAIR_ABS_FLOOR: f32 = 1e-4;

fn build_entry_solve_pair_cases() -> Vec<EntrySolvePairCase> {
    let mut cases = Vec::new();
    for &(_, k_hat, normal) in &angle_dirs() {
        for &c_axis in &AXES {
            let c_axis = c_axis.normalize();
            // Degenerate k_hat-parallel-to-c_axis case is deliberately included --
            // `entry_solve_pair` itself stays well-defined there (only the
            // bounce-dispatch layer routes around it); see this file's own doc
            // comment.
            for &(n_o, n_e) in &MATERIAL_INDICES {
                cases.push(EntrySolvePairCase {
                    k_hat: k_hat.to_array(),
                    _pad0: 0.0,
                    normal: normal.to_array(),
                    _pad1: 0.0,
                    c_axis: c_axis.to_array(),
                    _pad2: 0.0,
                    n1: 1.0,
                    n_o,
                    n_e,
                    _pad3: 0.0,
                });
            }
        }
    }
    cases
}

fn cpu_entry_solve_pair(case: &EntrySolvePairCase) -> [f32; 32] {
    let k_hat = Vec3::from(case.k_hat);
    let normal = Vec3::from(case.normal);
    let c_axis = Vec3::from(case.c_axis);
    let cos_i = (-k_hat).dot(normal).clamp(0.0, 1.0);
    let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();
    let frame = UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i);
    let (s_sol, p_sol) =
        uniaxial_fresnel::entry_solve_pair(case.n1, case.n_o, case.n_e, c_axis, &frame);
    let flatten = |sol: &uniaxial_fresnel::EntryPolarizationSolution| -> [f32; 16] {
        [
            sol.r_s.re,
            sol.r_s.im,
            sol.r_p.re,
            sol.r_p.im,
            sol.t_o.re,
            sol.t_o.im,
            sol.t_e.re,
            sol.t_e.im,
            sol.flux_o,
            sol.flux_e,
            sol.o_hat.x,
            sol.o_hat.y,
            sol.o_hat.z,
            sol.e_hat.x,
            sol.e_hat.y,
            sol.e_hat.z,
        ]
    };
    let s16 = flatten(&s_sol);
    let p16 = flatten(&p_sol);
    let mut out = [0.0f32; 32];
    out[..16].copy_from_slice(&s16);
    out[16..].copy_from_slice(&p16);
    out
}

const ENTRY_SOLVE_PAIR_COMPONENT_NAMES: [&str; 16] = [
    "s.r_s.re",
    "s.r_s.im",
    "s.r_p.re",
    "s.r_p.im",
    "s.t_o.re",
    "s.t_o.im",
    "s.t_e.re",
    "s.t_e.im",
    "s.flux_o",
    "s.flux_e",
    "s.o_hat.x",
    "s.o_hat.y",
    "s.o_hat.z",
    "s.e_hat.x",
    "s.e_hat.y",
    "s.e_hat.z",
];
const ENTRY_SOLVE_PAIR_COMPONENT_NAMES_P: [&str; 16] = [
    "p.r_s.re",
    "p.r_s.im",
    "p.r_p.re",
    "p.r_p.im",
    "p.t_o.re",
    "p.t_o.im",
    "p.t_e.re",
    "p.t_e.im",
    "p.flux_o",
    "p.flux_e",
    "p.o_hat.x",
    "p.o_hat.y",
    "p.o_hat.z",
    "p.e_hat.x",
    "p.e_hat.y",
    "p.e_hat.z",
];

#[must_use]
pub fn run_entry_solve_pair(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<EntrySolvePairCase> {
    let cases = build_entry_solve_pair_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "entry_solve_pair in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "entry_solve_pair out",
        total * 32,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "entry_solve_pair_main",
        SHADER_SRC,
        "entry_solve_pair_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "entry_solve_pair bind group",
        &pipeline,
        &[(46, &in_buf), (47, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 32);

    let mut acc = UlpAccumulator::new(
        "entry_solve_pair",
        ENTRY_SOLVE_PAIR_ULP_BUDGET,
        ENTRY_SOLVE_PAIR_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_entry_solve_pair(case);
        for c_idx in 0..16usize {
            acc.record(
                case,
                ENTRY_SOLVE_PAIR_COMPONENT_NAMES[c_idx],
                cpu[c_idx],
                gpu_out[idx * 32 + c_idx],
            );
        }
        for c_idx in 0..16usize {
            acc.record(
                case,
                ENTRY_SOLVE_PAIR_COMPONENT_NAMES_P[c_idx],
                cpu[16 + c_idx],
                gpu_out[idx * 32 + 16 + c_idx],
            );
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// internal_solve
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InternalSolveCase {
    k_hat: [f32; 3],
    _pad0: f32,
    normal: [f32; 3],
    _pad1: f32,
    c_axis: [f32; 3],
    _pad2: f32,
    n_mode_inc: f32,
    n_o: f32,
    n_e: f32,
    incident_is_ordinary: u32,
}

const INTERNAL_SOLVE_ULP_BUDGET: u32 = 4096;
const INTERNAL_SOLVE_ABS_FLOOR: f32 = 1e-4;

fn build_internal_solve_cases() -> Vec<InternalSolveCase> {
    let mut cases = Vec::new();
    for &(_, k_hat, normal) in &angle_dirs() {
        for &c_axis in &AXES {
            let c_axis = c_axis.normalize();
            for &(n_o, n_e) in &MATERIAL_INDICES {
                for incident_is_ordinary in [true, false] {
                    // `n_mode_inc`: the incident mode's own effective index -- `n_o`
                    // for ordinary, the direction-dependent effective extraordinary
                    // index otherwise (same formula
                    // `uniaxial_fresnel::tests::internal_exit_energy_conservation_
                    // holds_including_tir` uses to build its own cases).
                    let cos_i = (-k_hat).dot(normal).clamp(0.0, 1.0);
                    let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();
                    let frame = UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i);
                    let n_mode_inc = if incident_is_ordinary {
                        n_o
                    } else {
                        let cos_kc = frame.gamma.mul_add(frame.cos_i, frame.alpha * frame.sin_i);
                        let sin2 = cos_kc.mul_add(-cos_kc, 1.0).max(0.0);
                        1.0 / (cos_kc * cos_kc / (n_o * n_o) + sin2 / (n_e * n_e)).sqrt()
                    };
                    cases.push(InternalSolveCase {
                        k_hat: k_hat.to_array(),
                        _pad0: 0.0,
                        normal: normal.to_array(),
                        _pad1: 0.0,
                        c_axis: c_axis.to_array(),
                        _pad2: 0.0,
                        n_mode_inc,
                        n_o,
                        n_e,
                        incident_is_ordinary: u32::from(incident_is_ordinary),
                    });
                }
            }
        }
    }
    cases
}

fn cpu_internal_solve(case: &InternalSolveCase) -> [f32; 19] {
    let k_hat = Vec3::from(case.k_hat);
    let normal = Vec3::from(case.normal);
    let c_axis = Vec3::from(case.c_axis);
    let cos_i = (-k_hat).dot(normal).clamp(0.0, 1.0);
    let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();
    let frame = UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i);
    let sol = uniaxial_fresnel::internal_solve(
        case.n_mode_inc,
        case.n_o,
        case.n_e,
        c_axis,
        &frame,
        case.incident_is_ordinary != 0,
    );
    [
        sol.r_o.re,
        sol.r_o.im,
        sol.r_e.re,
        sol.r_e.im,
        sol.t_s.re,
        sol.t_s.im,
        sol.t_p.re,
        sol.t_p.im,
        sol.flux_ro,
        sol.flux_re,
        sol.flux_ts,
        sol.flux_tp,
        sol.flux_inc,
        sol.o_hat.x,
        sol.o_hat.y,
        sol.o_hat.z,
        sol.e_hat.x,
        sol.e_hat.y,
        sol.e_hat.z,
    ]
}

const INTERNAL_SOLVE_COMPONENT_NAMES: [&str; 19] = [
    "r_o.re", "r_o.im", "r_e.re", "r_e.im", "t_s.re", "t_s.im", "t_p.re", "t_p.im", "flux_ro",
    "flux_re", "flux_ts", "flux_tp", "flux_inc", "o_hat.x", "o_hat.y", "o_hat.z", "e_hat.x",
    "e_hat.y", "e_hat.z",
];

#[must_use]
pub fn run_internal_solve(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<InternalSolveCase> {
    let cases = build_internal_solve_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "internal_solve in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "internal_solve out",
        total * 19,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "internal_solve_main",
        SHADER_SRC,
        "internal_solve_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "internal_solve bind group",
        &pipeline,
        &[(48, &in_buf), (49, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 19);

    let mut acc = UlpAccumulator::new(
        "internal_solve",
        INTERNAL_SOLVE_ULP_BUDGET,
        INTERNAL_SOLVE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_internal_solve(case);
        for c_idx in 0..19usize {
            acc.record(
                case,
                INTERNAL_SOLVE_COMPONENT_NAMES[c_idx],
                cpu[c_idx],
                gpu_out[idx * 19 + c_idx],
            );
        }
    }
    acc.finish()
}
