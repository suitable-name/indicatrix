//! Tests for the worker-side request validation.

use super::*;
use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};

fn valid_scene() -> SceneState {
    SceneState {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        width: 64,
        height: 64,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 6,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
    }
}

#[test]
fn accepts_a_well_formed_scene() {
    assert!(validate_scene(&valid_scene()).is_ok());
}

#[test]
fn the_tray_radiance_must_lie_between_zero_and_sixteen() {
    for good in [0.0, 0.81, 3.2, 8.0, MAX_BACKDROP] {
        let mut scene = valid_scene();
        scene.backdrop = good;
        assert!(validate_scene(&scene).is_ok(), "{good}");
    }
    for bad in [-0.001, -1.0, 16.001, 1.0e30] {
        let mut scene = valid_scene();
        scene.backdrop = bad;
        let err = validate_scene(&scene).unwrap_err();
        assert!(
            err.contains("scene.backdrop must be between 0 and 16"),
            "{bad}: {err}"
        );
    }
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut scene = valid_scene();
        scene.backdrop = bad;
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("scene.backdrop must be finite"), "{err}");
    }
}

#[test]
fn rejects_empty_planes() {
    let mut scene = valid_scene();
    scene.planes.clear();
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn rejects_a_nan_normal() {
    let mut scene = valid_scene();
    scene.planes[0].normal = [f32::NAN, 0.0, 0.0];
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("non-finite"), "{err}");
}

#[test]
fn rejects_an_infinite_normal() {
    let mut scene = valid_scene();
    scene.planes[0].normal = [f32::INFINITY, 0.0, 0.0];
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn rejects_a_zero_length_normal() {
    let mut scene = valid_scene();
    scene.planes[0] = GpuFacetPlane {
        normal: [0.0, 0.0, 0.0],
        d: 1.0,
    };
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("degenerate"), "{err}");
}

#[test]
fn rejects_non_finite_plane_offset() {
    let mut scene = valid_scene();
    scene.planes[0].d = f32::NAN;
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn rejects_zero_distance() {
    let mut scene = valid_scene();
    scene.distance = 0.0;
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn rejects_non_finite_exposure() {
    let mut scene = valid_scene();
    scene.exposure = f32::INFINITY;
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn rejects_non_finite_surface_glare() {
    let mut scene = valid_scene();
    scene.surface_glare = f32::NAN;
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("surface_glare"), "{err}");
}

#[test]
fn rejects_non_finite_head_shadow() {
    let mut scene = valid_scene();
    scene.head_shadow_deg = f32::NAN;
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("head_shadow_deg"), "{err}");
}

#[test]
fn rejects_zero_and_excessive_max_bounces() {
    let mut scene = valid_scene();
    scene.max_bounces = 0;
    assert!(validate_scene(&scene).is_err());

    scene.max_bounces = MAX_BOUNCES + 1;
    assert!(validate_scene(&scene).is_err());

    scene.max_bounces = MAX_BOUNCES;
    assert!(validate_scene(&scene).is_ok());
}

/// Pins the cross-crate contract with the GUI's bounce ladder
/// (`apps/indicatrix-cut/src/bridge/export_thread/params.rs::MAX_EXPORT_BOUNCES`) as a
/// literal so drift on either side trips this test instead of silently reintroducing
/// the "remote export goes silent" bug.
#[test]
fn max_bounces_matches_the_guis_own_ladder_ceiling() {
    assert_eq!(MAX_BOUNCES, 128);
}

#[test]
fn rejects_too_many_absorption_bands() {
    use indicatrix::optics::absorption::AbsorptionBand;

    let mut scene = valid_scene();
    let too_many: Vec<AbsorptionBand> = (0..=indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS)
        .map(|i| AbsorptionBand::new(400.0 + i as f32, 10.0, 1.0))
        .collect();
    scene.material.absorption.o_ray = too_many.clone();
    scene.material.absorption.e_ray = too_many;
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("exceeding the GPU encoding's cap"), "{err}");
}

