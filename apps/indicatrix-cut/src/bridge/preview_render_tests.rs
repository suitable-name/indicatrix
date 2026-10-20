use super::*;

#[test]
fn classify_maps_terminal_and_progress_updates() {
    assert!(matches!(
        classify(RemoteUpdate::Done {
            request_id: 1,
            cancelled: false
        }),
        Step::Terminal(Ok(()))
    ));
    assert!(matches!(
        classify(RemoteUpdate::Done {
            request_id: 1,
            cancelled: true
        }),
        Step::Terminal(Err(RemoteShortfall::Cancelled))
    ));
    assert!(matches!(
        classify(RemoteUpdate::Failed {
            request_id: 1,
            message: "x".to_owned()
        }),
        Step::Terminal(Err(RemoteShortfall::Failed(_)))
    ));
    assert!(matches!(
        classify(RemoteUpdate::Progress {
            request_id: 1,
            samples_done: 7
        }),
        Step::Progress(7)
    ));
    assert!(matches!(
        classify(RemoteUpdate::Preview { request_id: 1 }),
        Step::Ignore
    ));
}

#[test]
fn preview_view_pitch_matches_the_documented_front_and_top_convention() {
    assert_eq!(PreviewView::Front.pitch(), 0.0);
    assert!((PreviewView::Top.pitch() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
}

/// `Camera::new`'s own origin formula (`d*sin(pitch)` for the Y component) must
/// place `PreviewView::Top`'s camera directly overhead -- pins the actual
/// consequence of the pitch convention above, not just the raw angle.
#[test]
fn top_pitch_places_the_camera_directly_overhead() {
    let camera = Camera::new(
        0.0,
        PreviewView::Top.pitch(),
        PREVIEW_DISTANCE,
        DEFAULT_FOV_DEG,
    );
    assert!((camera.origin.x).abs() < 1e-3);
    assert!((camera.origin.y - PREVIEW_DISTANCE).abs() < 1e-3);
    assert!((camera.origin.z).abs() < 1e-3);
}

/// `Front`'s pitch must place the camera ON the horizontal plane (zero Y), for any
/// yaw -- an edge-on/profile view by construction, not merely "not the top pole".
#[test]
fn front_pitch_places_the_camera_on_the_horizontal_plane() {
    for yaw in [0.0, 0.6, 3.0] {
        let camera = Camera::new(
            yaw,
            PreviewView::Front.pitch(),
            PREVIEW_DISTANCE,
            DEFAULT_FOV_DEG,
        );
        assert!((camera.origin.y).abs() < 1e-3, "yaw={yaw}");
    }
}

#[test]
fn ri_candidates_covers_every_unzoned_built_in_material_with_a_finite_ri() {
    let candidates = ri_candidates();
    let unzoned: Vec<String> = GemMaterial::all_materials()
        .into_iter()
        .filter(is_unzoned)
        .map(|material| material.name)
        .collect();
    assert_eq!(
        candidates
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>(),
        unzoned
    );
    assert!(
        !candidates.iter().any(|c| c.name == "Ametrine"),
        "a zoned built-in is never the write-once guess for a nameless design"
    );
    for c in &candidates {
        assert!(
            c.refractive_index.is_finite() && c.refractive_index > 1.0,
            "material {:?} produced an implausible RI {}",
            c.name,
            c.refractive_index
        );
    }
}

/// `ri_candidates` must agree with this crate's own documented convention
/// (`material.dispersion.evaluate(589.3)`, the sodium D line every other RI-reading
/// call site in `indicatrix::optics::materials` uses) for a material whose RI is
/// well known -- Diamond's is ~2.417.
#[test]
fn ri_candidates_evaluates_at_the_sodium_d_line() {
    let candidates = ri_candidates();
    let diamond = candidates
        .iter()
        .find(|c| c.name == "Diamond")
        .expect("Diamond is a built-in material");
    assert!(
        (diamond.refractive_index - 2.417).abs() < 0.01,
        "got {}",
        diamond.refractive_index
    );
}

/// A remote worker's `BATCH_ITEM_DONE` PNG and a local preview come from the same
/// shared function; this pins that function to the encoding the local path used
/// before it was shared (tone-map at `1 / spp`, then `image`'s default PNG writer),
/// so no cached thumbnail's bytes changed.
#[test]
fn the_shared_preview_png_matches_the_encoding_the_local_path_used_before() {
    let accum: Vec<Vec3> = (0..16)
        .map(|i| Vec3::new(0.3, 0.2, 0.1) * (i as f32 + 1.0) * 4.0)
        .collect();
    let rgba = tonemap_to_rgba(&accum, 1.0 / 4.0);
    let legacy = encode_png(4, 4, &rgba).expect("the legacy encoder accepts a 4x4 buffer");
    let shared = indicatrix::render_setup::encode_preview_png(4, 4, &accum, 4)
        .expect("the shared encoder accepts a 4x4 sum");
    assert_eq!(shared, legacy);
}

#[test]
fn render_view_produces_a_decodable_png_at_the_requested_size() {
    use indicatrix::geometry::cuts::StandardGemCuts;

    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let job = PreviewJob {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        planes: &planes,
        tools: &[],
        frosted: &[],
        material: &material,
        size: 8,
        spp: 1,
        max_bounces: 4,
    };
    let gpu = GpuBackend::disabled();
    let png = render_view(&job, PreviewView::Top, &gpu).expect("render must succeed");

    let decoded = image::load_from_memory(&png).expect("must be a valid PNG");
    assert_eq!(decoded.width(), 8);
    assert_eq!(decoded.height(), 8);
}

/// The catalogue preview scene is a bright white surround (the white tray: uniform walls,
/// bright floor, no black cards, no spot) with an 8 degree head shadow, and the remote
/// scene a worker traces carries the very same settings as the local one.
#[test]
fn the_catalogue_preview_scene_is_the_white_surround_with_an_8_degree_shadow() {
    use indicatrix::geometry::cuts::StandardGemCuts;

    // The preview's shadow is smaller than the live view's default. Both are constants, so the
    // build checks it.
    const _: () =
        assert!(PREVIEW_HEAD_SHADOW_DEG < crate::settings::model::DEFAULT_HEAD_SHADOW_DEG);

    assert_eq!(PREVIEW_LIGHTING_PRESET, LightingPreset::WhiteTray);
    assert!((PREVIEW_HEAD_SHADOW_DEG - 8.0).abs() < f32::EPSILON);

    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let job = PreviewJob {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        planes: &planes,
        tools: &[],
        frosted: &[],
        material: &material,
        size: 8,
        spp: 1,
        max_bounces: 4,
    };
    for view in [PreviewView::Front, PreviewView::Top] {
        let scene = remote_scene(&job, view);
        assert_eq!(scene.lighting_preset, LightingPreset::WhiteTray);
        assert!((scene.head_shadow_deg - 8.0).abs() < f32::EPSILON);
        assert!((scene.exposure - PREVIEW_EXPOSURE).abs() < f32::EPSILON);
    }

    // Relations that make it a white surround rather than a photo tent: brighter walls
    // and floor, and none of the tent's black cards.
    let tray = LightingPreset::WhiteTray.params().tent;
    let tent = LightingPreset::LightTent.params().tent;
    assert!(tray.walls > tent.walls);
    assert!(tray.ground > tent.ground);
    assert!(tray.cards.abs() < f32::EPSILON && tent.cards > 0.0);
}

/// The live render's default rig is still the light tent: only the catalogue previews
/// moved to the white surround.
#[test]
fn the_live_default_lighting_is_still_the_light_tent() {
    use crate::settings::model::{AppSettings, DEFAULT_LIGHTING_RIG};

    assert_eq!(
        LightingPreset::from_label(DEFAULT_LIGHTING_RIG),
        LightingPreset::LightTent
    );
    assert_eq!(
        LightingPreset::from_label(&AppSettings::default().lighting_rig),
        LightingPreset::LightTent
    );
    assert_eq!(LightingPreset::default(), LightingPreset::LightTent);
    assert_ne!(PREVIEW_LIGHTING_PRESET, LightingPreset::LightTent);
}

/// The remote scene of a preview job carries the job's tools (a worker traces the stone
/// with them), and none for a planar job.
#[test]
fn the_remote_scene_carries_the_jobs_tools() {
    use indicatrix::geometry::{cuts::StandardGemCuts, tool::ToolPrimitive};

    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let tool: ToolPrimitive = bytemuck::Zeroable::zeroed();
    let with_tool = PreviewJob {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        planes: &planes,
        tools: std::slice::from_ref(&tool),
        frosted: &[],
        material: &material,
        size: 8,
        spp: 1,
        max_bounces: 4,
    };
    let scene = remote_scene(&with_tool, PreviewView::Top);
    assert_eq!(scene.tools.len(), 1);
    let planar = PreviewJob {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        tools: &[],
        ..with_tool
    };
    assert_eq!(
        remote_scene(&planar, PreviewView::Top).tools,
        [] as [indicatrix::geometry::ToolPrimitive; 0]
    );
}
