//! Tier 2 check for `nee_contribution_hg_scatter`: the comparison is on the XYZ the
//! scattering event folds into `nee_xyz`, i.e. the per-channel deposit integrated with the
//! spectral-MIS family weights of the `path_pdf`/`compat` in force at that event.

use glam::Vec3;

use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        polarization::StokesVector,
        raytracer::{
            EnvironmentSource, FacetFinish, build_plane_soa, integrate_channels_to_xyz_families,
            scattering::{NeeContext, nee_contribution_hg_scatter},
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

/// One input case for the nee hg scatter check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NeeHgScatterCase {
    scatter_point: [f32; 3],
    n_inside_hero: f32,
    scatter_dir_in: [f32; 3],
    g: f32,
    rng_seed: u32,
    bounce: u32,
    /// `optics::materials::GemMaterial::scattering_sigma_s`, the SAME
    /// quantity `maybe_scatter_or_extinguish`'s survive branch uses.
    sigma_s: f32,
    /// `optics::materials::GemMaterial::absorption_path_scale`.
    absorption_path_scale: f32,
    /// `1` marks every one of the cube's six exit facets
    /// [`crate::optics::raytracer::FacetFinish::Frosted`] (see [`build_nee_hg_cases`]) so
    /// `nee_contribution_hg_scatter`'s frosted-exit skip fires regardless of which facet
    /// the sampled shadow ray actually hits; `0` leaves every facet
    /// [`crate::optics::raytracer::FacetFinish::Polished`] (the CPU function's default).
    frosted_exit: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    /// `optics::absorption::channel_absorption_alphas_assigned`'s per-channel
    /// output, the same array `maybe_scatter_or_extinguish` and `nee_contribution_hg_scatter`
    /// both read.
    alphas: [f32; 8],
    lambdas: [f32; 8],
    stokes: [[f32; 4]; 8],
    /// Per-channel path density at the scattering event (`try_scatter_step`'s `path_pdf`).
    path_pdf: [f32; 8],
    /// Per-channel MIS family bitmasks at the scattering event (`try_scatter_step`'s
    /// `compat`, widened to `u32` for the GPU).
    compat: [u32; 8],
}