#[test]
fn accepts_absorption_bands_up_to_the_cap() {
    use indicatrix::optics::absorption::AbsorptionBand;

    let mut scene = valid_scene();
    let at_cap: Vec<AbsorptionBand> = (0..indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS)
        .map(|i| AbsorptionBand::new(400.0 + i as f32, 10.0, 1.0))
        .collect();
    scene.material.absorption.o_ray = at_cap.clone();
    scene.material.absorption.e_ray = at_cap;
    assert!(validate_scene(&scene).is_ok());
}

#[test]
fn rejects_non_finite_absorption_path_scale() {
    let mut scene = valid_scene();
    scene.material.absorption_path_scale = f32::NAN;
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("absorption_path_scale"), "{err}");

    scene.material.absorption_path_scale = f32::INFINITY;
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn rejects_zero_and_negative_absorption_path_scale() {
    let mut scene = valid_scene();
    scene.material.absorption_path_scale = 0.0;
    assert!(validate_scene(&scene).is_err());

    scene.material.absorption_path_scale = -1.0;
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn accepts_a_scaled_absorption_path() {
    let mut scene = valid_scene();
    scene.material = scene.material.with_absorption_path_scale(2.5);
    assert!(validate_scene(&scene).is_ok());
}

#[test]
fn rejects_implausible_refractive_index() {
    let mut scene = valid_scene();
    // A custom material whose Cauchy fit is nowhere near a real gemstone's.
    scene.material = GemMaterial::new_custom("Absurd", 50.0, 0.0, 0.0, [0.0, 0.0, 0.0]);
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("refractive index"), "{err}");
}

#[test]
fn validate_request_rejects_a_zero_sample_count() {
    let scene = valid_scene();
    assert!(validate_request(&scene, 0, 0).is_err());
}

#[test]
fn validate_request_rejects_an_excessive_sample_count() {
    let scene = valid_scene();
    assert!(validate_request(&scene, 0, MAX_SAMPLES_PER_REQUEST + 1).is_err());
    assert!(validate_request(&scene, 0, MAX_SAMPLES_PER_REQUEST).is_ok());
}

#[test]
fn validate_request_rejects_first_sample_plus_samples_overflow() {
    let scene = valid_scene();
    assert!(validate_request(&scene, u32::MAX - 10, 100).is_err());
}

/// A request that doesn't literally overflow `u32` but leaves no headroom for a
/// further `MAX_SAMPLES_PER_REQUEST`-sized chunk below `u32::MAX` is still
/// rejected.
#[test]
fn validate_request_rejects_a_request_leaving_no_chunking_headroom() {
    let scene = valid_scene();
    let end_too_close = u32::MAX - MAX_SAMPLES_PER_REQUEST + 1;
    assert!(validate_request(&scene, end_too_close - 16, 16).is_err());
    // Exactly at the headroom boundary is still fine.
    let end_at_boundary = u32::MAX - MAX_SAMPLES_PER_REQUEST;
    assert!(validate_request(&scene, end_at_boundary - 16, 16).is_ok());
}

#[test]
fn rejects_too_many_planes() {
    let mut scene = valid_scene();
    scene.planes = vec![scene.planes[0]; MAX_PLANES + 1];
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("exceeding the maximum"), "{err}");
}

#[test]
fn accepts_planes_up_to_the_cap() {
    let mut scene = valid_scene();
    scene.planes = vec![scene.planes[0]; MAX_PLANES];
    assert!(validate_scene(&scene).is_ok());
}

#[test]
fn validate_scene_rejects_too_many_or_invalid_tools() {
    use indicatrix::geometry::{MAX_TOOL_PRIMITIVES, ToolPrimitive};

    let tool = ToolPrimitive::ball(Vec3::new(0.0, 0.0, 0.4), 0.2);
    assert!(
        tool.validate().is_ok(),
        "the fixture tool must itself be valid"
    );

    let mut scene = valid_scene();
    scene.tools = vec![tool];
    assert!(validate_scene(&scene).is_ok());

    scene.tools = vec![tool; MAX_TOOL_PRIMITIVES];
    assert!(
        validate_scene(&scene).is_ok(),
        "exactly the cap is accepted"
    );

    scene.tools = vec![tool; MAX_TOOL_PRIMITIVES + 1];
    let err = validate_scene(&scene).unwrap_err();
    assert!(
        err.contains("scene.tools") && err.contains("maximum"),
        "{err}"
    );

    scene.tools = vec![tool, ToolPrimitive::ball(Vec3::ZERO, f32::NAN)];
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("scene.tools[1]"), "{err}");
}

