//! Unit tests for the pure helpers scattered across this module group.

use super::{
    materials_symmetry::ri_override_for_material_pick,
    new_design::install_new_design,
    solid_picking::letterbox_margin,
    tier_form::tier_form_error_field,
    viewport_material::{parse_printed_proportions_field, parse_printed_proportions_form},
};
use crate::gui::editor::{loading::parse_new_design_form, state::EditorState};
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
    assert!(!st.is_dirty(), "a freshly created design starts clean");
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
