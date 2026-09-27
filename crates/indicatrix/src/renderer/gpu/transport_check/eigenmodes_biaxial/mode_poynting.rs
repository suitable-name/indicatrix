//! Tier 2 check for `BiaxialIndicatrix::mode_poynting_dir`.

use glam::Vec3;

use crate::{
    optics::birefringence::BiaxialIndicatrix,
    renderer::gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::{biaxial_test_directions, biaxial_test_indicatrices};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialModePoyntingCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    wave_normal: [f32; 3],
    want_slow: u32,
}

const BIAXIAL_MODE_POYNTING_ULP_BUDGET: u32 = 48;
const BIAXIAL_MODE_POYNTING_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_mode_poynting_cases() -> Vec<BiaxialModePoyntingCase> {
    let mut cases = Vec::new();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for wave_normal in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            for want_slow in [false, true] {
                cases.push(BiaxialModePoyntingCase {
                    n_alpha,
                    n_beta,
                    n_gamma,
                    _pad0: 0.0,
                    gamma_axis: gamma_axis.to_array(),
                    _pad1: 0.0,
                    wave_normal: wave_normal.to_array(),
                    want_slow: u32::from(want_slow),
                });
            }
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_mode_poynting(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialModePoyntingCase> {
    let cases = build_biaxial_mode_poynting_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial mode poynting in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial mode poynting out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_mode_poynting_main",
        SHADER_SRC,
        "biaxial_mode_poynting_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial mode poynting bind group",
        &pipeline,
        &[(40, &in_buf), (41, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::mode_poynting_dir",
        BIAXIAL_MODE_POYNTING_ULP_BUDGET,
        BIAXIAL_MODE_POYNTING_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let dir = ind.mode_poynting_dir(Vec3::from_array(case.wave_normal), case.want_slow != 0);
        for (c_idx, comp) in ["x", "y", "z"].iter().enumerate() {
            acc.record(case, comp, dir.to_array()[c_idx], gpu_out[idx * 3 + c_idx]);
        }
    }
    acc.finish()
}
