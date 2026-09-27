//! `compute_illuminant_white_balance` self-test, dispatched by [`run_white_balance`] for
//! all four `LightingPreset` variants.

use crate::{
    optics::raytracer::{LightingPreset, compute_illuminant_white_balance},
    renderer::gpu::compute,
};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct WhiteBalanceCase {
    temp_k: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

/// ULP budget for `compute_illuminant_white_balance`.
///
/// Wider than the per-lambda [`super::CMF_ULP_BUDGET`]/[`super::BLACKBODY_ULP_BUDGET`]
/// budgets since this is those two functions' PRODUCT, summed over 401 wavelength steps
/// -- 401 independent roundings' worth of accumulated driver-level floating-point
/// noise, not just one. The Bradford-space rework adds two more 3x3 matrix-vector
/// products (source white and target white) and a per-component division on top of that
/// same 401-step sum; those contribute only a handful of ULP each, well inside the
/// existing budget, so it is left unchanged rather than widened.
pub const WHITE_BALANCE_ULP_BUDGET: u32 = 4096;

/// Absolute-difference floor for `compute_illuminant_white_balance` comparisons.
///
/// See [`super::CMF_ABS_FLOOR`]'s doc comment for the rationale. Since the diagonal
/// scale this function returns is now computed in Bradford LMS space rather than raw
/// XYZ, none of the three components is pinned to exactly `1.0` by construction any
/// more (the old XYZ-space `y` component was, since Y was left unscaled) -- all three
/// are `LMS_target / LMS_source`-style ratios, and can range more widely than the old
/// `x`/`z` (measured: roughly `0.8`-`2.6` across this crate's four lighting presets,
/// widest for the 3200K incandescent preset's blue channel). A floor several orders of
/// magnitude below that range still cannot mask a real divergence.
pub const WHITE_BALANCE_ABS_FLOOR: f32 = 1e-5;

/// Runs the `compute_illuminant_white_balance` self-test against a live GPU, for all
/// four [`LightingPreset`] variants.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_white_balance(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<LightingPreset> {
    let presets = LightingPreset::ALL;
    let cases: Vec<WhiteBalanceCase> = presets
        .iter()
        .map(|p| WhiteBalanceCase {
            temp_k: p.params().temp_k,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        })
        .collect();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "white_balance cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "white_balance out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "white_balance_main",
        SHADER_SRC,
        "white_balance_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "white_balance bind group",
        &pipeline,
        &[(6, &in_buf), (7, &out_buf)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new(
        "compute_illuminant_white_balance",
        WHITE_BALANCE_ULP_BUDGET,
        WHITE_BALANCE_ABS_FLOOR,
    );
    for (idx, &preset) in presets.iter().enumerate() {
        let cpu = compute_illuminant_white_balance(preset.params().temp_k);
        acc.record(&preset, "x", cpu.x, gpu_out[idx * 3]);
        acc.record(&preset, "y", cpu.y, gpu_out[idx * 3 + 1]);
        acc.record(&preset, "z", cpu.z, gpu_out[idx * 3 + 2]);
    }
    acc.finish()
}
