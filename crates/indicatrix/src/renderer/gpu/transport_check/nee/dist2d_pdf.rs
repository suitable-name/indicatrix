//! Tier 2 check for `EnvironmentMap::pdf`'s piecewise-constant lookup (`dist2d_pdf`).

use glam::Vec3;

use crate::renderer::{
    env_map_gpu::HdrEnvGpuData,
    gpu::{
        compute,
        transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
    },
};

use super::synthetic_test_map;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dist2dPdfCase {
    dir: [f32; 3],
    _pad0: f32,
}

const _: () = assert!(size_of::<Dist2dPdfCase>() == 16);

const DIST2D_PDF_ULP_BUDGET: u32 = 32;
const DIST2D_PDF_ABS_FLOOR: f32 = 1e-5;

/// Every direction here must map to a `(u, v)` strictly INSIDE a texel bucket of the
/// 8x4 [`super::synthetic_test_map`], never onto a bucket edge.
///
/// `EnvironmentMap::pdf` is piecewise constant, so it is discontinuous at every
/// `u * width` / `v * height` integer: a direction sitting exactly on such an edge
/// gets bucket `k` or `k-1` depending on the last ULP of `atan2`/`acos`, and the two
/// neighbouring texels of this map differ by up to 10x in luminance. The CPU's `libm`
/// `atan2(x, x)` rounds to exactly `PI/4` (so `u = 0.125`, `u * 8 == 1.0`, column 1);
/// the GPU's lands one ULP below (column 0) -- `cpu = 5.706e-2`
/// vs `gpu = 5.915e-3`, a 27-million-ULP "divergence" purely from this edge
/// ambiguity (a naive grid puts many cases on an edge: 89 of 154 candidate directions
/// sat on an edge to within `1e-5`). The grid
/// below therefore offsets both angles by an irrational-ish fraction of a step, and
/// the hand-picked directions avoid `phi` multiples of `PI/8` and `theta` multiples of
/// `PI/4` (the two near-pole ones carry a small `x` so `phi` is neither `0` nor `PI`,
/// both of which are column edges); the two poles stay (both sides return exactly
/// `0.0` there).
fn build_dist2d_pdf_cases() -> Vec<Dist2dPdfCase> {
    let mut cases = Vec::new();
    let hand_picked = [
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::new(0.007, 0.9999, 0.01).normalize(),
        Vec3::new(0.007, -0.9999, 0.01).normalize(),
        Vec3::new(0.3, 0.4, 0.5).normalize(),
        Vec3::new(-1.0, 2.0, -0.5).normalize(),
        Vec3::new(0.9, -0.1, -0.2).normalize(),
        Vec3::new(-0.2, -0.7, 0.6).normalize(),
    ];
    for &d in &hand_picked {
        cases.push(Dist2dPdfCase {
            dir: d.to_array(),
            _pad0: 0.0,
        });
    }
    for theta_i in 0..10 {
        let theta = (theta_i as f32 + 0.37) * std::f32::consts::PI / 10.0;
        let (sin_t, cos_t) = theta.sin_cos();
        for phi_i in 0..16 {
            let phi = (phi_i as f32 + 0.29) * 2.0 * std::f32::consts::PI / 16.0;
            let (sin_p, cos_p) = phi.sin_cos();
            let d = Vec3::new(sin_t * sin_p, cos_t, sin_t * cos_p);
            cases.push(Dist2dPdfCase {
                dir: d.to_array(),
                _pad0: 0.0,
            });
        }
    }
    cases
}

/// Runs the Tier 2 check for [`EnvironmentMap::pdf`](crate::renderer::env_map::EnvironmentMap::pdf).
#[must_use]
pub fn run_dist2d_pdf(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<Dist2dPdfCase> {
    let map = synthetic_test_map();
    let gpu_data = HdrEnvGpuData::upload(&ctx.device, &map);
    let cases = build_dist2d_pdf_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "dist2d pdf in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "dist2d pdf out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "dist2d_pdf_main",
        SHADER_SRC,
        "dist2d_pdf_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "dist2d pdf bind group",
        &pipeline,
        &[
            (62, &gpu_data.dist_func),
            (64, &gpu_data.dist_dims),
            (71, &in_buf),
            (72, &out_buf),
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

    let mut acc = UlpAccumulator::new("dist2d_pdf", DIST2D_PDF_ULP_BUDGET, DIST2D_PDF_ABS_FLOOR);
    for (idx, case) in cases.iter().enumerate() {
        let cpu = map.pdf(Vec3::from_array(case.dir));
        acc.record(case, "pdf", cpu, gpu_out[idx]);
    }
    acc.finish()
}