const _: () = assert!(size_of::<NeeHgScatterCase>() == 320);

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
    // Spectral-MIS inputs: uniform densities, a ramp, and an uneven set, each paired
    // with either the full family or a +-1 neighbour band per channel, so the family
    // weights differ from 1 and from each other.
    let path_pdf_sets: [[f32; 8]; 3] = [
        [1.0; 8],
        std::array::from_fn(|k| (k as f32).mul_add(0.11, 0.3)),
        [0.5, 0.2, 1.0, 0.05, 0.7, 0.3, 0.9, 0.15],
    ];
    let compat_sets: [[u32; 8]; 2] = [
        [0xFF; 8],
        std::array::from_fn(|k| {
            (0..8usize)
                .filter(|j| j.abs_diff(k) <= 1)
                .fold(0, |m, j| m | (1 << j))
        }),
    ];

    // The medium-transmittance and frosted-exit-skip inputs. Cycled by case index (not cross-producted with the six axes
    // above) so the case count stays the same order of magnitude while still covering
    // lossless/unit-scale/polished alongside real absorbing, scattering and frosted
    // combinations.
    let sigma_s_values = [0.0f32, 0.4, 1.2];
    let path_scale_values = [1.0f32, 1.6];
    let alpha_sets: [[f32; 8]; 3] = [
        [0.0; 8],
        std::array::from_fn(|k| (k as f32).mul_add(0.02, 0.05)),
        std::array::from_fn(|k| (k as f32).mul_add(-0.01, 0.15)),
    ];

    let mut i: usize = 0;
    for &p in &points {
        for &d in &dirs_in {
            for &g in &gs {
                for &n_hero in &n_heroes {
                    for &s in &seeds {
                        for &b in &bounces {
                            let sigma_s = sigma_s_values[i % sigma_s_values.len()];
                            let absorption_path_scale =
                                path_scale_values[i % path_scale_values.len()];
                            let alphas = alpha_sets[i % alpha_sets.len()];
                            // Every seventh case exercises the frosted-exit
                            // skip; the rest stay polished.
                            let frosted_exit = u32::from(i.is_multiple_of(7));
                            let path_pdf = path_pdf_sets[(i / 7) % path_pdf_sets.len()];
                            let compat = compat_sets[(i / 5) % compat_sets.len()];
                            cases.push(NeeHgScatterCase {
                                scatter_point: p.to_array(),
                                n_inside_hero: n_hero,
                                scatter_dir_in: d.to_array(),
                                g,
                                rng_seed: s,
                                bounce: b,
                                sigma_s,
                                absorption_path_scale,
                                frosted_exit,
                                _pad0: 0.0,
                                _pad1: 0.0,
                                _pad2: 0.0,
                                alphas,
                                lambdas,
                                stokes,
                                path_pdf,
                                compat,
                            });
                            i += 1;
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
        total * 3,
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
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let plane_soa = build_plane_soa(&cube_planes);
    let nee_ctx = NeeContext {
        environment: EnvironmentSource::HdrMap(&map),
        plane_soa: &plane_soa,
        enabled: true,
    };

    let mut acc = UlpAccumulator::new("nee_hg_scatter", NEE_HG_ULP_BUDGET, NEE_HG_ABS_FLOOR);
    accumulate_nee_hg_cpu_results(&cases, nee_ctx, &gpu_out, &mut acc);
    acc.finish()
}

/// Runs the real CPU [`nee_contribution_hg_scatter`] for every case, integrates its
/// deposit to XYZ exactly as `try_scatter_step` does (hero channel 0, the case's
/// `path_pdf` and `compat`), and records each XYZ component's CPU-vs-GPU comparison into
/// `acc`. Split out of [`run_nee_hg_scatter`] to
/// keep that function under the house line-count limit.
fn accumulate_nee_hg_cpu_results(
    cases: &[NeeHgScatterCase],
    nee_ctx: NeeContext<'_>,
    gpu_out: &[f32],
    acc: &mut UlpAccumulator<NeeHgScatterCase>,
) {
    // Same six-facet cube `run_nee_hg_scatter`'s GPU side probes; marking
    // every facet `Frosted` (rather than trying to predict which one a given case's
    // shadow ray actually exits through) guarantees the skip fires whenever the
    // per-case `frosted_exit` flag is set, matching `frosted_exit: u32` driving the
    // WGSL twin's own skip (see `NeeHgScatterCase::frosted_exit`'s doc comment).
    let all_frosted = [FacetFinish::Frosted; 6];
    for (idx, case) in cases.iter().enumerate() {
        let mut cpu_deposit = [0.0f32; 8];
        let stokes_cpu: [StokesVector; 8] = std::array::from_fn(|k| {
            StokesVector::new(
                case.stokes[k][0],
                case.stokes[k][1],
                case.stokes[k][2],
                case.stokes[k][3],
            )
        });
        let facet_finishes: &[FacetFinish] = if case.frosted_exit != 0 {
            &all_frosted
        } else {
            &[]
        };
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
            &mut cpu_deposit,
            &case.alphas,
            case.sigma_s,
            case.absorption_path_scale,
            facet_finishes,
        );
        let compat: [u8; 8] = std::array::from_fn(|k| {
            u8::try_from(case.compat[k]).expect("family masks cover eight channels")
        });
        let cpu_xyz = integrate_channels_to_xyz_families(
            &cpu_deposit,
            &case.lambdas,
            &case.path_pdf,
            0,
            compat,
        );
        let base = idx * 3;
        for (offset, component, cpu) in [
            (0, "x", cpu_xyz.x),
            (1, "y", cpu_xyz.y),
            (2, "z", cpu_xyz.z),
        ] {
            acc.record(case, component, cpu, gpu_out[base + offset]);
        }
    }
}
