//! Smoke tests for studio-lit spectral rendering: background/environment colour,
//! basic diamond/colored-gem traces, custom material creation, and moving the
//! light source.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray, xyz_to_srgb_gamma},
    },
};

#[test]
fn test_background_studio_color_not_blown_out() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let ray_miss = Ray {
        origin: Vec3::new(0.0, 10.0, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let xyz = trace_spectral_ray(
        ray_miss,
        &planes,
        &diamond,
        12,
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        1337,
        (hash_u32(1337) as f32) / 4_294_967_295.0,
        None,
    );

    let rgba = xyz_to_srgb_gamma(xyz);
    assert!(
        rgba[0] < 100 && rgba[1] < 100 && rgba[2] < 100,
        "Background should be dark studio tone (got {rgba:?})"
    );
}

#[test]
fn test_daylight_background_is_neutral_and_not_blown_out() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let ray_miss = Ray {
        origin: Vec3::new(0.0, 10.0, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let xyz = trace_spectral_ray(
        ray_miss,
        &planes,
        &diamond,
        12,
        LightingPreset::Daylight.studio(1.0, 0.85, 0.95),
        1337,
        (hash_u32(1337) as f32) / 4_294_967_295.0,
        None,
    );

    let rgba = xyz_to_srgb_gamma(xyz);
    assert!(
        rgba[0] < 100 && rgba[1] < 100 && rgba[2] < 100,
        "Daylight background must be dark slate tone (got {rgba:?})"
    );
}

#[test]
fn test_spectral_raytrace_diamond() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let xyz = trace_spectral_ray(
        ray,
        &planes,
        &diamond,
        12,
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        1337,
        (hash_u32(1337) as f32) / 4_294_967_295.0,
        None,
    );

    assert!(xyz.y > 0.0, "Rendered luminance must be greater than zero");
    let rgba = xyz_to_srgb_gamma(xyz);
    assert_eq!(rgba[3], 255);
}

#[test]
fn test_spectral_raytrace_colored_gem() {
    let planes = StandardGemCuts::emerald_cut();
    let ruby = GemMaterial::ruby();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let xyz = trace_spectral_ray(
        ray,
        &planes,
        &ruby,
        12,
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        42,
        (hash_u32(42) as f32) / 4_294_967_295.0,
        None,
    );

    let rgba = xyz_to_srgb_gamma(xyz);
    assert!(
        rgba[0] > rgba[2],
        "Ruby must exhibit strong red spectral dominance (R > B)"
    );
}

#[test]
fn test_custom_gem_material_creation_and_rendering() {
    let custom_opal = GemMaterial::new_custom("Custom Opal", 1.450, 0.010, 0.000, [0.1, 0.1, 0.1]);
    assert_eq!(custom_opal.name, "Custom Opal");
    let nd = custom_opal.dispersion.evaluate(589.3);
    assert!(
        (nd - 1.450).abs() < 0.05,
        "Refractive index should be approximately 1.45"
    );

    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };
    let xyz = trace_spectral_ray(
        ray,
        &planes,
        &custom_opal,
        12,
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        42,
        (hash_u32(42) as f32) / 4_294_967_295.0,
        None,
    );
    assert!(
        xyz.y > 0.0,
        "Custom material rendering must produce positive luminance"
    );
}

#[test]
fn test_movable_light_source_changes_scene_radiance() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let xyz1 = trace_spectral_ray(
        ray,
        &planes,
        &diamond,
        12,
        LightingPreset::RingLights.studio(1.0, 0.0, 1.2),
        42,
        (hash_u32(42) as f32) / 4_294_967_295.0,
        None,
    );
    let xyz2 = trace_spectral_ray(
        ray,
        &planes,
        &diamond,
        12,
        LightingPreset::RingLights.studio(1.0, std::f32::consts::PI, 0.3),
        42,
        (hash_u32(42) as f32) / 4_294_967_295.0,
        None,
    );

    assert!(
        (xyz1 - xyz2).length() > 1e-4,
        "Moving the light position must dynamically change the raytraced gem radiance"
    );
}
