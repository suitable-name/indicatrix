//! End-to-end spectral estimator self-tests -- driven by
//! `shaders/spectral_transport.wgsl`'s `transport_main` entry point.
//!
//! - [`run_determinism`]: two dispatches against identical input, byte-for-byte.
//! - [`run_furnace`]: energy-conservation furnace anchor -- a colourless, non-dispersive,
//!   non-absorbing cubic gem (Standard Round Brilliant) inside a uniform environment
//!   must return exactly that uniform radiance in expectation, exercising
//!   Fresnel/TIR/Russian-roulette/spectral-MIS against a TRUTH anchor (not just
//!   CPU-vs-GPU).
//! - [`run_image_comparison`]: Tier 3 statistical image equivalence -- Welford mean/M2
//!   on CPU and GPU DISJOINT sample ranges, per-pixel z-score, and connected-component
//!   clustering of failing pixels -- against a dispersive, absorbing gem (Spinel).
//! - [`run_spectral_debug`]: see its own doc comment for the scope limit given the CPU
//!   visibility-only constraint.
//!
//! Split into one file per topic (see each submodule's doc comment); this file holds
//! what's shared: test scenes/materials, GPU dispatch, CPU reference sampling, and
//! Welford/z-score statistics.
//!
//! Per-channel exit-transmission primitives get their own kernel-level Tier 2 ULP
//! checks in `renderer::gpu::transport_check::p6_exit_splitting`, not here.

use crate::{
    geometry::{
        GpuFacetPlane,
        cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts},
    },
    optics::{
        absorption::AbsorptionTensor,
        dispersion::DispersionModel,
        materials::{CrystalSystem, GemMaterial, OpticalCharacter},
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, HERO_WAVELENGTH_ROTATION_STREAM,
            PIXEL_JITTER_X_ROTATION_STREAM, PIXEL_JITTER_Y_ROTATION_STREAM,
            cranley_patterson_rotate, hash_u32, low_discrepancy_base2, radical_inverse_base,
            trace_spectral_ray_with_finish,
        },
    },
    renderer::{
        buffers::{GpuCameraParams, GpuGemMaterial, GpuTransportParams, encode_facet_finishes},
        gpu::compute,
    },
};
use glam::Vec3;

mod determinism;
mod furnace;
mod image_comparison;
mod spectral_debug;

pub use determinism::{DeterminismResult, run_determinism};
pub use furnace::{
    FurnaceResult, run_furnace, run_furnace_edge_rounding, run_furnace_frosted_girdle,
    run_furnace_nee_equality_frosted, run_furnace_nee_equality_scattering, run_furnace_scattering,
};
pub use image_comparison::{
    ImageComparisonResult, run_image_comparison, run_image_comparison_absorption_path_scale,
    run_image_comparison_alexandrite, run_image_comparison_biaxial_scattering,
    run_image_comparison_daylight_dome, run_image_comparison_edge_rounding,
    run_image_comparison_frosted_girdle, run_image_comparison_hdr_nee,
    run_image_comparison_iso_hemisphere, run_image_comparison_quartz, run_image_comparison_rutile,
    run_image_comparison_scattering, run_image_comparison_soft_dome,
    run_image_comparison_synthetic_moissanite, run_image_comparison_tanzanite,
    run_image_comparison_topaz, run_image_comparison_tourmaline, run_image_comparison_zircon,
    run_specialisation_image_comparison,
};

pub use spectral_debug::{SpectralDebugResult, run_spectral_debug};

// `spectral_transport.wgsl` alone isn't valid WGSL: it assumes
// `shaders/transport_physics.wgsl`'s functions are already in scope. `build.rs`
// concatenates the two into `$OUT_DIR/spectral_transport.generated.wgsl`, compiled here
// via `frame::SHADER_SRC` (re-exported, not `include_str!`d again) so this check and the
// renderer provably compile the same source text.
use crate::renderer::gpu::frame::{self, SHADER_SRC};

// ---------------------------------------------------------------------------------
// Test scenes.
// ---------------------------------------------------------------------------------

