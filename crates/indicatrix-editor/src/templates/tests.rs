//! Tests of the gallery template builder.

use super::*;
use indicatrix_cut_core::{ConstraintTier, templates::ShapeFamily};

fn selection(name: &str) -> MaterialSelection {
    MaterialSelection {
        name: Some(name.to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
}

fn choice(template_index: i32, gear_teeth: i32, material: MaterialSelection) -> NewDesignChoice {
    NewDesignChoice {
        template_index,
        gear_teeth,
        material,
        girdle_diameter_mm: None,
        custom_materials: Vec::new(),
    }
}

/// The template's index for the first template of `family`.
fn index_of(family: ShapeFamily) -> i32 {
    let position = TEMPLATES
        .iter()
        .position(|spec| spec.family == family)
        .expect("the family has a template");
    session_index(position)
}

#[test]
fn every_template_gets_a_card_after_empty() {
    let cards = template_cards();
    assert_eq!(
        cards.len(),
        indicatrix_cut_core::templates::TEMPLATES.len() + 1
    );
    assert_eq!(cards[0].name, "Empty");
    assert!(cards.iter().all(|card| card.ready));
    assert_eq!(
        cards[1].name,
        indicatrix_cut_core::templates::TEMPLATES[0].name
    );
}

/// The flat list keeps the numbering the web app and the guide use: the five
/// templates that existed first keep indices 1 to 5.
#[test]
fn the_flat_numbering_is_stable() {
    let names: Vec<String> = template_cards().into_iter().map(|card| card.name).collect();
    assert_eq!(
        &names[..6],
        [
            "Empty",
            "Standard Round Brilliant",
            "Round Brilliant -- Shallow Pavilion",
            "Round Brilliant -- Deep Pavilion",
            "Simple Teaching Design",
            "Rich Teaching Design",
        ]
    );
}

#[test]
fn the_grouped_gallery_lists_the_top_five_then_the_variants_then_blank() {
    let cards = gallery_cards();
    let in_group = |group: GalleryGroup| -> Vec<(i32, usize, &str)> {
        cards
            .iter()
            .filter(|card| card.group == group)
            .map(|card| (card.template_index, card.slot, card.name.as_str()))
            .collect()
    };
    assert_eq!(
        in_group(GalleryGroup::Shapes),
        vec![
            (1, 0, "Standard Round Brilliant"),
            (6, 1, "Oval Brilliant"),
            (7, 2, "Cushion Brilliant"),
            (8, 3, "Emerald Step Cut"),
            (9, 4, "Princess Cut"),
        ]
    );
    assert_eq!(
        in_group(GalleryGroup::Variants),
        vec![
            (2, 0, "Round Brilliant -- Shallow Pavilion"),
            (3, 1, "Round Brilliant -- Deep Pavilion"),
            (4, 2, "Simple Teaching Design"),
            (5, 3, "Rich Teaching Design"),
        ]
    );
    assert_eq!(in_group(GalleryGroup::Blank), vec![(0, 0, "Empty")]);
    // The sections come out in order: shapes, variants, blank.
    let sections: Vec<i32> = cards.iter().map(|card| card.group.code()).collect();
    assert_eq!(sections, vec![1, 1, 1, 1, 1, 2, 2, 2, 2, 0]);
    assert_eq!(GalleryGroup::Shapes.title(), "Shapes");
    assert_eq!(
        GalleryGroup::Variants.title(),
        "Round variants and teaching designs"
    );
}

/// Every template index appears exactly once and names the template it should.
#[test]
fn grouped_cards_keep_their_session_indices() {
    let cards = gallery_cards();
    let mut indices: Vec<i32> = cards.iter().map(|card| card.template_index).collect();
    indices.sort_unstable();
    let all: Vec<i32> = (0..=i32::try_from(TEMPLATES.len()).unwrap()).collect();
    assert_eq!(indices, all);
    for card in cards.iter().filter(|card| card.template_index >= 1) {
        let spec = template_spec(card.template_index).expect("a template");
        assert_eq!(card.name, spec.name);
        assert_eq!(card.design_ri, Some(spec.design_ri));
        assert_eq!(
            card.default_material.as_deref(),
            Some(spec.default_material)
        );
        assert_eq!(card.allowed_gears[0], spec.gear_teeth);
        assert!(card.designed_for().starts_with("Designed for "));
    }
    assert!(template_spec(0).is_none());
    assert!(template_spec(-1).is_none());
    assert!(template_spec(i32::try_from(TEMPLATES.len()).unwrap() + 1).is_none());
}

/// The gear combo of each card lists the template's allowed gears, 96 first.
#[test]
fn card_gears_are_the_allowed_gears() {
    for card in gallery_cards() {
        assert_eq!(card.allowed_gears[0], 96, "{}", card.name);
        if card.template_index == 0 {
            assert_eq!(card.allowed_gears, GEAR_PRESETS.to_vec());
        } else if card.name == "Emerald Step Cut" {
            assert_eq!(card.allowed_gears, vec![96, 80, 72, 64, 120]);
        } else {
            assert_eq!(card.allowed_gears, vec![96, 80, 64], "{}", card.name);
        }
    }
}

/// Every card has a design to draw, and it is a closed solid.
#[test]
fn every_gallery_design_solves_and_closes() {
    for card in gallery_cards() {
        let design = gallery_design(card.template_index);
        design
            .solve()
            .unwrap_or_else(|e| panic!("{}: failed to solve: {e:?}", card.name));
        assert!(design.is_closed(), "{}", card.name);
        assert_eq!(
            design.tiers.is_empty(),
            card.template_index == 0,
            "{}",
            card.name
        );
    }
}

// ---- the pure adaptation decision ---------------------------------------------

#[test]
fn adapting_is_attempted_only_when_the_index_differs() {
    assert!(!adaptation_needed(1.768, 1.768));
    assert!(!adaptation_needed(1.544, 1.54));
    assert!(adaptation_needed(1.544, 1.768));
    assert!(adaptation_needed(2.417, 1.544));

    let same = settle_adaptation(1.768, 1.76808, || {
        panic!("the gate must not run when the index matches")
    });
    assert_eq!(same, AngleAdaptation::NotNeeded);
    assert_eq!(same.message(), None);

    let differs = settle_adaptation(1.768, 1.544, || Ok("Valid: girdle 2.0 %".to_string()));
    assert!(differs.adapted());
    assert_eq!(
        differs,
        AngleAdaptation::Adapted {
            n_from: 1.768,
            n_to: 1.544,
            headline: "Valid: girdle 2.0 %".to_string(),
        }
    );
    let text = differs.message().expect("a message");
    assert!(text.contains("1.768") && text.contains("1.544"), "{text}");
}

#[test]
fn a_refused_gate_keeps_the_template_angles_and_says_why() {
    let kept = settle_adaptation(2.417, 1.544, || {
        Err("Not valid: The girdle disappears.".to_string())
    });
    assert!(!kept.adapted());
    assert_eq!(
        kept,
        AngleAdaptation::KeptTemplateAngles {
            n_from: 2.417,
            n_to: 1.544,
            reason: "Not valid: The girdle disappears.".to_string(),
        }
    );
    let text = kept.message().expect("a message");
    assert!(text.contains("Kept the template's angles"), "{text}");
    assert!(text.contains("The girdle disappears"), "{text}");
}

#[test]
fn the_material_note_says_whether_angles_will_change() {
    let oval = &TEMPLATES[5];
    assert!(material_note(oval, "Sapphire", 1.768).contains("used as authored"));
    let adapted = material_note(oval, "Quartz", 1.544);
    assert!(adapted.contains("adapted"), "{adapted}");
    assert!(adapted.contains("Designed for Sapphire"), "{adapted}");
}

// ---- building a design from a choice -----------------------------------------

/// The default material and gear build the template exactly as authored: no
/// remap, no adaptation, no history, clean, on the template's own preform.
#[test]
fn the_default_choice_builds_the_template_as_authored() {
    for (position, spec) in TEMPLATES.iter().enumerate() {
        let mut request = choice(
            session_index(position),
            spec.gear_teeth,
            spec.default_material_selection(),
        );
        request.girdle_diameter_mm = Some(6.5);
        let created = create_from_template(&request).expect("the default choice builds");
        assert_eq!(
            created.adaptation,
            AngleAdaptation::NotNeeded,
            "{}",
            spec.name
        );
        let design = &created.session.design;
        let authored: Vec<ConstraintTier> = spec.tiers();
        assert_eq!(design.tiers.len(), authored.len(), "{}", spec.name);
        for (made, wanted) in design.tiers.iter().zip(&authored) {
            assert_eq!(made.name, wanted.name);
            assert_eq!(made.angle_deg.to_bits(), wanted.angle_deg.to_bits());
            assert_eq!(made.indices, wanted.indices);
            assert_eq!(made.constraint, wanted.constraint);
        }
        assert_eq!(design.preform, spec.preform, "{}", spec.name);
        assert_eq!(design.meta.gear_teeth, spec.gear_teeth);
        assert_eq!(design.meta.symmetry_order, spec.symmetry_order);
        assert_eq!(design.meta.mirror, spec.mirror);
        assert_eq!(design.girdle_diameter_mm, Some(6.5));
        assert_eq!(design.material.name.as_deref(), Some(spec.default_material));
        assert!(
            created.session.history.peek_undo().is_none(),
            "{}",
            spec.name
        );
        assert!(!created.session.is_dirty(), "{}", spec.name);
        design
            .solve()
            .unwrap_or_else(|e| panic!("{}: failed to solve: {e:?}", spec.name));
        assert!(design.is_closed(), "{}", spec.name);
    }
}

#[test]
fn a_blank_or_unknown_template_is_not_built_here() {
    for index in [0, -3, 99] {
        let request = choice(index, 96, MaterialSelection::none());
        assert!(create_from_template(&request).is_err(), "index {index}");
    }
}

#[test]
fn a_gear_the_template_does_not_allow_is_refused() {
    let oval = index_of(ShapeFamily::Oval);
    for gear in [72, 77, 120, 48, 0, -96] {
        let request = choice(oval, gear, selection("Sapphire"));
        let error = create_from_template(&request)
            .err()
            .unwrap_or_else(|| panic!("gear {gear} was accepted"));
        assert!(error.contains("cannot be cut"), "{error}");
    }
}

/// A chosen gear remaps every tier's positions the way the gear panel does and
/// rewrites the schedule, keeping the template's symmetry and mirror.
#[test]
fn a_chosen_gear_remaps_the_template() {
    for family in [
        ShapeFamily::Oval,
        ShapeFamily::Cushion,
        ShapeFamily::Princess,
    ] {
        let index = index_of(family);
        let spec = template_spec(index).expect("a template");
        for gear in [80, 64] {
            let created =
                create_from_template(&choice(index, gear, spec.default_material_selection()))
                    .expect("an allowed gear builds");
            let design = &created.session.design;
            assert_eq!(design.meta.gear_teeth, gear);
            assert_eq!(design.meta.symmetry_order, spec.symmetry_order);
            assert_eq!(design.meta.mirror, spec.mirror);
            let ratio = f64::from(gear) / f64::from(spec.gear_teeth);
            for (made, authored) in design.tiers.iter().zip(spec.tiers()) {
                let wanted: Vec<f64> = authored
                    .indices
                    .iter()
                    .map(|index| (index * ratio).round())
                    .collect();
                assert_eq!(made.indices, wanted, "{} on gear {gear}", made.name);
            }
            design
                .solve()
                .unwrap_or_else(|e| panic!("{} on {gear}: failed to solve: {e:?}", spec.name));
            assert!(design.is_closed(), "{} on {gear}", spec.name);
        }
    }
}

#[test]
fn a_missing_or_bad_stone_width_leaves_the_width_unset() {
    let index = index_of(ShapeFamily::Cushion);
    for width in [None, Some(0.0), Some(-2.0), Some(f64::NAN)] {
        let mut request = choice(index, 96, selection("Quartz"));
        request.girdle_diameter_mm = width;
        let created = create_from_template(&request).expect("builds");
        assert_eq!(created.session.design.girdle_diameter_mm, None);
    }
    let mut request = choice(index, 96, selection("Quartz"));
    request.girdle_diameter_mm = Some(8.25);
    let created = create_from_template(&request).expect("builds");
    assert_eq!(created.session.design.girdle_diameter_mm, Some(8.25));
}

/// A material of a different index either adapts the angles (the gate passed, so
/// the stone is closed and some angle moved) or keeps the template's angles
/// exactly (the gate refused). It never leaves angles half-changed, and the chosen
/// material is the design's either way.
#[test]
fn another_material_adapts_or_keeps_the_angles_never_half_way() {
    for spec_index in [1, 6, 7, 8, 9] {
        let spec = template_spec(spec_index).expect("a template");
        let authored = spec.tiers();
        for material in ["Diamond", "Quartz", "Sapphire"] {
            let created =
                create_from_template(&choice(spec_index, 96, selection(material))).expect("builds");
            let design = &created.session.design;
            assert_eq!(design.material.name.as_deref(), Some(material));
            let angles_moved = design
                .tiers
                .iter()
                .zip(&authored)
                .any(|(made, wanted)| made.angle_deg.to_bits() != wanted.angle_deg.to_bits());
            let label = format!("{} in {material}", spec.name);
            match &created.adaptation {
                AngleAdaptation::NotNeeded => {
                    assert_eq!(material, spec.default_material, "{label}");
                    assert!(!angles_moved, "{label}");
                }
                AngleAdaptation::Adapted {
                    n_from, headline, ..
                } => {
                    assert!((n_from - spec.design_ri).abs() < f64::EPSILON, "{label}");
                    assert!(headline.starts_with("Valid"), "{label}: {headline}");
                    assert!(angles_moved, "{label}: adapted but nothing moved");
                    assert!(design.is_closed(), "{label}");
                }
                AngleAdaptation::KeptTemplateAngles { reason, .. } => {
                    assert!(!reason.is_empty(), "{label}");
                    assert!(!angles_moved, "{label}: kept, but an angle changed");
                }
            }
            if material != spec.default_material {
                assert_ne!(created.adaptation, AngleAdaptation::NotNeeded, "{label}");
            }
            assert!(created.session.history.peek_undo().is_none(), "{label}");
        }
    }
}

/// "(none)" is a legacy index of 1.54, not diamond: a quartz-designed teaching
/// template cut from no material is left alone.
#[test]
fn no_material_counts_as_the_legacy_index() {
    let created = create_from_template(&choice(4, 96, MaterialSelection::none())).expect("builds");
    assert_eq!(created.adaptation, AngleAdaptation::NotNeeded);
    assert_eq!(created.session.design.material.name, None);
}

/// A typed index counts like a material of that index.
#[test]
fn a_typed_refractive_index_is_compared_too() {
    let oval = index_of(ShapeFamily::Oval);
    let same = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.77),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let created = create_from_template(&choice(oval, 96, same)).expect("builds");
    assert_eq!(created.adaptation, AngleAdaptation::NotNeeded);
    let other = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.45),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let created = create_from_template(&choice(oval, 96, other)).expect("builds");
    assert_ne!(created.adaptation, AngleAdaptation::NotNeeded);
}
