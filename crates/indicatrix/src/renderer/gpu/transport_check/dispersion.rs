//! `DispersionModel::evaluate`.

use crate::{optics::dispersion::DispersionModel, renderer::gpu::compute};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DispersionCase {
    model_type: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    param_a: [f32; 4],
    param_b: [f32; 4],
    lambda_nm: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

const DISPERSION_ULP_BUDGET: u32 = 24;
const DISPERSION_ABS_FLOOR: f32 = 1e-6;

fn build_dispersion_cases() -> Vec<(DispersionCase, DispersionModel)> {
    let models = [
        // Diamond's real Sellmeier3 fit.
        DispersionModel::Sellmeier3 {
            b: [4.3356, 0.3306, 0.0],
            c: [0.011_236, 0.030_625, 1.0],
        },
        // Cubic Zirconia's real Sellmeier3 fit.
        DispersionModel::Sellmeier3 {
            b: [1.347_091, 2.117_788, 9.452_943],
            c: [0.003_912, 0.027_802, 591.489],
        },
        // A synthetic Sellmeier1 (no built-in material uses this variant).
        DispersionModel::Sellmeier1 { b1: 1.0, c1: 0.01 },
        // A synthetic Cauchy.
        DispersionModel::Cauchy {
            a: 1.5,
            b: 0.004,
            c: 0.0001,
        },
    ];
    let mut lambdas: Vec<f32> = Vec::new();
    let steps = 100;
    for i in 0..=steps {
        lambdas.push((i as f32 / steps as f32).mul_add(400.0, 380.0));
    }
    lambdas.push(380.0);
    lambdas.push(780.0);

    let mut out = Vec::new();
    for model in models {
        let (model_type, param_a, param_b) = match model {
            DispersionModel::Sellmeier1 { b1, c1 } => {
                (0u32, [b1, 0.0, 0.0, 0.0], [c1, 0.0, 0.0, 0.0])
            }
            DispersionModel::Sellmeier3 { b, c } => {
                (1u32, [b[0], b[1], b[2], 0.0], [c[0], c[1], c[2], 0.0])
            }
            DispersionModel::Cauchy { a, b, c } => (2u32, [a, b, c, 0.0], [0.0; 4]),
        };
        for &lambda_nm in &lambdas {
            out.push((
                DispersionCase {
                    model_type,
                    _pad0: 0,
                    _pad1: 0,
                    _pad2: 0,
                    param_a,
                    param_b,
                    lambda_nm,
                    _pad3: 0.0,
                    _pad4: 0.0,
                    _pad5: 0.0,
                },
                model,
            ));
        }
    }
    out
}

#[must_use]
pub fn run_dispersion(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<DispersionCase> {
    let with_models = build_dispersion_cases();
    let cases: Vec<DispersionCase> = with_models.iter().map(|(c, _)| *c).collect();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "dispersion in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "dispersion out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "dispersion_main",
        SHADER_SRC,
        "dispersion_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "dispersion bind group",
        &pipeline,
        &[(12, &in_buf), (13, &out_buf)],
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
        "DispersionModel::evaluate",
        DISPERSION_ULP_BUDGET,
        DISPERSION_ABS_FLOOR,
    );
    for (idx, (case, model)) in with_models.iter().enumerate() {
        let cpu = model.evaluate(case.lambda_nm);
        acc.record(case, "n", cpu, gpu_out[idx]);
    }
    acc.finish()
}
