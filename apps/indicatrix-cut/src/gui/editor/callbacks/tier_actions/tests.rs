//! Unit tests for the pure helpers scattered across this module group.

use super::{
    materials_symmetry::{explicit_edits, ri_override_for_material_pick},
    new_design::install_new_design,
    solid_picking::letterbox_margin,
    tier_form::tier_form_error_field,
    viewport_material::{parse_printed_proportions_field, parse_printed_proportions_form},
};
use crate::gui::editor::{loading::parse_new_design_form, state::EditorState};
use glam::DVec3;
use indicatrix_cut_core::PreformSpec;

// --- ri_override_for_material_pick ---

#[test]
fn ri_override_for_material_pick_does_not_pin_when_the_design_already_had_a_material() {
    // Diamond -> Quartz on a design whose CURRENT material is already named
    // "Diamond": pinning the outgoing material's (Diamond's) RI onto the
    // incoming pick (Quartz) is exactly the bug this guard prevents.
    assert_eq!(
        ri_override_for_material_pick(Some("Quartz"), Some("Diamond"), 2.417),
        None
    );
}

#[test]
fn ri_override_for_material_pick_pins_the_legacy_ri_on_a_first_pick_that_drifts() {
    // No material named yet (fresh design, legacy schedule RI 1.54): picking
    // Diamond (n_D ~1.5442) should pin the legacy figure so the exported I
    // line does not silently move.
    assert_eq!(
        ri_override_for_material_pick(Some("Diamond"), None, 1.54),
        Some(1.54)
    );
}

#[test]
fn ri_override_for_material_pick_does_nothing_for_a_non_built_in_name() {
    assert_eq!(
        ri_override_for_material_pick(Some("Not A Real Material"), None, 1.54),
        None
    );
}

// `next_free_block_name` and its tests moved to `indicatrix_editor::loading::naming`.

// `unique_duplicate_name` / `split_duplicate_suffix` and their tests moved to
// `indicatrix_editor::loading::naming`.

// --- parse_printed_proportions_form/_field  ---

#[test]
fn parse_printed_proportions_field_treats_blank_text_as_none() {
    assert_eq!(parse_printed_proportions_field("L/W", ""), Ok(None));
    assert_eq!(parse_printed_proportions_field("L/W", "   "), Ok(None));
}

#[test]
fn parse_printed_proportions_field_parses_a_positive_number() {
    assert_eq!(
        parse_printed_proportions_field("L/W", "1.05"),
        Ok(Some(1.05))
    );
}

#[test]
fn parse_printed_proportions_field_rejects_unparsable_text() {
    assert!(parse_printed_proportions_field("L/W", "abc").is_err());
}

#[test]
fn parse_printed_proportions_field_rejects_zero_and_negative() {
    assert!(parse_printed_proportions_field("L/W", "0").is_err());
    assert!(parse_printed_proportions_field("L/W", "-1.0").is_err());
}

#[test]
fn parse_printed_proportions_form_builds_every_field() {
    let props = parse_printed_proportions_form("1.30", "1.05", "0.61", "0.43", "0.60").unwrap();
    assert_eq!(props.vol_w3, Some(1.30));
    assert_eq!(props.lw, Some(1.05));
    assert_eq!(props.cw, Some(0.61));
    assert_eq!(props.pw, Some(0.43));
    assert_eq!(props.hw, Some(0.60));
}

#[test]
fn parse_printed_proportions_form_allows_every_field_blank() {
    let props = parse_printed_proportions_form("", "", "", "", "").unwrap();
    assert_eq!(props.vol_w3, None);
    assert_eq!(props.lw, None);
    assert_eq!(props.cw, None);
    assert_eq!(props.pw, None);
    assert_eq!(props.hw, None);
}

#[test]
fn parse_printed_proportions_form_names_the_failing_field() {
    let err =
        parse_printed_proportions_form("1.30", "not a number", "0.61", "0.43", "0.60").unwrap_err();
    assert!(err.contains("L/W"), "error should name the field: {err}");
}

// --- letterbox_margin  ---

#[test]
fn letterbox_margin_is_zero_when_the_pick_buffer_fills_the_viewport() {
    assert_eq!(letterbox_margin((800, 600), (800, 600)), (0.0, 0.0));
}

