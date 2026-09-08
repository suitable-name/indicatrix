//! The furnace anchor -- driven by `shaders/furnace.wgsl`.
//!
//! Ties every ported function together (camera ray generation with RNG-derived jitter,
//! the hero-wavelength comb, CIE 1931 CMF integration) into one
//! end-to-end pipeline against a deliberately uniform (direction- and
//! wavelength-independent) constant-radiance environment. Because the environment is
//! uniform, the expected XYZ is analytically computable directly from the CMF
//! integral -- this checks BOTH the CPU reference and the GPU port against that
//! independently-computed TRUTH, not merely against each other, so a porting mistake
//! shared by both translations cannot self-certify. Mirrors
//! `tests/env_map_tests.rs::white_furnace_test_converges_to_the_true_uniform_radiance`'s
//! furnace pattern, applied to this crate's own hero-wavelength/CMF-integration
//! machinery instead of `EnvironmentMap`'s importance sampler.
//!
//! # `intersect_polyhedron(ray, &[])` is not `None`
//!
//! A common intuition for "empty plane list" is "every ray misses". That is not what
//! `optics::raytracer::intersect_polyhedron` actually returns for zero planes: its
//! `t_near`/`t_far` sentinels (`-1e30`/`1e30`) fall through to the "origin is inside
//! the solid" EXIT branch (vacuously true with zero half-space constraints), producing
//! `Some(HitRecord { t: 1e30, normal: Vec3::ZERO, facet_idx: 0 })` -- see
//! [`empty_planes_intersect_returns_the_sentinel_hit_not_a_miss`] below, which pins
//! this down directly against the real function. Functionally this IS "the ray
//! reaches the environment unobstructed" (there is no real geometry at `t = 1e30`),
//! which is exactly the property `shaders/furnace.wgsl`'s "uniform environment,
//! unconditionally sampled, no branching on the intersect result" design relies on.
//! This module does not model `trace_spectral_ray`'s `Option`-branching at all (that
//! belongs to the transport port); it deliberately never calls
//! `intersect_polyhedron` in the per-tuple XYZ estimator for exactly this reason -- see
//! [`cpu_furnace_xyz`].

use crate::{
    color::cie1931::cie_1931_cmf,
    optics::raytracer::{Camera, hash_u32, integrate_channels_to_xyz, wrapped_hero_wavelengths},
    renderer::{
        buffers::GpuCameraParams,
        gpu::{
            compute,
            ulp::{ulp_distance, within_tolerance},
        },
    },
};
use glam::Vec3;

const SHADER_SRC: &str = include_str!("../shaders/furnace.wgsl");

/// Constant spectral radiance the furnace's uniform "environment" returns for every
/// direction and wavelength. Any nonzero value works for the analytic derivation below;
/// deliberately not `1.0` so a bug that silently dropped the radiance factor entirely
/// (leaving an implicit `1.0`) would still be caught.
const L0: f32 = 3.25;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
const NUM_PIXELS: u32 = WIDTH * HEIGHT; // 4096
const NUM_SAMPLES: u32 = 64;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct FurnaceExtra {
    l0: f32,
    num_pixels: u32,
    _pad0: u32,
    _pad1: u32,
}

/// The analytically true XYZ this furnace's per-sample estimator converges to.
///
/// `L0 * integral / 106.856`, where `integral` is the CMF fit ([`cie_1931_cmf`])
/// integrated over 380..=780nm -- using the SAME 401-point (1nm step) quadrature
/// convention `optics::raytracer::compute_illuminant_white_balance` uses for its own
/// von-Kries integration (calling the real, single-source-of-truth fit function
/// directly, never a reimplementation of it).
///
/// # Derivation
///
/// `integrate_channels_to_xyz`'s `norm_factor` is `(400/8) / 106.856`. Its 8-channel
/// comb is systematic sampling (fixed 50nm spacing) with one shared random offset
/// (`hero_rand`) -- a standard unbiased-in-expectation quadrature for ANY integrand,
/// smooth or not: `E[sum_k f(lambda_k)] = (N / period) * integral_period f`. With
/// `N = 8`, `period = 400`: `E[xyz_sample] = norm_factor * L0 * (8/400) * I = L0 * I /
/// 106.856`, where `I` is the true integral this function approximates by quadrature.
#[must_use]
fn analytic_target(l0: f32) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        sum += Vec3::from_array(cie_1931_cmf(lambda));
    }
    sum * (l0 / 106.856)
}