/// The furnace anchor's gem: colourless (empty band set), non-dispersive
/// (`Cauchy { a, b: 0, c: 0 }` reduces to bare `a`), cubic (`birefringence_delta:
/// 0.0`).
///
/// `n = 1.5` is well above 1.0 (Fresnel reflection/TIR genuinely exercised)
/// and below any real gem's critical-angle edge cases (covered by this module's
/// Tier 2 sibling).
#[must_use]
pub fn furnace_material() -> GemMaterial {
    GemMaterial {
        name: "Phase2 furnace anchor (colourless n=1.5 cubic)".to_string(),
        crystal_system: CrystalSystem::Cubic,
        optical_character: OpticalCharacter::Isotropic,
        dispersion: DispersionModel::Cauchy {
            a: 1.5,
            b: 0.0,
            c: 0.0,
        },
        birefringence_delta: 0.0,
        absorption: AbsorptionTensor::isotropic(vec![]),
        c_axis: Vec3::Y,
        biaxial_delta_beta_alpha: None,
        scattering_sigma_s: 0.0,
        scattering_g: 0.0,
        edge_rounding_radius: 0.0,
        absorption_path_scale: 1.0,
        uniaxial_extraordinary_dispersion: None,
    }
}

/// Tier 3's gem: the built-in "Spinel" (cubic, genuinely dispersive Sellmeier3,
/// genuinely absorbing via `legacy_rgb_bands`).
///
/// Exercises dispersion-driven spectral MIS and pleochroic absorption together, not
/// just the furnace's degenerate case.
///
/// # Panics
///
/// If `"Spinel"` is ever removed from `GemMaterial::all_materials()` -- self-test
/// scaffolding, not a real caller path.
#[must_use]
pub fn tier3_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Spinel")
        .expect("\"Spinel\" is a built-in cubic material in GemMaterial::all_materials()")
}

#[must_use]
pub fn test_camera() -> Camera {
    // distance=5.0 against the ~1-unit SRB (girdle radius ~1.0, table at y=0.32);
    // narrow fov so most rays hit the gem, not just the miss branch.
    Camera::new(0.35, 0.28, 5.0, 18.0)
}

fn round_brilliant_planes() -> Vec<GpuFacetPlane> {
    StandardGemCuts::standard_round_brilliant()
}

/// All-`Polished` `facet_finish::*` buffer sized to `planes.len()`, so the
/// megakernel's `FACET_FINISH_FROSTED` branch is never taken.
#[must_use]
fn all_polished_finishes(num_planes: usize) -> Vec<u32> {
    encode_facet_finishes(&[], num_planes)
}

/// `FacetFinish::Polished` everywhere except the girdle band
/// (`STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS`), which is `Frosted` -- the source of truth
/// this module's frosted GPU self-tests build both their CPU reference and
/// GPU-encoded buffer from.
#[must_use]
fn bruted_girdle_finishes(num_planes: usize) -> Vec<FacetFinish> {
    let mut finishes = vec![FacetFinish::Polished; num_planes];
    for i in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
        finishes[i] = FacetFinish::Frosted;
    }
    finishes
}

/// Zircon: largest birefringence in the built-in set (`birefringence_delta =
/// +0.0590`), positive uniaxial.
///
/// Exercises `theta_c` iteration, the ordinary/extraordinary eigenmode split, and
/// `extraordinary_poynting_dir`'s walk-off at the strongest birefringence this crate
/// ships.
///
/// # Panics
///
/// If `"Zircon"` is ever removed from `GemMaterial::all_materials()` -- see
/// [`tier3_material`] for the scaffolding rationale.
#[must_use]
pub fn zircon_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Zircon")
        .expect("\"Zircon\" is a built-in uniaxial material in GemMaterial::all_materials()")
}

/// Quartz: the only built-in with a genuine, independent extraordinary-ray dispersion
/// curve (`uniaxial_extraordinary_dispersion.is_some()`, Ghosh 1999 -- see that
/// field's doc).
///
/// Exercises `extraordinary_index_at`'s genuine-curve branch end to end, unlike every
/// other uniaxial built-in which only hits the constant-offset fallback.
///
/// # Panics
///
/// If `"Quartz"` is ever removed from `GemMaterial::all_materials()` -- see
/// [`tier3_material`] for the scaffolding rationale.
#[must_use]
pub fn quartz_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Quartz")
        .expect("\"Quartz\" is a built-in uniaxial material in GemMaterial::all_materials()")
}

/// Tourmaline: strongly negative uniaxial (`birefringence_delta = -0.0210`), opposite
/// Zircon's sign.
///
/// The two together exercise both branches of `effective_extraordinary_index`'s
/// `n_e > n_o` / `n_e < n_o` behaviour.
///
/// # Panics
///
/// If `"Tourmaline"` is ever removed from `GemMaterial::all_materials()` -- see
/// [`tier3_material`] for the scaffolding rationale.
#[must_use]
pub fn tourmaline_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Tourmaline")
        .expect("\"Tourmaline\" is a built-in uniaxial material in GemMaterial::all_materials()")
}

