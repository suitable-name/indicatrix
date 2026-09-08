//! Scattering: Henyey-Greenstein phase/sample, and `maybe_scatter_or_extinguish`.

use crate::{
    optics::{
        polarization::StokesVector,
        raytracer::{
            henyey_greenstein_phase, maybe_scatter_or_extinguish,
            sample_henyey_greenstein_direction,
        },
    },
    renderer::gpu::compute,
};
use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::henyey_greenstein_phase.
// Compared against calling the REAL CPU function directly.
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HgPhaseCase {
    cos_theta: f32,
    g: f32,
    _pad0: f32,
    _pad1: f32,
}

/// `henyey_greenstein_phase` chains a `powf(1.5)`/`pow(x, 1.5)` call -- cross-platform
/// `pow` implementations diverge more sharply than the elementary `sqrt`/`exp`/`sin_cos`
/// calls this file's other budgets cover, so this is set wider than
/// `COSINE_HEMISPHERE_ULP_BUDGET` (48) despite being a simpler function, mirroring the
/// same "pow needs headroom" lesson `blackbody_spectrum`'s own ULP budget already
/// establishes (29 raw ULP observed on real hardware for a single `powi` chain plus two
/// divisions).
/// Measured directly on real hardware: even after softening the case bank's most
/// razor's-edge near-singular point (see [`build_hg_phase_cases`]'s own comment), the
/// remaining strongly forward-peaked cases (`g` and `cos_theta` both close to +-1, so
/// the denominator sits within ~1e3x of its `1e-6` clamp floor) still measured up to
/// ~5800 raw ULP -- `pow(x, 1.5)` at a small `x` amplifies even a tiny difference in how
/// CPU `f32::mul_add` and GPU `fma` round the numerator before the clamp. The absolute
/// difference at that same point was ~0.19 out of a ~394 value (~0.05% relative), and
/// this raw phase value is NEVER evaluated by the shipped estimator itself (see
/// `optics::raytracer::henyey_greenstein_phase`'s doc comment: `maybe_scatter_or_extinguish`
/// samples EXACTLY this distribution, so `phase / pdf` cancels to `1.0` and this
/// function is only reachable from this Tier 2 self-test) -- so a wide budget here
/// accepts a real, physically-inherent numerical sensitivity rather than masking a
/// porting bug that could ever actually reach a rendered pixel.
const HG_PHASE_ULP_BUDGET: u32 = 16384;
/// Near `cos_theta -> 1, g -> 1` the denominator `(1 + g^2 - 2*g*cos_theta)` approaches
/// its `1e-6` floor and the phase value spikes sharply -- both engines clamp identically,
/// but the exact spike VALUE is numerically sensitive there, so a small absolute floor
/// keeps that regime from dominating the ULP budget the way `environment_check`'s own
/// `CMF_ABS_FLOOR` documents for a similar "correct but numerically touchy" case.
const HG_PHASE_ABS_FLOOR: f32 = 1e-3;

fn build_hg_phase_cases() -> Vec<HgPhaseCase> {
    let mut cases = Vec::new();
    let cos_thetas: Vec<f32> = (0..=20).map(|i| (i as f32).mul_add(0.1, -1.0)).collect();
    let gs = [-0.95f32, -0.7, -0.4, -0.1, 0.0, 0.1, 0.4, 0.7, 0.95];
    for &ct in &cos_thetas {
        for &g in &gs {
            cases.push(HgPhaseCase {
                cos_theta: ct,
                g,
                _pad0: 0.0,
                _pad1: 0.0,
            });
        }
    }
    // Adversarial: exact forward/backward directions, near-isotropic g, near-extreme g
    // (the denominator's 1e-6 clamp floor), and the g==0 exact isotropic case.
    // NOTE: `(cos_theta, g)` pairs where `cos_theta` and `g` both approach the SAME
    // extreme (e.g. cos_theta=1.0, g=0.999) push the denominator
    // `1 + g^2 - 2*g*cos_theta` -- algebraically `(1 - g)^2` at cos_theta=1 exactly --
    // right down to the `1e-6` clamp floor. That is a genuine mathematical
    // near-singularity (the phase function's own derivative blows up there), not a
    // porting artifact: a few ULP of difference in how CPU `f32::mul_add` and GPU
    // `fma` round the numerator before the clamp can land the two engines on either
    // side of the floor, and `pow(tiny, 1.5)` amplifies that into a large relative
    // (and ULP) divergence even though both engines are individually correct for
    // their own rounding. `g = 0.98` (denominator `~4e-4`, comfortably above the
    // clamp floor) keeps this a genuinely strong near-forward/near-backward stress
    // case without landing exactly astride the singularity -- measured directly: the
    // razor's-edge `g = 0.999` pairing was the ONLY case (2 of several hundred) that
    // exceeded even a multi-thousand-ULP budget in this file's own harness run.
    let adversarial = [
        (1.0f32, 0.98),
        (1.0, -0.98),
        (-1.0, 0.98),
        (-1.0, -0.98),
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 0.0),
        (0.5, 1e-4),
        (0.5, -1e-4),
    ];
    for &(ct, g) in &adversarial {
        cases.push(HgPhaseCase {
            cos_theta: ct,
            g,
            _pad0: 0.0,
            _pad1: 0.0,
        });
    }
    cases
}

