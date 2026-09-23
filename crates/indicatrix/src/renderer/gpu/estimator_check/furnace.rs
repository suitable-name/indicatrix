//! The energy-conservation furnace anchors: a colourless, non-dispersive, non-absorbing
//! cubic gem inside a uniform (direction-independent) environment must return exactly
//! that uniform radiance in expectation, on both CPU and GPU -- against a TRUTH anchor,
//! not merely CPU-vs-GPU. See [`run_furnace`]'s doc comment.

use crate::{
    color::cie1931::cie_1931_cmf,
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, FacetFinish},
    },
    renderer::{
        buffers::{GpuGemMaterial, GpuTransportParams, encode_facet_finishes, transport_env_mode},
        env_map::{EnvironmentMap, rgb_to_spectral_radiance},
    },
};
use glam::Vec3;

use super::{
    CpuScene, MaterialForDispatch, SceneBuffersForDispatch, WelfordXyz, bruted_girdle_finishes,
    camera_params_for, cpu_sample_xyz, cpu_samples, dispatch_transport,
    dispatch_transport_for_class, furnace_material, round_brilliant_planes, test_camera, z_score,
};

#[derive(Debug, Clone)]
pub struct FurnaceResult {
    pub analytic_target: Vec3,
    pub cpu_mean: Vec3,
    pub gpu_mean: Vec3,
    pub cpu_relative_error: f32,
    pub gpu_relative_error: f32,
    /// Aggregate (pooled over every pixel*sample tuple) CPU-vs-GPU z-score per XYZ
    /// component.
    pub cpu_gpu_z: [f64; 3],
    pub total_cpu_samples: usize,
    pub total_gpu_samples: usize,
}

const FURNACE_CONVERGENCE_TOLERANCE: f32 = 0.02;
/// Aggregate z-score gate: with tens of thousands of pooled samples per side, a
/// genuine porting bug moves this by many standard errors; `4.0` leaves headroom above
/// the `~3` a single unlucky draw could plausibly produce.
const FURNACE_Z_GATE: f64 = 4.0;

/// The relative-error-vs-analytic-target tolerance for [`run_furnace_frosted_girdle`] --
/// matches `tests/raytracer_tests.rs`'s own
/// `frosted_girdle_white_furnace_energy_conservation_still_holds` tolerance exactly,
/// rather than inventing a new number.
///
/// # Why this converges more slowly than the polished furnace
///
/// A frosted facet's cosine-weighted-hemisphere scattering keeps more paths alive past
/// `bounce > 4`, where Russian Roulette's `q.clamp(0.05, 1.0)` floor can rescale a
/// surviving path up to 20x -- individually unbiased (`E[survive]*(1/q) = 1` for any
/// q) but producing rare, extremely bright samples that make the running mean converge
/// slowly rather than monotonically. This is a property of the CPU
/// `apply_frosted_bounce`/Russian-Roulette interaction, identical on CPU and GPU (the
/// tight [`FURNACE_Z_GATE`] cross-check below confirms agreement even while both differ
/// from the analytic target by more than [`FURNACE_CONVERGENCE_TOLERANCE`]), not a bug.
const FROSTED_FURNACE_CONVERGENCE_TOLERANCE: f32 = 0.06;

/// The relative-error-vs-analytic-target tolerance for
/// [`run_furnace_scattering`], matching `optics::raytracer::scattering_tests::
/// lossless_scattering_white_furnace_energy_conservation_holds`'s own CPU-only tolerance
/// rather than inventing a new number. Wider than the polished furnace's 0.02 for the
/// same reason as [`FROSTED_FURNACE_CONVERGENCE_TOLERANCE`]: a scattering event keeps
/// more paths alive past `bounce > 4`, producing a heavier-tailed but still-unbiased
/// estimator -- identically on CPU and GPU, confirmed by the tight `FURNACE_Z_GATE`
/// cross-check below.
const SCATTERING_FURNACE_CONVERGENCE_TOLERANCE: f32 = 0.08;

/// The relative-error-vs-analytic-target tolerance for
/// [`run_furnace_edge_rounding`], matching `optics::raytracer::edge_rounding_tests::
/// edge_rounding_white_furnace_energy_conservation_holds`'s own CPU-only tolerance
/// (0.06) rather than inventing a new number.
const EDGE_ROUNDING_FURNACE_CONVERGENCE_TOLERANCE: f32 = 0.06;

/// Every furnace anchor except the lossless-scattering ones below runs at this bounce
/// budget -- see [`SCATTERING_FURNACE_MAX_BOUNCES`]'s doc comment for the bounce-cap
/// measurement this budget is sized against.
const FURNACE_DEFAULT_MAX_BOUNCES: u32 = 12;

