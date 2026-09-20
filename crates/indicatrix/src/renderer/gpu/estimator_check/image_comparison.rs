//! Tier 3: statistical image equivalence -- Welford mean/M2 on CPU and GPU DISJOINT
//! sample ranges (as production renders would split work), per-pixel z-score, and
//! connected-component clustering of failing pixels (a structured bias clusters; noise
//! salts-and-peppers).

use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GEMRAY_GREY, FacetFinish, LightingPreset,
            environment::environment_white_balance, illuminant_temperature_k,
        },
    },
    renderer::{
        buffers::{GpuGemMaterial, GpuTransportParams, encode_facet_finishes, transport_env_mode},
        env_map::EnvironmentMap,
        env_map_gpu::HdrEnvGpuData,
    },
};
use glam::Vec3;

use super::{
    CpuScene, MaterialForDispatch, SceneBuffersForDispatch, Welford, alexandrite_material,
    bruted_girdle_finishes, camera_params_for, cpu_sample_xyz, cpu_samples,
    dispatch_transport_for_class, quartz_material, round_brilliant_planes, rutile_material,
    tanzanite_material, test_camera, tier3_material, topaz_material, tourmaline_material, z_score,
    zircon_material,
};

use crate::renderer::gpu::frame::{self, classify_material};

#[derive(Debug, Clone)]
pub struct ImageComparisonResult {
    pub width: u32,
    pub height: u32,
    pub cpu_samples_per_pixel: u32,
    pub gpu_samples_per_pixel: u32,
    /// Image-aggregate mean z (averaged over every pixel's per-pixel z, luminance
    /// channel).
    pub mean_z: f64,
    pub over_3_sigma_count: usize,
    pub total_pixels: usize,
    /// Binomial expectation for `|z| > 3` under the null (two-sided): ~0.27%.
    pub over_3_sigma_expected: f64,
    /// Sizes of every connected component (4-connectivity) of `|z| > 3` pixels,
    /// largest first.
    pub cluster_sizes: Vec<usize>,
    pub max_abs_z: f64,
    pub max_abs_z_pixel: (u32, u32),
}

impl ImageComparisonResult {
    /// Passes if the observed `|z|>3` rate is within a generous multiple of its
    /// binomial expectation AND no cluster is larger than a handful of pixels --
    /// see this module's doc comment: a structured bias clusters, noise does not.
    #[must_use]
    pub fn passed(&self) -> bool {
        let expected_count = self.over_3_sigma_expected * self.total_pixels as f64;
        let observed_ok = (self.over_3_sigma_count as f64) <= (expected_count * 5.0).max(10.0);
        let largest = self.cluster_sizes.first().copied().unwrap_or(0);
        observed_ok && largest <= 6
    }
}

#[must_use]
pub fn run_image_comparison(ctx: &crate::renderer::gpu::GpuContext) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &tier3_material(), &[], LightingPreset::Daylight)
}

#[must_use]
pub fn run_image_comparison_zircon(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &zircon_material(), &[], LightingPreset::Daylight)
}

#[must_use]
pub fn run_image_comparison_tourmaline(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &tourmaline_material(), &[], LightingPreset::Daylight)
}

/// Quartz: the one built-in whose extraordinary ray disperses with a genuine
/// independent curve (`uniaxial_extraordinary_dispersion.is_some()`).
///
/// Rather than the constant-offset approximation the other uniaxial materials here use,
/// this exercises `has_extraordinary_dispersion != 0` through the real megakernel
/// dispatch -- see [`super::quartz_material`]'s doc comment.
#[must_use]
pub fn run_image_comparison_quartz(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &quartz_material(), &[], LightingPreset::Daylight)
}

/// Rutile: full uniaxial Fresnel (Lekner 1991), Tier 3.
///
/// See [`super::rutile_material`]'s own doc comment for why this is the strongest
/// single end-to-end CPU-vs-GPU check the closed-form uniaxial solve's WGSL mirror has.
#[must_use]
pub fn run_image_comparison_rutile(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &rutile_material(), &[], LightingPreset::Daylight)
}