/// The exact CPU reference for one `(pixel, sample)` tuple's furnace XYZ estimate.
/// Calls the real `hash_u32`, `wrapped_hero_wavelengths`, and
/// `integrate_channels_to_xyz` directly -- never a reimplementation. Does not call
/// `Camera::generate_ray`/`intersect_polyhedron` at all: this furnace's environment is
/// direction-independent by construction, so (per this module's own doc comment on why
/// `intersect_polyhedron(ray, &[])` is not a miss) there is nothing for those to
/// meaningfully contribute to the XYZ estimate; `shaders/furnace.wgsl` still computes
/// (and discards) a ray direction for pipeline fidelity, but the value used to seed the
/// CMF integration comes from the RNG/hero-wavelength chain alone.
#[must_use]
fn cpu_furnace_xyz(pixel: u32, sample: u32, l0: f32) -> Vec3 {
    let seed = hash_u32(pixel.wrapping_mul(0x9e37_79b9) ^ sample.wrapping_mul(0x85eb_ca6b));
    let hero_hash = hash_u32(seed);
    let hero_rand = (hero_hash as f32) / 4_294_967_295.0;
    let lambdas: [f32; 8] = wrapped_hero_wavelengths(hero_rand);
    let radiance = [l0; 8];
    let path_pdf = [1.0f32; 8];
    integrate_channels_to_xyz(&radiance, &lambdas, &path_pdf, 0)
}

/// Full furnace-anchor result: per-tuple ULP agreement between CPU and GPU, both sides'
/// convergence against [`analytic_target`], and the two-runs-byte-identical
/// determinism check.
#[derive(Debug, Clone)]
pub struct FurnaceCheckResult {
    pub total_tuples: usize,
    pub analytic_target: Vec3,

    /// Per-tuple CPU-vs-GPU ULP agreement (the `furnace_samples_main` dispatch). Uses
    /// the same hybrid ULP-OR-absolute-floor rule as `environment_check` (see
    /// [`crate::renderer::gpu::ulp::within_tolerance`]).
    pub per_tuple_ulp_budget: u32,
    pub per_tuple_abs_floor: f32,
    pub per_tuple_max_ulp: u32,
    pub per_tuple_over_budget_count: usize,
    pub per_tuple_argmax: Option<FurnaceUlpArgmax>,

    /// Mean over every tuple of the CPU reference estimator, vs [`analytic_target`].
    pub cpu_mean: Vec3,
    /// Mean over every pixel of the GPU accumulate-kernel's per-pixel average, vs
    /// [`analytic_target`].
    pub gpu_mean: Vec3,
    /// Relative error (`|mean - target| / |target|`, componentwise max) each side's
    /// mean has from the analytic target.
    pub cpu_relative_error: f32,
    pub gpu_relative_error: f32,