/// Rutile (full uniaxial Fresnel, Lekner 1991): this crate's most extreme
/// birefringence built-in (`birefringence_delta = +0.287`).
///
/// Its closed-form Fresnel reflectance diverges from the interim single-effective-index
/// approximation by ~15% (see
/// `uniaxial_fresnel::tests::rutile_fresnel_diverges_from_isotropic_effective_index_approximation_by_about_15_percent`).
/// A CPU/GPU image comparison here is the strongest Tier 3 check of the closed-form
/// solve, catching divergence before weaker-birefringence materials would.
///
/// # Panics
///
/// If `"Rutile"` is ever removed from `GemMaterial::all_materials()` -- see
/// [`tier3_material`] for the scaffolding rationale.
#[must_use]
pub fn rutile_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Rutile")
        .expect("\"Rutile\" is a built-in uniaxial material in GemMaterial::all_materials()")
}

/// Alexandrite: genuinely biaxial with a full three-band-set (`beta_ray`) absorption
/// tensor.
///
/// Exercises `biaxial_wave_indices`/`biaxial_eigen_polarizations`/
/// `biaxial_mode_poynting_dir`/`biaxial_resolve_entry_mode` end to end, plus
/// `pleochroic_channel_alpha_biaxial`'s three-coefficient absorption path.
///
/// # Panics
///
/// If `"Alexandrite"` is ever removed from `GemMaterial::all_materials()` -- see
/// [`tier3_material`] for the scaffolding rationale.
#[must_use]
pub fn alexandrite_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Alexandrite")
        .expect("\"Alexandrite\" is a built-in biaxial material in GemMaterial::all_materials()")
}

/// Topaz: genuinely biaxial (three distinct principal indices) but still using the
/// two-band-set (`o_ray`/`e_ray`) absorption approximation (`beta_ray = None`).
///
/// Exercises the biaxial index path (mode-A/mode-B, walk-off) feeding the uniaxial
/// two-coefficient `pleochroic_channel_alpha`, the other `is_biaxial && has_beta_ray`
/// combination the megakernel must get right.
///
/// # Panics
///
/// If `"Topaz"` is ever removed from `GemMaterial::all_materials()` -- see
/// [`tier3_material`] for the scaffolding rationale.
#[must_use]
pub fn topaz_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Topaz")
        .expect("\"Topaz\" is a built-in biaxial material in GemMaterial::all_materials()")
}

/// Tanzanite: genuinely biaxial with its own three-band-set absorption tensor
/// (distinct values and sign/magnitude from Alexandrite).
///
/// A second trichroic material exercising the same code path with different data.
///
/// # Panics
///
/// If `"Tanzanite"` is ever removed from `GemMaterial::all_materials()` -- see
/// [`tier3_material`] for the scaffolding rationale.
#[must_use]
pub fn tanzanite_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Tanzanite")
        .expect("\"Tanzanite\" is a built-in biaxial material in GemMaterial::all_materials()")
}

// ---------------------------------------------------------------------------------
// Shared GPU dispatch.
// ---------------------------------------------------------------------------------

struct TransportDispatch {
    xyz: Vec<f32>,
    radiance: Vec<f32>,
    lambdas: Vec<f32>,
    path_pdf: Vec<f32>,
    /// `out_compat`'s readback: one `compat[k]` mask per channel, same indexing as
    /// `radiance`/`lambdas`/`path_pdf` above. See `spectral_debug::run_spectral_debug`.
    compat: Vec<u32>,
}

/// Dispatches `transport_main` once over `total_tuples` (pixel, sample) threads and
/// reads back all four output buffers (final XYZ plus pre-integration per-channel debug
/// arrays -- see `spectral_transport.wgsl`'s doc for why one entry point writes all four).
///
/// Always the GENERIC (`MATERIAL_CLASS = 0`) pipeline; see
/// [`dispatch_transport_for_class`] for the specialised-pipeline variant Tier 3's image
/// comparisons use. Every other caller here deliberately exercises the general
/// runtime-dispatch kernel, not a specialised pipeline.
#[derive(Clone, Copy)]
struct SceneBuffersForDispatch<'a> {
    planes: &'a [GpuFacetPlane],
    facet_finishes: &'a [u32],
    hdr_env: Option<&'a crate::renderer::env_map_gpu::HdrEnvGpuData>,
}