/// [`run_furnace_scattering`] and
/// [`run_furnace_nee_equality_scattering`] both trace `furnace_material().with_scattering(1.2,
/// 0.4)` -- `sigma_a == 0.0`, so nothing along the path is ever absorbed, only redirected;
/// a path only stops via a direct escape or hitting [`FURNACE_DEFAULT_MAX_BOUNCES`]. That
/// makes the bounce cap itself a real, if unsurprising, truncation of the estimator (not a
/// CPU/WGSL twin bug) -- and, because CPU and GPU truncate at slightly different points
/// along the SAME nominally-identical-but-not-bit-identical-after-thousands-of-ops paths,
/// the truncation bias also shows up as a CPU-vs-GPU divergence, not just an
/// undershoot-vs-analytic-target one.
///
/// Measured on this adapter (AMD Radeon(TM) Graphics, Vulkan), `sigma_s=1.2, g=0.4`,
/// 614400 samples/side:
///
/// | `max_bounces` | CPU rel. err | GPU rel. err | CPU-vs-GPU pooled `z` (X,Y,Z) |
/// | --- | --- | --- | --- |
/// | 12 (`FURNACE_DEFAULT_MAX_BOUNCES`) | 0.0402 | 0.0447 | (-10.89, -10.92, -10.80) |
/// | 48 (this constant) | 0.00015 | 0.00003 | (0.28, 0.25, 0.30) |
///
/// Both engines' relative error shrinks by ~270x and the pooled `z` drops from "off the
/// scale" to well inside [`FURNACE_Z_GATE`] as the bounce budget grows -- exactly the
/// signature of bounce-cap truncation, not a remaining energy bug in either twin (a real
/// twin divergence would not shrink like this; see this constant's own doc comment,
/// which is the read-back-later record of that measurement). `48` (4x the default) is
/// itself generous, not the minimum that works -- picked to leave headroom rather than
/// chase the exact convergence knee.
const SCATTERING_FURNACE_MAX_BOUNCES: u32 = 48;

#[must_use]
pub fn run_furnace(ctx: &crate::renderer::gpu::GpuContext) -> FurnaceResult {
    run_furnace_for(ctx, &furnace_material(), &[], FURNACE_DEFAULT_MAX_BOUNCES)
}

/// Same energy-conservation furnace anchor as [`run_furnace`], but with the girdle
/// band bruted.
///
/// GPU mirror of `tests/raytracer_tests.rs`'s
/// `frosted_girdle_white_furnace_energy_conservation_still_holds`:
/// `apply_frosted_bounce`'s `r_unpol`/`1-r_unpol` split carries the same total energy
/// budget as the polished Fresnel formula, just redirected diffusely, so this proves
/// the GPU port is actually energy-conserving, not merely "doesn't crash".
#[must_use]
pub fn run_furnace_frosted_girdle(ctx: &crate::renderer::gpu::GpuContext) -> FurnaceResult {
    let num_planes = round_brilliant_planes().len();
    run_furnace_for(
        ctx,
        &furnace_material(),
        &bruted_girdle_finishes(num_planes),
        FURNACE_DEFAULT_MAX_BOUNCES,
    )
}

/// Same furnace anchor as [`run_furnace`], but with a lossless scattering medium
/// (`sigma_a == 0`, `sigma_s > 0` via [`furnace_material`]).
///
/// GPU mirror of
/// `scattering_tests::lossless_scattering_white_furnace_energy_conservation_holds`:
/// scattering redirects energy via the free-path sampler's albedo/unity-survival
/// identities (`maybe_scatter_or_extinguish`'s doc, hazard 2) but must never create or
/// destroy it, on either engine.
///
/// Uses [`SCATTERING_FURNACE_MAX_BOUNCES`], not [`FURNACE_DEFAULT_MAX_BOUNCES`] -- see
/// that constant's own doc comment (T3) for the measurement establishing why a lossless
/// scattering medium needs the wider bounce budget to converge tightly enough for
/// [`FURNACE_Z_GATE`], on EITHER engine, let alone to agree with each other.
#[must_use]
pub fn run_furnace_scattering(ctx: &crate::renderer::gpu::GpuContext) -> FurnaceResult {
    let material = furnace_material().with_scattering(1.2, 0.4);
    run_furnace_for(ctx, &material, &[], SCATTERING_FURNACE_MAX_BOUNCES)
}

