//! Next-event estimation (NEE), balance-heuristic MIS, and 1D/2D distribution
//! importance sampling ULP equivalence checks.
//!
//! Compares `transport_functions.wgsl`'s GPU implementations against the real CPU
//! functions in `optics::raytracer::scattering` and `renderer::env_map`.

use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        polarization::StokesVector,
        raytracer::{
            EnvironmentSource, balance_heuristic, build_plane_soa,
            scattering::{
                NeeContext, nee_contribution_frosted_exterior, nee_contribution_hg_scatter,
            },
        },
    },
    renderer::{
        env_map::{Distribution1D, Distribution2D, EnvironmentMap},
        env_map_gpu::HdrEnvGpuData,
        gpu::compute,
    },
};

// ---------------------------------------------------------------------------------
// 1. Balance heuristic
// ---------------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------------
// Synthetic environment map for distribution tests
// ---------------------------------------------------------------------------------

fn synthetic_test_map() -> EnvironmentMap {
    let width = 8;
    let height = 4;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let r = (((x * 3 + y * 5) % 7) as f32).mul_add(0.4, 0.1);
            let g = (((x * 2 + y * 3) % 5) as f32).mul_add(0.3, 0.1);
            let b = (((x * 5 + y * 2) % 6) as f32).mul_add(0.5, 0.1);
            pixels.push([r, g, b]);
        }
    }
    EnvironmentMap::from_rgb(width, height, pixels).expect("valid synthetic env map")
}

// ---------------------------------------------------------------------------------
// 2. 1D distribution binary search (`dist1d_find_bucket`)
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dist1dFindBucketCase {
    cdf_start: u32,
    count: u32,
    u: f32,
    _pad0: f32,
}

const _: () = assert!(size_of::<Dist1dFindBucketCase>() == 16);