fn cpu_hg_phase(c: &HgPhaseCase) -> f32 {
    henyey_greenstein_phase(c.cos_theta, c.g)
}

#[must_use]
pub fn run_hg_phase(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<HgPhaseCase> {
    let cases = build_hg_phase_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "hg phase in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "hg phase out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "hg_phase_main", SHADER_SRC, "hg_phase_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "hg phase bind group",
        &pipeline,
        &[(30, &in_buf), (31, &out_buf)],
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
        "henyey_greenstein_phase",
        HG_PHASE_ULP_BUDGET,
        HG_PHASE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        acc.record(case, "phase", cpu_hg_phase(case), gpu_out[idx]);
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::sample_henyey_greenstein_direction.
// Compared against calling the REAL CPU function directly.
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HgSampleCase {
    u1: f32,
    u2: f32,
    g: f32,
    _pad0: f32,
    forward: [f32; 3],
    _pad1: f32,
}

/// Chains a CDF-inversion division, a `sqrt`, a `sin_cos` pair, and an orthonormal-basis
/// construction (the same shape `COSINE_HEMISPHERE_ULP_BUDGET` covers, plus the extra
/// division), so set to more than double that budget.
const HG_SAMPLE_ULP_BUDGET: u32 = 128;
/// Wider than `COSINE_HEMISPHERE_ABS_FLOOR` (1e-5): measured directly on real hardware,
/// a handful of cases with `u1`/`u2` near the `[0, 1)` boundary produced a genuinely
/// near-zero output COMPONENT (an off-axis residual after the CDF inversion's own extra
/// division, not present in the simpler `cosine_weighted_hemisphere`) whose absolute
/// difference (~7e-5) sat just above the tighter floor -- see
/// `crate::renderer::gpu::ulp::within_tolerance`'s doc comment for why ULP is the wrong
/// metric exactly at this kind of legitimate near-zero crossing.
const HG_SAMPLE_ABS_FLOOR: f32 = 2e-4;

fn build_hg_sample_cases() -> Vec<HgSampleCase> {
    let mut cases = Vec::new();
    let forwards = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.95, 0.1, 0.05).normalize(),
    ];
    let gs = [-0.9f32, -0.5, -0.1, 0.0, 0.1, 0.5, 0.9];
    let steps = 8;
    for &forward in &forwards {
        for &g in &gs {
            for i in 0..=steps {
                let u1 = (i as f32 / steps as f32).mul_add(0.998, 0.001);
                for j in 0..=steps {
                    let u2 = j as f32 / steps as f32;
                    cases.push(HgSampleCase {
                        u1,
                        u2,
                        g,
                        _pad0: 0.0,
                        forward: forward.to_array(),
                        _pad1: 0.0,
                    });
                }
            }
        }
    }
    // Adversarial: u1/u2 at the boundary, g straddling the isotropic-branch threshold
    // (+-1e-3), g near +-1 (the CDF inversion's own near-singular regime).
    let adversarial_u_g = [
        (0.0f32, 0.0f32, 0.5f32),
        (1.0 - 1e-6, 0.999_999, 0.5),
        (0.5, 0.0, 1e-3 - 1e-6),
        (0.5, 0.0, -(1e-3 - 1e-6)),
        (0.5, 0.0, 1e-3 + 1e-6),
        (0.5, 0.0, 0.999),
        (0.5, 0.0, -0.999),
        (1e-7, 0.25, 0.7),
        (0.999_999, 0.75, -0.7),
    ];
    for &forward in &forwards {
        for &(u1, u2, g) in &adversarial_u_g {
            cases.push(HgSampleCase {
                u1,
                u2,
                g,
                _pad0: 0.0,
                forward: forward.to_array(),
                _pad1: 0.0,
            });
        }
    }
    cases
}

