//! Tier 2 check for `nee_contribution_frosted_exterior`.

use glam::Vec3;

use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        polarization::StokesVector,
        raytracer::{
            EnvironmentSource, build_plane_soa,
            scattering::{NeeContext, nee_contribution_frosted_exterior},
        },
    },
    renderer::{
        env_map_gpu::HdrEnvGpuData,
        gpu::{
            compute,
            transport_check::{SHADER_SRC, UlpAccumulator, UlpCheckResult},
        },
    },
};

use super::synthetic_test_map;

/// One input case for the nee frosted exterior check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NeeFrostedExteriorCase {
    ext_normal: [f32; 3],
    rng_seed: u32,
    bounce: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    lambdas: [f32; 8],
    stokes: [[f32; 4]; 8],
    radiance_in: [f32; 8],
}

const _: () = assert!(size_of::<NeeFrostedExteriorCase>() == 224);

const NEE_FROSTED_ULP_BUDGET: u32 = 64;
const NEE_FROSTED_ABS_FLOOR: f32 = 1e-4;

fn build_nee_frosted_cases() -> Vec<NeeFrostedExteriorCase> {
    let mut cases = Vec::new();
    let normals = [
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::X,
        Vec3::Z,
        Vec3::new(0.5, 0.8, 0.3).normalize(),
        Vec3::new(-0.7, 0.2, -0.6).normalize(),
    ];
    let seeds = [12_345u32, 98_765, 314_159, 271_828];
    let bounces = [0u32, 1, 2, 4];
    let lambdas: [f32; 8] = std::array::from_fn(|k| (k as f32).mul_add(45.0, 400.0));
    let stokes: [[f32; 4]; 8] = std::array::from_fn(|k| {
        [
            (k as f32).mul_add(0.1, 1.0),
            (k as f32) * 0.05,
            -(k as f32) * 0.03,
            0.01,
        ]
    });
    let radiance_in: [f32; 8] = std::array::from_fn(|k| 0.1 * (k as f32));

    for &n in &normals {
        for &s in &seeds {
            for &b in &bounces {
                cases.push(NeeFrostedExteriorCase {
                    ext_normal: n.to_array(),
                    rng_seed: s,
                    bounce: b,
                    _pad0: 0.0,
                    _pad1: 0.0,
                    _pad2: 0.0,
                    lambdas,
                    stokes,
                    radiance_in,
                });
            }
        }
    }
    cases
}

/// Runs the Tier 2 check for [`nee_contribution_frosted_exterior`](crate::optics::raytracer::scattering::nee_contribution_frosted_exterior).
#[must_use]
pub fn run_nee_frosted_exterior(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<NeeFrostedExteriorCase> {
    let map = synthetic_test_map();
    let gpu_data = HdrEnvGpuData::upload(&ctx.device, &map);
    let cases = build_nee_frosted_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "nee frosted in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "nee frosted out",
        total * 8,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "nee_frosted_exterior_main",
        SHADER_SRC,
        "nee_frosted_exterior_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "nee frosted bind group",
        &pipeline,
        &[
            (62, &gpu_data.dist_func),
            (63, &gpu_data.dist_cdf),
            (64, &gpu_data.dist_dims),
            (65, &gpu_data.texels),
            (66, &gpu_data.dims),
            (73, &in_buf),
            (74, &out_buf),
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 8);

    let empty_planes: [GpuFacetPlane; 0] = [];
    let empty_soa = build_plane_soa(&empty_planes);
    let nee_ctx = NeeContext {
        environment: EnvironmentSource::HdrMap(&map),
        plane_soa: &empty_soa,
        tools: &[],
        enabled: true,
    };

    let mut acc = UlpAccumulator::new(
        "nee_frosted_exterior",
        NEE_FROSTED_ULP_BUDGET,
        NEE_FROSTED_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let mut cpu_rad = case.radiance_in;
        let stokes_cpu: [StokesVector; 8] = std::array::from_fn(|k| {
            StokesVector::new(
                case.stokes[k][0],
                case.stokes[k][1],
                case.stokes[k][2],
                case.stokes[k][3],
            )
        });
        nee_contribution_frosted_exterior(
            nee_ctx,
            Vec3::ZERO,
            &case.lambdas,
            Vec3::from_array(case.ext_normal),
            case.rng_seed,
            case.bounce,
            &stokes_cpu,
            &mut cpu_rad,
        );
        let base = idx * 8;
        for k in 0..8 {
            acc.record(case, "rad", cpu_rad[k], gpu_out[base + k]);
        }
    }
    acc.finish()
}
