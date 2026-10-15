use super::*;

fn physics_ruby_without_bands() -> indicatrix::optics::materials::GemMaterial {
    // A pure-host recipe resolves to NO bands: the flag must not be inferred from them.
    indicatrix::optics::materials::GemMaterial::new_custom(
        "Pure Host",
        1.76,
        0.018,
        -0.008,
        [0.0; 3],
    )
}

fn pure_host_context() -> RenderContext {
    RenderContext {
        custom_materials: Arc::new(vec![physics_ruby_without_bands()]),
        material_name: "Pure Host".to_string(),
        ..RenderContext::default()
    }
}

#[test]
fn the_render_context_derives_the_flag_from_the_explicit_list() {
    let mut ctx = pure_host_context();
    assert!(!ctx.physics_color(), "not flagged yet");
    ctx.set_custom_material_physics("pure host", true);
    assert!(ctx.physics_color(), "case-insensitive, and no bands needed");
    // The editor's override (what the design traces) wins over the selected name.
    ctx.material_override = Some(indicatrix::optics::materials::GemMaterial::diamond());
    assert!(!ctx.physics_color());
    ctx.material_override = None;
    ctx.set_custom_material_physics("Pure Host", false);
    assert!(!ctx.physics_color());
}

/// The absorption scale follows the material's own `AbsorptionUnit` tag, not the physics-colour
/// flag: a `ModelUnit` material (flagged or not) gets `1 / K` while no size is set, a `PerMm`
/// one (what a physics recipe builds) takes the 7 mm default over the design's model width.
/// The two values are NOT compared for inequality: with K = 2.52 and a ~17.6 model-unit
/// width they nearly coincide (7 / 17.6 ~ 1 / 2.52), so only exact formulas are asserted.
#[test]
fn export_and_remote_snapshots_apply_the_default_width_to_a_physics_material() {
    use crate::bridge::export_thread::SceneSnapshot;
    use indicatrix::{
        optics::materials::AbsorptionUnit,
        render_setup::{MODEL_UNIT_FACE_UP_PATH, absorption_path_scale_for, measure_model_width},
    };
    let capture = |physics: bool, unit: AbsorptionUnit| {
        let mut ctx = pure_host_context();
        ctx.custom_materials = Arc::new(vec![
            physics_ruby_without_bands().with_absorption_unit(unit),
        ]);
        ctx.set_custom_material_physics("Pure Host", physics);
        let model_width = measure_model_width(&ctx.active_planes);
        let snapshot = SceneSnapshot::capture(&Mutex::new(ctx)).expect("resolves");
        (snapshot, model_width)
    };
    let calibrated = 1.0 / MODEL_UNIT_FACE_UP_PATH;
    for physics in [false, true] {
        let (model_unit, _) = capture(physics, AbsorptionUnit::ModelUnit);
        assert_eq!(model_unit.material.absorption_path_scale, calibrated);
    }
    let (per_mm, model_width) = capture(true, AbsorptionUnit::PerMm);
    let expected = absorption_path_scale_for(AbsorptionUnit::PerMm, 0.0, model_width)
        .expect("the default design measures");
    assert_eq!(per_mm.material.absorption_path_scale, expected);
}

/// Without the physics editor an opened file's "edited by an older version" question is
/// answered "keep the recipe" on the spot: the recipe's material replaces the edited one and
/// renders as physics again.
#[test]
fn keeping_the_pending_recipe_restores_the_recipe_material() {
    use indicatrix::optics::{fluorescence::Fluorescence, materials::GemMaterial};
    let edited = GemMaterial::new_custom("Pure Host", 1.76, 0.018, -0.008, [0.9, 0.1, 0.1]);
    let from_recipe = GemMaterial::new_custom("Pure Host", 1.76, 0.018, -0.008, [0.0; 3]);
    assert_ne!(edited.absorption, from_recipe.absorption);
    let mut ctx = RenderContext {
        custom_materials: Arc::new(vec![edited]),
        material_name: "Pure Host".to_string(),
        dirty: false,
        pending_color_choice: Some((
            "Pure Host".to_string(),
            from_recipe.clone(),
            Fluorescence::new(Vec::new()),
        )),
        ..RenderContext::default()
    };
    assert!(
        !ctx.physics_color(),
        "the edited color is in use until answered"
    );

    assert_eq!(keep_pending_recipe(&mut ctx).as_deref(), Some("Pure Host"));
    assert!(ctx.pending_color_choice.is_none());
    assert_eq!(ctx.custom_materials[0].absorption, from_recipe.absorption);
    assert!(ctx.physics_color());
    assert!(ctx.dirty, "the viewport re-renders with the recipe");

    assert_eq!(keep_pending_recipe(&mut ctx), None, "nothing left to apply");
}

#[test]
fn hex_formats_and_parses() {
    assert_eq!(
        format_hex(slint::Color::from_rgb_u8(255, 0, 128)),
        "#ff0080"
    );
    assert_eq!(parse_hex("#ff0080"), Some([255, 0, 128]));
    assert_eq!(parse_hex("FF0080"), Some([255, 0, 128]));
    assert_eq!(parse_hex(" #f08 "), Some([255, 0, 136]));
    assert_eq!(parse_hex("#ff00"), None);
    assert_eq!(parse_hex("#gg0000"), None);
    assert_eq!(parse_hex(""), None);
}
