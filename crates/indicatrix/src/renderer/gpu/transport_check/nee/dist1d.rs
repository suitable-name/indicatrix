//! Tier 2 check for `Distribution1D::find_bucket`'s binary search
//! (`dist1d_find_bucket`).

use crate::renderer::{
    env_map::{Distribution1D, Distribution2D},
    env_map_gpu::HdrEnvGpuData,
    gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::synthetic_test_map;

/// One input case for the dist1d find bucket check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dist1dFindBucketCase {
    cdf_start: u32,
    count: u32,
    u: f32,
    _pad0: f32,
}

const _: () = assert!(size_of::<Dist1dFindBucketCase>() == 16);

fn build_dist1d_find_bucket_cases(
    map: &crate::renderer::env_map::EnvironmentMap,
) -> (Vec<Dist1dFindBucketCase>, Vec<usize>) {
    let mut cases = Vec::new();
    let mut cpu_expected = Vec::new();
    let width = map.width() as u32;
    let height = map.height() as u32;
    let dist: &Distribution2D = map.distribution();

    let u_vals = [
        0.0f32, 0.001, 0.05, 0.12, 0.25, 0.38, 0.5, 0.63, 0.77, 0.89, 0.95, 0.999, 1.0,
    ];

    // Conditional distributions (rows)
    for (row_idx, row_dist) in dist.conditional().iter().enumerate() {
        let row_dist: &Distribution1D = row_dist;
        let cdf_start = (row_idx as u32) * (width + 1);
        for &u in &u_vals {
            cases.push(Dist1dFindBucketCase {
                cdf_start,
                count: width,
                u,
                _pad0: 0.0,
            });
            cpu_expected.push(row_dist.find_bucket(u));
        }
    }

    // Marginal distribution
    let marginal = dist.marginal();
    let marginal_cdf_start = height * (width + 1);
    for &u in &u_vals {
        cases.push(Dist1dFindBucketCase {
            cdf_start: marginal_cdf_start,
            count: height,
            u,
            _pad0: 0.0,
        });
        cpu_expected.push(marginal.find_bucket(u));
    }

    (cases, cpu_expected)
}

/// Runs the Tier 2 check for [`Distribution1D::find_bucket`].
#[must_use]
pub fn run_dist1d_find_bucket(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<Dist1dFindBucketCase> {
    let map = synthetic_test_map();
    let gpu_data = HdrEnvGpuData::upload(&ctx.device, &map);
    let (cases, cpu_expected) = build_dist1d_find_bucket_cases(&map);
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "dist1d find bucket in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<u32>(
        &ctx.device,
        "dist1d find bucket out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "dist1d_find_bucket_main",
        SHADER_SRC,
        "dist1d_find_bucket_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "dist1d find bucket bind group",
        &pipeline,
        &[(63, &gpu_data.dist_cdf), (67, &in_buf), (68, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<u32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new("dist1d_find_bucket", 0, 0.0);
    for (idx, case) in cases.iter().enumerate() {
        acc.record(
            case,
            "bucket",
            cpu_expected[idx] as f32,
            gpu_out[idx] as f32,
        );
    }
    acc.finish()
}
