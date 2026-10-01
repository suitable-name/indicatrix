//! Tier 2 check for `EnvironmentMap::sample`'s continuous 2D importance sampling
//! (`dist2d_sample`).

use glam::Vec3;

use crate::renderer::{
    env_map_gpu::HdrEnvGpuData,
    gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::synthetic_test_map;

/// One input case for the dist2d sample check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dist2dSampleCase {
    u0: f32,
    u1: f32,
    _pad0: f32,
    _pad1: f32,
}

const _: () = assert!(size_of::<Dist2dSampleCase>() == 16);

const DIST2D_SAMPLE_ULP_BUDGET: u32 = 48;
const DIST2D_SAMPLE_ABS_FLOOR: f32 = 1e-5;

fn build_dist2d_sample_cases() -> Vec<Dist2dSampleCase> {
    let mut cases = Vec::new();
    let steps = 14;
    for i in 0..=steps {
        let u0 = (i as f32 / steps as f32).mul_add(0.998, 0.001);
        for j in 0..=steps {
            let u1 = (j as f32 / steps as f32).mul_add(0.998, 0.001);
            cases.push(Dist2dSampleCase {
                u0,
                u1,
                _pad0: 0.0,
                _pad1: 0.0,
            });
        }
    }
    let adversarial = [
        (0.0f32, 0.0f32),
        (0.0, 0.5),
        (0.5, 0.0),
        (0.999_999, 0.999_999),
        (1e-6, 1e-6),
        (0.5, 0.999_999),
        (0.999_999, 0.5),
        (0.25, 0.75),
        (0.75, 0.25),
    ];
    for &(u0, u1) in &adversarial {
        cases.push(Dist2dSampleCase {
            u0,
            u1,
            _pad0: 0.0,
            _pad1: 0.0,
        });
    }
    cases
}

/// `dir.y` below which [`run_dist2d_sample`] compares `pdf * sin(theta)` (each side's
/// own `sin(theta)`, read off its own returned direction) instead of the raw pdf.
///
/// The solid-angle pdf is `pdf_uv / (2 PI^2 sin(theta))` with `theta = v * PI`, and
/// `v` is an `f32` in `[0, 1)`: next to the SOUTH pole (`v -> 1`) its spacing is `6e-8`,
/// so one rounding difference in `v` -- e.g. the GPU's division in
/// `dist1d_sample_continuous` rounding the other way than the CPU's -- moves the
/// reported pdf by `6e-8 / (1 - v)` relative. At `u1 = 0.999999` that is `1 - v =
/// 1.6e-6`, a 3.7 % swing (559043 ULP) from a single ULP of
/// `v`, on a direction whose own components are already exempted as near-zero. Both
/// sides are self-consistent (each pdf IS the density of its own direction), so the
/// ULP comparison is meaningless there; `pdf * sin(theta)` cancels the `1/sin(theta)`
/// amplification and compares the well-conditioned `pdf_uv / (2 PI^2)` instead. The
/// budget-48 threshold: `6e-8 / (1 - v) < 48 * 6e-8` needs `1 - v > 0.02`, i.e.
/// `theta < 176.4 degrees`, `y > -cos(0.02 PI) = -0.998`. The north pole needs no such
/// treatment: `f32` is dense near `v = 0`.
const SOUTH_POLE_CONDITIONING_Y: f32 = -0.998;

/// `sin(theta)` of a unit direction, `|(x, z)|` -- the same expression
/// `EnvironmentMap::pdf` uses.
fn sin_theta_of(dir: Vec3) -> f32 {
    dir.x.hypot(dir.z)
}

/// Runs the Tier 2 check for [`EnvironmentMap::sample`](crate::renderer::env_map::EnvironmentMap::sample).
#[must_use]
pub fn run_dist2d_sample(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<Dist2dSampleCase> {
    let map = synthetic_test_map();
    let gpu_data = HdrEnvGpuData::upload(&ctx.device, &map);
    let cases = build_dist2d_sample_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "dist2d sample in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "dist2d sample out",
        total * 7,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "dist2d_sample_main",
        SHADER_SRC,
        "dist2d_sample_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "dist2d sample bind group",
        &pipeline,
        &[
            (62, &gpu_data.dist_func),
            (63, &gpu_data.dist_cdf),
            (64, &gpu_data.dist_dims),
            (65, &gpu_data.texels),
            (66, &gpu_data.dims),
            (69, &in_buf),
            (70, &out_buf),
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 7);

    let mut acc = UlpAccumulator::new(
        "dist2d_sample",
        DIST2D_SAMPLE_ULP_BUDGET,
        DIST2D_SAMPLE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let (cpu_dir, cpu_rgb, cpu_pdf) = map.sample(case.u0, case.u1);
        let base = idx * 7;
        for (c_idx, comp) in ["dir_x", "dir_y", "dir_z"].iter().enumerate() {
            acc.record(case, comp, cpu_dir[c_idx], gpu_out[base + c_idx]);
        }
        for (c_idx, comp) in ["rgb_r", "rgb_g", "rgb_b"].iter().enumerate() {
            acc.record(case, comp, cpu_rgb[c_idx], gpu_out[base + 3 + c_idx]);
        }
        let gpu_pdf = gpu_out[base + 6];
        if cpu_dir.y < SOUTH_POLE_CONDITIONING_Y {
            let gpu_dir = Vec3::new(gpu_out[base], gpu_out[base + 1], gpu_out[base + 2]);
            acc.record(
                case,
                "pdf*sin_theta (south pole, see SOUTH_POLE_CONDITIONING_Y)",
                cpu_pdf * sin_theta_of(cpu_dir),
                gpu_pdf * sin_theta_of(gpu_dir),
            );
        } else {
            acc.record(case, "pdf", cpu_pdf, gpu_pdf);
        }
    }
    acc.finish()
}
