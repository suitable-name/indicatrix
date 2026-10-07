//! Settings persistence and the settings-to-scene conversion.

use super::*;
use crate::scene::OwnedScene;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    render_setup::{
        MaterialOverrides, apply_material_overrides, measure_model_width,
        resolve_material_with_override,
    },
};
use indicatrix_cut_core::MaterialSelection;
use indicatrix_editor::material_lookup::{EditorMaterialLookup, traced_gem_material};

/// A payload exactly as the first web build wrote it (before the link, denoise, HDR and
/// auto-solve fields existed).
const FIRST_BUILD_PAYLOAD: &str = r#"{"render":{"material":"Sapphire","lighting":"Light tent + black cards","exposure":1.25,"light_yaw_deg":30.0,"light_pitch_deg":60.0,"backdrop_index":2,"max_bounces":16,"target_spp":512,"camera_yaw":0.7,"camera_pitch":0.3,"camera_distance":3.0,"view_tab":1},"design_name":"round.asc","design_unsaved":true}"#;

#[test]
fn a_first_build_payload_still_loads_with_the_same_values() {
    let payload = SessionPayload::from_json(FIRST_BUILD_PAYLOAD).expect("parses");
    let r = &payload.render;
    assert_eq!(r.material, "Sapphire");
    assert_eq!(r.lighting, "Light tent + black cards");
    assert_eq!(r.exposure, 1.25);
    assert_eq!((r.light_yaw_deg, r.light_pitch_deg), (30.0, 60.0));
    assert_eq!(r.backdrop_index, 2);
    assert_eq!((r.max_bounces, r.target_spp), (16, 512));
    assert_eq!(
        (r.camera_yaw, r.camera_pitch, r.camera_distance),
        (0.7, 0.3, 3.0)
    );
    assert_eq!(r.view_tab, 1);
    assert_eq!(payload.design_name.as_deref(), Some("round.asc"));
    assert!(payload.design_unsaved);
    // Fields the first build did not write take the desktop defaults.
    let defaults = RenderSettings::default();
    assert!(r.link_material && r.denoise && r.use_hdr);
    assert!(!r.frosted_girdle && !r.c_axis_override);
    assert_eq!(r.export, defaults.export);
    // Sanitising changes nothing that was in range.
    assert_eq!(&r.clone().sanitized(), r);

    // Written back, every original key keeps its value.
    let original: serde_json::Value = serde_json::from_str(FIRST_BUILD_PAYLOAD).expect("json");
    let written: serde_json::Value =
        serde_json::from_str(&payload.to_json().expect("writes")).expect("json");
    for (key, value) in original["render"].as_object().expect("object") {
        assert_eq!(&written["render"][key], value, "render.{key}");
    }
    assert_eq!(written["design_name"], original["design_name"]);
    assert_eq!(written["design_unsaved"], original["design_unsaved"]);
    assert_eq!(
        SessionPayload::from_json(&payload.to_json().expect("writes")),
        Ok(payload)
    );
}

#[test]
fn empty_partial_and_out_of_range_payloads_are_repaired() {
    assert_eq!(
        SessionPayload::from_json("{}"),
        Ok(SessionPayload::default())
    );
    assert!(SessionPayload::from_json("not json").is_err());
    let wild = SessionPayload::from_json(
        r#"{"render":{"exposure":99.0,"max_bounces":1,"target_spp":100000,"lighting":"nope",
            "backdrop_index":9,"head_shadow_deg":80.0,"inclusion_sigma_s":-1.0,"edge_rounding_radius":1.0,
            "stone_width_mm":50.0,"c_axis_tilt_deg":400.0,"view_tab":7,
            "export":{"long_edge":99999,"spp":1,"color_space":5}}}"#,
    )
    .expect("parses")
    .render
    .sanitized();
    assert_eq!(wild.exposure, EXPOSURE_RANGE.1);
    assert_eq!(wild.max_bounces, BOUNCE_RANGE.0);
    assert_eq!(wild.target_spp, LIVE_SPP_RANGE.1);
    assert_eq!(wild.lighting_preset(), LightingPreset::LightTent);
    assert_eq!(wild.backdrop_index, Backdrop::from_index(9).index());
    assert_eq!(wild.head_shadow_deg, HEAD_SHADOW_RANGE.1);
    assert_eq!(wild.inclusion_sigma_s, 0.0);
    assert_eq!(wild.edge_rounding_radius, EDGE_ROUNDING_RANGE.1);
    assert_eq!(wild.stone_width_mm, STONE_WIDTH_RANGE.1);
    assert_eq!(wild.c_axis_tilt_deg, 90.0);
    assert_eq!(wild.view_tab, 2);
    assert_eq!(wild.export.long_edge, MAX_EXPORT_EDGE);
    assert_eq!(wild.export.spp, EXPORT_MIN_SPP);
    assert_eq!(wild.export.color_space, 1);
    let nan = RenderSettings {
        exposure: f32::NAN,
        camera_distance: f32::INFINITY,
        ..RenderSettings::default()
    }
    .sanitized();
    assert_eq!(nan.exposure, 1.0);
    let nan_shadow = RenderSettings {
        head_shadow_deg: f32::NAN,
        ..RenderSettings::default()
    }
    .sanitized();
    assert_eq!(nan_shadow.head_shadow_deg, DEFAULT_HEAD_SHADOW_DEG);
    assert_eq!(
        RenderSettings::default().lighting_spec().head_shadow_deg,
        16.0
    );
    assert_eq!(nan.camera_distance, 2.4, "non-finite takes the default");
}

