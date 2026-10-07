//! Material/RI combo, Yield-form parsing, gear preset/remap, and RI-source-text tests.

use crate::material::*;
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{ConstraintTier, Design, MaterialSelection, RemapRounding};

#[test]
fn material_index_and_name_round_trip_for_every_preset() {
    let names = builtin_preset_names();
    for (index, name) in names.iter().enumerate() {
        let expected_name = (name != "(none)").then(|| name.clone());
        assert_eq!(material_name_from_index(index as i32), expected_name);
        assert_eq!(
            material_index_from_name(expected_name.as_deref()),
            index as i32
        );
    }
}

/// `builtin_preset_names` must list EVERY built-in
/// `GemMaterial::all_materials()` preset, not a hardcoded subset. This ensures the
/// CAD part supports all materials from the renderer.
#[test]
fn builtin_preset_names_covers_every_renderer_built_in() {
    let names = builtin_preset_names();
    let all_material_names: Vec<String> =
        indicatrix::optics::materials::GemMaterial::all_materials()
            .into_iter()
            .map(|m| m.name)
            .collect();
    assert_eq!(names[0], "(none)");
    assert_eq!(&names[1..], all_material_names.as_slice());
    for expected in [
        "Aquamarine",
        "Morganite",
        "Chrysoberyl (Yellow)",
        "Amethyst",
        "Citrine",
        "Pyrope Garnet",
        "Almandine Garnet",
        "Spessartine Garnet",
        "Grossular Garnet (Tsavorite)",
        "Andradite Garnet (Demantoid)",
        "Peridot",
        "YAG",
        "GGG",
        "Benitoite",
        "Andalusite",
        "Opal",
        "Glass (N-BK7)",
        "Glass (F2)",
        "Rutile",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "{expected} must be offered -- it was traceable in Live Render but never \
             namable on a design before this fix"
        );
    }
}

#[test]
fn material_index_from_name_falls_back_to_zero_for_an_unknown_name() {
    assert_eq!(material_index_from_name(Some("Garnet")), 0);
    assert_eq!(material_index_from_name(None), 0);
}

#[test]
fn parse_yield_form_blank_fields_mean_unset() {
    let (girdle_diameter_mm, material) =
        parse_yield_form("", 0, "", &MaterialSelection::none()).unwrap();
    assert_eq!(girdle_diameter_mm, None);
    assert_eq!(material, MaterialSelection::none());
}

#[test]
fn parse_yield_form_reads_girdle_diameter_and_specific_gravity_override() {
    let current = MaterialSelection {
        name: Some("Diamond".to_string()),
        ..MaterialSelection::none()
    };
    let (girdle_diameter_mm, material) = parse_yield_form("8.2", 1, "3.515", &current).unwrap();
    assert_eq!(girdle_diameter_mm, Some(8.2));
    assert_eq!(material.name.as_deref(), Some("Diamond"));
    assert_eq!(material.specific_gravity_override, Some(3.515));
}