/// Exit-event spectral splitting, Tier 3: Synthetic Moissanite.
///
/// The strongest single end-to-end CPU-vs-GPU check the WGSL exit-event splitting port
/// has: a real 57-facet Standard Round Brilliant with every facet polished, so refractive
/// non-TIR-only paths dominate -- exactly where a companion channel's own Snell
/// direction is most likely to diverge from the hero's. Synthetic Moissanite's
/// `birefringence_delta = +0.0415` combined with its high Abbe dispersion makes that
/// divergence large and common, under the analytic studio lighting rig every other
/// Tier 3 scene in this module uses.
///
/// # Panics
///
/// Panics if `"Synthetic Moissanite"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_synthetic_moissanite(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Synthetic Moissanite")
        .expect("\"Synthetic Moissanite\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

#[must_use]
pub fn run_image_comparison_alexandrite(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &alexandrite_material(), &[], LightingPreset::Daylight)
}

#[must_use]
pub fn run_image_comparison_topaz(ctx: &crate::renderer::gpu::GpuContext) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &topaz_material(), &[], LightingPreset::Daylight)
}

#[must_use]
pub fn run_image_comparison_tanzanite(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &tanzanite_material(), &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on a scene with a frosted girdle.
///
/// GPU mirror of `tests/raytracer_tests.rs`'s
/// `frosted_girdle_changes_face_up_appearance_measurably`, compared statistically
/// (variance-scaled z-score, connected-component clustering) rather than against a
/// polished CPU baseline. Diamond, girdle band bruted via [`bruted_girdle_finishes`].
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_frosted_girdle(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    let num_planes = round_brilliant_planes().len();
    let finishes = bruted_girdle_finishes(num_planes);
    run_image_comparison_for(ctx, &material, &finishes, LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on a scene with
/// Henyey-Greenstein scattering enabled.
///
/// A forward-biased (`g = 0.3`) scattering medium (`sigma_s = 1.5`) layered on built-in
/// Ruby (chromatic absorption AND scattering both active, exercising the per-channel
/// `sigma_t_k` differentiation hazard 2 requires) -- the GPU mirror of
/// `optics::raytracer::scattering_tests::scattering_measurably_changes_face_up_appearance`'s
/// scene, compared statistically rather than against a clear-stone CPU baseline.
///
/// # Panics
///
/// Panics if `"Ruby"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_scattering(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()")
        .with_scattering(1.5, 0.3);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison covering the inclusion-scattering block's
/// `is_biaxial` branching.
///
/// `spectral_transport.wgsl`'s scatter block mirrors the absorption block below it: a
/// biaxial material's scattering alphas come from `biaxial_eigen_polarizations`/
/// `pleochroic_channel_alpha_biaxial`, not the uniaxial ordinary/extraordinary
/// approximation `channel_absorption_alphas` would pick. Alexandrite
/// ([`super::alexandrite_material`], biaxial trichroic with a populated `beta_ray`
/// absorption band) with scattering enabled -- the same combination
/// [`run_image_comparison_scattering`] exercises for a uniaxial material (Ruby), but
/// biaxial so the `is_biaxial && has_beta_ray` path is actually reached.
#[must_use]
pub fn run_image_comparison_biaxial_scattering(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = super::alexandrite_material().with_scattering(1.5, 0.3);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on
/// a scene with a nonzero `edge_rounding_radius`.
///
/// The GPU mirror of
/// `optics::raytracer::edge_rounding_tests::edge_rounding_measurably_changes_face_up_appearance`'s
/// scene, but compared statistically against the GPU rather than against a sharp-edge
/// baseline on the CPU alone.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_edge_rounding(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()")
        .with_edge_rounding(0.02);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on a
/// coloured (chromatically absorbing) stone at a non-1.0 `absorption_path_scale`.
///
/// Ruby (built-in chromatic Beer-Lambert absorption, exactly what
/// `absorption_path_scale` multiplies the path length before) at `scale = 3.0` --
/// large enough that a scale/no-scale mismatch between the CPU and WGSL mirrors would
/// show up as a strong, whole-image colour/brightness bias, not just isolated pixels.
///
/// # Panics
///
/// Panics if `"Ruby"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_absorption_path_scale(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()")
        .with_absorption_path_scale(3.0);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison under the ISO hemisphere lighting model.
///
/// Diamond on the standard 57-facet Round Brilliant.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_iso_hemisphere(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::IsoHemisphere,
        BACKDROP_GEMRAY_GREY,
    )
}

/// Tier 3 statistical image comparison under the Light tent + black cards model.
///
/// Diamond on the standard 57-facet Round Brilliant.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_light_tent(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::LightTent,
        BACKDROP_GEMRAY_GREY,
    )
}

