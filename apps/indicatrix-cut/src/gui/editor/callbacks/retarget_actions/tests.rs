//! Tests for the retarget dialog's Slint-free decision logic, spanning
//! [`super::material`], [`super::proposal_view`], [`super::apply`], and
//! [`super::snapshot`] -- see this group's own `mod.rs` doc comment ("Slint-free
//! view-model split").

use super::{
    apply::{RetargetApplyError, apply_pending_retarget},
    material::{
        initial_target_index, resolve_target_selection, resolved_material_from_selection,
        target_display_name, target_material_selection,
    },
    proposal_view::retarget_view,
    snapshot::{diff_row_view, diff_rows_from_deltas},
};
use crate::gui::editor::{
    retarget::{CrownShift, RetargetMode},
    state::{EditorState, design_material_index_from_name, design_material_options},
};
use indicatrix::{geometry::meet_solver::MeetConstraint, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    ConstraintTier, Design, History, MaterialSelection, OptimizeConfig, PreformSpec,
    ResolvedMaterial, ScheduleMeta, diff_tiers,
};
use std::sync::atomic::Ordering as AtomicOrdering;

/// Test-only convenience composing [`target_material_selection`] (the lenient,
/// production wrapper) with [`resolved_material_from_selection`] -- these tests
/// exercise the RESOLVED material, not the [`MaterialSelection`] on its own, and
/// production code no longer has a use for that exact composition (both real call
/// sites need the `Result`-returning [`resolve_target_selection`] instead, for
/// error surfacing).
fn resolve_target_material(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> ResolvedMaterial {
    let selection = target_material_selection(design, custom, combo_index, ri_override_text);
    resolved_material_from_selection(&selection, custom)
}

fn diamond_design() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::default(),
        vec![ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }],
    );
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    design
}

// --- resolve_target_material ---

#[test]
fn resolve_target_material_defaults_to_the_designs_own_current_material() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let own_index = design_material_index_from_name(design.material.name.as_deref(), &options);
    let target = resolve_target_material(&design, &custom, own_index, "");
    assert_eq!(target.gem.name, "Diamond");
    assert!((target.n_d - design.effective_refractive_index()).abs() < 1e-9);
}

#[test]
fn resolve_target_material_resolves_a_different_combo_selection() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let target = resolve_target_material(&design, &custom, quartz_index, "");
    assert_eq!(target.gem.name, "Quartz");
    assert!((target.n_d - design.effective_refractive_index()).abs() > 0.1);
}

#[test]
fn resolve_target_material_falls_back_when_the_ri_override_text_is_invalid() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let own_index = design_material_index_from_name(design.material.name.as_deref(), &options);
    let target = resolve_target_material(&design, &custom, own_index, "not-a-number");
    assert_eq!(target.gem.name, "Diamond");
}

// --- retarget_view routing ---

#[test]
fn retarget_view_shift_mode_populates_rows_not_errors() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let target = resolve_target_material(&design, &custom, quartz_index, "");
    let (view, proposal) = retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    );
    assert!(!view.rows.is_empty());
    assert_eq!(view.anchored_errors, Vec::<String>::new());
    assert_eq!(view.solve_error, "");
    assert!(proposal.is_some());
}

#[test]
fn retarget_view_optimize_mode_populates_anchored_errors_not_rows_when_anchored() {
    // `diamond_design`'s one tier is a `ScaleReference` -- always anchored, so
    // `RetargetMode::Optimize` must refuse rather than silently seed-and-stop.
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let target = resolve_target_material(&design, &custom, quartz_index, "");
    let config = OptimizeConfig {
        max_evaluations: 4,
        ..OptimizeConfig::default()
    };
    let (view, proposal) = retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Optimize(config),
        &[],
    );
    assert!(view.rows.is_empty());
    assert_ne!(view.anchored_errors, Vec::<String>::new());
    assert!(view.anchored_errors[0].contains("P1"));
    assert!(proposal.is_none());
}

