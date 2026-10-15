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
/// Ruby-scale (small, negative), Zircon-scale (large, positive), and a Rutile-like pair
/// (`+0.287`; real Rutile is `+0.2957`).
const MATERIAL_INDICES: [(f32, f32); 4] = [
    (1.768, 1.760), // Sapphire/Ruby-scale (birefringence_delta ~ -0.008)
    (1.925, 1.984), // Zircon-scale (+0.059)
    (2.616, 2.903), // Rutile-like pair (+0.287; real Rutile is +0.2957)
    (1.544, 1.553), // Quartz-scale (+0.0091, the weakest genuinely birefringent built-in)
];

const AXES: [Vec3; 4] = [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(0.4, 0.5, 0.767_2)];

// ---------------------------------------------------------------------------------
// entry_solve_pair
// ---------------------------------------------------------------------------------

/// One input case for the entry solve pair check.
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

/// Runs the entry solve pair check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
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
    let workgroups = u32::try_from(total)
        .expect("the dispatched case count fits in u32")
        .div_ceil(64);
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

/// One input case for the internal solve check.
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

/// Runs the internal solve check against the CPU reference.
///
/// # Panics
///
/// Panics if the number of cases does not fit in `u32`, which the fixed case table never reaches.
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
    let workgroups = u32::try_from(total)
        .expect("the dispatched case count fits in u32")
        .div_ceil(64);
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

// ---------------------------------------------------------------------------------
// solve4 unrolled vs rolled: the same solution within rounding
// ---------------------------------------------------------------------------------

/// Equivalence check of the constant-index unrolled `solve4_single` / `solve4_two_rhs`
/// (generated into `06_uniaxial_fullwave_complex.wgsl` by `tools/unroll_solve4.py`)
/// against the original rolled, dynamically-pivoting forms.
///
/// The unrolled text performs the same IEEE operations in the same order (strict `>`
/// pivot choice over ascending rows, whole-row conditional swap, `!= 0.0` factor skip),
/// but the two are NOT bit-identical on every backend: WGSL has no way to forbid FMA
/// contraction or reassociation, and the owner's GPU compiler contracts the two code
/// shapes differently (1-3 ULP in a few components; 2026-10-14, still 1 ULP after making
/// `cplx_mul` an explicit `fma`, which was reverted). So each solution is compared with a
/// relative tolerance against its own magnitude (`SOLVE4_RELATIVE_TOLERANCE`): far above
/// rounding through a pivoted 4x4 elimination, far below what a wrong pivot, a skipped row
/// operation or a swapped right-hand side produces. The largest deviation and the number of
/// bit-identical solutions are printed. Needs a real adapter: it skips cleanly without one
/// unless `INDICATRIX_REQUIRE_GPU=1`. Run with
///
/// ```text
/// cargo test -p indicatrix --features gpu unrolled_solve4 -- --nocapture
/// INDICATRIX_REQUIRE_GPU=1 cargo test -p indicatrix --features gpu unrolled_solve4
/// ```
#[cfg(test)]
mod unrolled_solve4 {
    use super::{SHADER_SRC, compute};
    use crate::renderer::gpu::GpuContext;

    /// Floats per case: 4x4 complex `A` (32), complex `b1` (8), complex `b2` (8).
    const CASE_FLOATS: usize = 48;