/// Tier 3 statistical image comparison under the Daylight sky + sun model.
///
/// Diamond on the standard 57-facet Round Brilliant.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_daylight_dome(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::DaylightDome,
        BACKDROP_GEMRAY_GREY,
    )
}

/// Shared Tier 3 statistical image comparison body -- parameterized on `material` so the
/// isotropic (Spinel, [`run_image_comparison`]) and uniaxial-birefringent (Zircon,
/// Tourmaline, [`run_image_comparison_zircon`]/[`run_image_comparison_tourmaline`])
/// instances share one implementation rather than three independent copies. GPU
/// port added `facet_finishes` (`&[]` for every caller that doesn't need a frosted
/// finish, exactly equivalent to all-`Polished` -- see
/// [`super::CpuScene::facet_finishes`]'s doc comment) so
/// [`run_image_comparison_frosted_girdle`] can reuse this same body too.
fn run_image_comparison_for(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
    facet_finishes: &[FacetFinish],
    preset: LightingPreset,
) -> ImageComparisonResult {
    run_image_comparison_with_backdrop(ctx, material, facet_finishes, preset, 0.0)
}

/// [`run_image_comparison_for`] with a backdrop card behind the stone (see
/// `EnvironmentSource::Studio::backdrop`), so the camera-ray miss branch is compared
/// too; `0.0` is the plain environment.
fn run_image_comparison_with_backdrop(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
    facet_finishes: &[FacetFinish],
    preset: LightingPreset,
    backdrop: f32,
) -> ImageComparisonResult {
    let camera = test_camera();
    let (width, height) = (48u32, 48u32);
    let planes = round_brilliant_planes();
    let gpu_material = GpuGemMaterial::encode(material);
    let gpu_finishes = encode_facet_finishes(facet_finishes, planes.len());
    let max_bounces = 10u32;
    let (exposure, light_yaw, light_pitch) = (1.0f32, 0.4f32, 0.35f32);
    let temp_k = illuminant_temperature_k(preset);
    let environment = preset
        .studio(exposure, light_yaw, light_pitch)
        .with_backdrop(backdrop);
    let wb = environment_white_balance(environment);

    let cpu_samples_per_pixel = 500u32;
    let gpu_samples_per_pixel = 500u32;

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
    let params = GpuTransportParams::new(
        width * height,
        max_bounces,
        cpu_samples_per_pixel,
        transport_env_mode::STUDIO_RIG,
        0.0,
        temp_k,
        preset.params().spot_mult,
        exposure,
        light_yaw,
        light_pitch,
        wb.to_array(),
    )
    // The CPU side (`cpu_sample_xyz` above, via `sample_studio_environment_with_rig`)
    // already routes presets through uses_d65() / model() -- the GPU
    // dispatch must match or this comparison reintroduces CPU/GPU divergence.
    .with_studio_use_d65(preset.uses_d65())
    .with_studio_model(preset.model().gpu_id())
    .with_backdrop(backdrop);
    let total_gpu = (width * height * gpu_samples_per_pixel) as usize;
    // Dispatch through whichever pipeline `GpuFrameRenderer::accumulate` would actually
    // pick for `material` (via `frame::classify_material`), not the GENERIC one every
    // other check in this module uses -- see `dispatch_transport_for_class`'s doc
    // comment for why Tier 3's image comparisons need the production path.
    let gpu_dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: classify_material(material),
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &gpu_finishes,
            hdr_env: None,
        },
        total_gpu,
    );

    compute_image_comparison_result(
        width,
        height,
        cpu_samples_per_pixel,
        gpu_samples_per_pixel,
        &cpu_flat,
        &gpu_dispatch.xyz,
    )
}

