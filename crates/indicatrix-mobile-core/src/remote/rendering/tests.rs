use super::*;
use indicatrix::{
    geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
    optics::raytracer::LightingPreset,
    render_setup::Backdrop,
};
use indicatrix_web_core::scene::{
    CameraSpec, FinishSpec, LightingSpec, MaterialSpec, SceneSpec, planes_to_data,
};

fn spec(hdr_id: Option<u64>) -> GeometrySceneSpec {
    GeometrySceneSpec {
        scene: SceneSpec {
            planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
            finishes: FinishSpec::AllPolished,
            material: MaterialSpec::catalogue("Diamond"),
            camera: CameraSpec {
                yaw: 0.25,
                pitch: 0.75,
                distance: 3.25,
            },
            lighting: LightingSpec::new(
                LightingPreset::Daylight,
                1.25,
                0.3,
                0.4,
                Backdrop::Grey,
                12.0,
            )
            .with_surface_glare(0.6),
            max_bounces: 4,
            width: 8,
            height: 6,
            hdr_id,
        },
        tools: vec![ToolPrimitive::ball(glam::Vec3::new(0.0, 0.3, 0.0), 0.1)],
    }
}

fn map(width: usize, height: usize) -> Arc<EnvironmentMap> {
    let texels = (0..width * height)
        .map(|i| {
            let t = i as f32 / (width * height) as f32;
            [t.mul_add(3.0, 0.125), 0.5, (t * 9.0).cos().abs()]
        })
        .collect();
    Arc::new(EnvironmentMap::from_rgb(width, height, texels).unwrap())
}

#[test]
fn wire_scene_preserves_resolved_tools_material_pose_and_lighting() {
    let spec = spec(None);
    let built = spec.build(None).unwrap();
    let wire = scene_state(&spec, &built, None);
    assert_eq!(wire.tools, spec.tools);
    assert_eq!(wire.material, *built.material());
    assert_eq!(wire.planes, built.planes());
    assert_eq!((wire.yaw, wire.pitch, wire.distance), (0.25, 0.75, 3.25));
    assert_eq!(
        (wire.exposure, wire.surface_glare, wire.head_shadow_deg),
        (1.25, 0.6, 12.0)
    );
    assert_eq!(wire.environment, SceneEnvironment::Studio);
    assert!(wire.engine.is_default());
}

#[test]
fn the_wire_backdrop_is_the_tray_radiance_under_the_scenes_own_rig() {
    let spec = spec(None);
    let built = spec.build(None).unwrap();
    let lighting = &spec.scene.lighting;
    let wire = scene_state(&spec, &built, None);
    // Grey under the spec's preset and pose (exposure 1), not a fixed card level.
    let tray = Backdrop::Grey.resolve(
        LightingPreset::Daylight,
        lighting.light_yaw,
        lighting.light_pitch,
    );
    assert!(tray > 0.0);
    assert_eq!(wire.backdrop.to_bits(), tray.to_bits());
    // As Lit stays the preset's own ground (the value 0.0 means exactly that on the wire).
    let mut as_lit = spec.clone();
    as_lit.scene.lighting.backdrop_index = Backdrop::AsLit.index();
    let wire = scene_state(&as_lit, &as_lit.build(None).unwrap(), None);
    assert_eq!(wire.backdrop.to_bits(), 0.0f32.to_bits());
}

#[test]
fn the_remote_scene_carries_the_exact_image_engine_and_tap_options() {
    use indicatrix::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
    let spec = spec(None);
    let mut built = spec.build(None).unwrap();
    let engine = EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: 4,
            fire: false,
        },
    };
    built.set_engine(engine).unwrap();
    let wire = scene_state(&spec, &built, None);
    assert_eq!(wire.engine, engine);
    assert_eq!(wire.tools, spec.tools);
    assert_eq!(wire.material, *built.material());
    assert!(!wire.engine.denoises());
}