#[test]
fn letterbox_margin_is_half_the_leftover_on_each_side() {
    // A 1920x1080 (16:9) render letterboxed into an 800x600 (4:3) viewport --
    // `contained_request_size` would fit the width and shrink the height.
    assert_eq!(letterbox_margin((800, 600), (800, 450)), (0.0, 75.0));
}

#[test]
fn letterbox_margin_never_goes_negative_if_the_pick_buffer_is_larger() {
    assert_eq!(letterbox_margin((800, 600), (900, 700)), (0.0, 0.0));
}

// --- tier_form_error_field  ---

#[test]
fn tier_form_error_field_classifies_every_documented_message_shape() {
    assert_eq!(
        tier_form_error_field("Angle '41x' is not a number."),
        "angle"
    );
    assert_eq!(
        tier_form_error_field(
            "Angle 91.00\u{b0} exceeds 90\u{b0} -- angles are measured from the girdle \
             plane, so no crown or pavilion facet can be steeper than that."
        ),
        "angle"
    );
    assert_eq!(
        tier_form_error_field(
            "Another tier is already named 'P1' -- facet names must be unique so meet \
             constraints resolve to the right tier."
        ),
        "name"
    );
    assert_eq!(
        tier_form_error_field("Index '99' is off the gear. Indices run from 0 to 96 on this gear."),
        "indices"
    );
    assert_eq!(
        tier_form_error_field("\"Meet named\" needs at least one facet name."),
        "constraint"
    );
    assert_eq!(
        tier_form_error_field("Scale reference 'x' is not a number."),
        "constraint"
    );
    assert_eq!(
        tier_form_error_field("No facet named 'C1' -- check the Meets field."),
        "constraint"
    );
    assert_eq!(
        tier_form_error_field("Unknown constraint kind 7."),
        "constraint"
    );
}

#[test]
fn tier_form_error_field_is_empty_for_an_unrecognized_message() {
    assert_eq!(
        tier_form_error_field("Failed to remap this design's indices to the new gear."),
        ""
    );
}

// --- install_new_design / the viewport overlay's two conditions ---

/// The worked example's New Design form: Cylinder 1.50/1.00/1.50, 96 teeth,
/// 8-fold, mirror on, no starting material.
fn worked_example_spec() -> indicatrix_cut_core::FreshDesignSpec {
    let preform = PreformSpec::cylinder(96, 1.5, 1.0, 1.5);
    parse_new_design_form(96, preform, "8", true, 0).expect("the worked example's form parses")
}

/// Mirror of `EditorViewportOverlay`'s card-grid condition
/// (`ui/components/app/viewport_overlay.slint`): `enabled && !has_design`.
const fn shows_empty_state(enabled: bool, has_design: bool) -> bool {
    enabled && !has_design
}

/// Mirror of `EditorViewportOverlay`'s zero-tier hint condition:
/// `enabled && has_design && tiers.length == 0`.
const fn shows_zero_tier_hint(enabled: bool, has_design: bool, tier_count: usize) -> bool {
    enabled && has_design && tier_count == 0
}

/// The "New Design never shows a design" bug: template 0 ("Empty") yields a
/// zero-tier design, which must still count as a design.
#[test]
fn creating_from_the_empty_template_marks_the_design_as_present() {
    let mut st = EditorState::fresh();
    assert!(!st.has_design, "the startup placeholder is not a design");
    install_new_design(&mut st, worked_example_spec(), 0);
    assert!(st.has_design);
    assert_eq!(st.design.tiers.len(), 0, "no template tiers were seeded");
    assert!(st.is_dirty(), "a design the user created is unsaved");
    st.session.mark_saved();
    assert!(!st.is_dirty(), "saving makes it clean");
}

#[test]
fn creating_from_a_template_seeds_its_tiers_and_marks_the_design_as_present() {
    for template_index in
        1..=i32::try_from(indicatrix_cut_core::templates::TEMPLATES.len()).unwrap()
    {
        let mut st = EditorState::fresh();
        install_new_design(&mut st, worked_example_spec(), template_index);
        assert!(st.has_design);
        assert!(
            !st.design.tiers.is_empty(),
            "template {template_index} seeded no tiers"
        );
    }
    // A stale index past the end is treated as Empty -- still a real design.
    let mut st = EditorState::fresh();
    install_new_design(&mut st, worked_example_spec(), 99);
    assert!(st.has_design);
    assert_eq!(st.design.tiers.len(), 0, "no template tiers were seeded");
}

