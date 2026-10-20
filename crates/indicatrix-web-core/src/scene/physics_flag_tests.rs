//! Tests for the physics-color flag of a custom material spec and the scene it builds: the
//! flag follows the recipe, the default stone width, and the fluorescence a recipe fills.

use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
};
use indicatrix_cut_core::{material::ColorMode, native::color_recipe_dto};

fn spec_with(mode: Option<&ColorMode>, rgb: [f32; 3]) -> CustomMaterialSpec {
    CustomMaterialSpec {
        name: "Ruby X".to_string(),
        mean_ri: 1.77,
        dispersion_delta: 0.018,
        birefringence_delta: -0.008,
        absorption_rgb: Some(rgb.map(f64::from)),
        color_recipe: mode.map(color_recipe_dto),
        absorption_bands: Vec::new(),
        crystal_system: String::new(),
        optical_character: String::new(),
        biaxial_delta_beta_alpha: None,
    }
}

fn pure_host_mode() -> ColorMode {
    let cat = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("corundum", cat.data_version);
    let (tensor, _) = resolve(&recipe, cat).expect("pure host resolves");
    recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    ColorMode::physics(recipe, [0.0; 3])
}

/// The flag follows the recipe the spec carries, not the shape of the built bands: a
/// pure-host recipe (no bands at all) is still physics.
#[test]
fn the_flag_follows_the_recipe_presence() {
    let mode = pure_host_mode();
    let fallback = mode.fallback_rgb();
    assert!(spec_with(Some(&mode), fallback).is_physics_color());
    assert!(!spec_with(None, [0.0; 3]).is_physics_color());
    // A fantasy-active material parks its recipe but renders as fantasy.
    let mut parked = mode.clone();
    parked.switch_to_fantasy();
    assert!(!spec_with(Some(&parked), parked.fallback_rgb()).is_physics_color());
    // An older build edited the top-level color: not physics until the user chooses.
    assert!(!spec_with(Some(&mode), [0.1, 0.2, 2.5]).is_physics_color());
}

/// A zero-band physics recipe is per-millimetre and still gets the 7 mm default width; a
/// fantasy-colour custom (per model unit) gets the face-up calibration scale `1 / K` while no
/// size is set.
#[test]
fn a_pure_host_physics_material_takes_the_default_stone_width() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let mode = pure_host_mode();
    let spec = spec_with(Some(&mode), mode.fallback_rgb());
    assert!(spec.is_physics_color());
    let material_spec = MaterialSpec {
        custom_materials: vec![spec.to_gem_material()],
        ..MaterialSpec::catalogue("Ruby X")
    };
    let physics = resolve_scene_material(&material_spec, &planes).expect("resolves");
    assert!(
        (physics.absorption_path_scale - 1.0 / indicatrix::render_setup::MODEL_UNIT_FACE_UP_PATH)
            .abs()
            > 1e-3,
        "7 mm / model width must rescale the path, got {}",
        physics.absorption_path_scale
    );
    let fantasy = MaterialSpec {
        custom_materials: vec![spec_with(None, [0.2, 0.6, 0.3]).to_gem_material()],
        ..MaterialSpec::catalogue("Ruby X")
    };
    let plain = resolve_scene_material(&fantasy, &planes).expect("resolves");
    assert!(
        (plain.absorption_path_scale - 1.0 / indicatrix::render_setup::MODEL_UNIT_FACE_UP_PATH)
            .abs()
            < f32::EPSILON
    );
}

/// A physics ruby recipe fills the scene's fluorescence (`build_with_customs`), a catalogue
/// material or a fantasy color leaves it empty, and the scene then traces through
/// `handle_trace_chunk` with the emitters beside it.
#[test]
fn a_physics_recipe_fills_the_scenes_fluorescence() {
    let cat = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("corundum", cat.data_version);
    recipe.set_amount("Cr", 0.5);
    let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
    recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    let mode = ColorMode::physics(recipe, [0.0; 3]);
    let custom = spec_with(Some(&mode), mode.fallback_rgb());
    assert_eq!(custom.fluorescence().emitters().len(), 1);
    assert!(spec_with(None, [0.0; 3]).fluorescence().is_empty());

    let scene_spec = |material: MaterialSpec| SceneSpec {
        planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
        finishes: FinishSpec::AllPolished,
        material,
        camera: CameraSpec {
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.2,
        },
        lighting: LightingSpec::new(
            LightingPreset::UvLamp365,
            1.0,
            0.4,
            0.35,
            Backdrop::Grey,
            16.0,
        ),
        max_bounces: 4,
        width: 4,
        height: 4,
        hdr_id: None,
    };
    let ruby = scene_spec(MaterialSpec {
        custom_materials: vec![custom.to_gem_material()],
        ..MaterialSpec::catalogue("Ruby X")
    });
    let customs = [custom];
    let glowing = OwnedScene::build_with_customs(&ruby, None, &customs).expect("builds");
    assert!(!glowing.fluorescence().is_empty());
    let plain = OwnedScene::build(&ruby, None).expect("builds");
    assert!(plain.fluorescence().is_empty());
    // The tool-carrying spec fills it the same way and, without recipes, builds as before.
    let geometry = GeometrySceneSpec {
        scene: ruby,
        tools: Vec::new(),
    };
    let glowing_geometry = geometry.build_with_customs(None, &customs).expect("builds");
    assert!(!glowing_geometry.fluorescence().is_empty());
    assert!(
        geometry
            .build(None)
            .expect("builds")
            .fluorescence()
            .is_empty()
    );
    let diamond = OwnedScene::build_with_customs(
        &scene_spec(MaterialSpec::catalogue("Diamond")),
        None,
        &customs,
    )
    .expect("builds");
    assert!(diamond.fluorescence().is_empty());
    let chunk = crate::render::handle_trace_chunk(&glowing, glowing.plane_soa(), 0, 1, 0, 1);
    assert_eq!(chunk.len(), 16);
}
