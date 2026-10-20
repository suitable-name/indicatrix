//! Tests for the retarget dialog's Slint-free decision logic, spanning
//! [`super::material`], [`super::proposal_view`], [`super::apply`], and
//! [`super::snapshot`] -- see this group's own `mod.rs` doc comment ("Slint-free
//! view-model split").

use super::{
    material::{
        initial_target_index, resolve_target_selection, resolved_material_from_selection,
        target_display_name, target_material_selection,
    },
    proposal_view::{candidate_row_views, plan_view, retarget_view, search_summary_line},
    snapshot::{diff_row_view, diff_rows_from_deltas},
};
use crate::gui::editor::{
    retarget::{CrownShift, RetargetMode},
    state::{design_material_index_from_name, design_material_options},
};
use indicatrix::{geometry::meet_solver::MeetConstraint, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    ConstraintTier, Design, MaterialSelection, OptimizeConfig, PreformSpec, ResolvedMaterial,
    ScheduleMeta, diff_tiers,
};
use indicatrix_editor::retarget::{
    metrics::RetargetMetrics,
    search::{
        CandidateKind, CandidateNumbers, CandidateRole, SearchCandidate, SearchReport, StartPoint,
    },
    validity::{InvalidReason, RetargetValidity, ValidityStatus},
};

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
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
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

/// The dialog seeds the RI field from the design's own override and the combo's `selected`
/// handler clears it (`retarget_dialog.slint`): with the field cleared, a different material
/// resolves to its own `n_D`; left at the seeded number it would win over the material.
#[test]
fn choosing_a_material_changes_the_resolved_target_ri_once_the_seeded_override_is_cleared() {
    let mut design = diamond_design();
    design.material.refractive_index_override = Some(1.54);
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let sapphire = design_material_index_from_name(Some("Sapphire"), &options);

    let seeded = resolve_target_material(&design, &custom, sapphire, "1.54");
    assert!(
        (seeded.n_d - 1.54).abs() < 1e-9,
        "override wins: {}",
        seeded.n_d
    );

    let cleared = resolve_target_material(&design, &custom, sapphire, "");
    assert_eq!(cleared.gem.name, "Sapphire");
    assert!(
        (cleared.n_d - 1.768).abs() < 0.02,
        "Sapphire n_D expected near 1.768, got {}",
        cleared.n_d
    );
}

// --- retarget_view routing ---