// `parse_yield_form` ignores the `material_index` parameter -- `name` and
// `refractive_index_override` always carry through from `current` unchanged,
// regardless of what the combo (still passed as `material_index`, here `0`) shows.
// The Yield tab's Material combo has no way to represent a custom catalogue
// material or an RI override, so the form preserves them from the current design.
#[test]
fn parse_yield_form_preserves_the_current_material_name_and_ri_override_regardless_of_material_index()
 {
    let current = MaterialSelection {
        name: Some("My Custom Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.74),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let (_, material) = parse_yield_form("", 0, "4.1", &current).unwrap();
    assert_eq!(material.name.as_deref(), Some("My Custom Garnet"));
    assert_eq!(material.refractive_index_override, Some(1.74));
    assert_eq!(material.specific_gravity_override, Some(4.1));
}

#[test]
fn parse_yield_form_rejects_zero_or_negative_or_unparseable_values() {
    let none = MaterialSelection::none();
    assert!(parse_yield_form("0.0", 0, "", &none).is_err());
    assert!(parse_yield_form("-1.0", 0, "", &none).is_err());
    assert!(parse_yield_form("wide", 0, "", &none).is_err());
    assert!(parse_yield_form("", 0, "0.0", &none).is_err());
    assert!(parse_yield_form("", 0, "-1.0", &none).is_err());
    assert!(parse_yield_form("", 0, "heavy", &none).is_err());
}

// --- design_material_options / index / name ---

#[test]
fn design_material_options_lists_built_ins_then_customs_then_the_custom_ri_sentinel() {
    let custom = [{
        let mut m = GemMaterial::diamond();
        m.name = "MyGarnet".to_string();
        m
    }];
    let options = design_material_options(&custom);
    assert_eq!(options[0], "(none)");
    assert_eq!(options[1], "Diamond");
    assert_eq!(options[options.len() - 2], "MyGarnet");
    assert_eq!(options[options.len() - 1], "Custom RI\u{2026}");
}

#[test]
fn design_material_options_does_not_duplicate_a_custom_material_sharing_a_built_in_name() {
    let custom = [{
        let mut m = GemMaterial::diamond();
        m.name = "Diamond".to_string();
        m
    }];
    let options = design_material_options(&custom);
    assert_eq!(options.iter().filter(|&n| n == "Diamond").count(), 1);
}

#[test]
fn design_material_index_and_name_round_trip_for_a_built_in_and_a_custom() {
    let custom = [{
        let mut m = GemMaterial::diamond();
        m.name = "MyGarnet".to_string();
        m
    }];
    let options = design_material_options(&custom);
    let diamond_index = design_material_index_from_name(Some("Diamond"), &options);
    assert_eq!(
        design_material_name_from_index(diamond_index, &options).as_deref(),
        Some("Diamond")
    );
    let custom_index = design_material_index_from_name(Some("MyGarnet"), &options);
    assert_eq!(
        design_material_name_from_index(custom_index, &options).as_deref(),
        Some("MyGarnet")
    );
}

#[test]
fn design_material_name_from_index_is_none_for_none_and_the_custom_ri_sentinel() {
    let options = design_material_options(&[]);
    assert_eq!(design_material_name_from_index(0, &options), None);
    let last = (options.len() - 1) as i32;
    assert_eq!(design_material_name_from_index(last, &options), None);
}

#[test]
fn design_material_index_from_name_falls_back_to_zero_when_absent() {
    let options = design_material_options(&[]);
    assert_eq!(
        design_material_index_from_name(Some("Not A Material"), &options),
        0
    );
    assert_eq!(design_material_index_from_name(None, &options), 0);
}

// --- parse_design_material_form ---

#[test]
fn parse_design_material_form_blank_override_means_use_the_resolved_material() {
    let options = design_material_options(&[]);
    let current = MaterialSelection::none();
    let material = parse_design_material_form(1, "", &options, &current).unwrap();
    assert_eq!(material.name.as_deref(), Some("Diamond"));
    assert_eq!(material.refractive_index_override, None);
}

#[test]
fn parse_design_material_form_reads_a_valid_override_and_keeps_the_sg_override() {
    let options = design_material_options(&[]);
    let mut current = MaterialSelection::none();
    current.specific_gravity_override = Some(3.5);
    let material = parse_design_material_form(9, "1.544", &options, &current).unwrap();
    assert_eq!(material.name.as_deref(), Some("Quartz"));
    assert_eq!(material.refractive_index_override, Some(1.544));
    assert_eq!(material.specific_gravity_override, Some(3.5));
}

#[test]
fn parse_design_material_form_rejects_an_ri_at_or_below_one() {
    let options = design_material_options(&[]);
    let current = MaterialSelection::none();
    assert!(parse_design_material_form(0, "1.0", &options, &current).is_err());
    assert!(parse_design_material_form(0, "0.5", &options, &current).is_err());
    assert!(parse_design_material_form(0, "not-a-number", &options, &current).is_err());
}

/// The body color carries through while the material stays the same species and
/// falls back to the new material's own color when the species changes.
#[test]
fn parse_design_material_form_keeps_the_body_color_only_for_the_same_material() {
    let options = design_material_options(&[]);
    let yellow = [0.2f32, 0.4, 2.8];
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let diamond_index = design_material_index_from_name(Some("Diamond"), &options);
    let current = MaterialSelection {
        name: Some("Quartz".to_string()),
        ..MaterialSelection::none()
    }
    .with_body_color(Some(yellow));
    let same = parse_design_material_form(quartz_index, "1.55", &options, &current).unwrap();
    assert_eq!(same.body_color_override, Some(yellow));
    let other = parse_design_material_form(diamond_index, "", &options, &current).unwrap();
    assert_eq!(other.body_color_override, None);
}

// --- body color combo ---

#[test]
fn body_color_combo_index_round_trips_every_preset_and_the_default() {
    let options = body_color_options();
    assert_eq!(options[0], "Material default");
    assert_eq!(options.len(), 11);
    assert_eq!(
        options.last().map(String::as_str),
        Some(BODY_COLOR_CUSTOM_LABEL)
    );
    assert_eq!(body_color_custom_index(), 10);
    assert_eq!(body_color_from_index(0), None);
    assert_eq!(body_color_index_for(None), 0);
    for index in 1..body_color_custom_index() {
        let rgb = body_color_from_index(index);
        assert!(rgb.is_some(), "index {index} must name a preset");
        assert_eq!(body_color_index_for(rgb), index);
    }
    assert_eq!(body_color_from_index(99), None);
    assert_eq!(body_color_from_index(-1), None);
    assert_eq!(options[6], "Yellow");
}

#[test]
fn an_unlisted_body_color_triple_maps_to_the_custom_entry() {
    let custom = Some([0.5, 0.5, 0.5]);
    assert_eq!(body_color_index_for(custom), body_color_custom_index());
    // The custom entry names no table triple; an Apply keeps the design's own.
    assert_eq!(body_color_from_index(body_color_custom_index()), None);
    assert_eq!(
        body_color_for_apply(body_color_custom_index(), custom),
        custom
    );
    assert_eq!(body_color_for_apply(body_color_custom_index(), None), None);
    assert_eq!(body_color_for_apply(0, custom), None);
    assert_eq!(body_color_for_apply(2, custom), body_color_from_index(2));
    // A solved triple (any finite triple) round-trips through the index.
    let solved = Some([0.123, 0.456, 0.789]);
    assert_eq!(body_color_index_for(solved), body_color_custom_index());
}

// --- gear_index_from_teeth / gear_choice_to_teeth ---

#[test]
fn gear_index_and_choice_round_trip_for_every_preset() {
    for (i, &teeth) in GEAR_PRESETS.iter().enumerate() {
        assert_eq!(gear_index_from_teeth(teeth), i as i32);
        assert_eq!(gear_choice_to_teeth(i as i32, "").unwrap(), teeth);
    }
}

#[test]
fn gear_index_from_teeth_is_the_custom_slot_for_an_unlisted_gear() {
    assert_eq!(gear_index_from_teeth(50), GEAR_PRESETS.len() as i32);
}

#[test]
fn gear_choice_to_teeth_reads_the_custom_text_field_outside_the_preset_list() {
    let custom_index = GEAR_PRESETS.len() as i32;
    assert_eq!(gear_choice_to_teeth(custom_index, "50").unwrap(), 50);
    assert!(gear_choice_to_teeth(custom_index, "0").is_err());
    assert!(gear_choice_to_teeth(custom_index, "-5").is_err());
    assert!(gear_choice_to_teeth(custom_index, "wide").is_err());
}

// --- gear_remap_preview ---

#[test]
fn gear_remap_preview_flags_non_integral_positions_independently_of_rounding() {
    let mut design = Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        96,
        8,
        1.54,
    );
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![0.0, 12.0, 24.0], // multiples of 12: land cleanly at 80/96 * 12 = 10
        constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    let rows = gear_remap_preview(&design, 96, 80, RemapRounding::Nearest);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name.as_str(), "P1");
    // 12 * 80/96 = 10.0 exactly -- every position lands on a whole tooth.
    assert!(
        !rows[0].non_integral,
        "expected a clean remap: {}",
        rows[0].new_indices
    );
    assert_eq!(rows[0].new_indices.as_str(), "0, 10, 20");
}