fn dispatch_transport(
    ctx: &crate::renderer::gpu::GpuContext,
    camera_params: &GpuCameraParams,
    params: &GpuTransportParams,
    material: &GpuGemMaterial,
    planes: &[GpuFacetPlane],
    facet_finishes: &[u32],
    total_tuples: usize,
) -> TransportDispatch {
    dispatch_transport_for_class(
        ctx,
        camera_params,
        params,
        MaterialForDispatch {
            encoded: material,
            class: frame::material_class::GENERIC,
        },
        SceneBuffersForDispatch {
            planes,
            facet_finishes,
            hdr_env: None,
        },
        total_tuples,
    )
}

/// Bundles an encoded material with the `MATERIAL_CLASS` pipeline-overridable constant
/// its dispatch should use, keeping [`dispatch_transport_for_class`]'s argument count
/// within clippy's `too_many_arguments` limit.
///
/// `Copy`: a reference plus a `u32`, so passing by value is as cheap as a reference to
/// it (clippy's own suggested fix), and reads as "here is the material to dispatch".
#[derive(Clone, Copy)]
struct MaterialForDispatch<'a> {
    encoded: &'a GpuGemMaterial,
    class: u32,
}

/// Like [`dispatch_transport`], but compiles the pipeline with `MATERIAL_CLASS` fixed to
/// `material.class` rather than the GENERIC default -- [`dispatch_transport`] is this
/// function called with `frame::material_class::GENERIC`.
///
/// [`image_comparison::run_image_comparison_for`] is the only caller with a non-GENERIC
/// class: routing Tier 3's comparisons through the same specialised pipeline
/// `GpuFrameRenderer::accumulate` would pick makes those checks exercise the production
/// path, not a lookalike GENERIC dispatch.
fn dispatch_transport_for_class(
    ctx: &crate::renderer::gpu::GpuContext,
    camera_params: &GpuCameraParams,
    params: &GpuTransportParams,
    material: MaterialForDispatch<'_>,
    scene: SceneBuffersForDispatch<'_>,
    total_tuples: usize,
) -> TransportDispatch {
    let pipeline = compute::create_compute_pipeline_with_constants(
        &ctx.device,
        "transport_main",
        SHADER_SRC,
        "transport_main",
        &[("MATERIAL_CLASS", f64::from(material.class))],
    );
    let material = material.encoded;
    let outputs = frame::TransportOutputs::new(&ctx.device, total_tuples);
    let dummy_hdr = scene
        .hdr_env
        .is_none()
        .then(|| crate::renderer::env_map_gpu::HdrEnvGpuData::dummy(&ctx.device));
    let hdr_env = scene
        .hdr_env
        .or(dummy_hdr.as_ref())
        .expect("dummy HDR allocated when scene.hdr_env is None");
    frame::encode_and_dispatch(
        &frame::TransportDispatchArgs {
            ctx,
            pipeline: &pipeline,
            camera_params,
            params,
            material,
            planes: scene.planes,
            facet_finishes: scene.facet_finishes,
            outputs: &outputs,
            hdr_env,
        },
        total_tuples,
    );
    TransportDispatch {
        xyz: compute::readback(&ctx.device, &ctx.queue, outputs.xyz(), total_tuples * 3),
        radiance: compute::readback(
            &ctx.device,
            &ctx.queue,
            outputs.radiance(),
            total_tuples * 8,
        ),
        lambdas: compute::readback(&ctx.device, &ctx.queue, outputs.lambdas(), total_tuples * 8),
        path_pdf: compute::readback(
            &ctx.device,
            &ctx.queue,
            outputs.path_pdf(),
            total_tuples * 8,
        ),
        compat: compute::readback(&ctx.device, &ctx.queue, outputs.compat(), total_tuples * 8),
    }
}

const fn camera_params_for(
    camera: &Camera,
    width: u32,
    height: u32,
    samples: u32,
) -> GpuCameraParams {
    GpuCameraParams {
        origin: camera.origin.to_array(),
        fov_tan: camera.fov_tan,
        forward: camera.forward.to_array(),
        width: width as f32,
        right: camera.right.to_array(),
        height: height as f32,
        up: camera.up.to_array(),
        num_samples: samples,
    }
}

// ---------------------------------------------------------------------------------
// CPU reference sample generation: mirrors `render_core.rs`'s seed/jitter/hero-
// wavelength derivation (stratified, not plain-uniform), calling the real
// `trace_spectral_ray` directly.
// ---------------------------------------------------------------------------------

/// Bundles the fixed-for-a-run scene inputs [`cpu_sample_xyz`] needs, keeping its
/// argument count within clippy's `too_many_arguments` limit.
#[derive(Clone, Copy)]
struct CpuScene<'a> {
    camera: &'a Camera,
    width: u32,
    height: u32,
    planes: &'a [GpuFacetPlane],
    /// `&[]` is exactly equivalent to the all-`Polished` behavior (see
    /// [`trace_spectral_ray_with_finish`]'s doc), so routing every CPU reference sample
    /// through this unconditionally is a no-op except for the frosted girdle checks.
    facet_finishes: &'a [FacetFinish],
    material: &'a GemMaterial,
    max_bounces: u32,
}

