use super::{
    AxesCacheKey, AxesStep, HdrIdentity, axes_step, cached_curve_material_is_stale,
    should_recompute_axes, tilt_sweep_material,
};

/// Review fix (absorption units): the tilt sweep traces the very material the render builds
/// for a sized stone (`apply_material_overrides` with only the stone size dialled in), and
/// an unsized stone gets the face-up calibration scale only.
#[test]
fn the_tilt_sweep_material_is_the_sized_render_material() {
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        render_setup::{MaterialOverrides, apply_material_overrides, measure_model_width},
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    let width = measure_model_width(&planes);
    let override_banded = GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
    for (name, material_override) in [("Emerald", None), ("Sapphire", Some(&override_banded))] {
        let bare = material_override
            .cloned()
            .or_else(|| GemMaterial::by_name(name))
            .expect("resolves");
        for stone_width_mm in [0.0_f32, 10.87] {
            let got = tilt_sweep_material(&[], material_override, name, stone_width_mm, &planes)
                .expect("resolves");
            let render = apply_material_overrides(
                bare.clone(),
                &MaterialOverrides {
                    inclusion_sigma_s: 0.0,
                    c_axis_override: None,
                    edge_rounding_radius: 0.0,
                    stone_width_mm,
                },
                width,
            );
            assert_eq!(got, render, "{name} at {stone_width_mm} mm");
        }
    }
    // Sized differs from unsized, and an unknown name resolves to nothing.
    let sized = tilt_sweep_material(&[], None, "Emerald", 10.87, &planes).expect("resolves");
    let unsized_ = tilt_sweep_material(&[], None, "Emerald", 0.0, &planes).expect("resolves");
    assert_ne!(sized.absorption_path_scale, unsized_.absorption_path_scale);
    assert!(tilt_sweep_material(&[], None, "No Such Stone", 10.87, &planes).is_none());
}
use crate::gui::editor::Withheld;
use indicatrix::optics::{materials::GemMaterial, raytracer::LightingPreset};
use indicatrix_solid::preview::StoneGeometryBuf;

/// F4-6: a finished stone that was solved for a design the editor has since left must not
/// launch a sweep -- that would bump the sweep generation and supersede the newer edit's
/// own sweep. A design that does not solve marks the curves stale instead of sweeping the
/// half-cut stone; a good outcome sweeps as before.
#[test]
fn a_stale_stone_launches_no_sweep_and_an_unsolvable_one_marks_the_curves_stale() {
    assert_eq!(axes_step(Err(Withheld::Stale)), AxesStep::Skip);
    assert_eq!(
        axes_step(Err(Withheld::Displaced)),
        AxesStep::Skip,
        "a displaced solve says nothing about the design: no sweep, no stale curves"
    );
    assert_eq!(
        axes_step(Err(Withheld::Unsolvable(
            "tier C1 meets nothing".to_string()
        ))),
        AxesStep::MarkStale
    );
    assert_eq!(axes_step(Ok(None)), AxesStep::Sweep(None));
    let stone = StoneGeometryBuf::default();
    assert_eq!(
        axes_step(Ok(Some(stone.clone()))),
        AxesStep::Sweep(Some(stone))
    );
}

#[test]
fn no_cached_material_is_never_stale() {
    assert!(!cached_curve_material_is_stale(None, "Diamond"));
}

#[test]
fn matching_material_is_not_stale() {
    assert!(!cached_curve_material_is_stale(Some("Diamond"), "Diamond"));
    assert!(!cached_curve_material_is_stale(Some("diamond"), "Diamond"));
}

#[test]
fn differing_material_is_stale() {
    assert!(cached_curve_material_is_stale(Some("Diamond"), "Sapphire"));
}

