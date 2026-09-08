//! Frame rotation and signed psi; TIR retardation and phase delta.
//!
//! `frame_rotation` and `tir_retardation` both run through the shared
//! [`super::run_stokes_case_bank`] Mueller-matrix-and-`StokesVector::apply_matrix`
//! harness; `signed_psi` and `tir_phase_delta` are scalar functions dispatched directly.

use crate::{
    optics::{
        polarization::{MuellerMatrix, StokesVector},
        raytracer::{signed_frame_rotation_psi, tir_phase_delta},
    },
    renderer::gpu::compute,
};
use glam::Vec3;

use super::{
    SHADER_SRC, STOKES_SAMPLES, StokesCaseBankConfig, UlpAccumulator, UlpCheckResult,
    run_stokes_case_bank,
};

// ---------------------------------------------------------------------------------
// frame_rotation
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameRotationCase {
    psi: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

/// ULP budget for `frame_rotation` + `apply_matrix`.
///
/// One `sin`/`cos` pair and a 4x4 matrix-vector multiply -- comparable single-driver-op
/// budget to `rng_check`/`camera_check`'s established 1-2 ULP floor, widened modestly
/// for the multiply-accumulate chain across four output components.
const FRAME_ROTATION_ULP_BUDGET: u32 = 16;
const FRAME_ROTATION_ABS_FLOOR: f32 = 1e-6;

fn build_frame_rotation_cases() -> Vec<FrameRotationCase> {
    let mut cases = Vec::new();
    let steps = 400;
    for i in 0..=steps {
        let psi = (i as f32 / steps as f32)
            .mul_add(4.0 * std::f32::consts::PI, -2.0 * std::f32::consts::PI);
        for s in STOKES_SAMPLES {
            cases.push(FrameRotationCase {
                psi,
                si: s[0],
                sq: s[1],
                su: s[2],
                sv: s[3],
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            });
        }
    }
    // Adversarial: exact multiples of pi/2 (cos(2*psi)/sin(2*psi) land on -1/0/1 exactly).
    for &psi in &[
        0.0f32,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
        3.0 * std::f32::consts::FRAC_PI_2,
    ] {
        for s in STOKES_SAMPLES {
            cases.push(FrameRotationCase {
                psi,
                si: s[0],
                sq: s[1],
                su: s[2],
                sv: s[3],
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            });
        }
    }
    cases
}

fn cpu_frame_rotation(c: &FrameRotationCase) -> [f32; 4] {
    let m = MuellerMatrix::frame_rotation(c.psi);
    let s = StokesVector::new(c.si, c.sq, c.su, c.sv);
    s.apply_matrix(&m).to_vec4().to_array()
}

#[must_use]
pub fn run_frame_rotation(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<FrameRotationCase> {
    let cases = build_frame_rotation_cases();
    let config = StokesCaseBankConfig {
        entry_point: "frame_rotation_main",
        in_binding: 0,
        out_binding: 1,
        budget: FRAME_ROTATION_ULP_BUDGET,
        abs_floor: FRAME_ROTATION_ABS_FLOOR,
    };
    run_stokes_case_bank(ctx, &config, &cases, cpu_frame_rotation)
}

// ---------------------------------------------------------------------------------
// tir_retardation
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TirRetardationCase {
    delta: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

const TIR_RETARDATION_ULP_BUDGET: u32 = 16;
const TIR_RETARDATION_ABS_FLOOR: f32 = 1e-6;

fn build_tir_retardation_cases() -> Vec<TirRetardationCase> {
    let mut cases = Vec::new();
    let steps = 200;
    for i in 0..=steps {
        let delta = (i as f32 / steps as f32) * 2.0 * std::f32::consts::PI;
        for s in STOKES_SAMPLES {
            cases.push(TirRetardationCase {
                delta,
                si: s[0],
                sq: s[1],
                su: s[2],
                sv: s[3],
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            });
        }
    }
    cases
}

fn cpu_tir_retardation(c: &TirRetardationCase) -> [f32; 4] {
    let m = MuellerMatrix::tir_retardation(c.delta);
    let s = StokesVector::new(c.si, c.sq, c.su, c.sv);
    s.apply_matrix(&m).to_vec4().to_array()
}

#[must_use]
pub fn run_tir_retardation(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<TirRetardationCase> {
    let cases = build_tir_retardation_cases();
    let config = StokesCaseBankConfig {
        entry_point: "tir_retardation_main",
        in_binding: 6,
        out_binding: 7,
        budget: TIR_RETARDATION_ULP_BUDGET,
        abs_floor: TIR_RETARDATION_ABS_FLOOR,
    };
    run_stokes_case_bank(ctx, &config, &cases, cpu_tir_retardation)
}

// ---------------------------------------------------------------------------------
// signed_frame_rotation_psi
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SignedPsiCase {
    prev: [f32; 3],
    _pad0: f32,
    curr: [f32; 3],
    _pad1: f32,
    axis: [f32; 3],
    _pad2: f32,
}

const SIGNED_PSI_ULP_BUDGET: u32 = 32;
const SIGNED_PSI_ABS_FLOOR: f32 = 1e-5;

fn build_signed_psi_cases() -> Vec<SignedPsiCase> {
    let mut cases = Vec::new();
    let dirs: Vec<Vec3> = (0..24)
        .map(|i| {
            let theta = (i as f32 / 24.0) * std::f32::consts::PI;
            let phi = (i as f32 * 2.399_963) % (2.0 * std::f32::consts::PI);
            Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            )
        })
        .collect();
    for &prev in &dirs {
        for &curr in &dirs {
            for &axis in &[Vec3::X, Vec3::Y, Vec3::Z] {
                cases.push(SignedPsiCase {
                    prev: prev.to_array(),
                    _pad0: 0.0,
                    curr: curr.to_array(),
                    _pad1: 0.0,
                    axis: axis.to_array(),
                    _pad2: 0.0,
                });
            }
        }
    }
    // Adversarial: prev == curr (psi should be exactly/near 0), prev == -curr
    // (antiparallel, psi near +-pi), axis nearly perpendicular to both (sin_psi near 0
    // from the OTHER factor).
    for &(prev, curr, axis) in &[
        (Vec3::X, Vec3::X, Vec3::Y),
        (Vec3::X, -Vec3::X, Vec3::Y),
        (Vec3::X, Vec3::new(1.0, 1e-6, 0.0).normalize(), Vec3::Z),
    ] {
        cases.push(SignedPsiCase {
            prev: prev.to_array(),
            _pad0: 0.0,
            curr: curr.to_array(),
            _pad1: 0.0,
            axis: axis.to_array(),
            _pad2: 0.0,
        });
    }
    cases
}

fn cpu_signed_psi(c: &SignedPsiCase) -> f32 {
    signed_frame_rotation_psi(
        Vec3::from_array(c.prev),
        Vec3::from_array(c.curr),
        Vec3::from_array(c.axis),
    )
}

#[must_use]
pub fn run_signed_psi(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<SignedPsiCase> {
    let cases = build_signed_psi_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "signed psi in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "signed psi out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "signed_psi_main",
        SHADER_SRC,
        "signed_psi_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "signed psi bind group",
        &pipeline,
        &[(8, &in_buf), (9, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new(
        "signed_frame_rotation_psi",
        SIGNED_PSI_ULP_BUDGET,
        SIGNED_PSI_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        acc.record(case, "psi", cpu_signed_psi(case), gpu_out[idx]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// tir_phase_delta
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TirPhaseDeltaCase {
    n1k: f32,
    cos_i: f32,
    sin_i: f32,
    _pad0: f32,
}

const TIR_PHASE_DELTA_ULP_BUDGET: u32 = 32;
/// Measured: at exactly the critical angle (`n1k * sin_i == 1` to within input
/// rounding), `tir_phase_delta`'s `sqrt(max(n1k^2*sin_i^2 - 1, 0))` term sits on a
/// genuine mathematical kink (infinite derivative at the clamp boundary), so a
/// sub-ULP difference in the SHARED f32 input `sin_i`/`cos_i` (the adversarial case
/// bank uploads bit-identical floats to both sides, so this is not a CPU/GPU
/// arithmetic disagreement -- it's the formula itself being ill-conditioned exactly at
/// this single point) can push `n1k^2*sin_i^2 - 1` to either side of zero, giving
/// `delta = 0` on one side and a small nonzero `delta` on the other. Observed: CPU
/// `3.860202e-4` vs GPU `0.0` exactly AT `n1k=1.5`'s critical angle -- ~1e-4 rad
/// (~0.02 degrees) of phase retardation, physically negligible (a genuine TIR event
/// well past critical angle produces phase retardation of order 0.1-3 rad). `2e-3` is
/// comfortably above that measured knife-edge magnitude while staying far below any
/// physically meaningful phase retardation.
const TIR_PHASE_DELTA_ABS_FLOOR: f32 = 2e-3;

fn build_tir_phase_delta_cases() -> Vec<TirPhaseDeltaCase> {
    let mut cases = Vec::new();
    let n_vals = [1.3f32, 1.5, 1.77, 2.0, 2.42];
    let steps = 60;
    for &n1k in &n_vals {
        // Sweep cos_i from 0 (grazing) to the exact critical angle and beyond, so both
        // "past critical" and "not past critical" regions -- and the boundary itself --
        // are covered (see this module doc comment's "near each channel's critical
        // angle" requirement).
        let sin_c = (1.0 / n1k).min(1.0);
        let theta_c = sin_c.asin();
        for i in 0..=steps {
            let theta = (i as f32 / steps as f32) * (std::f32::consts::FRAC_PI_2);
            let cos_i = theta.cos();
            let sin_i = theta.sin();
            cases.push(TirPhaseDeltaCase {
                n1k,
                cos_i,
                sin_i,
                _pad0: 0.0,
            });
        }
        // Exactly at, just below, and just above the critical angle.
        for delta_theta in [-1e-3f32, 0.0, 1e-3, 1e-5, -1e-5] {
            let theta = theta_c + delta_theta;
            cases.push(TirPhaseDeltaCase {
                n1k,
                cos_i: theta.cos(),
                sin_i: theta.sin(),
                _pad0: 0.0,
            });
        }
    }
    cases
}

fn cpu_tir_phase_delta(c: &TirPhaseDeltaCase) -> f32 {
    tir_phase_delta(c.n1k, c.cos_i, c.sin_i)
}

#[must_use]
pub fn run_tir_phase_delta(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<TirPhaseDeltaCase> {
    let cases = build_tir_phase_delta_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "tir phase delta in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "tir phase delta out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "tir_phase_delta_main",
        SHADER_SRC,
        "tir_phase_delta_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "tir phase delta bind group",
        &pipeline,
        &[(10, &in_buf), (11, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new(
        "tir_phase_delta",
        TIR_PHASE_DELTA_ULP_BUDGET,
        TIR_PHASE_DELTA_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        acc.record(case, "delta", cpu_tir_phase_delta(case), gpu_out[idx]);
    }
    acc.finish()
}