    /// Two independent dispatches of `furnace_accumulate_main` against identical
    /// input, compared byte-for-byte.
    pub determinism_mismatches: usize,
    pub determinism_sample_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct FurnaceUlpArgmax {
    pub pixel: u32,
    pub sample: u32,
    pub component: &'static str,
    pub cpu: f32,
    pub gpu: f32,
    pub ulp: u32,
}

/// ULP budget for the per-tuple furnace estimate.
///
/// Chains `hash_u32` (bit-exact) into `wrapped_hero_wavelengths` (already
/// budgeted at 4 ULP by `rng_check::FLOAT_ULP_BUDGET`) into `cie_1931_cmf` (budgeted at
/// `environment_check::CMF_ULP_BUDGET` = 32 ULP, evaluated 8 times and summed) --
/// accumulated rounding across all of that justifies a wider budget than any single
/// stage alone.
pub const FURNACE_ULP_BUDGET: u32 = 256;

/// Absolute-difference floor for per-tuple furnace comparisons.
///
/// See [`crate::renderer::gpu::ulp::within_tolerance`]'s general rationale. Each channel's
/// contribution is `cie_1931_cmf(lambda) * L0 * norm_factor`; with `L0 = 3.25` this
/// stays comfortably above `1e-4` for any wavelength `wrapped_hero_wavelengths` can
/// actually produce (always within `[380, 780]`, never the deep Gaussian tail
/// `environment_check::CMF_ABS_FLOOR` exists for), so this floor exists only to absorb
/// the same kind of near-zero rounding-direction noise `camera_check` measured, not to
/// mask a real divergence.
pub const FURNACE_ABS_FLOOR: f32 = 1e-4;

/// A converged Monte-Carlo mean's relative error from the analytic target must fall
/// under this bound.
///
/// Calibrated the same way
/// `tests/env_map_tests.rs::white_furnace_test_converges_to_the_true_uniform_radiance`
/// calibrates its own `0.005` bound for a similarly-sized sample count (`NUM_PIXELS *
/// NUM_SAMPLES` = 262,144 total tuples here, comparable order of magnitude to that
/// test's 400,000): both sides of this estimator have far lower per-sample variance
/// than a general importance-sampled furnace test, though, since `wrapped_hero_wavelengths`
/// is systematic (fixed 50nm spacing) rather than fully random -- so this bound is
/// intentionally tight.
pub const CONVERGENCE_RELATIVE_TOLERANCE: f32 = 0.01;

/// Runs the furnace-anchor self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run(ctx: &crate::renderer::gpu::GpuContext) -> FurnaceCheckResult {
    let camera = Camera::new(0.4, 0.3, 5.0, 40.0);
    let camera_params = GpuCameraParams {
        origin: camera.origin.to_array(),
        fov_tan: camera.fov_tan,
        forward: camera.forward.to_array(),
        width: WIDTH as f32,
        right: camera.right.to_array(),
        height: HEIGHT as f32,
        up: camera.up.to_array(),
        num_samples: NUM_SAMPLES,
    };
    let extra = FurnaceExtra {
        l0: L0,
        num_pixels: NUM_PIXELS,
        _pad0: 0,
        _pad1: 0,
    };

    let total_tuples = (NUM_PIXELS as usize) * (NUM_SAMPLES as usize);

    let camera_buf = compute::upload(
        &ctx.device,
        "furnace camera",
        std::slice::from_ref(&camera_params),
        wgpu::BufferUsages::UNIFORM,
    );
    let extra_buf = compute::upload(
        &ctx.device,
        "furnace extra",
        std::slice::from_ref(&extra),
        wgpu::BufferUsages::UNIFORM,
    );

    let gpu_persample = dispatch_furnace_samples(ctx, &camera_buf, &extra_buf, total_tuples);
    let (per_tuple_max_ulp, per_tuple_over_budget, per_tuple_argmax, cpu_mean) =
        compare_per_tuple(&gpu_persample, total_tuples);

    let (run1, run2) = dispatch_furnace_accumulate_twice(ctx, &camera_buf, &extra_buf);
    let determinism_mismatches = run1
        .iter()
        .zip(run2.iter())
        .filter(|(a, b)| a.to_bits() != b.to_bits())
        .count();

    let gpu_mean = mean_of_interleaved_component(&run1, NUM_PIXELS * NUM_SAMPLES);

    let target = analytic_target(L0);
    let cpu_relative_error = componentwise_relative_error(cpu_mean, target);
    let gpu_relative_error = componentwise_relative_error(gpu_mean, target);

    FurnaceCheckResult {
        total_tuples,
        analytic_target: target,
        per_tuple_ulp_budget: FURNACE_ULP_BUDGET,
        per_tuple_abs_floor: FURNACE_ABS_FLOOR,
        per_tuple_max_ulp,
        per_tuple_over_budget_count: per_tuple_over_budget,
        per_tuple_argmax,
        cpu_mean,
        gpu_mean,
        cpu_relative_error,
        gpu_relative_error,
        determinism_mismatches,
        determinism_sample_count: run1.len(),
    }
}

/// Dispatches `furnace_samples_main` once (one thread per `(pixel, sample)` tuple,
/// each writing its own independent XYZ slot) and reads back the flat
/// `[x0,y0,z0,x1,y1,z1,...]` per-tuple result buffer.
///
/// `furnace_samples_main` only references bindings 0/1/2 (it never writes
/// `out_pixel_sum`) -- `wgpu`'s auto-inferred bind group layout (`layout: None`) is
/// scoped to exactly the bindings the chosen entry point's body reaches, so the bind
/// group here must match that exactly (unlike [`dispatch_furnace_accumulate_twice`],
/// which references 0/1/3 instead -- neither entry point touches all four).
fn dispatch_furnace_samples(
    ctx: &crate::renderer::gpu::GpuContext,
    camera_buf: &wgpu::Buffer,
    extra_buf: &wgpu::Buffer,
    total_tuples: usize,
) -> Vec<f32> {
    let persample_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "furnace persample",
        total_tuples * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let samples_pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "furnace_samples",
        SHADER_SRC,
        "furnace_samples_main",
    );
    let samples_bind_group = compute::bind_buffers(
        &ctx.device,
        "furnace samples bind group",
        &samples_pipeline,
        &[(0, camera_buf), (1, extra_buf), (2, &persample_buf)],
    );
    let samples_workgroups = (total_tuples as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &samples_pipeline,
        &samples_bind_group,
        (samples_workgroups, 1, 1),
    );
    compute::readback(&ctx.device, &ctx.queue, &persample_buf, total_tuples * 3)
}