#[test]
fn the_auto_solve_budget_defaults_round_trips_and_is_capped() {
    // A payload from before the budget existed takes the desktop's 300 ms.
    let old = SessionPayload::from_json(FIRST_BUILD_PAYLOAD).expect("parses");
    assert_eq!(old.auto_solve_budget_ms, DEFAULT_AUTO_SOLVE_BUDGET_MS);
    assert_eq!(old.auto_solve_budget(), 300);
    // Off (0) is a real choice and survives the round trip.
    let off = SessionPayload {
        auto_solve_budget_ms: 0,
        ..SessionPayload::default()
    };
    let back = SessionPayload::from_json(&off.to_json().expect("writes")).expect("parses");
    assert_eq!(back.auto_solve_budget(), 0);
    let wild = SessionPayload::from_json(r#"{"auto_solve_budget_ms":99999999}"#).expect("parses");
    assert_eq!(wild.auto_solve_budget(), 60_000);
    // The combo's five entries, and the desktop's "anything else reads 3 s".
    assert_eq!(AUTO_SOLVE_BUDGETS_MS, [0, 150, 300, 1000, 3000]);
    assert_eq!(auto_solve_budget_index(0), 0);
    assert_eq!(auto_solve_budget_index(300), 2);
    assert_eq!(auto_solve_budget_index(1000), 3);
    assert_eq!(auto_solve_budget_index(777), 4);
}

#[test]
fn the_c_axis_matches_the_desktop_angles() {
    let close = |a: Vec3, b: Vec3| (a - b).length() < 1e-6;
    assert!(close(c_axis_from_angles(0.0, 0.0), Vec3::Y));
    assert!(close(c_axis_from_angles(90.0, 0.0), Vec3::X));
    assert!(close(c_axis_from_angles(90.0, 90.0), Vec3::Z));
    // An off-axis pair against hand-computed values (tilt 37 degrees from +Y, azimuth
    // 211 degrees): x = sin 37 * cos 211, y = cos 37, z = sin 37 * sin 211.
    let axis = c_axis_from_angles(37.0, 211.0);
    assert!(
        (axis - Vec3::new(-0.515_856, 0.798_636, -0.309_958)).length() < 1e-5,
        "{axis:?}"
    );
    assert!((axis.length() - 1.0).abs() < 1e-6, "a unit vector");
}

fn linked_design() -> Design {
    let mut design = indicatrix_editor::EditorSession::fresh().design;
    design.material = MaterialSelection {
        name: Some("Sapphire".to_string()),
        refractive_index_override: Some(1.765),
        body_color_override: Some([0.2, 0.3, 0.9]),
        ..MaterialSelection::default()
    };
    design.girdle_diameter_mm = Some(6.5);
    design
}

/// The desktop's render context for the same settings, built with its own
/// conversions, against the web spec: field by field, then the built scene.
#[test]
fn a_linked_scene_matches_the_desktop_render_context() {
    let settings = RenderSettings {
        material: "Diamond".to_string(),
        lighting: LightingPreset::DaylightDome.label().to_string(),
        exposure: 1.3,
        light_yaw_deg: 200.0,
        light_pitch_deg: 89.9,
        backdrop_index: Backdrop::White.index(),
        max_bounces: 9,
        frosted_girdle: true,
        c_axis_override: true,
        c_axis_tilt_deg: 30.0,
        c_axis_azimuth_deg: 45.0,
        inclusion_sigma_s: 0.4,
        edge_rounding_radius: 0.01,
        stone_width_mm: 3.0,
        ..RenderSettings::default()
    };
    let design = linked_design();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = render_material(&settings, Some(&design), &[]).expect("resolves");
    assert_eq!(material.name, "Sapphire");
    assert_eq!(
        material.stone_width_mm, 6.5,
        "the design's girdle, not the slider"
    );
    let spec = scene_spec(
        &settings,
        &SceneInputs {
            planes: &planes,
            material: &material,
            custom_materials: &[],
            width: 10,
            height: 7,
            hdr_id: None,
        },
    );

    // The desktop's `RenderContext` fields for these settings.
    let light_yaw = 200.0_f32.to_radians();
    let light_pitch = 89.9_f32.to_radians().clamp(0.15, 1.55);
    assert_eq!(spec.lighting.light_yaw.to_bits(), light_yaw.to_bits());
    assert_eq!(spec.lighting.light_pitch.to_bits(), light_pitch.to_bits());
    assert_eq!(spec.lighting.preset(), LightingPreset::DaylightDome);
    assert_eq!(spec.lighting.backdrop(), Backdrop::White);
    assert_eq!(spec.lighting.exposure, 1.3);
    assert_eq!(spec.finishes, FinishSpec::FrostedGirdle);
    assert_eq!(spec.max_bounces, 9);
    assert_eq!(
        (spec.camera.yaw, spec.camera.pitch, spec.camera.distance),
        (0.35, 1.15, 2.4)
    );
    let overrides = MaterialOverrides {
        inclusion_sigma_s: 0.4,
        c_axis_override: Some(c_axis_from_angles(30.0, 45.0)),
        edge_rounding_radius: 0.01,
        stone_width_mm: 6.5,
    };
    let web = spec.material.overrides.to_overrides();
    assert_eq!(web.inclusion_sigma_s, overrides.inclusion_sigma_s);
    assert_eq!(web.c_axis_override, overrides.c_axis_override);
    assert_eq!(web.edge_rounding_radius, overrides.edge_rounding_radius);
    assert_eq!(web.stone_width_mm, overrides.stone_width_mm);

    // The desktop's material: `sync_viewport_material_link`'s override, then
    // `resolve_material_and_quality` and `apply_material_overrides`.
    let material_override = traced_gem_material(
        "Sapphire",
        &design.material,
        &EditorMaterialLookup::new(&[]),
    );
    let desktop_material = apply_material_overrides(
        resolve_material_with_override(
            &GemMaterial::all_materials(),
            &[],
            material_override.as_ref(),
            "Sapphire",
        )
        .expect("resolves"),
        &overrides,
        measure_model_width(&planes),
    );
    let scene = OwnedScene::build(&spec, None).expect("builds");
    assert_eq!(scene.material(), &desktop_material);
}

#[test]
fn unlinked_uses_the_header_material_and_the_stone_width_control() {
    let settings = RenderSettings {
        material: "Spinel".to_string(),
        link_material: false,
        stone_width_mm: 4.0,
        ..RenderSettings::default()
    };
    let material = render_material(&settings, Some(&linked_design()), &[]).expect("resolves");
    assert_eq!(
        material,
        RenderMaterial {
            name: "Spinel".to_string(),
            linked_design: None,
            stone_width_mm: 4.0,
        }
    );
    // No design: the header material even while linked.
    let linked = RenderSettings::default();
    assert_eq!(
        render_material(&linked, None, &[]).expect("resolves").name,
        "Diamond"
    );
    // A linked design naming a material nobody has is refused, not substituted.
    let mut design = linked_design();
    design.material.name = Some("Unobtainium".to_string());
    assert!(render_material(&linked, Some(&design), &[]).is_err());
}

#[test]
fn the_option_lists_match_the_desktop() {
    let options = material_options(&[]);
    assert_eq!(options.len(), GemMaterial::all_materials().len());
    assert!(
        options
            .windows(2)
            .all(|w| w[0].to_ascii_lowercase() <= w[1].to_ascii_lowercase())
    );
    let custom = GemMaterial::new_custom("Zz custom", 1.6, 0.01, 0.0, [0.0; 3]);
    assert_eq!(
        material_options(&[custom]).last().map(String::as_str),
        Some("Zz custom")
    );
    assert_eq!(lighting_options()[0], LightingPreset::LightTent.label());
    assert_eq!(lighting_options()[1], LightingPreset::IsoHemisphere.label());
    assert_eq!(lighting_options()[2], LightingPreset::WhiteTray.label());
    assert_eq!(lighting_options()[5], LightingPreset::DaylightDome.label());
    assert_eq!(lighting_options().len(), 13, "no UV lamp is offered");
    let uv = RenderSettings {
        lighting: LightingPreset::UvLamp365.label().to_string(),
        ..RenderSettings::default()
    };
    assert_eq!(uv.lighting_preset(), LightingPreset::default());
    assert_eq!(uv.sanitized().lighting, LightingPreset::default().label());
}