/// The card grid and the zero-tier hint key off different things (a design
/// existing vs. it having tiers), so they never show together, and a fresh
/// Empty design gets the hint -- the preform stays visible -- not the grid.
#[test]
fn zero_tier_hint_condition_differs_from_the_empty_state_condition() {
    let mut st = EditorState::fresh();
    let placeholder = (st.has_design, st.design.tiers.len());
    assert!(shows_empty_state(true, placeholder.0));
    assert!(!shows_zero_tier_hint(true, placeholder.0, placeholder.1));

    install_new_design(&mut st, worked_example_spec(), 0);
    assert!(!shows_empty_state(true, st.has_design));
    assert!(shows_zero_tier_hint(
        true,
        st.has_design,
        st.design.tiers.len()
    ));

    for enabled in [false, true] {
        for has_design in [false, true] {
            for tier_count in [0, 8] {
                assert!(
                    !(shows_empty_state(enabled, has_design)
                        && shows_zero_tier_hint(enabled, has_design, tier_count)),
                    "both overlays at enabled={enabled} has_design={has_design} tiers={tier_count}"
                );
            }
        }
    }
    // A design with tiers shows neither.
    assert!(!shows_empty_state(true, true));
    assert!(!shows_zero_tier_hint(true, true, 8));
}

// --- the Stone tab is the design: no silent unlink, one stone width ---

/// The Stone tab's width field and the Preform tab's Girdle Diameter field are one value:
/// both go through `parse_yield_form` into `Edit::SetGirdleDiameterMm`, so the same text gives
/// the same width (arithmetic included), and the design holds exactly that one number.
#[test]
fn the_stone_tab_and_the_preform_field_edit_the_same_design_width() {
    use indicatrix_cut_core::Edit;
    use indicatrix_editor::material::parse_yield_form;

    let mut state = EditorState::fresh();
    assert_eq!(state.design.girdle_diameter_mm, None);
    let current = state.design.material.clone();

    // The Stone tab sends the width alone; the Preform tab sends it with its specific gravity.
    let (from_stone, _) = parse_yield_form("6.5+0.25", 0, "", &current).expect("parses");
    let (from_preform, _) = parse_yield_form("6.5+0.25", 0, "3.52", &current).expect("parses");
    assert_eq!(from_stone, Some(6.75));
    assert_eq!(from_stone, from_preform);

    state
        .apply(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: from_stone,
        })
        .expect("the width applies");
    assert_eq!(state.design.girdle_diameter_mm, Some(6.75));
    assert!(state.undo().expect("undoes"));
    assert_eq!(state.design.girdle_diameter_mm, None);

    assert!(
        parse_yield_form("0", 0, "", &current).is_err(),
        "a zero width is refused"
    );
    assert!(parse_yield_form("abc", 0, "", &current).is_err());
}

/// A design refresh pushes the design's values into the forms only for the groups that
/// changed since the last push. A material edit leaves the width group alone (so a width the
/// user is still typing is never overwritten), a width edit flags only the width, and a
/// refresh with nothing changed flags nothing.
#[test]
fn a_design_refresh_never_flags_a_group_the_edit_did_not_touch() {
    use indicatrix_cut_core::{Edit, MaterialSelection};

    let mut state = EditorState::fresh();
    let first = state.record_scratch_push();
    assert!(
        first.material && first.girdle,
        "the first push seeds every group"
    );

    state
        .apply(Edit::SetMaterial {
            material: MaterialSelection {
                name: Some("Ruby".to_string()),
                ..MaterialSelection::default()
            },
        })
        .expect("the material applies");
    let after_material = state.record_scratch_push();
    assert!(after_material.material);
    assert!(
        !after_material.girdle,
        "a pending width edit survives a material change"
    );

    state
        .apply(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(7.0),
        })
        .expect("the width applies");
    let after_width = state.record_scratch_push();
    assert!(after_width.girdle);
    assert!(!after_width.material);

    let unchanged = state.record_scratch_push();
    assert!(!unchanged.material && !unchanged.girdle && !unchanged.preform);
}