/// Compares every `(pixel, sample)` tuple's GPU result against
/// [`cpu_furnace_xyz`], returning `(max_genuine_ulp, over_budget_count, argmax,
/// cpu_mean)`. Pulled out of [`run`] to keep that function's line count down.
fn compare_per_tuple(
    gpu_persample: &[f32],
    total_tuples: usize,
) -> (u32, usize, Option<FurnaceUlpArgmax>, Vec3) {
    let mut max_ulp = 0u32;
    let mut argmax = None;
    let mut over_budget = 0usize;
    let mut cpu_sum = Vec3::ZERO;
    for idx in 0..total_tuples {
        let pixel = (idx as u32) / NUM_SAMPLES;
        let sample = (idx as u32) % NUM_SAMPLES;
        let cpu_xyz = cpu_furnace_xyz(pixel, sample, L0);
        cpu_sum += cpu_xyz;
        let gpu_xyz = Vec3::new(
            gpu_persample[idx * 3],
            gpu_persample[idx * 3 + 1],
            gpu_persample[idx * 3 + 2],
        );
        for (component, cpu, gpu) in [
            ("x", cpu_xyz.x, gpu_xyz.x),
            ("y", cpu_xyz.y, gpu_xyz.y),
            ("z", cpu_xyz.z, gpu_xyz.z),
        ] {
            let ulp = ulp_distance(cpu, gpu);
            if within_tolerance(cpu, gpu, FURNACE_ULP_BUDGET, FURNACE_ABS_FLOOR) {
                continue;
            }
            over_budget += 1;
            if ulp > max_ulp {
                max_ulp = ulp;
                argmax = Some(FurnaceUlpArgmax {
                    pixel,
                    sample,
                    component,
                    cpu,
                    gpu,
                    ulp,
                });
            }
        }
    }
    let cpu_mean = cpu_sum / (total_tuples as f32);
    (max_ulp, over_budget, argmax, cpu_mean)
}

/// Dispatches `furnace_accumulate_main` twice against identical input -- the
/// two-runs-byte-identical determinism check -- and returns both runs' flat
/// `[x0,y0,z0,...]` per-pixel sum buffers.
///
/// `furnace_accumulate_main` references bindings 0/1/3 (never `out_persample` at
/// binding 2) -- see [`dispatch_furnace_samples`]'s doc comment for why the two
/// pipelines need different binding sets.
fn dispatch_furnace_accumulate_twice(
    ctx: &crate::renderer::gpu::GpuContext,
    camera_buf: &wgpu::Buffer,
    extra_buf: &wgpu::Buffer,
) -> (Vec<f32>, Vec<f32>) {
    let pixel_sum_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "furnace pixel sum",
        (NUM_PIXELS as usize) * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let accumulate_pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "furnace_accumulate",
        SHADER_SRC,
        "furnace_accumulate_main",
    );
    let accumulate_bind_group = compute::bind_buffers(
        &ctx.device,
        "furnace accumulate bind group",
        &accumulate_pipeline,
        &[(0, camera_buf), (1, extra_buf), (3, &pixel_sum_buf)],
    );
    let pixel_workgroups = NUM_PIXELS.div_ceil(64);

    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &accumulate_pipeline,
        &accumulate_bind_group,
        (pixel_workgroups, 1, 1),
    );
    let run1: Vec<f32> = compute::readback(
        &ctx.device,
        &ctx.queue,
        &pixel_sum_buf,
        (NUM_PIXELS as usize) * 3,
    );

    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &accumulate_pipeline,
        &accumulate_bind_group,
        (pixel_workgroups, 1, 1),
    );
    let run2: Vec<f32> = compute::readback(
        &ctx.device,
        &ctx.queue,
        &pixel_sum_buf,
        (NUM_PIXELS as usize) * 3,
    );

    (run1, run2)
}