    /// Verbatim copy of the original rolled solvers, renamed. Kept here (not in the
    /// production shader) purely as the reference side of this comparison. Reuses
    /// `Cplx`, `cplx_*` and `Solve4TwoRhsResult` from the production text.
    const ROLLED_REFERENCE_WGSL: &str = r"
fn solve4_single_rolled(a_in: array<array<Cplx, 4>, 4>, b_in: array<Cplx, 4>) -> array<Cplx, 4> {
    var a = a_in;
    var b = b_in;
    for (var col: u32 = 0u; col < 4u; col = col + 1u) {
        var piv = col;
        var piv_mag = cplx_norm_sqr(a[col][col]);
        for (var row: u32 = col + 1u; row < 4u; row = row + 1u) {
            let mag = cplx_norm_sqr(a[row][col]);
            if (mag > piv_mag) {
                piv = row;
                piv_mag = mag;
            }
        }
        for (var j: u32 = 0u; j < 4u; j = j + 1u) {
            let tmp = a[col][j];
            a[col][j] = a[piv][j];
            a[piv][j] = tmp;
        }
        let tmpb = b[col];
        b[col] = b[piv];
        b[piv] = tmpb;

        let pivot = a[col][col];
        for (var j: u32 = col; j < 4u; j = j + 1u) {
            a[col][j] = cplx_div(a[col][j], pivot);
        }
        b[col] = cplx_div(b[col], pivot);
        for (var row: u32 = 0u; row < 4u; row = row + 1u) {
            if (row == col) {
                continue;
            }
            let factor = a[row][col];
            if (cplx_norm_sqr(factor) == 0.0) {
                continue;
            }
            for (var j: u32 = col; j < 4u; j = j + 1u) {
                a[row][j] = cplx_sub(a[row][j], cplx_mul(factor, a[col][j]));
            }
            b[row] = cplx_sub(b[row], cplx_mul(factor, b[col]));
        }
    }
    return b;
}

fn solve4_two_rhs_rolled(a_in: array<array<Cplx, 4>, 4>, b1_in: array<Cplx, 4>, b2_in: array<Cplx, 4>) -> Solve4TwoRhsResult {
    var a = a_in;
    var b1 = b1_in;
    var b2 = b2_in;
    for (var col: u32 = 0u; col < 4u; col = col + 1u) {
        var piv = col;
        var piv_mag = cplx_norm_sqr(a[col][col]);
        for (var row: u32 = col + 1u; row < 4u; row = row + 1u) {
            let mag = cplx_norm_sqr(a[row][col]);
            if (mag > piv_mag) {
                piv = row;
                piv_mag = mag;
            }
        }
        for (var j: u32 = 0u; j < 4u; j = j + 1u) {
            let tmp = a[col][j];
            a[col][j] = a[piv][j];
            a[piv][j] = tmp;
        }
        let tmpb1 = b1[col];
        b1[col] = b1[piv];
        b1[piv] = tmpb1;
        let tmpb2 = b2[col];
        b2[col] = b2[piv];
        b2[piv] = tmpb2;

        let pivot = a[col][col];
        for (var j: u32 = col; j < 4u; j = j + 1u) {
            a[col][j] = cplx_div(a[col][j], pivot);
        }
        b1[col] = cplx_div(b1[col], pivot);
        b2[col] = cplx_div(b2[col], pivot);
        for (var row: u32 = 0u; row < 4u; row = row + 1u) {
            if (row == col) {
                continue;
            }
            let factor = a[row][col];
            if (cplx_norm_sqr(factor) == 0.0) {
                continue;
            }
            for (var j: u32 = col; j < 4u; j = j + 1u) {
                a[row][j] = cplx_sub(a[row][j], cplx_mul(factor, a[col][j]));
            }
            b1[row] = cplx_sub(b1[row], cplx_mul(factor, b1[col]));
            b2[row] = cplx_sub(b2[row], cplx_mul(factor, b2[col]));
        }
    }
    var result: Solve4TwoRhsResult;
    result.b1 = b1;
    result.b2 = b2;
    return result;
}

// Free binding slots: 78/79 (transport_functions uses 0..=77).
@group(0) @binding(78) var<storage, read> solve4_eq_cases: array<f32>;
// 48 words per case, f32 bit patterns: [0..8) rolled single, [8..16) unrolled single,
// [16..24) rolled two-rhs b1, [24..32) rolled b2, [32..40) unrolled b1, [40..48) unrolled b2.
@group(0) @binding(79) var<storage, read_write> solve4_eq_out: array<u32>;

fn solve4_eq_load_a(base: u32) -> array<array<Cplx, 4>, 4> {
    var a: array<array<Cplx, 4>, 4>;
    for (var r: u32 = 0u; r < 4u; r = r + 1u) {
        for (var c: u32 = 0u; c < 4u; c = c + 1u) {
            let o = base + (r * 4u + c) * 2u;
            a[r][c] = Cplx(solve4_eq_cases[o], solve4_eq_cases[o + 1u]);
        }
    }
    return a;
}

fn solve4_eq_load_b(base: u32) -> array<Cplx, 4> {
    var b: array<Cplx, 4>;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        let o = base + i * 2u;
        b[i] = Cplx(solve4_eq_cases[o], solve4_eq_cases[o + 1u]);
    }
    return b;
}

fn solve4_eq_store(base: u32, b: array<Cplx, 4>) {
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        solve4_eq_out[base + i * 2u] = bitcast<u32>(b[i].re);
        solve4_eq_out[base + i * 2u + 1u] = bitcast<u32>(b[i].im);
    }
}

