//! Tier 2 check for `BiaxialIndicatrix::eigen_polarizations`.

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
pub struct BiaxialEigenPolarizationCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    wave_normal: [f32; 3],
    _pad2: f32,
}

const BIAXIAL_EIGEN_POLARIZATION_ULP_BUDGET: u32 = 48;
/// Absolute floor after the `BiaxialIndicatrix` eigenvector conditioning fix (see
/// `optics::birefringence::BiaxialIndicatrix::eigenvector_world`'s and
/// `precise_root_near`'s doc comments): every remaining comparison sits within
/// `[1.0e-5, 3.5e-5]` absolute difference on a component of modest (0.02-0.9) magnitude
/// -- genuine last-few-ULP f32 cross-platform rounding noise on an eigenvector component
/// that is itself small or near a direction-dependent zero-crossing (worst case:
/// `wave_normal = (0,0,1)`, where CPU computes a component EXACTLY `0.0` by symmetry and
/// the GPU's independently-rounded arithmetic lands a few ULP off zero instead), not a
/// residual algorithmic defect (the dedicated residual/orthonormality tests in
/// `birefringence.rs` and this file's own Tier-3 image comparisons for all three biaxial
/// built-ins pass cleanly). `5e-5` sits ~1.4x above the measured `3.5e-5` worst case:
/// headroom without weakening the check for a genuine bug, which would show up as a
/// large FRACTION of a unit vector's own magnitude, not a few times `1e-5`.
const BIAXIAL_EIGEN_POLARIZATION_ABS_FLOOR: f32 = 5e-5;

fn build_biaxial_eigen_polarization_cases() -> Vec<BiaxialEigenPolarizationCase> {
    let mut cases = Vec::new();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for wave_normal in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            cases.push(BiaxialEigenPolarizationCase {
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

#[must_use]
pub fn run_biaxial_eigen_polarization(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialEigenPolarizationCase> {
    let cases = build_biaxial_eigen_polarization_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial eigen polarization in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial eigen polarization out",
        total * 6,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_eigen_polarization_main",
        SHADER_SRC,
        "biaxial_eigen_polarization_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial eigen polarization bind group",
        &pipeline,
        &[(38, &in_buf), (39, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 6);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::eigen_polarizations",
        BIAXIAL_EIGEN_POLARIZATION_ULP_BUDGET,
        BIAXIAL_EIGEN_POLARIZATION_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let (d_slow, d_fast) = ind.eigen_polarizations(Vec3::from_array(case.wave_normal));
        for (c_idx, comp) in ["slow_x", "slow_y", "slow_z"].iter().enumerate() {
            acc.record(
                case,
                comp,
                d_slow.to_array()[c_idx],
                gpu_out[idx * 6 + c_idx],
            );
        }
        for (c_idx, comp) in ["fast_x", "fast_y", "fast_z"].iter().enumerate() {
            acc.record(
                case,
                comp,
                d_fast.to_array()[c_idx],
                gpu_out[idx * 6 + 3 + c_idx],
            );
        }
    }
    acc.finish()
}