fn synthetic_image_comparison_hdr_map() -> EnvironmentMap {
    let width = 16;
    let height = 8;
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

/// Tier 3 statistical image comparison on a scene under a non-uniform HDR environment
/// map with next-event estimation (NEE) and balance-heuristic MIS active on both CPU and GPU.
///
/// Exercises light importance sampling, Henyey-Greenstein shadow ray probing, exit
/// transmittance, and balance heuristic MIS.
///
/// # Panics
///
/// Panics if `"Ruby"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_hdr_nee(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()")
        .with_scattering(1.0, 0.3);
    let camera = test_camera();
    let (width, height) = (48u32, 48u32);
    let planes = round_brilliant_planes();
    let gpu_material = GpuGemMaterial::encode(&material);
    let gpu_finishes = encode_facet_finishes(&[], planes.len());
    let max_bounces = 10u32;

    let env_map = synthetic_image_comparison_hdr_map();
    let environment = crate::optics::raytracer::EnvironmentSource::HdrMap(&env_map);
    let scene = CpuScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces,
    };
    let cpu_samples_per_pixel = 500u32;
    let gpu_samples_per_pixel = 500u32;

    let cpu_flat = cpu_samples(width, height, cpu_samples_per_pixel, 0, |pixel, sample| {
        cpu_sample_xyz(&scene, pixel, sample, environment)
    });

    let camera_params = camera_params_for(&camera, width, height, gpu_samples_per_pixel);
    let params = GpuTransportParams::new(
        width * height,
        max_bounces,
        cpu_samples_per_pixel,
        transport_env_mode::HDR_MAP,
        1.0,
        6500.0,
        1.0,
        1.0,
        0.0,
        0.0,
        [1.0, 1.0, 1.0],
    );
    let total_gpu = (width * height * gpu_samples_per_pixel) as usize;
    let gpu_hdr = HdrEnvGpuData::upload(&ctx.device, &env_map);
    let gpu_dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: classify_material(&material),
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &gpu_finishes,
            hdr_env: Some(&gpu_hdr),
        },
        total_gpu,
    );

    compute_image_comparison_result(
        width,
        height,
        cpu_samples_per_pixel,
        gpu_samples_per_pixel,
        &cpu_flat,
        &gpu_dispatch.xyz,
    )
}

fn compute_image_comparison_result(
    width: u32,
    height: u32,
    cpu_samples_per_pixel: u32,
    gpu_samples_per_pixel: u32,
    cpu_flat: &[Vec3],
    gpu_xyz: &[f32],
) -> ImageComparisonResult {
    let num_pixels = (width * height) as usize;
    let mut z_grid = vec![0.0f64; num_pixels];
    let mut mean_z_sum = 0.0f64;
    let mut max_abs_z = 0.0f64;
    let mut max_abs_z_pixel = (0u32, 0u32);

    for pixel in 0..num_pixels {
        let mut cpu_acc = Welford::default();
        for s in 0..cpu_samples_per_pixel as usize {
            let v = cpu_flat[pixel * cpu_samples_per_pixel as usize + s];
            cpu_acc.update(f64::from(luminance(v)));
        }
        let mut gpu_acc = Welford::default();
        for s in 0..gpu_samples_per_pixel as usize {
            let idx = (pixel * gpu_samples_per_pixel as usize + s) * 3;
            let v = Vec3::new(gpu_xyz[idx], gpu_xyz[idx + 1], gpu_xyz[idx + 2]);
            gpu_acc.update(f64::from(luminance(v)));
        }
        let z = z_score(&cpu_acc, &gpu_acc);
        z_grid[pixel] = z;
        mean_z_sum += z;
        if z.abs() > max_abs_z {
            max_abs_z = z.abs();
            max_abs_z_pixel = ((pixel as u32) % width, (pixel as u32) / width);
        }
    }

    let over_3_sigma_count = z_grid.iter().filter(|z| z.abs() > 3.0).count();
    let cluster_sizes = connected_components(&z_grid, width, height, 3.0);

    ImageComparisonResult {
        width,
        height,
        cpu_samples_per_pixel,
        gpu_samples_per_pixel,
        mean_z: mean_z_sum / num_pixels as f64,
        over_3_sigma_count,
        total_pixels: num_pixels,
        over_3_sigma_expected: 0.0027,
        cluster_sizes,
        max_abs_z,
        max_abs_z_pixel,
    }
}