fn sample_key(material: GemMaterial) -> AxesCacheKey {
    AxesCacheKey {
        lighting_preset: LightingPreset::RingLights,
        light_yaw: 48.0,
        light_pitch: 54.0,
        material,
        planes_hash: 42,
        head_shadow_bits: 16.0f32.to_bits(),
        hdr_identity: HdrIdentity(None),
    }
}

#[test]
fn should_recompute_axes_is_true_the_first_time() {
    let key = sample_key(GemMaterial::all_materials()[0].clone());
    assert!(should_recompute_axes(None, &key));
}

#[test]
fn should_recompute_axes_is_false_once_the_exact_key_was_already_launched() {
    let key = sample_key(GemMaterial::all_materials()[0].clone());
    assert!(!should_recompute_axes(Some(&key), &key));
}

#[test]
fn should_recompute_axes_is_true_when_light_or_geometry_differ() {
    let material = GemMaterial::all_materials()[0].clone();
    let current = sample_key(material);
    let mut moved_light = current.clone();
    moved_light.light_yaw += 1.0;
    assert!(should_recompute_axes(Some(&current), &moved_light));
    let mut moved_geometry = current.clone();
    moved_geometry.planes_hash = 43;
    assert!(should_recompute_axes(Some(&current), &moved_geometry));
}

/// The curves are scored under the selected preset, so switching preset alone must
/// re-launch the sweep.
#[test]
fn should_recompute_axes_is_true_when_only_the_lighting_preset_differs() {
    let current = sample_key(GemMaterial::all_materials()[0].clone());
    let mut other_preset = current.clone();
    other_preset.lighting_preset = LightingPreset::LightTent;
    assert!(should_recompute_axes(Some(&current), &other_preset));
}

/// The curves are scored under the head shadow and the HDR map in force, so changing either
/// must re-launch the sweep. An HDR map is compared by identity: the same `Arc` is the same
/// map, another `Arc` (even of equal content) is a new one.
#[test]
fn head_shadow_and_hdr_identity_force_a_resweep() {
    use indicatrix::renderer::env_map::EnvironmentMap;
    use std::sync::Arc;
    let current = sample_key(GemMaterial::all_materials()[0].clone());

    let mut other_head_shadow = current.clone();
    other_head_shadow.head_shadow_bits = 30.0f32.to_bits();
    assert!(should_recompute_axes(Some(&current), &other_head_shadow));

    let map = Arc::new(EnvironmentMap::uniform(4, 2, [1.0; 3]));
    let mut with_map = current.clone();
    with_map.hdr_identity = HdrIdentity(Some(Arc::clone(&map)));
    assert!(should_recompute_axes(Some(&current), &with_map));
    assert!(should_recompute_axes(Some(&with_map), &current));

    let mut same_map = current.clone();
    same_map.hdr_identity = HdrIdentity(Some(Arc::clone(&map)));
    assert!(
        !should_recompute_axes(Some(&with_map), &same_map),
        "the same Arc is the same map"
    );

    let mut equal_content = current;
    equal_content.hdr_identity =
        HdrIdentity(Some(Arc::new(EnvironmentMap::uniform(4, 2, [1.0; 3]))));
    assert!(
        should_recompute_axes(Some(&with_map), &equal_content),
        "another Arc is another map"
    );
}

/// A custom material can be edited (RI, birefringence, dispersion, ...) while a
/// tier keeps referencing the same name -- so the dedup key must change even
/// though the name didn't. Folding the fully resolved `GemMaterial` into
/// `AxesCacheKey` is what makes that true, via its derived `PartialEq`.
#[test]
fn should_recompute_axes_is_true_when_only_the_resolved_materials_optics_differ() {
    let mut edited = GemMaterial::all_materials()[0].clone();
    let current = sample_key(edited.clone());
    edited.birefringence_delta += 0.01;
    let requested = sample_key(edited);
    assert_eq!(
        current.material.name, requested.material.name,
        "the display name must be unchanged -- that is the whole point of this case"
    );
    assert!(should_recompute_axes(Some(&current), &requested));
}