/// Mean XYZ over `total_samples` samples, from a flat `[x0,y0,z0,x1,y1,z1,...]`
/// per-pixel sum buffer, accumulated in `f64` to avoid the mean itself introducing
/// extra rounding on top of what's being measured.
fn mean_of_interleaved_component(flat_xyz: &[f32], total_samples: u32) -> Vec3 {
    let mut sum = [0.0f64; 3];
    for chunk in flat_xyz.as_chunks::<3>().0 {
        sum[0] += f64::from(chunk[0]);
        sum[1] += f64::from(chunk[1]);
        sum[2] += f64::from(chunk[2]);
    }
    let n = f64::from(total_samples);
    Vec3::new(
        (sum[0] / n) as f32,
        (sum[1] / n) as f32,
        (sum[2] / n) as f32,
    )
}

fn componentwise_relative_error(value: Vec3, target: Vec3) -> f32 {
    let dx = (value.x - target.x).abs() / target.x.abs().max(1e-6);
    let dy = (value.y - target.y).abs() / target.y.abs().max(1e-6);
    let dz = (value.z - target.z).abs() / target.z.abs().max(1e-6);
    dx.max(dy).max(dz)
}

impl FurnaceCheckResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.per_tuple_over_budget_count == 0
            && self.cpu_relative_error <= CONVERGENCE_RELATIVE_TOLERANCE
            && self.gpu_relative_error <= CONVERGENCE_RELATIVE_TOLERANCE
            && self.determinism_mismatches == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::plane::GpuFacetPlane,
        optics::raytracer::{HitRecord, Ray, intersect_polyhedron},
    };

    /// Pins the exact, mildly surprising CPU behavior this module's design relies on:
    /// with zero facet planes, `intersect_polyhedron` returns a `Some` sentinel hit at
    /// `t = 1e30`, never `None`. See this module's own doc comment.
    #[test]
    fn empty_planes_intersect_returns_the_sentinel_hit_not_a_miss() {
        let ray = Ray {
            origin: Vec3::ZERO,
            dir: Vec3::new(0.3, 0.5, 0.8).normalize(),
        };
        let planes: [GpuFacetPlane; 0] = [];
        let hit = intersect_polyhedron(ray, &planes);
        let hit: HitRecord =
            hit.expect("zero planes should still produce the vacuous sentinel Some(..), not None");
        assert!(
            (hit.t - 1e30).abs() < 1.0,
            "expected t == 1e30 sentinel, got {}",
            hit.t
        );
        assert_eq!(hit.normal, Vec3::ZERO);
        assert_eq!(hit.facet_idx, 0);
    }

    /// Sanity check on [`analytic_target`]'s derivation: a furnace with `L0 = 0` must
    /// integrate to exactly zero (no illuminant, no radiance, regardless of the CMF
    /// shape).
    #[test]
    fn analytic_target_is_zero_for_zero_radiance() {
        assert_eq!(analytic_target(0.0), Vec3::ZERO);
    }

    /// [`cpu_furnace_xyz`]'s mean over many samples should already converge to
    /// [`analytic_target`] on the CPU side alone, independent of any GPU dispatch --
    /// this is the same claim [`run`] verifies against the GPU, checked here without
    /// needing a live adapter (this test runs under plain `cargo test`, no `gpu`
    /// feature GPU dependency beyond the feature gate itself).
    #[test]
    fn cpu_furnace_estimator_converges_to_the_analytic_target() {
        let l0 = 3.25;
        let total = 200_000u32;
        let mut sum = Vec3::ZERO;
        for idx in 0..total {
            let pixel = idx / 64;
            let sample = idx % 64;
            sum += cpu_furnace_xyz(pixel, sample, l0);
        }
        let mean = sum / (total as f32);
        let target = analytic_target(l0);
        let rel_err = componentwise_relative_error(mean, target);
        assert!(
            rel_err < CONVERGENCE_RELATIVE_TOLERANCE,
            "CPU furnace estimator mean {mean:?} should converge to analytic target {target:?} \
             (relative error {rel_err:.6}, tolerance {CONVERGENCE_RELATIVE_TOLERANCE})"
        );
    }
}