#[test]
fn all_remote_output_modes_stop_at_the_selected_aa_tap_count() {
    use indicatrix::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
    let ray = EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: 4,
            fire: true,
        },
    };
    assert!(check_sample_target(ray, 0).is_err());
    assert!(check_sample_target(ray, 1).is_ok());
    assert!(check_sample_target(ray, 4).is_ok());
    assert!(check_sample_target(ray, 5).is_err());
    assert!(check_sample_target(EngineSpec::default(), 4096).is_ok());
}

#[test]
fn a_studio_scene_needs_no_asset() {
    let map = map(8, 4);
    assert!(
        HdrAsset::prepare(&spec(None), Some(&map))
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_hdr_scene_names_the_effective_map_by_the_hash_of_its_asset_bytes() {
    let map = map(16, 8);
    let asset = HdrAsset::prepare(&spec(Some(1)), Some(&map))
        .unwrap()
        .expect("an HDR scene has an asset");
    let environment = asset.environment();
    assert_eq!((environment.width, environment.height), (16, 8));
    // The scene's hash is the SHA-256 of exactly the bytes that would be uploaded.
    let bytes = encode_envmap_asset(16, 8, map.texels()).unwrap();
    assert_eq!(environment.content_hash, messages::content_hash(&bytes));
    // And those bytes decode to the same texels bit for bit.
    let (width, height, texels) = messages::envmap_asset::decode_envmap_asset(&bytes).unwrap();
    assert_eq!((width, height), (16, 8));
    let back = EnvironmentMap::from_rgb(16, 8, texels).unwrap();
    assert!(back.bitwise_eq(&map));
    let wire = scene_state(
        &spec(Some(1)),
        &spec(Some(1)).build(Some(Arc::clone(&map))).unwrap(),
        Some(environment),
    );
    assert_eq!(wire.environment, SceneEnvironment::Hdr(environment));
}

#[test]
fn an_hdr_scene_without_its_map_is_refused_and_nothing_is_substituted() {
    let error = HdrAsset::prepare(&spec(Some(1)), None).err().unwrap();
    assert!(error.contains("not loaded"), "{error}");
}

#[test]
fn the_hash_follows_the_texels() {
    let a = HdrAsset::prepare(&spec(Some(1)), Some(&map(8, 4)))
        .unwrap()
        .unwrap();
    let b = HdrAsset::prepare(&spec(Some(1)), Some(&map(4, 8)))
        .unwrap()
        .unwrap();
    assert_ne!(a.environment().content_hash, b.environment().content_hash);
}

#[test]
fn transport_loss_is_an_interruption_and_a_protocol_fault_is_not() {
    let lost = NetError::Framing(FramingError::Io(std::io::Error::from(
        std::io::ErrorKind::ConnectionReset,
    )));
    assert!(matches!(net_error(&lost), RunError::Interrupted(_)));
    let truncated = NetError::Framing(FramingError::TruncatedFrame);
    assert!(matches!(net_error(&truncated), RunError::Interrupted(_)));
    let huge = NetError::Framing(FramingError::FrameTooLarge { len: 9, max: 1 });
    assert!(matches!(net_error(&huge), RunError::Failed(_)));
    assert_eq!(String::from(RunError::Failed("x".into())), "x");
}

#[test]
fn a_metered_network_refuses_heavy_sessions_unless_allowed() {
    let platform = crate::platform_mock::MockPlatform::default();
    let mut settings = RemoteSettings::default();
    // Unknown metering (the default, and the desktop preview) never refuses.
    assert!(metered_refusal(&platform, &settings, true).is_ok());
    platform.set_network_metered(Some(false));
    assert!(metered_refusal(&platform, &settings, true).is_ok());
    platform.set_network_metered(Some(true));
    let error = metered_refusal(&platform, &settings, true).unwrap_err();
    assert!(error.contains("Allow Metered Networks"), "{error}");
    // A light session (DisplayOnly, studio lighting) is not blocked.
    assert!(metered_refusal(&platform, &settings, false).is_ok());
    settings.allow_metered = true;
    assert!(metered_refusal(&platform, &settings, true).is_ok());
}
