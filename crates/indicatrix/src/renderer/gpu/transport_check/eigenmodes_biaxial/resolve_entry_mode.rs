//! Tier 2 check for `BiaxialIndicatrix::resolve_entry_mode`.

use glam::Vec3;

use crate::{
    optics::birefringence::BiaxialIndicatrix,
    renderer::gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::biaxial_test_indicatrices;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialResolveEntryModeCase {
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    _pad0: f32,
    gamma_axis: [f32; 3],
    _pad1: f32,
    incident_dir: [f32; 3],
    _pad2: f32,
    normal: [f32; 3],
    _pad3: f32,
    cos_i: f32,
    n_seed: f32,
    want_slow: u32,
    _pad4: f32,
}

/// Measured: max genuine ULP ~3035 for Alexandrite specifically (`n_alpha=1.740778`,
/// `n_beta=1.742729` -- a real cited-data gap of 0.00195, ABOVE
/// `BiaxialIndicatrix::indices_are_degenerate`'s own `sqrt(f32::EPSILON) ~= 3.45e-4`
/// relative threshold, so the general quadratic solve is legitimately used rather than
/// the closed-form uniaxial shortcut, per that function's own doc comment -- but still
/// close enough that its own documented `sqrt(f32::EPSILON)` relative-error bound
/// applies almost exactly: 3035 ULP at n~1.74 is ~3.6e-4 absolute, i.e. ~2.1e-4
/// relative, the same order the CPU source itself derives and accepts for this regime.
/// Unlike the near-optic-axis case `biaxial_test_directions` filters out, this is not
/// filtered here: it is a genuine material property Alexandrite's real cited index
/// data has at EVERY air->crystal entry (not a rare direction), and Tier 3's
/// statistical image comparison on real Alexandrite renders already confirms this
/// level of per-bounce numerical noise does not observably bias a real render.
const BIAXIAL_RESOLVE_ENTRY_MODE_ULP_BUDGET: u32 = 4096;
const BIAXIAL_RESOLVE_ENTRY_MODE_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_resolve_entry_mode_cases() -> Vec<BiaxialResolveEntryModeCase> {
    let mut cases = Vec::new();
    // Representative (incident_dir, normal) pairs spanning a range of incidence
    // angles, mirroring the geometric setup `theta_c_for_bounce`'s own iteration is
    // exercised against.
    let incidences: Vec<(Vec3, Vec3, f32)> = (1..10)
        .map(|i| {
            let cos_i = i as f32 / 10.0;
            let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();
            let normal = Vec3::Y;
            let incident_dir = Vec3::new(sin_i, -cos_i, 0.0).normalize();
            (incident_dir, normal, cos_i)
        })
        .collect();
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        for &(incident_dir, normal, cos_i) in &incidences {
            for want_slow in [false, true] {
                cases.push(BiaxialResolveEntryModeCase {
                    n_alpha,
                    n_beta,
                    n_gamma,
                    _pad0: 0.0,
                    gamma_axis: gamma_axis.to_array(),
                    _pad1: 0.0,
                    incident_dir: incident_dir.to_array(),
                    _pad2: 0.0,
                    normal: normal.to_array(),
                    _pad3: 0.0,
                    cos_i,
                    n_seed: n_beta,
                    want_slow: u32::from(want_slow),
                    _pad4: 0.0,
                });
            }
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_resolve_entry_mode(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialResolveEntryModeCase> {
    let cases = build_biaxial_resolve_entry_mode_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial resolve entry mode in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial resolve entry mode out",
        total * 4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_resolve_entry_mode_main",
        SHADER_SRC,
        "biaxial_resolve_entry_mode_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial resolve entry mode bind group",
        &pipeline,
        &[(42, &in_buf), (43, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 4);

    let mut acc = UlpAccumulator::new(
        "BiaxialIndicatrix::resolve_entry_mode",
        BIAXIAL_RESOLVE_ENTRY_MODE_ULP_BUDGET,
        BIAXIAL_RESOLVE_ENTRY_MODE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let ind = BiaxialIndicatrix::from_gamma_axis(
            case.n_alpha,
            case.n_beta,
            case.n_gamma,
            Vec3::from_array(case.gamma_axis),
        );
        let (n, wave_dir) = ind.resolve_entry_mode(
            Vec3::from_array(case.incident_dir),
            Vec3::from_array(case.normal),
            case.cos_i,
            case.n_seed,
            case.want_slow != 0,
        );
        acc.record(case, "n", n, gpu_out[idx * 4]);
        for (c_idx, comp) in ["wave_dir_x", "wave_dir_y", "wave_dir_z"]
            .iter()
            .enumerate()
        {
            acc.record(
                case,
                comp,
                wave_dir.to_array()[c_idx],
                gpu_out[idx * 4 + 1 + c_idx],
            );
        }
    }
    acc.finish()
}