fn cpu_sample_xyz(
    scene: &CpuScene<'_>,
    pixel: u32,
    sample_num: u32,
    environment: EnvironmentSource<'_>,
) -> Vec3 {
    let CpuScene {
        camera,
        width,
        height,
        planes,
        facet_finishes,
        material,
        max_bounces,
    } = *scene;
    let x = pixel % width;
    let y = pixel / width;
    let seed = hash_u32(pixel.wrapping_mul(0x9e37_79b9) ^ sample_num.wrapping_mul(0x85eb_ca6b));

    // Stratified pixel jitter and hero wavelength; mirrors
    // `render_core.rs::trace_samples`.
    let rot_jx = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM));
    let rot_jy = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM));
    let rot_hero = low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM));
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample_num, 3), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample_num, 5), rot_hero);

    let ray = camera.generate_ray(x as f32, y as f32, width as f32, height as f32, jx, jy);
    trace_spectral_ray_with_finish(
        ray,
        planes,
        facet_finishes,
        material,
        max_bounces,
        environment,
        seed,
        hero_rand,
        None,
    )
}

/// Threaded CPU reference: `num_pixels * samples_per_pixel` samples, pixel-major order
/// (matching the GPU output buffer's `idx = pixel * samples_per_dispatch + local_sample`
/// layout), sample indices `[sample_offset, sample_offset + samples_per_pixel)`.
fn cpu_samples<F>(
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    sample_offset: u32,
    trace_one: F,
) -> Vec<Vec3>
where
    F: Fn(u32, u32) -> Vec3 + Sync,
{
    let num_pixels = width * height;
    let num_threads = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(16);
    let mut out = vec![Vec3::ZERO; (num_pixels as usize) * (samples_per_pixel as usize)];
    let rows_per_chunk = (num_pixels as usize).div_ceil(num_threads);
    std::thread::scope(|s| {
        let chunks: Vec<&mut [Vec3]> = out
            .chunks_mut(rows_per_chunk * samples_per_pixel as usize)
            .collect();
        for (chunk_idx, chunk) in chunks.into_iter().enumerate() {
            let start_pixel = (chunk_idx * rows_per_chunk) as u32;
            let trace_one = &trace_one;
            s.spawn(move || {
                for (local_pixel, pixel_slot) in
                    chunk.chunks_mut(samples_per_pixel as usize).enumerate()
                {
                    let pixel = start_pixel + local_pixel as u32;
                    if pixel >= num_pixels {
                        break;
                    }
                    for (local_sample, slot) in pixel_slot.iter_mut().enumerate() {
                        let sample_num = sample_offset + local_sample as u32;
                        *slot = trace_one(pixel, sample_num);
                    }
                }
            });
        }
    });
    out
}

// ---------------------------------------------------------------------------------
// Welford online mean/M2 in f64 for accumulator stability (analysis tool applied
// after the f32 estimator produced its samples).
// ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
struct Welford {
    n: u64,
    mean: f64,
    m2: f64,
}

impl Welford {
    fn update(&mut self, x: f64) {
        self.n += 1;
        let delta = x - self.mean;
        self.mean += delta / self.n as f64;
        let delta2 = x - self.mean;
        self.m2 = delta.mul_add(delta2, self.m2);
    }

    fn variance(&self) -> f64 {
        if self.n < 2 {
            0.0
        } else {
            self.m2 / (self.n as f64 - 1.0)
        }
    }

    fn standard_error_sq(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.variance() / self.n as f64
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct WelfordXyz {
    x: Welford,
    y: Welford,
    z: Welford,
}

impl WelfordXyz {
    fn update(&mut self, v: Vec3) {
        self.x.update(f64::from(v.x));
        self.y.update(f64::from(v.y));
        self.z.update(f64::from(v.z));
    }

    const fn mean(&self) -> Vec3 {
        Vec3::new(self.x.mean as f32, self.y.mean as f32, self.z.mean as f32)
    }
}

fn z_score(cpu: &Welford, gpu: &Welford) -> f64 {
    let se2 = cpu.standard_error_sq() + gpu.standard_error_sq();
    if se2 <= 0.0 {
        return 0.0;
    }
    (gpu.mean - cpu.mean) / se2.sqrt()
}