fn build_dist1d_find_bucket_cases(map: &EnvironmentMap) -> (Vec<Dist1dFindBucketCase>, Vec<usize>) {
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

// ---------------------------------------------------------------------------------
// 3. 2D distribution continuous sampling (`dist2d_sample`)
// ---------------------------------------------------------------------------------

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

/// Runs the Tier 2 check for [`EnvironmentMap::sample`].
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
        acc.record(case, "pdf", cpu_pdf, gpu_out[base + 6]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// 4. 2D distribution pdf lookup (`dist2d_pdf`)
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Dist2dPdfCase {
    dir: [f32; 3],
    _pad0: f32,
}

const _: () = assert!(size_of::<Dist2dPdfCase>() == 16);

const DIST2D_PDF_ULP_BUDGET: u32 = 32;
const DIST2D_PDF_ABS_FLOOR: f32 = 1e-5;

fn build_dist2d_pdf_cases() -> Vec<Dist2dPdfCase> {
    let mut cases = Vec::new();
    let cardinals = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
        Vec3::new(0.0, 0.9999, 0.01).normalize(),
        Vec3::new(0.0, -0.9999, 0.01).normalize(),
        Vec3::new(1.0, 1.0, 1.0).normalize(),
        Vec3::new(-1.0, 2.0, -0.5).normalize(),
    ];
    for &d in &cardinals {
        cases.push(Dist2dPdfCase {
            dir: d.to_array(),
            _pad0: 0.0,
        });
    }
    for theta_i in 1..10 {
        let theta = (theta_i as f32) * std::f32::consts::PI / 10.0;
        let (sin_t, cos_t) = theta.sin_cos();
        for phi_i in 0..16 {
            let phi = (phi_i as f32) * 2.0 * std::f32::consts::PI / 16.0;
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

/// Runs the Tier 2 check for [`EnvironmentMap::pdf`].
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

// ---------------------------------------------------------------------------------
// 5. NEE contribution: frosted exterior
// ---------------------------------------------------------------------------------

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

/// Runs the Tier 2 check for [`nee_contribution_frosted_exterior`].
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

// ---------------------------------------------------------------------------------
// 6. NEE contribution: Henyey-Greenstein scattering
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NeeHgScatterCase {
    scatter_point: [f32; 3],
    n_inside_hero: f32,
    scatter_dir_in: [f32; 3],
    g: f32,
    rng_seed: u32,
    bounce: u32,
    _pad0: f32,
    _pad1: f32,
    lambdas: [f32; 8],
    stokes: [[f32; 4]; 8],
    radiance_in: [f32; 8],
}

const _: () = assert!(size_of::<NeeHgScatterCase>() == 240);

const NEE_HG_ULP_BUDGET: u32 = 64;
const NEE_HG_ABS_FLOOR: f32 = 1e-4;

fn build_nee_hg_cases() -> Vec<NeeHgScatterCase> {
    let mut cases = Vec::new();
    let points = [
        Vec3::ZERO,
        Vec3::new(0.2, -0.3, 0.1),
        Vec3::new(-0.4, 0.2, 0.3),
    ];
    let dirs_in = [
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.7, -0.1, 0.5).normalize(),
    ];
    let gs = [-0.6f32, 0.0, 0.4, 0.8];
    let n_heroes = [1.33f32, 1.5, 1.77];
    let seeds = [42u32, 9999, 123_456];
    let bounces = [0u32, 1, 3];
    let lambdas: [f32; 8] = std::array::from_fn(|k| (k as f32).mul_add(45.0, 400.0));
    let stokes: [[f32; 4]; 8] = std::array::from_fn(|k| {
        [
            (k as f32).mul_add(0.1, 1.0),
            (k as f32) * 0.05,
            -(k as f32) * 0.03,
            0.01,
        ]
    });
    let radiance_in: [f32; 8] = std::array::from_fn(|k| 0.05 * (k as f32));

    for &p in &points {
        for &d in &dirs_in {
            for &g in &gs {
                for &n_hero in &n_heroes {
                    for &s in &seeds {
                        for &b in &bounces {
                            cases.push(NeeHgScatterCase {
                                scatter_point: p.to_array(),
                                n_inside_hero: n_hero,
                                scatter_dir_in: d.to_array(),
                                g,
                                rng_seed: s,
                                bounce: b,
                                _pad0: 0.0,
                                _pad1: 0.0,
                                lambdas,
                                stokes,
                                radiance_in,
                            });
                        }
                    }
                }
            }
        }
    }
    cases
}

/// Runs the Tier 2 check for [`nee_contribution_hg_scatter`].
#[must_use]
pub fn run_nee_hg_scatter(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<NeeHgScatterCase> {
    let map = synthetic_test_map();
    let gpu_data = HdrEnvGpuData::upload(&ctx.device, &map);
    let cases = build_nee_hg_cases();
    let total = cases.len();

    let cube_planes = [
        GpuFacetPlane::new(Vec3::X, -1.0),
        GpuFacetPlane::new(Vec3::NEG_X, -1.0),
        GpuFacetPlane::new(Vec3::Y, -1.0),
        GpuFacetPlane::new(Vec3::NEG_Y, -1.0),
        GpuFacetPlane::new(Vec3::Z, -1.0),
        GpuFacetPlane::new(Vec3::NEG_Z, -1.0),
    ];
    let planes_buf = compute::upload(
        &ctx.device,
        "tf_planes",
        &cube_planes,
        wgpu::BufferUsages::STORAGE,
    );

    let in_buf = compute::upload(
        &ctx.device,
        "nee hg in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "nee hg out",
        total * 8,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "nee_hg_scatter_main",
        SHADER_SRC,
        "nee_hg_scatter_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "nee hg bind group",
        &pipeline,
        &[
            (62, &gpu_data.dist_func),
            (63, &gpu_data.dist_cdf),
            (64, &gpu_data.dist_dims),
            (65, &gpu_data.texels),
            (66, &gpu_data.dims),
            (75, &planes_buf),
            (76, &in_buf),
            (77, &out_buf),
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

    let plane_soa = build_plane_soa(&cube_planes);
    let nee_ctx = NeeContext {
        environment: EnvironmentSource::HdrMap(&map),
        plane_soa: &plane_soa,
        enabled: true,
    };

    let mut acc = UlpAccumulator::new("nee_hg_scatter", NEE_HG_ULP_BUDGET, NEE_HG_ABS_FLOOR);
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
        nee_contribution_hg_scatter(
            nee_ctx,
            &case.lambdas,
            case.n_inside_hero,
            Vec3::from_array(case.scatter_point),
            Vec3::from_array(case.scatter_dir_in),
            case.g,
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