/// The GENERIC-vs-specialised correctness gate.
///
/// `renderer::gpu::frame::run_specialisation_equivalence` documents why the GENERIC and
/// specialised pipelines are not guaranteed byte-identical on the same input
/// (dead-code elimination can flip a stochastic-branch threshold comparison by 1 ULP).
/// That is the same class of divergence this module already tolerates statistically
/// between the CPU and GPU estimators, so this function verifies it the same way: two
/// GPU dispatches of `material` over DISJOINT sample ranges (mirroring
/// [`run_image_comparison_for`]'s CPU/GPU split, both sides now GPU) -- one through the
/// GENERIC pipeline, one through whichever specialised pipeline [`classify_material`]
/// picks -- compared via the same Welford mean/M2, per-pixel z-score, and
/// connected-component clustering [`ImageComparisonResult`] already carries. A wrong
/// specialisation guard would bias the whole image, not flip isolated pixels, so
/// [`ImageComparisonResult::passed`]'s existing criteria apply unchanged.
///
/// # Panics
///
/// Panics if `material.gpu_supported()` is false for `material` -- self-test scaffolding
/// callers always pass a GPU-supported built-in.
#[must_use]
pub fn run_specialisation_image_comparison(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
) -> ImageComparisonResult {
    let camera = test_camera();
    let (width, height) = (48u32, 48u32);
    let planes = round_brilliant_planes();
    let gpu_material = GpuGemMaterial::encode(material);
    let gpu_finishes = encode_facet_finishes(&[], planes.len());
    let max_bounces = 10u32;
    let preset = LightingPreset::Daylight;
    let (exposure, light_yaw, light_pitch) = (1.0f32, 0.4f32, 0.35f32);
    let temp_k = illuminant_temperature_k(preset);
    let wb = environment_white_balance(preset.studio(exposure, light_yaw, light_pitch));

    let samples_per_pixel = 500u32;
    let camera_params = camera_params_for(&camera, width, height, samples_per_pixel);
    let total = (width * height * samples_per_pixel) as usize;

    // Shared by both dispatches below (`sample_offset` is the only thing that differs)
    // so the D65 flag can't independently drift between the GENERIC and specialised
    // params the way two separate call sites could.
    let build_params = |sample_offset: u32| {
        GpuTransportParams::new(
            width * height,
            max_bounces,
            sample_offset,
            transport_env_mode::STUDIO_RIG,
            0.0,
            temp_k,
            preset.params().spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            wb.to_array(),
        )
        .with_studio_use_d65(preset.uses_d65())
        .with_studio_model(preset.model().gpu_id())
    };

    // GENERIC pipeline: samples [0, samples_per_pixel).
    let generic_params = build_params(0);
    let generic_dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &generic_params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: frame::material_class::GENERIC,
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &gpu_finishes,
            hdr_env: None,
        },
        total,
    );

    // Specialised pipeline: samples [samples_per_pixel, 2*samples_per_pixel) -- a
    // DISJOINT range from the GENERIC dispatch above, like
    // `run_image_comparison_for`'s CPU/GPU split.
    let specialised_params = build_params(samples_per_pixel);
    let specialised_dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &specialised_params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: classify_material(material),
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &gpu_finishes,
            hdr_env: None,
        },
        total,
    );

    let num_pixels = (width * height) as usize;
    let z_grid: Vec<f64> = (0..num_pixels)
        .map(|pixel| {
            pixel_specialisation_z_score(
                pixel,
                samples_per_pixel,
                &generic_dispatch.xyz,
                &specialised_dispatch.xyz,
            )
        })
        .collect();

    let mut mean_z_sum = 0.0f64;
    let mut max_abs_z = 0.0f64;
    let mut max_abs_z_pixel = (0u32, 0u32);
    for (pixel, &z) in z_grid.iter().enumerate() {
        mean_z_sum += z;
        if z.abs() > max_abs_z {
            max_abs_z = z.abs();
            max_abs_z_pixel = ((pixel as u32) % width, (pixel as u32) / width);
        }
    }

    let over_3_sigma_count = z_grid.iter().filter(|z| z.abs() > 3.0).count();
    let cluster_sizes = connected_components(&z_grid, width, height, 3.0);

    ImageComparisonResult {
        width,
        height,
        cpu_samples_per_pixel: samples_per_pixel,
        gpu_samples_per_pixel: samples_per_pixel,
        mean_z: mean_z_sum / num_pixels as f64,
        over_3_sigma_count,
        total_pixels: num_pixels,
        over_3_sigma_expected: 0.0027,
        cluster_sizes,
        max_abs_z,
        max_abs_z_pixel,
    }
}