#[test]
fn shift_plan_view_populates_rows_before_the_geometry_check() {
    let design = diamond_design();
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
    let target = resolve_target_material(&design, &custom, quartz_index, "");
    let plan =
        indicatrix_editor::retarget::build_plan(&design, &target, CrownShift::default(), &[]);
    let view = plan_view(&plan);
    assert!(!view.rows.is_empty());
    assert_eq!(view.anchored_errors, Vec::<String>::new());
    assert_eq!(view.solve_error, "");
    // Showing the preferred angles is independent of permission to apply them.
    // Apply tests exercise the real, completed preservation check.
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

// --- initial_target_index ---

#[test]
fn initial_target_index_finds_the_designs_own_named_material() {
    let custom: [GemMaterial; 0] = [];
    let options = design_material_options(&custom);
    let material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
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
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
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
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    assert_eq!(target_display_name(&selection), "Custom RI");
}

#[test]
fn target_display_name_shows_the_picked_material_name() {
    let selection = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
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

// --- The Optimize candidate list ---

fn candidate_with(kind: CandidateKind, score: f32) -> SearchCandidate {
    let mut validity = RetargetValidity::unchecked(&InvalidReason::NotClosed);
    validity.status = ValidityStatus::Valid;
    validity.reasons.clear();
    SearchCandidate {
        preservation: None,
        kind,
        role: None,
        angles: Vec::new(),
        anchors: Vec::new(),
        validity,
        metrics: RetargetMetrics::default(),
        numbers: CandidateNumbers {
            score,
            windowing_pct: 4.5,
            brilliance_pct: 81.0,
            extinction_pct: 3.0,
            yield_loss_pct: 40.5,
        },
        design: diamond_design(),
    }
}

fn report_with(candidates: Vec<SearchCandidate>, evaluations: usize) -> SearchReport {
    SearchReport {
        start: StartPoint::Shift,
        shift_validity: RetargetValidity::unchecked(&InvalidReason::NotClosed),
        start_numbers: CandidateNumbers {
            score: 10.0,
            windowing_pct: 5.0,
            brilliance_pct: 80.0,
            extinction_pct: 3.0,
            yield_loss_pct: 40.0,
        },
        candidates,
        dropped: Vec::new(),
        evaluations,
        free_tiers: 4,
        keep_look: false,
        baseline: None,
    }
}

#[test]
fn the_candidate_list_numbers_the_options_and_says_where_each_came_from() {
    let rows = candidate_row_views(&[
        candidate_with(CandidateKind::Optimized, 8.0),
        candidate_with(CandidateKind::Shift, 9.5),
        candidate_with(CandidateKind::Partial { percent: 40 }, 11.0),
    ]);
    assert_eq!(
        rows.iter().map(|row| row.rank).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(rows[0].label, "Searched angles");
    assert_eq!(rows[1].label, "Shift result");
    assert_eq!(rows[2].label, "Part of the Shift change (40 %)");
    assert_eq!(rows[0].score, "8.0");
    assert_eq!(rows[0].windowing, "4.5 %");
    assert_eq!(rows[0].brilliance, "81.0 %");
    assert_eq!(rows[0].extinction, "3.0 %");
    assert_eq!(rows[0].yield_loss, "40.5 %");
    assert!(rows.iter().all(|row| row.verdict == "Valid"));
}

/// Real facts for a candidate: the identity validation of a closed bipyramid.
fn real_facts() -> indicatrix_cut_core::meet_preservation::PreservationFacts {
    use indicatrix_cut_core::meet_preservation::{MeetPreservationBaseline, PreservationPolicy};
    let tiers = [45.0, -45.0]
        .into_iter()
        .enumerate()
        .map(|(index, angle_deg)| ConstraintTier {
            angle_deg,
            name: format!("Tier {index}"),
            indices: vec![0.0, 24.0, 48.0, 72.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    let meta = ScheduleMeta {
        symmetry_order: 4,
        ..ScheduleMeta::standard_round_brilliant()
    };
    let mut design = Design::new(PreformSpec::block(4.0, 4.0, 4.0), meta, tiers);
    design.ensure_tier_ids();
    MeetPreservationBaseline::capture(&design)
        .expect("closed baseline")
        .validate(&design, PreservationPolicy::RETARGET)
        .expect("the design preserves its own meets")
}

#[test]
fn the_role_label_follows_the_selected_role_not_the_list_position() {
    let facts = real_facts();
    let with_role = |role: Option<CandidateRole>, kind: CandidateKind| {
        let mut candidate = candidate_with(kind, 8.0);
        candidate.preservation = Some(facts);
        candidate.role = role;
        candidate
    };
    // The closest option is listed FIRST here: a positional rule would call it "Best".
    let rows = candidate_row_views(&[
        with_role(Some(CandidateRole::Closest), CandidateKind::Optimized),
        with_role(Some(CandidateRole::BestOptics), CandidateKind::Shift),
        with_role(
            Some(CandidateRole::Balanced),
            CandidateKind::Partial { percent: 40 },
        ),
        with_role(None, CandidateKind::Optimized),
    ]);
    assert_eq!(rows[0].label, "Closest to original - Searched angles");
    assert_eq!(rows[1].label, "Best optics within limits - Shift result");
    assert_eq!(rows[2].label, "Balanced - Part of the Shift change (40 %)");
    // An option with no role is named by its origin alone, never called "Balanced".
    assert_eq!(rows[3].label, "Searched angles");
}

#[test]
fn an_option_without_preservation_facts_gets_no_role_label() {
    let mut candidate = candidate_with(CandidateKind::Optimized, 8.0);
    candidate.role = Some(CandidateRole::BestOptics);
    assert_eq!(
        candidate_row_views(&[candidate])[0].label,
        "Searched angles"
    );
}

#[test]
fn an_option_that_is_not_valid_is_not_called_valid() {
    let mut candidate = candidate_with(CandidateKind::Optimized, 8.0);
    candidate.validity.status = ValidityStatus::Invalid;
    let rows = candidate_row_views(&[candidate]);
    assert_eq!(rows[0].verdict, "Not valid");
}

#[test]
fn the_search_summary_counts_the_options_and_the_steps() {
    let two = report_with(
        vec![
            candidate_with(CandidateKind::Optimized, 8.0),
            candidate_with(CandidateKind::Shift, 9.5),
        ],
        120,
    );
    assert_eq!(
        search_summary_line(&two),
        "2 valid options, best score first (lower is better). The search used 120 steps."
    );
    let one = report_with(vec![candidate_with(CandidateKind::Shift, 9.5)], 40);
    assert!(search_summary_line(&one).starts_with("1 valid option, "));
    assert_eq!(
        search_summary_line(&report_with(Vec::new(), 40)),
        "No valid option was found."
    );
}

#[test]
fn the_original_snapshot_label_names_the_target_and_is_told_from_a_users_own() {
    use super::snapshot::original_snapshot_label;
    let label = original_snapshot_label("Sapphire");
    assert!(label.contains("Sapphire"));
}

#[test]
fn the_original_snapshot_solve_prefers_the_cached_one_then_solves_never_a_wrong_length() {
    use super::apply::original_solve_for_snapshot;
    let session = indicatrix_editor::EditorSession::from_template(
        indicatrix_cut_core::FreshDesignSpec {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material: indicatrix_cut_core::MaterialSelection::none(),
            preform: indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        },
        1,
    );
    let design = session.design;
    let solved = design.solve().expect("the fresh template solves");
    // A matching cached solve is used as is.
    let kept = original_solve_for_snapshot(Some(solved.clone()), &design, true).expect("cached");
    assert_eq!(kept.len(), design.tiers.len());
    // A cached solve of another design (wrong tier count) is dropped and the design solved.
    let mut wrong = solved;
    wrong.pop();
    let fresh = original_solve_for_snapshot(Some(wrong.clone()), &design, true)
        .expect("solved on the spot");
    assert_eq!(fresh.len(), design.tiers.len());
    // No cache: solved on the spot when the policy allows it ...
    assert!(original_solve_for_snapshot(None, &design, true).is_some());
    // ... and never on the UI thread when it does not (the masts are filled in later).
    assert!(original_solve_for_snapshot(None, &design, false).is_none());
    assert!(original_solve_for_snapshot(Some(wrong), &design, false).is_none());
}