// --- apply_pending_retarget ---

fn fresh_state_with(design: Design) -> EditorState {
    let mut state = EditorState::fresh();
    state.design = design;
    state.history = History::new();
    state
}

#[test]
fn apply_pending_retarget_pushes_exactly_one_retarget_angles_edit_and_solves() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let target = resolve_target_material(&design, &custom, quartz_index, "");
    let (_, proposal) = retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    );
    let proposal = proposal.expect("shift mode never fails");

    let mut state = fresh_state_with(design);
    let generation = state.generation.load(AtomicOrdering::Relaxed);
    let applied = apply_pending_retarget(&mut state, (proposal, generation), None)
        .unwrap_or_else(|_| panic!("a fresh, matching-generation proposal must apply"));
    assert_eq!(applied, 1);
    assert!(state.history.can_undo());
    assert!(
        state.design.solve().is_ok(),
        "the retargeted design must still solve"
    );
}

#[test]
fn apply_pending_retarget_combines_a_material_change_into_one_undo_step() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let target = resolve_target_material(&design, &custom, quartz_index, "");
    let (_, proposal) = retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    );
    let proposal = proposal.expect("shift mode never fails");

    let mut state = fresh_state_with(design);
    let generation = state.generation.load(AtomicOrdering::Relaxed);
    let material_change = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    apply_pending_retarget(&mut state, (proposal, generation), Some(material_change))
        .unwrap_or_else(|_| panic!("a fresh, matching-generation proposal must apply"));
    assert_eq!(state.design.material.name.as_deref(), Some("Quartz"));

    // ONE undo step reverts both the angle retarget and the material change.
    assert!(state.undo().unwrap());
    assert_eq!(state.design.material.name.as_deref(), Some("Diamond"));
    assert!(!state.history.can_undo());
}

#[test]
fn apply_pending_retarget_refuses_a_stale_generation() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let target = resolve_target_material(&design, &custom, quartz_index, "");
    let (_, proposal) = retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    );
    let proposal = proposal.expect("shift mode never fails");

    let mut state = fresh_state_with(design);
    let stale_generation = state.generation.load(AtomicOrdering::Relaxed) + 1;
    let result = apply_pending_retarget(&mut state, (proposal, stale_generation), None);
    assert!(matches!(result, Err(RetargetApplyError::Stale)));
    assert!(
        !state.history.can_undo(),
        "nothing should have been applied"
    );
}

// --- initial_target_index ---

#[test]
fn initial_target_index_finds_the_designs_own_named_material() {
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    assert_eq!(initial_target_index(&material, &options), quartz_index);
}

#[test]
fn initial_target_index_seeds_the_custom_ri_sentinel_for_a_nameless_override() {
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let material = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.6),
    };
    assert_eq!(
        initial_target_index(&material, &options),
        i32::try_from(options.len()).unwrap() - 1
    );
    assert_eq!(
        options.last().map(String::as_str),
        Some("Custom RI\u{2026}")
    );
}

#[test]
fn initial_target_index_falls_back_to_none_for_a_plain_nameless_material() {
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let material = MaterialSelection::default();
    assert_eq!(initial_target_index(&material, &options), 0);
}

// --- target_material_selection ---

#[test]
fn target_material_selection_falls_back_to_the_current_material_on_unparseable_ri() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let selection = target_material_selection(&design, &custom, 0, "not-a-number");
    assert_eq!(selection, design.material);
}

// --- resolve_target_selection ---

#[test]
fn resolve_target_selection_reports_an_unparseable_ri_override_instead_of_falling_back() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let err = resolve_target_selection(&design, &custom, 0, "1,74")
        .expect_err("a comma-separated RI override must not silently parse");
    assert!(
        err.contains("1,74"),
        "error should name the bad text: {err}"
    );
}

#[test]
fn resolve_target_selection_reports_a_non_positive_ri_override() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let err = resolve_target_selection(&design, &custom, 0, "0.5")
        .expect_err("an RI override of 0.5 is not physically valid");
    assert!(err.contains("greater than 1.0"), "got: {err}");
}