// --- explicit_edits: the one apply path of "use this material / width / preform" ---

/// A planned piece of rough as the Rough Planner hands it over: the box `|x|, |y|, |z| <= 1`.
fn planned_piece() -> PreformSpec {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 1.0),
        (DVec3::NEG_Y, 1.0),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    PreformSpec::from_planes(planes, "Rough plan, result 1, stone 1".to_owned())
        .expect("a closed box is a preform")
}

/// A design with a block preform and no girdle diameter, as a freshly loaded library design is.
fn state_with_a_block_preform() -> EditorState {
    use indicatrix_cut_core::Edit;

    let mut state = EditorState::fresh();
    state
        .apply(Edit::SetPreform {
            preform: PreformSpec::block(1.0, 1.0, 1.0),
        })
        .expect("the block applies");
    assert_eq!(state.design.girdle_diameter_mm, None);
    assert!(!state.design.preform.is_plane_set());
    state
}

#[test]
fn an_explicit_apply_lists_the_material_then_the_width_then_the_preform() {
    use indicatrix_cut_core::Edit;

    let mut state = state_with_a_block_preform();
    let piece = planned_piece();

    let edits = explicit_edits(
        &state.design,
        Some("Melon - Round color"),
        Some(6.5),
        Some(&piece),
    );
    assert_eq!(edits.len(), 3, "{edits:?}");
    assert!(matches!(edits[0], Edit::SetMaterial { .. }), "{edits:?}");
    assert_eq!(
        edits[1],
        Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(6.5)
        }
    );
    assert_eq!(
        edits[2],
        Edit::SetPreform {
            preform: piece.clone()
        }
    );

    // A design whose preform was shifted gets the shift put back: the planes place the piece.
    state
        .apply(Edit::SetPreformYOffset { y_offset: 0.25 })
        .expect("the offset applies");
    let material_before = state.design.material.clone();
    let preform_before = state.design.preform.clone();
    let _ = state.record_scratch_push();
    let shifted = explicit_edits(
        &state.design,
        Some("Melon - Round color"),
        Some(6.5),
        Some(&piece),
    );
    assert_eq!(shifted.len(), 4, "{shifted:?}");
    assert_eq!(shifted[3], Edit::SetPreformYOffset { y_offset: 0.0 });

    // The whole action is ONE undo step, and it reaches the Preform tab's form.
    state
        .apply(Edit::Batch(shifted))
        .expect("the batch applies");
    assert_eq!(
        state.design.material.name.as_deref(),
        Some("Melon - Round color")
    );
    assert_eq!(state.design.girdle_diameter_mm, Some(6.5));
    assert_eq!(state.design.preform, piece);
    assert_eq!(state.design.preform_y_offset, 0.0);
    assert!(state.record_scratch_push().preform);
    assert!(state.undo().expect("undoes"));
    assert_eq!(state.design.material, material_before);
    assert_eq!(state.design.girdle_diameter_mm, None);
    assert_eq!(state.design.preform, preform_before);
    assert_eq!(state.design.preform_y_offset, 0.25);
}

#[test]
fn an_explicit_apply_the_design_already_has_makes_no_edit() {
    use indicatrix_cut_core::Edit;

    let mut state = state_with_a_block_preform();
    let piece = planned_piece();
    let first = explicit_edits(
        &state.design,
        Some("Melon - Round color"),
        Some(6.5),
        Some(&piece),
    );
    state.apply(Edit::Batch(first)).expect("the batch applies");
    assert!(
        explicit_edits(
            &state.design,
            Some("Melon - Round color"),
            Some(6.5),
            Some(&piece)
        )
        .is_empty(),
        "the second apply finds everything in place"
    );

    // What is not asked for is not touched.
    assert!(
        explicit_edits(&state.design, None, None, None).is_empty(),
        "no request asks for no edits"
    );
    // A width that is no width asks nothing.
    for width in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            explicit_edits(&state.design, None, Some(width), None).is_empty(),
            "{width}"
        );
    }
    // Another width is the one edit.
    assert_eq!(
        explicit_edits(&state.design, None, Some(7.0), None),
        vec![Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(7.0)
        }]
    );
}
