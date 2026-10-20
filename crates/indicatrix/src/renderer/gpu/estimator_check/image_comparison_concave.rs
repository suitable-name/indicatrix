//! Tier 3 for a concave stone: the CPU tracer with its tools against the megakernel with the
//! tool records packed after the planes, through the same disjoint-sample-range z-score
//! criteria as every other image comparison here ([`ImageComparisonResult::passed`]).
//!
//! The stone is the Diamond Standard Round Brilliant with a ball dimple in the table and a
//! cylinder groove across the crown (`tool_check::image_check_tools`), so the comparison
//! covers the tool intersection, the tool-surface normals, the shadow probe of the frosted
//! NEE, the Henyey-Greenstein exit walk and the edge-rounding rule end to end; the
//! per-primitive agreement is `tool_check`'s Tier 2.

use crate::{
    geometry::tool::{StoneGeometry, pack_planes_and_tools},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, LightingPreset, build_plane_soa, environment::environment_white_balance,
            illuminant_temperature_k, pixel_rotations, sample_draws,
            trace_spectral_ray_with_finish_soa_geom,
        },
    },
    renderer::{
        buffers::{GpuGemMaterial, GpuTransportParams, encode_facet_finishes, transport_env_mode},
        gpu::{frame::classify_material, tool_check::image_check_tools},
    },
};

use super::{
    MaterialForDispatch, SceneBuffersForDispatch, camera_params_for, cpu_samples,
    dispatch_transport_for_class,
    image_comparison::{ImageComparisonResult, compute_image_comparison_result},
    round_brilliant_planes, test_camera,
};

/// Runs the concave-stone image comparison against the CPU reference.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_concave(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    let preset = LightingPreset::Daylight;
    let camera = test_camera();
    let (width, height) = (48u32, 48u32);
    let planes = round_brilliant_planes();
    let tools = image_check_tools();
    let packed = pack_planes_and_tools(&planes, &tools);
    let gpu_material = GpuGemMaterial::encode(&material);
    // One finish entry per packed `planes` entry, as the renderer sizes it.
    let gpu_finishes = encode_facet_finishes(&[], packed.len());
    let max_bounces = 10u32;
    let (exposure, light_yaw, light_pitch) = (1.0f32, 0.4f32, 0.35f32);
    let temp_k = illuminant_temperature_k(preset);
    let environment = preset
        .studio(exposure, light_yaw, light_pitch)
        .with_backdrop(BACKDROP_GREY);
    let wb = environment_white_balance(environment);

    let cpu_samples_per_pixel = 500u32;
    let gpu_samples_per_pixel = 500u32;

    let soa = build_plane_soa(&planes);
    let geom = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    let cpu_flat = cpu_samples(width, height, cpu_samples_per_pixel, 0, |pixel, sample| {
        let rot = pixel_rotations(pixel);
        let draws = sample_draws(pixel, sample, &rot);
        let ray = camera.generate_ray(
            (pixel % width) as f32,
            (pixel / width) as f32,
            width as f32,
            height as f32,
            draws.jitter_x,
            draws.jitter_y,
        );
        trace_spectral_ray_with_finish_soa_geom(
            ray,
            geom,
            &soa,
            &[],
            &material,
            Fluorescence::none(),
            max_bounces,
            environment,
            draws.seed,
            draws.hero_rand,
            None,
        )
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
    .with_studio_use_d65(preset.uses_d65())
    .with_studio_model(preset.model().gpu_id())
    .with_tent(preset.params().tent)
    .with_backdrop(BACKDROP_GREY)
    .with_tool_count(u32::try_from(tools.len()).expect("a few tools"));
    let total_gpu = (width * height * gpu_samples_per_pixel) as usize;
    let gpu_dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: classify_material(&material),
        },
        SceneBuffersForDispatch {
            planes: &packed[..],
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