#[test]
fn resolve_target_selection_accepts_a_valid_ri_override() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let selection = resolve_target_selection(&design, &custom, 0, "1.74")
        .expect("a well-formed RI override must parse");
    assert_eq!(selection.refractive_index_override, Some(1.74));
}

// --- target_display_name ---

#[test]
fn target_display_name_shows_none_for_a_nameless_uncustomized_selection() {
    let selection = MaterialSelection::default();
    assert_eq!(target_display_name(&selection), "(none)");
}

#[test]
fn target_display_name_shows_custom_ri_for_a_nameless_override() {
    let selection = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.74),
    };
    assert_eq!(target_display_name(&selection), "Custom RI");
}

#[test]
fn target_display_name_shows_the_picked_material_name() {
    let selection = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    assert_eq!(target_display_name(&selection), "Quartz");
}

// --- diff_row_view / diff_rows_from_deltas ---

fn tier_for_diff(name: &str, angle_deg: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

#[test]
fn diff_row_view_labels_an_unchanged_position_same() {
    let before = vec![tier_for_diff("Table", 0.0)];
    let after = before.clone();
    let deltas = diff_tiers(&before, None, &after, None);
    let row = diff_row_view(&deltas[0]);
    assert_eq!(row.status_label, "Same");
    assert_eq!(row.mast_delta, "-");
}

#[test]
fn diff_row_view_labels_a_moved_angle_changed() {
    let before = vec![tier_for_diff("Star", 15.0)];
    let mut after = before.clone();
    after[0].angle_deg = 16.0;
    let deltas = diff_tiers(&before, None, &after, None);
    let row = diff_row_view(&deltas[0]);
    assert_eq!(row.status_label, "Changed");
    assert_eq!(row.old_angle, "15.00\u{b0}");
    assert_eq!(row.new_angle, "16.00\u{b0}");
}

#[test]
fn diff_row_view_reports_a_signed_mast_delta_when_both_masts_are_known() {
    use indicatrix::geometry::meet_solver::{SolveStrategy, SolvedTier};
    let before = vec![tier_for_diff("Table", 0.0)];
    let after = before.clone();
    let before_solved = vec![SolvedTier {
        mast: 0.5,
        strategy: SolveStrategy::ScaleReference,
        detail: String::new(),
    }];
    let after_solved = vec![SolvedTier {
        mast: 0.55,
        strategy: SolveStrategy::ScaleReference,
        detail: String::new(),
    }];
    let deltas = diff_tiers(&before, Some(&before_solved), &after, Some(&after_solved));
    let row = diff_row_view(&deltas[0]);
    assert_eq!(row.mast_delta, "+0.0500");
}

#[test]
fn diff_row_view_labels_added_and_removed_positions() {
    let before = vec![tier_for_diff("Table", 0.0)];
    let after = vec![tier_for_diff("Table", 0.0), tier_for_diff("Star", 15.0)];
    let deltas = diff_tiers(&before, None, &after, None);
    assert_eq!(diff_row_view(&deltas[0]).status_label, "Same");
    assert_eq!(diff_row_view(&deltas[1]).status_label, "Added");

    let deltas_reverse = diff_tiers(&after, None, &before, None);
    assert_eq!(diff_row_view(&deltas_reverse[1]).status_label, "Removed");
}

#[test]
fn diff_rows_from_deltas_carries_the_tier_index_and_name_through() {
    let before = vec![tier_for_diff("Table", 0.0), tier_for_diff("Star", 15.0)];
    let mut after = before.clone();
    after[1].angle_deg = 16.0;
    let deltas = diff_tiers(&before, None, &after, None);
    let rows = diff_rows_from_deltas(&deltas);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].tier_index, 1);
    assert_eq!(rows[1].name.as_str(), "Star");
    assert_eq!(rows[1].risk_label.as_str(), "Changed");
}