#[test]
fn validate_scene_bounds_the_frosted_plane_list_by_the_planes() {
    let mut scene = valid_scene();
    let planes = scene.planes.len();
    scene.frosted_planes = (0..u32::try_from(planes).unwrap()).collect();
    assert!(
        validate_scene(&scene).is_ok(),
        "every plane frosted is a valid scene"
    );
    scene.frosted_planes.push(0);
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("scene.frosted_planes"), "{err}");
}

#[test]
fn validate_scene_rejects_invalid_fluorescence() {
    use indicatrix::optics::{
        absorption::AbsorptionBand,
        fluorescence::{EmissionBand, Fluorescence, FluorescentEmitter, MAX_BANDS, MAX_EMITTERS},
    };

    let emitter = FluorescentEmitter {
        excitation: vec![AbsorptionBand::new(410.0, 20.0, 1.0)],
        emission: vec![EmissionBand::new(694.0, 2.0, 1.0)],
        quantum_yield: 0.9,
    };
    let mut scene = valid_scene();
    scene.fluorescence = Fluorescence::new(vec![emitter.clone(); MAX_EMITTERS]);
    assert!(
        validate_scene(&scene).is_ok(),
        "exactly the cap is accepted"
    );

    scene.fluorescence = Fluorescence::new(vec![emitter.clone(); MAX_EMITTERS + 1]);
    let err = validate_scene(&scene).unwrap_err();
    assert!(err.contains("scene.fluorescence"), "{err}");

    let mut bad = emitter.clone();
    bad.quantum_yield = 1.5;
    scene.fluorescence = Fluorescence::new(vec![bad]);
    assert!(validate_scene(&scene).is_err(), "quantum yield above 1");

    let mut bad = emitter.clone();
    bad.quantum_yield = f32::NAN;
    scene.fluorescence = Fluorescence::new(vec![bad]);
    assert!(validate_scene(&scene).is_err(), "NaN quantum yield");

    let mut bad = emitter.clone();
    bad.emission[0].fwhm_nm = f32::INFINITY;
    scene.fluorescence = Fluorescence::new(vec![bad]);
    assert!(validate_scene(&scene).is_err(), "non-finite emission width");

    let mut bad = emitter;
    bad.excitation = vec![AbsorptionBand::new(410.0, 20.0, 1.0); MAX_BANDS + 1];
    scene.fluorescence = Fluorescence::new(vec![bad]);
    assert!(validate_scene(&scene).is_err(), "too many bands");
}

#[test]
fn validate_request_rejects_oversized_scene_dimensions() {
    let mut scene = valid_scene();
    scene.width = 100_000;
    scene.height = 100_000;
    assert!(validate_request(&scene, 0, 16).is_err());
}

#[test]
fn validate_request_accepts_a_well_formed_request() {
    let scene = valid_scene();
    assert!(validate_request(&scene, 128, 64).is_ok());
}

use indicatrix_net::messages::{PreviewConfig, StreamConfig, TransferMode};

fn stream_config(preview: Option<PreviewConfig>) -> StreamConfig {
    StreamConfig {
        transfer_mode: TransferMode::LiveProgressive,
        cadence_ms: 250,
        preview,
    }
}

#[test]
fn validate_stream_config_accepts_no_preview() {
    let scene = valid_scene();
    assert!(validate_stream_config(&mut stream_config(None), &scene).is_ok());
}

#[test]
fn validate_stream_config_accepts_a_smaller_preview() {
    let scene = valid_scene();
    let mut stream = stream_config(Some(PreviewConfig {
        width: 16,
        height: 16,
    }));
    assert!(validate_stream_config(&mut stream, &scene).is_ok());
}

#[test]
fn validate_stream_config_rejects_a_zero_dimension_preview() {
    let scene = valid_scene();
    let mut stream = stream_config(Some(PreviewConfig {
        width: 0,
        height: 16,
    }));
    let err = validate_stream_config(&mut stream, &scene).unwrap_err();
    assert!(err.contains("must be positive"), "{err}");
}