/// Same furnace anchor as [`run_furnace`], but with a nonzero `edge_rounding_radius`.
///
/// GPU mirror of
/// `edge_rounding_tests::edge_rounding_white_furnace_energy_conservation_holds`: edge
/// rounding only perturbs the shading normal fed into the already energy-conserving
/// Fresnel reflect/transmit split, so it must not create or destroy energy either.
#[must_use]
pub fn run_furnace_edge_rounding(ctx: &crate::renderer::gpu::GpuContext) -> FurnaceResult {
    let material = furnace_material().with_edge_rounding(0.03);
    run_furnace_for(ctx, &material, &[], FURNACE_DEFAULT_MAX_BOUNCES)
}

/// Same furnace anchor as [`run_furnace_scattering`] with NEE explicitly enabled.
///
/// Dispatched on the GPU (`env_mode = HDR_MAP`) against a uniform HDR environment map,
/// proving that the GPU's next-event estimation with balance-heuristic MIS preserves energy
/// conservation on scattering media.
///
/// Also uses [`SCATTERING_FURNACE_MAX_BOUNCES`] -- see that constant's own doc comment;
/// the same lossless-medium truncation applies whether or not NEE is enabled, since NEE
/// only adds a direct-light estimate alongside the phase-sampled continuation, it does
/// not change how many bounces that continuation itself needs to converge.
#[must_use]
pub fn run_furnace_nee_equality_scattering(
    ctx: &crate::renderer::gpu::GpuContext,
) -> FurnaceResult {
    let material = furnace_material().with_scattering(1.2, 0.4);
    run_furnace_for_env(ctx, &material, &[], true, SCATTERING_FURNACE_MAX_BOUNCES)
}

/// Same furnace anchor as [`run_furnace_frosted_girdle`] with NEE explicitly enabled.
///
/// Dispatched on the GPU (`env_mode = HDR_MAP`) against a uniform HDR environment map,
/// proving that the GPU's next-event estimation with balance-heuristic MIS preserves energy
/// conservation on frosted-facet boundaries.
#[must_use]
pub fn run_furnace_nee_equality_frosted(ctx: &crate::renderer::gpu::GpuContext) -> FurnaceResult {
    let num_planes = round_brilliant_planes().len();
    run_furnace_for_env(
        ctx,
        &furnace_material(),
        &bruted_girdle_finishes(num_planes),
        true,
        FURNACE_DEFAULT_MAX_BOUNCES,
    )
}

fn run_furnace_for(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
    facet_finishes: &[FacetFinish],
    max_bounces: u32,
) -> FurnaceResult {
    run_furnace_for_env(ctx, material, facet_finishes, false, max_bounces)
}

fn run_furnace_for_env(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
    facet_finishes: &[FacetFinish],
    use_hdr_nee: bool,
    max_bounces: u32,
) -> FurnaceResult {
    let camera = test_camera();
    let (width, height) = (32u32, 32u32);
    let planes = round_brilliant_planes();
    let gpu_material = GpuGemMaterial::encode(material);
    let gpu_finishes = encode_facet_finishes(facet_finishes, planes.len());
    let l0 = 2.5f32;
    let cpu_samples_per_pixel = 600u32;
    let gpu_samples_per_pixel = 600u32;

    let env_map = EnvironmentMap::uniform(4, 4, [l0, l0, l0]);
    let environment = EnvironmentSource::HdrMap(&env_map);
    let scene = CpuScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes,
        material,
        max_bounces,
    };
    let cpu_flat = cpu_samples(width, height, cpu_samples_per_pixel, 0, |pixel, sample| {
        cpu_sample_xyz(&scene, pixel, sample, environment)
    });

    let camera_params = camera_params_for(&camera, width, height, gpu_samples_per_pixel);
    let mode = if use_hdr_nee {
        transport_env_mode::HDR_MAP
    } else {
        transport_env_mode::UNIFORM_FURNACE
    };
    let params = GpuTransportParams::new(
        width * height,
        max_bounces,
        cpu_samples_per_pixel,
        mode,
        l0,
        6500.0,
        1.0,
        1.0,
        0.0,
        0.0,
        [1.0, 1.0, 1.0],
    );
    let total_gpu = (width * height * gpu_samples_per_pixel) as usize;
    let gpu_dispatch = if use_hdr_nee {
        let gpu_hdr = crate::renderer::env_map_gpu::HdrEnvGpuData::upload(&ctx.device, &env_map);
        dispatch_transport_for_class(
            ctx,
            &camera_params,
            &params,
            MaterialForDispatch {
                encoded: &gpu_material,
                class: crate::renderer::gpu::frame::material_class::GENERIC,
            },
            SceneBuffersForDispatch {
                planes: &planes,
                facet_finishes: &gpu_finishes,
                hdr_env: Some(&gpu_hdr),
            },
            total_gpu,
        )
    } else {
        dispatch_transport(
            ctx,
            &camera_params,
            &params,
            &gpu_material,
            &planes,
            &gpu_finishes,
            total_gpu,
        )
    };

    let mut cpu_acc = WelfordXyz::default();
    for v in &cpu_flat {
        cpu_acc.update(*v);
    }
    let mut gpu_acc = WelfordXyz::default();
    for chunk in gpu_dispatch.xyz.as_chunks::<3>().0 {
        gpu_acc.update(Vec3::new(chunk[0], chunk[1], chunk[2]));
    }

    let target = analytic_furnace_target(l0);
    let cpu_mean = cpu_acc.mean();
    let gpu_mean = gpu_acc.mean();

    FurnaceResult {
        analytic_target: target,
        cpu_mean,
        gpu_mean,
        cpu_relative_error: componentwise_relative_error(cpu_mean, target),
        gpu_relative_error: componentwise_relative_error(gpu_mean, target),
        cpu_gpu_z: [
            z_score(&cpu_acc.x, &gpu_acc.x),
            z_score(&cpu_acc.y, &gpu_acc.y),
            z_score(&cpu_acc.z, &gpu_acc.z),
        ],
        total_cpu_samples: cpu_flat.len(),
        total_gpu_samples: gpu_dispatch.xyz.len() / 3,
    }
}

