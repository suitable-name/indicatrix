//! `hdr_env_radiance_at` ULP-budget self-test, dispatched by [`run_hdr_env_radiance`].
//!
//! Exercises `shaders/environment.wgsl`'s standalone `hdr_env_radiance_main` kernel --
//! its own duplicate of `spectral_transport.wgsl`'s `hdr_env_radiance_at` (see that
//! kernel's own header comment for why this file keeps an independent copy rather than
//! sharing code across WGSL modules) -- against
//! `renderer::env_map::EnvironmentMap::radiance_at` directly, independent of the
//! megakernel's own Tier 3 image comparisons for a loaded HDR map.

use glam::Vec3;

use crate::renderer::gpu::compute;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult, fibonacci_sphere};

/// One input case for the hdr env check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HdrEnvCase {
    dir: [f32; 3],
    lambda_nm: f32,
}

/// ULP budget for `hdr_env_radiance_at`.
///
/// Chains `acos`/`atan2`/`normalize` (direction -> uv) with a bilinear interpolation (four
/// texel fetches, three `fma`s) and `rgb_to_spectral_radiance`'s neutral-plus-chroma bump
/// sum (two 3x3 RGB-to-coefficient matrices in fused multiply-adds, then six asymmetric
/// Gaussians with one `exp()` each, clamped at zero) -- fewer transcendental evaluations than
/// `sample_studio_environment`'s ring-light loop, but still trig-driven, so this starts
/// from [`super::STUDIO_ENV_ULP_BUDGET`]'s order of magnitude rather than
/// [`super::CMF_ULP_BUDGET`]'s polynomial-only one. See [`super::CMF_ULP_BUDGET`]'s doc
/// comment for the calibration philosophy; kept as a conservative ceiling pending a
/// fresh measurement on live GPU hardware.
pub const HDR_ENV_ULP_BUDGET: u32 = 4096;

/// Absolute-difference floor for `hdr_env_radiance_at` comparisons.
///
/// See [`super::CMF_ABS_FLOOR`]'s doc comment for the rationale -- covers the
/// equirectangular seam (`u` wrapping through `0.0`/`1.0`) and the poles (`v` clamped to
/// `0.0`/`1.0`), where a one-texel rounding-direction disagreement between CPU and GPU
/// changes which neighbour `sample_bilinear` blends toward.
pub const HDR_ENV_ABS_FLOOR: f32 = 1e-4;

/// A small, deliberately non-uniform synthetic HDR map: a smooth RGB gradient across
/// `(u, v)`, so `sample_bilinear`'s interpolation is genuinely exercised (a uniform map,
/// like [`crate::renderer::env_map::EnvironmentMap::uniform`], would return the same
/// value regardless of a wrong texel index or a wrong interpolation weight).
///
/// `pub(crate)`: `estimator_check::spectral_debug::run_spectral_debug` reuses this same
/// map for its own `EnvironmentSource::HdrMap` case rather than building a second
/// synthetic map.
pub fn build_hdr_test_map() -> crate::renderer::env_map::EnvironmentMap {
    let width = 16usize;
    let height = 8usize;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let u = x as f32 / width as f32;
            let v = y as f32 / height as f32;
            pixels.push([
                0.8f32.mul_add(u, 0.2),
                0.6f32.mul_add(v, 0.1),
                0.9f32.mul_add(f32::midpoint(u, v), 0.05),
            ]);
        }
    }
    crate::renderer::env_map::EnvironmentMap::from_rgb(width, height, pixels)
        .expect("width/height/pixels are self-consistent by construction")
}

/// Dense direction/wavelength sweep plus adversarial points: both poles (`v` clamped),
/// and directions straddling the `u == 0.0`/`1.0` equirectangular wrap seam.
#[must_use]
pub fn build_hdr_env_cases() -> Vec<HdrEnvCase> {
    let mut cases = Vec::new();
    let directions = fibonacci_sphere(256);
    let lambdas = [400.0f32, 500.0, 560.0, 650.0, 700.0];
    for &dir in &directions {
        for &lambda_nm in &lambdas {
            cases.push(HdrEnvCase {
                dir: dir.to_array(),
                lambda_nm,
            });
        }
    }
    let adversarial_dirs = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1e-4, 0.0, -1.0).normalize(),
        Vec3::new(-1e-4, 0.0, -1.0).normalize(),
    ];
    for dir in adversarial_dirs {
        for &lambda_nm in &lambdas {
            cases.push(HdrEnvCase {
                dir: dir.to_array(),
                lambda_nm,
            });
        }
    }
    cases
}

/// Runs the `hdr_env_radiance_at` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_hdr_env_radiance(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<HdrEnvCase> {
    let map = build_hdr_test_map();
    let cases = build_hdr_env_cases();
    let total = cases.len();

    let hdr_env = crate::renderer::env_map_gpu::HdrEnvGpuData::upload(&ctx.device, &map);
    let in_buf = compute::upload(
        &ctx.device,
        "hdr_env cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "hdr_env out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "hdr_env_radiance_main",
        SHADER_SRC,
        "hdr_env_radiance_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "hdr_env bind group",
        &pipeline,
        &[
            (8, &in_buf),
            (9, &out_buf),
            (10, &hdr_env.texels),
            (11, &hdr_env.dims),
        ],
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

    let mut acc = UlpAccumulator::new("hdr_env_radiance_at", HDR_ENV_ULP_BUDGET, HDR_ENV_ABS_FLOOR);
    for (idx, case) in cases.iter().enumerate() {
        let cpu = map.radiance_at(Vec3::from_array(case.dir), case.lambda_nm);
        acc.record(case, "radiance", cpu, gpu_out[idx]);
    }
    acc.finish()
}
