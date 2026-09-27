//! Tier 2 check for `pleochroic_channel_alpha` in the biaxial case.

use glam::Vec3;

use crate::{
    optics::{
        birefringence::{BiaxialIndicatrix, pleochroic_channel_alpha},
        polarization::StokesVector,
    },
    renderer::gpu::{
        compute,
        transport_check::{SHADER_SRC, STOKES_SAMPLES, UlpAccumulator, UlpCheckResult},
    },
};

use super::{biaxial_test_directions, biaxial_test_indicatrices};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BiaxialPleochroicCase {
    alpha_o: f32,
    alpha_beta: f32,
    alpha_e: f32,
    _pad0: f32,
    c_axis: [f32; 3],
    _pad1: f32,
    s_axis: [f32; 3],
    _pad2: f32,
    propagation_dir: [f32; 3],
    _pad3: f32,
    eigen_a: [f32; 3],
    _pad4: f32,
    eigen_b: [f32; 3],
    _pad5: f32,
    stokes: [f32; 4],
}

const BIAXIAL_PLEOCHROIC_ULP_BUDGET: u32 = 48;
const BIAXIAL_PLEOCHROIC_ABS_FLOOR: f32 = 1e-5;

fn build_biaxial_pleochroic_cases() -> Vec<BiaxialPleochroicCase> {
    let mut cases = Vec::new();
    let alpha_triples = [
        (0.0f32, 0.0f32, 0.0f32),
        (1.0, 1.0, 1.0),
        (0.5, 2.0, 3.5),
        (3.0, 0.2, 1.7),
    ];
    for (_, n_alpha, n_beta, n_gamma, gamma_axis) in biaxial_test_indicatrices() {
        let ind = BiaxialIndicatrix::from_gamma_axis(n_alpha, n_beta, n_gamma, gamma_axis);
        for prop in biaxial_test_directions(n_alpha, n_beta, n_gamma, gamma_axis) {
            let (eigen_a, eigen_b) = ind.eigen_polarizations(prop);
            let s_axis = if prop.cross(Vec3::Y).length_squared() > 1e-6 {
                prop.cross(Vec3::Y).normalize()
            } else {
                prop.cross(Vec3::X).normalize()
            };
            for &(alpha_o, alpha_beta, alpha_e) in &alpha_triples {
                for s in [STOKES_SAMPLES[0], STOKES_SAMPLES[4]] {
                    cases.push(BiaxialPleochroicCase {
                        alpha_o,
                        alpha_beta,
                        alpha_e,
                        _pad0: 0.0,
                        c_axis: gamma_axis.to_array(),
                        _pad1: 0.0,
                        s_axis: s_axis.to_array(),
                        _pad2: 0.0,
                        propagation_dir: prop.to_array(),
                        _pad3: 0.0,
                        eigen_a: eigen_a.to_array(),
                        _pad4: 0.0,
                        eigen_b: eigen_b.to_array(),
                        _pad5: 0.0,
                        stokes: s,
                    });
                }
            }
        }
    }
    cases
}

#[must_use]
pub fn run_biaxial_pleochroic(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BiaxialPleochroicCase> {
    let cases = build_biaxial_pleochroic_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "biaxial pleochroic in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "biaxial pleochroic out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "biaxial_pleochroic_main",
        SHADER_SRC,
        "biaxial_pleochroic_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "biaxial pleochroic bind group",
        &pipeline,
        &[(44, &in_buf), (45, &out_buf)],
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
        "pleochroic_channel_alpha (biaxial)",
        BIAXIAL_PLEOCHROIC_ULP_BUDGET,
        BIAXIAL_PLEOCHROIC_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let s = StokesVector::new(
            case.stokes[0],
            case.stokes[1],
            case.stokes[2],
            case.stokes[3],
        );
        let cpu = pleochroic_channel_alpha(
            case.alpha_o,
            case.alpha_e,
            Some(case.alpha_beta),
            Vec3::from_array(case.c_axis),
            Vec3::from_array(case.s_axis),
            Vec3::from_array(case.propagation_dir),
            Vec3::from_array(case.eigen_a),
            Vec3::from_array(case.eigen_b),
            &s,
        );
        acc.record(case, "alpha", cpu, gpu_out[idx]);
    }
    acc.finish()
}