#[test]
fn gear_remap_preview_flags_a_lossy_remap_as_non_integral() {
    let mut design = Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        96,
        8,
        1.54,
    );
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![1.0], // 1 * 80/96 = 0.8333... -- not a whole tooth
        constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    let rows = gear_remap_preview(&design, 96, 80, RemapRounding::Nearest);
    assert!(rows[0].non_integral);
}

#[test]
fn gear_remap_preview_does_not_mutate_the_original_design() {
    let mut design = Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        96,
        8,
        1.54,
    );
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![0.0, 12.0],
        constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    let before = design.clone();
    let _ = gear_remap_preview(&design, 96, 80, RemapRounding::Nearest);
    assert_eq!(design, before);
}

// --- ri_source_text ---

#[test]
fn ri_source_text_reports_a_typed_override_first_regardless_of_material() {
    let material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.62),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let text = ri_source_text(&material, &[]);
    assert!(text.contains("override"));
    assert!(text.contains("1.6200"));
}

#[test]
fn ri_source_text_names_a_custom_catalogue_material_before_a_same_named_built_in_would_apply() {
    // `Design::effective_refractive_index_with` resolves a custom material by name
    // BEFORE falling back to a built-in -- this must report exactly that source,
    // never silently claim "built-in" for a name a custom entry also matches.
    let mut custom = GemMaterial::diamond();
    custom.name = "My Custom Garnet".to_string();
    let material = MaterialSelection {
        name: Some("My Custom Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let text = ri_source_text(&material, std::slice::from_ref(&custom));
    assert!(text.contains("custom catalogue material"));
    assert!(text.contains("My Custom Garnet"));
}

#[test]
fn ri_source_text_names_a_built_in_when_no_custom_entry_matches() {
    let material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let text = ri_source_text(&material, &[]);
    assert!(text.contains("built-in material"));
    assert!(text.contains("Quartz"));
}

#[test]
fn ri_source_text_flags_an_unrecognized_name_and_no_selection_as_the_legacy_fallback() {
    let unrecognized = MaterialSelection {
        name: Some("Not A Real Material".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    assert!(ri_source_text(&unrecognized, &[]).contains("legacy imported value"));
    assert!(ri_source_text(&MaterialSelection::none(), &[]).contains("legacy imported value"));
}