@compute @workgroup_size(64)
fn solve4_unrolled_equivalence_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let base = idx * 48u;
    if (base + 48u > arrayLength(&solve4_eq_cases)) {
        return;
    }
    let a = solve4_eq_load_a(base);
    let b1 = solve4_eq_load_b(base + 32u);
    let b2 = solve4_eq_load_b(base + 40u);
    solve4_eq_store(base, solve4_single_rolled(a, b1));
    solve4_eq_store(base + 8u, solve4_single(a, b1));
    let rolled2 = solve4_two_rhs_rolled(a, b1, b2);
    solve4_eq_store(base + 16u, rolled2.b1);
    solve4_eq_store(base + 24u, rolled2.b2);
    let unrolled2 = solve4_two_rhs(a, b1, b2);
    solve4_eq_store(base + 32u, unrolled2.b1);
    solve4_eq_store(base + 40u, unrolled2.b2);
}
";

    /// Deterministic splitmix64 stream (no `rand` dependency in this crate's tests).
    struct SplitMix(u64);

    impl SplitMix {
        const fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Uniform in `[lo, hi)`.
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            let unit = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
            unit.mul_add(hi - lo, lo)
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next_u64() % n as u64)
                .expect("a remainder below a usize bound fits in usize")
        }
    }

    /// All 24 permutations of `0..4`.
    fn permutations4() -> Vec<[usize; 4]> {
        let mut out = Vec::new();
        for a in 0..4 {
            for b in 0..4 {
                for c in 0..4 {
                    for d in 0..4 {
                        let p = [a, b, c, d];
                        let distinct = (0..4).all(|i| (i + 1..4).all(|j| p[i] != p[j]));
                        if distinct {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out
    }

    /// One case: `A` (row-major complex), `b1`, `b2`, as 48 floats.
    type CaseWords = [f32; CASE_FLOATS];

    fn dense(rng: &mut SplitMix) -> CaseWords {
        let mut w = [0.0f32; CASE_FLOATS];
        for v in &mut w {
            *v = rng.range(-2.0, 2.0);
        }
        w
    }

    /// Dense matrix whose rows are permuted by `perm`, so the strict-`>` pivot lands on a
    /// different row for every permutation (every conditional-swap branch is exercised).
    fn permuted(rng: &mut SplitMix, perm: [usize; 4]) -> CaseWords {
        let base = dense(rng);
        let mut w = base;
        for (dst_row, &src_row) in perm.iter().enumerate() {
            for k in 0..8 {
                w[dst_row * 8 + k] = base[src_row * 8 + k];
            }
        }
        // Spread the row magnitudes so the pivot is decided by more than the permutation.
        for row in 0..4 {
            let scale = rng.range(0.25, 4.0);
            for k in 0..8 {
                w[row * 8 + k] *= scale;
            }
        }
        w
    }

    /// Sparse matrix (roughly 40 % exact zeros, so the `!= 0.0` factor skip fires) that
    /// always keeps a non-zero permutation support so the system is not exactly singular.
    fn sparse(rng: &mut SplitMix, perms: &[[usize; 4]]) -> CaseWords {
        let mut w = dense(rng);
        for pair in 0..16 {
            if rng.below(10) < 4 {
                w[pair * 2] = 0.0;
                w[pair * 2 + 1] = 0.0;
            }
        }
        let p = perms[rng.below(perms.len())];
        for (row, &col) in p.iter().enumerate() {
            let o = (row * 4 + col) * 2;
            w[o] = rng.range(0.5, 2.0);
            w[o + 1] = rng.range(-0.5, 0.5);
        }
        w
    }

    /// Small-integer palette: many exact magnitude ties, which is where the strict `>`
    /// (first maximum wins) rule matters. Keeps a unit support so it stays solvable.
    fn ties(rng: &mut SplitMix, perms: &[[usize; 4]]) -> CaseWords {
        const PALETTE: [f32; 5] = [0.0, 1.0, -1.0, 2.0, -2.0];
        let mut w = [0.0f32; CASE_FLOATS];
        for v in &mut w {
            *v = PALETTE[rng.below(PALETTE.len())];
        }
        let p = perms[rng.below(perms.len())];
        for (row, &col) in p.iter().enumerate() {
            let o = (row * 4 + col) * 2;
            w[o] = 1.0;
            w[o + 1] = 0.0;
        }
        w
    }

    fn build_cases() -> Vec<CaseWords> {
        let perms = permutations4();
        let mut rng = SplitMix(0x50_4C_56_34);
        let mut cases = Vec::new();
        for &perm in &perms {
            for _ in 0..64 {
                cases.push(permuted(&mut rng, perm));
            }
        }
        for _ in 0..4096 {
            cases.push(dense(&mut rng));
        }
        for _ in 0..4096 {
            cases.push(sparse(&mut rng, &perms));
        }
        for _ in 0..4096 {
            cases.push(ties(&mut rng, &perms));
        }
        cases
    }

    fn all_finite(words: &[u32]) -> bool {
        words.iter().all(|&w| f32::from_bits(w).is_finite())
    }

    /// Largest allowed `|rolled - unrolled|` per component, relative to the largest
    /// component magnitude of the rolled solution. f32 epsilon is 1.2e-7; a pivoted 4x4
    /// complex elimination of these well-conditioned cases amplifies rounding by well under
    /// 100x, so 1e-4 leaves a wide margin while any structural mistake is off by O(1).
    const SOLVE4_RELATIVE_TOLERANCE: f64 = 1e-4;

    /// `max |r_i - u_i| / max |r_i|` over one solution's eight words (0 for two zero
    /// vectors).
    fn relative_deviation(rolled: &[u32], unrolled: &[u32]) -> f64 {
        let value = |w: u32| f64::from(f32::from_bits(w));
        let scale = rolled.iter().map(|&w| value(w).abs()).fold(0.0, f64::max);
        let worst = rolled
            .iter()
            .zip(unrolled)
            .map(|(&r, &u)| (value(r) - value(u)).abs())
            .fold(0.0, f64::max);
        if scale > 0.0 { worst / scale } else { worst }
    }

    #[test]
    fn unrolled_solve4_matches_rolled() {
        let ctx = match GpuContext::acquire() {
            Ok(ctx) => ctx,
            Err(error) => {
                assert!(
                    std::env::var("INDICATRIX_REQUIRE_GPU")
                        .ok()
                        .is_none_or(|v| v != "1"),
                    "INDICATRIX_REQUIRE_GPU=1 but no usable GPU adapter: {error}"
                );
                println!("skipping unrolled_solve4: no GPU adapter");
                return;
            }
        };
        let cases = build_cases();
        let total = cases.len();
        let flat: Vec<f32> = cases.iter().flat_map(|c| c.iter().copied()).collect();
        let source = format!("{SHADER_SRC}\n{ROLLED_REFERENCE_WGSL}");
        let in_buf = compute::upload(
            &ctx.device,
            "solve4 equivalence in",
            &flat,
            wgpu::BufferUsages::STORAGE,
        );
        let out_buf = compute::zeroed_buffer::<u32>(
            &ctx.device,
            "solve4 equivalence out",
            total * CASE_FLOATS,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "solve4_unrolled_equivalence_main",
            &source,
            "solve4_unrolled_equivalence_main",
        );
        let bind_group = compute::bind_buffers(
            &ctx.device,
            "solve4 equivalence bind group",
            &pipeline,
            &[(78, &in_buf), (79, &out_buf)],
        );
        let workgroups = u32::try_from(total)
            .expect("the dispatched case count fits in u32")
            .div_ceil(64);
        compute::dispatch_and_wait(
            &ctx.device,
            &ctx.queue,
            &pipeline,
            &bind_group,
            (workgroups, 1, 1),
        );
        let out: Vec<u32> =
            compute::readback(&ctx.device, &ctx.queue, &out_buf, total * CASE_FLOATS);

        let mut compared = 0usize;
        let mut identical = 0usize;
        let mut worst = 0.0_f64;
        let mut skipped_non_finite = 0usize;
        for (idx, words) in out.as_chunks::<CASE_FLOATS>().0.iter().enumerate() {
            // (rolled, unrolled) word ranges: single, two-rhs b1, two-rhs b2.
            let pairs = [(0..8, 8..16), (16..24, 32..40), (24..32, 40..48)];
            for (rolled, unrolled) in pairs {
                let (r, u) = (&words[rolled], &words[unrolled]);
                match (all_finite(r), all_finite(u)) {
                    (true, true) => {
                        compared += 1;
                        if r == u {
                            identical += 1;
                            continue;
                        }
                        let deviation = relative_deviation(r, u);
                        worst = worst.max(deviation);
                        assert!(
                            deviation <= SOLVE4_RELATIVE_TOLERANCE,
                            "case {idx}: unrolled solve4 differs from rolled by {deviation:e} \
                             relative (tolerance {SOLVE4_RELATIVE_TOLERANCE:e}; input {:?})\n  \
                             rolled:   {r:?}\n  unrolled: {u:?}",
                            cases[idx]
                        );
                    }
                    (false, false) => skipped_non_finite += 1,
                    _ => panic!(
                        "case {idx}: finite on one side only (input {:?})",
                        cases[idx]
                    ),
                }
            }
        }
        println!(
            "unrolled_solve4: {compared} comparisons, {identical} bit-identical, largest \
             relative deviation {worst:e}, {skipped_non_finite} skipped (non-finite on both \
             sides), {total} cases"
        );
        assert!(
            skipped_non_finite * 100 <= total * 3,
            "too many non-finite comparisons ({skipped_non_finite} of {})",
            total * 3
        );
    }
}