/// One pixel's z-score between a GENERIC-pipeline and a specialised-pipeline sample set
/// -- factored out of [`run_specialisation_image_comparison`] so that function's own
/// per-pixel loop stays a plain `enumerate()` over the result (clippy's function-length
/// and index-loop lints both want this split out, and it also documents the per-pixel
/// Welford construction as its own named step).
fn pixel_specialisation_z_score(
    pixel: usize,
    samples_per_pixel: u32,
    generic_xyz: &[f32],
    specialised_xyz: &[f32],
) -> f64 {
    let mut generic_acc = Welford::default();
    let mut specialised_acc = Welford::default();
    for s in 0..samples_per_pixel as usize {
        let idx = (pixel * samples_per_pixel as usize + s) * 3;
        let generic_v = Vec3::new(generic_xyz[idx], generic_xyz[idx + 1], generic_xyz[idx + 2]);
        generic_acc.update(f64::from(luminance(generic_v)));
        let specialised_v = Vec3::new(
            specialised_xyz[idx],
            specialised_xyz[idx + 1],
            specialised_xyz[idx + 2],
        );
        specialised_acc.update(f64::from(luminance(specialised_v)));
    }
    z_score(&generic_acc, &specialised_acc)
}

const fn luminance(xyz: Vec3) -> f32 {
    xyz.y
}

/// 4-connectivity connected-component sizes of `|z| > threshold` pixels in a `width x
/// height` grid, largest first. Standard flood fill -- see [`ImageComparisonResult`]'s
/// doc comment for why this matters more than a bare failing-pixel count: a connected
/// region (one facet, a grazing-angle band, one branch of the physics) is what a real
/// porting bug leaves behind, while independent per-sample noise crossing the 3-sigma
/// threshold lands as scattered singletons.
fn connected_components(z_grid: &[f64], width: u32, height: u32, threshold: f64) -> Vec<usize> {
    let w = width as usize;
    let h = height as usize;
    let mut visited = vec![false; z_grid.len()];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..z_grid.len() {
        if visited[start] || z_grid[start].abs() <= threshold {
            continue;
        }
        stack.push(start);
        visited[start] = true;
        let mut size = 0usize;
        while let Some(idx) = stack.pop() {
            size += 1;
            let x = idx % w;
            let y = idx / w;
            let neighbors = [
                (x.checked_sub(1), Some(y)),
                (Some(x + 1).filter(|&nx| nx < w), Some(y)),
                (Some(x), y.checked_sub(1)),
                (Some(x), Some(y + 1).filter(|&ny| ny < h)),
            ];
            for (nx, ny) in neighbors {
                if let (Some(nx), Some(ny)) = (nx, ny) {
                    let nidx = ny * w + nx;
                    if !visited[nidx] && z_grid[nidx].abs() > threshold {
                        visited[nidx] = true;
                        stack.push(nidx);
                    }
                }
            }
        }
        sizes.push(size);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes
}