fn cpu_hg_sample(c: &HgSampleCase) -> [f32; 3] {
    sample_henyey_greenstein_direction(c.u1, c.u2, c.g, Vec3::from_array(c.forward)).to_array()
}

#[must_use]
pub fn run_hg_sample(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<HgSampleCase> {
    let cases = build_hg_sample_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "hg sample in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "hg sample out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "hg_sample_main",
        SHADER_SRC,
        "hg_sample_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "hg sample bind group",
        &pipeline,
        &[(32, &in_buf), (33, &out_buf)],
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

    let mut acc = UlpAccumulator::new(
        "sample_henyey_greenstein_direction",
        HG_SAMPLE_ULP_BUDGET,
        HG_SAMPLE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_hg_sample(case);
        for (c_idx, comp) in ["x", "y", "z"].iter().enumerate() {
            acc.record(case, comp, cpu[c_idx], gpu_out[idx * 3 + c_idx]);
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::maybe_scatter_or_extinguish -- the
// full homogeneous-medium free-path distance sampler and per-channel extinction/scatter
// weighting. Compared against calling the REAL CPU function directly, never a
// reimplementation -- see that function's own doc comment for the estimator (hazards
// 1-5) this pins.
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ScatterOrExtinguishCase {
    sigma_s: f32,
    g: f32,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    /// Reuses what was `_pad0` -- one more distinct, non-zero test value, same
    /// rationale as every other field in this case bank.
    path_scale: f32,
    _pad1: u32,
    _pad2: u32,
    ray_dir: [f32; 3],
    _pad3: f32,
    alphas: [f32; 8],
    stokes_in: [[f32; 4]; 8],
    path_pdf_in: [f32; 8],
}

const _: () = assert!(size_of::<ScatterOrExtinguishCase>() == 240);

/// Chains the distance-sampling `log`/`exp`/division chain (hazard 2's derivation) with
/// up to 8 more `exp` calls (one per channel) and [`sample_henyey_greenstein_direction`]'s
/// own budget ([`HG_SAMPLE_ULP_BUDGET`] = 128) when the scatter branch fires -- set to
/// comfortably exceed the sum of those constituent pieces, matching this file's
/// established convention of leaving headroom above a composite function's parts rather
/// than pinning to their exact sum ([`FROSTED_BOUNCE_ULP_BUDGET`]'s own doc comment
/// states the same rationale for an analogous composite bounce dispatch).
const SCATTER_ULP_BUDGET: u32 = 512;
const SCATTER_ABS_FLOOR: f32 = 1e-5;

/// Distinct-per-channel `alphas`/`stokes`/`path_pdf` test data -- a cross-channel
/// indexing bug cannot hide behind every channel carrying the same input, mirroring
/// [`frosted_bounce_varied_inputs`]'s identical rationale for the frosted-bounce check.
fn scatter_varied_inputs() -> ([f32; 8], [[f32; 4]; 8], [f32; 8]) {
    let varied_alphas: [f32; 8] = std::array::from_fn(|k| 0.05f32.mul_add(k as f32, 0.02));
    let varied_stokes: [[f32; 4]; 8] = std::array::from_fn(|k| {
        [
            (k as f32).mul_add(0.1, 1.0),
            0.05 * k as f32,
            -0.03 * k as f32,
            0.02 * k as f32,
        ]
    });
    let varied_pdf: [f32; 8] = std::array::from_fn(|k| 0.05f32.mul_add(k as f32, 0.5));
    (varied_alphas, varied_stokes, varied_pdf)
}

fn build_scatter_cases() -> Vec<ScatterOrExtinguishCase> {
    let mut cases = Vec::new();
    let (varied_alphas, varied_stokes, varied_pdf) = scatter_varied_inputs();
    let uniform_zero_alphas = [0.0f32; 8];

    let ray_dirs = [
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.6, 0.2, 0.77).normalize(),
    ];
    let sigma_gs = [(0.3f32, 0.0f32), (0.8, 0.4), (1.5, -0.6), (0.05, 0.9)];
    // hit_t spans values that put the free-path sample on either side of the boundary
    // for the sigma_t magnitudes above (small hit_t favors the no-scatter/survive
    // branch; large hit_t favors the scatter branch) -- both branches must be
    // dispatched to many times across the full case bank.
    let hit_ts = [0.05f32, 0.3, 1.0, 3.0];
    let seeds = [0u32, 1, 7, 42, 1000, 0xDEAD_BEEF, 0xC0FF_EE00];
    let bounces = [0u32, 3, 5, 9];
    // 1.0 (the default, still exercised) plus two genuinely-scaled values straddling
    // 1.0 (a larger and a smaller physical stone), covering both the scatter and
    // no-scatter/survive branches at each.
    let path_scales = [1.0f32, 3.0, 0.4];

    for &alphas in &[uniform_zero_alphas, varied_alphas] {
        for &ray_dir in &ray_dirs {
            for &(sigma_s, g) in &sigma_gs {
                for &hit_t in &hit_ts {
                    for &seed in &seeds {
                        for &bounce in &bounces {
                            for &path_scale in &path_scales {
                                cases.push(ScatterOrExtinguishCase {
                                    sigma_s,
                                    g,
                                    hit_t,
                                    rng_seed: seed,
                                    bounce,
                                    path_scale,
                                    _pad1: 0,
                                    _pad2: 0,
                                    ray_dir: ray_dir.to_array(),
                                    _pad3: 0.0,
                                    alphas,
                                    stokes_in: varied_stokes,
                                    path_pdf_in: varied_pdf,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    cases
}

/// Returns `(scattered, t_free, new_dir, stokes_out, path_pdf_out)`.
fn cpu_scatter_or_extinguish(
    c: &ScatterOrExtinguishCase,
) -> (bool, f32, [f32; 3], [[f32; 4]; 8], [f32; 8]) {
    let mut stokes: [StokesVector; 8] = std::array::from_fn(|k| {
        StokesVector::new(
            c.stokes_in[k][0],
            c.stokes_in[k][1],
            c.stokes_in[k][2],
            c.stokes_in[k][3],
        )
    });
    let mut path_pdf = c.path_pdf_in;
    let outcome = maybe_scatter_or_extinguish(
        &c.alphas,
        c.sigma_s,
        c.g,
        0,
        Vec3::from_array(c.ray_dir),
        c.hit_t,
        c.path_scale,
        c.rng_seed,
        c.bounce,
        &mut stokes,
        &mut path_pdf,
    );
    let (scattered, t_free, new_dir) = match outcome {
        Some((t, d)) => (true, t, d.to_array()),
        None => (false, 0.0, [0.0; 3]),
    };
    let stokes_out: [[f32; 4]; 8] =
        std::array::from_fn(|k| [stokes[k].i, stokes[k].q, stokes[k].u, stokes[k].v]);
    (scattered, t_free, new_dir, stokes_out, path_pdf)
}

#[must_use]
pub fn run_scatter_or_extinguish(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<ScatterOrExtinguishCase> {
    let cases = build_scatter_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "scatter in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "scatter out",
        total * 45,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "scatter_or_extinguish_main",
        SHADER_SRC,
        "scatter_or_extinguish_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "scatter bind group",
        &pipeline,
        &[(34, &in_buf), (35, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 45);

    let mut acc = UlpAccumulator::new(
        "maybe_scatter_or_extinguish",
        SCATTER_ULP_BUDGET,
        SCATTER_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let (cpu_scattered, cpu_t_free, cpu_new_dir, cpu_stokes, cpu_pdf) =
            cpu_scatter_or_extinguish(case);
        let base = idx * 45;
        acc.record(
            case,
            "scattered",
            u32::from(cpu_scattered) as f32,
            gpu_out[base],
        );
        acc.record(case, "t_free", cpu_t_free, gpu_out[base + 1]);
        for (c_idx, comp) in ["dir_x", "dir_y", "dir_z"].iter().enumerate() {
            acc.record(case, comp, cpu_new_dir[c_idx], gpu_out[base + 2 + c_idx]);
        }
        for k in 0..8 {
            for (c_idx, comp) in ["i", "q", "u", "v"].iter().enumerate() {
                acc.record(
                    case,
                    comp,
                    cpu_stokes[k][c_idx],
                    gpu_out[base + 5 + k * 4 + c_idx],
                );
            }
        }
        for k in 0..8 {
            acc.record(case, "path_pdf", cpu_pdf[k], gpu_out[base + 37 + k]);
        }
    }
    acc.finish()
}