#[test]
fn validate_stream_config_rejects_a_preview_larger_than_the_scene() {
    let scene = valid_scene(); // 64x64
    let mut stream = stream_config(Some(PreviewConfig {
        width: 128,
        height: 128,
    }));
    let err = validate_stream_config(&mut stream, &scene).unwrap_err();
    assert!(err.contains("must not exceed"), "{err}");
}

#[test]
fn validate_stream_config_accepts_a_preview_equal_to_the_scene_under_live_progressive() {
    let scene = valid_scene(); // 64x64
    let mut stream = stream_config(Some(PreviewConfig {
        width: 64,
        height: 64,
    }));
    assert!(validate_stream_config(&mut stream, &scene).is_ok());
}

/// A full-scale preview is fine under `LiveProgressive` (the emitter's
/// own redundancy skip handles it), but must be rejected outright under
/// `FinalOnly`, where that skip never applies.
#[test]
fn validate_stream_config_rejects_a_preview_equal_to_the_scene_under_final_only() {
    let scene = valid_scene(); // 64x64
    let mut stream = stream_config(Some(PreviewConfig {
        width: 64,
        height: 64,
    }));
    stream.transfer_mode = TransferMode::FinalOnly;
    let err = validate_stream_config(&mut stream, &scene).unwrap_err();
    assert!(err.contains("FinalOnly"), "{err}");
}

/// `cadence_ms = 0` ("as fast as possible") is clamped up to the
/// worker's advertised floor, not left as an unbounded pacing request.
#[test]
fn validate_stream_config_clamps_a_zero_cadence_up_to_the_floor() {
    let scene = valid_scene();
    let mut stream = stream_config(None);
    stream.cadence_ms = 0;
    validate_stream_config(&mut stream, &scene).unwrap();
    assert_eq!(stream.cadence_ms, crate::stream_emit::MIN_CADENCE_FLOOR_MS);
}

#[test]
fn validate_stream_config_leaves_a_cadence_at_or_above_the_floor_untouched() {
    let scene = valid_scene();
    let mut stream = stream_config(None);
    stream.cadence_ms = crate::stream_emit::MIN_CADENCE_FLOOR_MS + 500;
    let original = stream.cadence_ms;
    validate_stream_config(&mut stream, &scene).unwrap();
    assert_eq!(stream.cadence_ms, original);
}

#[test]
fn ray_requests_must_fit_the_fixed_absolute_tap_pattern() {
    let mut scene = valid_scene();
    scene.engine.engine = indicatrix::renderer::engine::RenderEngine::RayTraced;
    for taps in [1, 4, 8, 16] {
        scene.engine.ray.aa_taps = taps;
        assert!(validate_request(&scene, 0, u32::from(taps)).is_ok());
        assert!(validate_request(&scene, u32::from(taps) - 1, 1).is_ok());
        assert!(validate_request(&scene, u32::from(taps), 1).is_err());
        assert!(validate_request(&scene, u32::MAX, 1).is_err());
    }
    scene.engine.ray.aa_taps = 3;
    assert!(validate_request(&scene, 0, 1).is_err());
    scene.engine.ray.aa_taps = 4;
    scene.lighting_preset = LightingPreset::UvLamp365;
    assert!(validate_request(&scene, 0, 1).unwrap_err().contains("UV"));
}

#[test]
fn ray_preparation_bounds_the_beta_absorption_band_list() {
    use indicatrix::optics::absorption::AbsorptionBand;
    let mut scene = valid_scene();
    scene.engine.engine = indicatrix::renderer::engine::RenderEngine::RayTraced;
    let band = AbsorptionBand::new(545.0, 20.0, 1.0);
    let cap = indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS;
    scene.material.absorption.beta_ray = Some(vec![band; cap]);
    assert!(validate_scene(&scene).is_ok());
    scene
        .material
        .absorption
        .beta_ray
        .as_mut()
        .unwrap()
        .push(band);
    let error = validate_scene(&scene).unwrap_err();
    assert!(error.contains("beta_ray"), "{error}");
}