impl FurnaceResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.passed_with_tolerance(FURNACE_CONVERGENCE_TOLERANCE)
    }

    /// Like [`Self::passed`], against an explicit relative-error tolerance; see
    /// [`FROSTED_FURNACE_CONVERGENCE_TOLERANCE`] for why some callers need one wider
    /// than the default. The CPU-vs-GPU z-score gate is unaffected -- always
    /// [`FURNACE_Z_GATE`].
    #[must_use]
    pub fn passed_with_tolerance(&self, relative_error_tolerance: f32) -> bool {
        self.cpu_relative_error <= relative_error_tolerance
            && self.gpu_relative_error <= relative_error_tolerance
            && self.cpu_gpu_z.iter().all(|z| z.abs() <= FURNACE_Z_GATE)
    }

    /// GPU port: [`Self::passed_with_tolerance`] against
    /// [`FROSTED_FURNACE_CONVERGENCE_TOLERANCE`] -- what [`run_furnace_frosted_girdle`]'s
    /// result should be checked with.
    #[must_use]
    pub fn passed_frosted_girdle(&self) -> bool {
        self.passed_with_tolerance(FROSTED_FURNACE_CONVERGENCE_TOLERANCE)
    }

    /// [`Self::passed_with_tolerance`] against
    /// [`SCATTERING_FURNACE_CONVERGENCE_TOLERANCE`] -- what [`run_furnace_scattering`]'s
    /// result should be checked with.
    #[must_use]
    pub fn passed_scattering(&self) -> bool {
        self.passed_with_tolerance(SCATTERING_FURNACE_CONVERGENCE_TOLERANCE)
    }

    /// [`Self::passed_with_tolerance`] against
    /// [`EDGE_ROUNDING_FURNACE_CONVERGENCE_TOLERANCE`] -- what
    /// [`run_furnace_edge_rounding`]'s result should be checked with.
    #[must_use]
    pub fn passed_edge_rounding(&self) -> bool {
        self.passed_with_tolerance(EDGE_ROUNDING_FURNACE_CONVERGENCE_TOLERANCE)
    }
}

/// Analytic furnace target: `EnvironmentMap::uniform(_, _, [l0,l0,l0])`'s spectral
/// reconstruction (`rgb_to_spectral_radiance`), integrated against the CIE 1931 CMF
/// over 380..=780nm at 1nm steps (same quadrature convention as
/// `compute_illuminant_white_balance`). With environment radiance independent of
/// direction, every unbiased path has expectation exactly this value regardless of
/// pixel or bounce count -- see this module's doc comment.
fn analytic_furnace_target(l0: f32) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        let spec = rgb_to_spectral_radiance([l0, l0, l0], lambda);
        sum += Vec3::from_array(cie_1931_cmf(lambda)) * spec;
    }
    sum / 106.856
}

fn componentwise_relative_error(value: Vec3, target: Vec3) -> f32 {
    let dx = (value.x - target.x).abs() / target.x.abs().max(1e-6);
    let dy = (value.y - target.y).abs() / target.y.abs().max(1e-6);
    let dz = (value.z - target.z).abs() / target.z.abs().max(1e-6);
    dx.max(dy).max(dz)
}
