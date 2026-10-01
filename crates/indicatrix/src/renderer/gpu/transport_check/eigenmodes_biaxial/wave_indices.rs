//! Tier 2 check for `BiaxialIndicatrix::wave_indices`.

use glam::Vec3;

use crate::{
    optics::birefringence::BiaxialIndicatrix,
    renderer::gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::{biaxial_test_directions_index_stable, biaxial_test_indicatrices};

/// One input case for the biaxial wave indices check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialWaveIndicesCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    wave_normal: [f32; 3],
    _pad2: f32,
}

/// Measured: max genuine ULP = 0 (raw ULP <= 29) once `biaxial_test_directions`'s
/// near-optic-axis filter excludes directions where this material's own two modes are
/// within 10% of each other -- see that function's doc comment for why comparing an
/// ill-posed eigenmode assignment near a true optic axis is not a meaningful ULP
/// check. The budget stays modest (not 0) purely as headroom, matching this file's
/// convention elsewhere.
const BIAXIAL_WAVE_INDICES_ULP_BUDGET: u32 = 64;
const BIAXIAL_WAVE_INDICES_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_wave_indices_cases() -> Vec<BiaxialWaveIndicesCase> {
    let mut cases = Vec::new();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for wave_normal in
            biaxial_test_directions_index_stable(n_alpha, n_beta, n_gamma, gamma_axis)
        {
            cases.push(BiaxialWaveIndicesCase {
                n_alpha,
                n_beta,
                n_gamma,
                _pad0: 0.0,
                gamma_axis: gamma_axis.to_array(),
                _pad1: 0.0,
                wave_normal: wave_normal.to_array(),
                _pad2: 0.0,
            });
        }
    }
    cases
}

/// Runs the biaxial wave indices check against the CPU reference.
#[must_use]
pub fn run_biaxial_wave_indices(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialWaveIndicesCase> {
    let cases = build_biaxial_wave_indices_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial wave indices in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial wave indices out",
        total * 2,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_wave_indices_main",
        SHADER_SRC,
        "biaxial_wave_indices_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial wave indices bind group",
        &pipeline,
        &[(36, &in_buf), (37, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 2);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::wave_indices",
        BIAXIAL_WAVE_INDICES_ULP_BUDGET,
        BIAXIAL_WAVE_INDICES_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let (n_slow, n_fast) = ind.wave_indices(Vec3::from_array(case.wave_normal));
        acc.record(case, "n_slow", n_slow, gpu_out[idx * 2]);
        acc.record(case, "n_fast", n_fast, gpu_out[idx * 2 + 1]);
    }
    acc.finish()
}
