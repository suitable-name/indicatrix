//! Tier 2 ULP check for `balance_heuristic`.

use crate::{
    optics::raytracer::balance_heuristic,
    renderer::gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BalanceHeuristicCase {
    pdf_a: f32,
    pdf_b: f32,
    _pad0: f32,
    _pad1: f32,
}

const _: () = assert!(size_of::<BalanceHeuristicCase>() == 16);

const BALANCE_HEURISTIC_ULP_BUDGET: u32 = 4;
const BALANCE_HEURISTIC_ABS_FLOOR: f32 = 1e-7;

fn build_balance_heuristic_cases() -> Vec<BalanceHeuristicCase> {
    let mut cases = Vec::new();
    let pdfs = [
        0.0f32, 1e-6, 1e-4, 0.01, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 100.0, 1e4,
    ];
    for &a in &pdfs {
        for &b in &pdfs {
            cases.push(BalanceHeuristicCase {
                pdf_a: a,
                pdf_b: b,
                _pad0: 0.0,
                _pad1: 0.0,
            });
        }
    }
    let adversarial = [
        (-1.0f32, 1.0f32),
        (1.0, -1.0),
        (-0.5, -0.5),
        (0.0, 0.0),
        (1.0, 0.0),
        (0.0, 1.0),
        (1e-7, 1e-7),
        (1e7, 1e7),
        (1e-5, 1e5),
    ];
    for &(a, b) in &adversarial {
        cases.push(BalanceHeuristicCase {
            pdf_a: a,
            pdf_b: b,
            _pad0: 0.0,
            _pad1: 0.0,
        });
    }
    cases
}

/// Runs the Tier 2 ULP check for [`balance_heuristic`].
#[must_use]
pub fn run_balance_heuristic(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<BalanceHeuristicCase> {
    let cases = build_balance_heuristic_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "balance heuristic in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "balance heuristic out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "balance_heuristic_main",
        SHADER_SRC,
        "balance_heuristic_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "balance heuristic bind group",
        &pipeline,
        &[(60, &in_buf), (61, &out_buf)],
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
        "balance_heuristic",
        BALANCE_HEURISTIC_ULP_BUDGET,
        BALANCE_HEURISTIC_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = balance_heuristic(case.pdf_a, case.pdf_b);
        acc.record(case, "weight", cpu, gpu_out[idx]);
    }
    acc.finish()
}
