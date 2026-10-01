//! Optimize view-model tests: the result table and status line, the preview
//! candidate, the weight parser, the hint, the change rows and the facet count.

use super::*;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{AngleChange, ConstraintTier, ObjectiveComponents};

fn scale_reference_tier(value: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(value),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// A hand-built [`OptimizeOutcome`] whose `after` deliberately makes extinction
/// WORSE while windowing and tilt brilliance improve -- proving
/// [`optimize_result_rows`] reports every component's real number rather than
/// only the still-improved blended score.
fn sample_outcome(changed: bool, cancelled: bool) -> OptimizeOutcome {
    OptimizeOutcome {
        before: ObjectiveComponents {
            windowing_pct: 12.5,
            extinction_pct: 8.0,
            tilt_brilliance_pct: 60.0,
        },
        before_score: 20.0,
        before_yield_loss_pct: 30.0,
        after: ObjectiveComponents {
            windowing_pct: 9.25,
            extinction_pct: 11.0,
            tilt_brilliance_pct: 65.0,
        },
        after_score: 15.0,
        after_yield_loss_pct: 25.0,
        evaluations: 42,
        changes: if changed {
            vec![AngleChange {
                index: 3,
                from_deg: -40.0,
                to_deg: -41.5,
            }]
        } else {
            Vec::new()
        },
        cancelled,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    }
}

#[test]
fn optimize_result_rows_reports_each_component_separately_never_collapsed() {
    let outcome = sample_outcome(true, false);
    let rows = optimize_result_rows(&outcome);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].label.as_str(), "Windowing");
    assert_eq!(rows[0].before.as_str(), "12.50%");
    // Lower windowing is better -- a negative delta reads as "better".
    assert_eq!(rows[0].after.as_str(), "9.25% (-3.25%, better)");
    // Extinction got WORSE -- shown honestly, not hidden by the improved score.
    assert_eq!(rows[1].label.as_str(), "Extinction");
    assert_eq!(rows[1].before.as_str(), "8.00%");
    assert_eq!(rows[1].after.as_str(), "11.00% (+3.00%, worse)");
    assert_eq!(rows[2].label.as_str(), "Tilt brilliance");
    assert_eq!(rows[2].before.as_str(), "60.00%");
    // Higher tilt brilliance is better -- a positive delta reads as "better".
    assert_eq!(rows[2].after.as_str(), "65.00% (+5.00%, better)");
    // Yield loss went DOWN (less preform thrown away) -- reads as "better",
    // same polarity as windowing/extinction, even though this fixture's
    // `ObjectiveWeights` (implicit -- `OptimizeOutcome` carries no weights of
    // its own) never actually weighed it into the blended score.
    assert_eq!(rows[3].label.as_str(), "Yield loss");
    assert_eq!(rows[3].before.as_str(), "30.00%");
    assert_eq!(rows[3].after.as_str(), "25.00% (-5.00%, better)");
    assert_eq!(rows[4].label.as_str(), "Blended score");
    assert_eq!(rows[4].before.as_str(), "20.00");
    assert_eq!(rows[4].after.as_str(), "15.00 (-5.00, better)");
}

#[test]
fn build_optimize_preview_design_moves_only_the_changed_tiers_angle() {
    // The ghost-preview candidate must apply every
    // `AngleChange` to the right tier and leave every other tier's angle (and
    // every other field) untouched.
    let mut design = Design::new(
        indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
        indicatrix_cut_core::ScheduleMeta::default(),
        vec![
            scale_reference_tier(0.5),
            scale_reference_tier(0.6),
            scale_reference_tier(0.7),
            ConstraintTier {
                angle_deg: -40.0,
                name: "P1".to_string(),
                indices: vec![0.0, 24.0],
                constraint: MeetConstraint::ScaleReference(0.8),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        ],
    );
    design.tiers[3].angle_deg = -40.0;
    let outcome = sample_outcome(true, false); // changes tier index 3 to -41.5
    let preview = build_optimize_preview_design(&design, &outcome);
    assert!((preview.tiers[3].angle_deg - (-41.5)).abs() < 1e-9);
    // Every other tier is untouched.
    for i in 0..3 {
        assert!((preview.tiers[i].angle_deg - design.tiers[i].angle_deg).abs() < 1e-9);
    }
    // The original design is never mutated.
    assert!((design.tiers[3].angle_deg - (-40.0)).abs() < 1e-9);
}

#[test]
fn build_optimize_preview_design_ignores_an_out_of_range_change_index() {
    // A design edited between when Optimize ran and when the preview toggle is
    // flipped can shrink the tier list out from under a stale outcome -- this
    // must degrade gracefully (skip that change), never panic.
    let design = Design::new(
        indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
        indicatrix_cut_core::ScheduleMeta::default(),
        vec![scale_reference_tier(0.5)],
    );
    let outcome = sample_outcome(true, false); // names tier index 3, out of range
    let preview = build_optimize_preview_design(&design, &outcome);
    assert_eq!(preview.tiers.len(), 1);
}

#[test]
fn after_with_delta_reports_unchanged_when_the_value_did_not_move() {
    assert_eq!(
        after_with_delta(5.0, 5.0, false, "%"),
        ("5.00% (+0.00%, unchanged)".to_string(), 0)
    );
}

#[test]
fn optimize_status_text_reports_the_change_and_evaluation_count() {
    let text = optimize_status_text(&sample_outcome(true, false));
    assert!(text.contains("1 tier(s)"));
    assert!(text.contains("42 evaluation(s)"));
    assert!(!text.contains("cancelled"));
}

#[test]
fn optimize_status_text_reports_no_improving_move_when_nothing_changed() {
    let text = optimize_status_text(&sample_outcome(false, false));
    assert!(text.contains("no improving move"));
}

#[test]
fn optimize_status_text_notes_cancellation_without_hiding_the_partial_result() {
    // `after`/`changes` still reflect the best REAL partial result found, never
    // discarded -- the cancellation note must be additive, not replace the summary.
    let text = optimize_status_text(&sample_outcome(true, true));
    assert!(text.contains("cancelled"));
    assert!(text.contains("1 tier(s)"));
}

#[test]
fn optimize_status_text_names_the_polish_stages_own_contribution_when_it_ran() {
    // Without this reader, `polish_evaluations`/`polish_improvement` would
    // have no consumer anywhere in this crate -- whether the ridge-following
    // polish stage did anything at all would be invisible to a cutter.
    let mut outcome = sample_outcome(true, false);
    outcome.polish_evaluations = 31;
    outcome.polish_improvement = 0.42;
    let text = optimize_status_text(&outcome);
    assert!(text.contains("polish: +0.42 in 31 evaluation(s)"));
}

#[test]
fn optimize_status_text_omits_the_polish_note_when_the_stage_never_ran() {
    let text = optimize_status_text(&sample_outcome(true, false));
    assert!(!text.contains("polish"));
}

#[test]
fn parse_optimize_weights_accepts_well_formed_input() {
    let weights = parse_optimize_weights("1.0", "2.5", "0", 0.0).unwrap();
    assert_eq!(weights.windowing, 1.0);
    assert_eq!(weights.extinction, 2.5);
    assert_eq!(weights.tilt_brilliance, 0.0);
    assert_eq!(weights.yield_weight, 0.0);
}

#[test]
fn parse_optimize_weights_rejects_a_non_numeric_field() {
    let err = parse_optimize_weights("not-a-number", "1.0", "1.0", 0.0).unwrap_err();
    assert!(err.contains("Windowing"));
}

#[test]
fn parse_optimize_weights_rejects_a_negative_weight() {
    // A negative weight is not merely out of range -- it would invert that
    // component's polarity -- so this is checked separately from finiteness.
    let err = parse_optimize_weights("1.0", "-0.5", "1.0", 0.0).unwrap_err();
    assert!(err.contains("Extinction"));
}

#[test]
fn parse_optimize_weights_rejects_non_finite_values() {
    assert!(parse_optimize_weights("NaN", "1.0", "1.0", 0.0).is_err());
    assert!(parse_optimize_weights("1.0", "inf", "1.0", 0.0).is_err());
}

/// `yield_weight` comes
/// straight from the Optimize tab's `0..1` slider, not a parsed text field --
/// it passes through into `ObjectiveWeights` untouched, whatever value it is
/// (the slider itself is what keeps it in range).
#[test]
fn parse_optimize_weights_carries_the_yield_slider_value_through_untouched() {
    let weights = parse_optimize_weights("1.0", "1.0", "1.0", 0.4).unwrap();
    assert_eq!(weights.yield_weight, 0.4);
}

fn meet_existing_tier(name: &str) -> ConstraintTier {
    ConstraintTier {
        angle_deg: -40.0,
        name: name.to_string(),
        indices: vec![0.0, 24.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn fresh_design() -> Design {
    crate::EditorSession::fresh().design
}

#[test]
fn optimize_hint_is_unavailable_when_every_tier_is_pinned() {
    // A design with only `ScaleReference` tiers has zero free tiers. Must read
    // as "nothing to optimize yet, and that's expected," never as broken.
    let mut design = fresh_design();
    design.tiers.push(scale_reference_tier(0.5));
    design.tiers.push(scale_reference_tier(0.8));

    let (available, hint) = optimize_hint(&design, 200);
    assert!(!available);
    assert!(hint.contains("pinned"));
    assert!(hint.contains("Adopt"));
}

#[test]
fn optimize_hint_is_available_once_a_tier_is_free_to_move() {
    let mut design = fresh_design();
    design.tiers.push(scale_reference_tier(0.5));
    design.tiers.push(meet_existing_tier("P1"));

    let (available, hint) = optimize_hint(&design, 200);
    assert!(available);
    assert!(
        hint.contains('1'),
        "expected the free-tier count in: {hint}"
    );
    assert!(hint.to_lowercase().contains("cancel"));
}

#[test]
fn optimize_hint_quotes_the_configured_budget_not_a_hardcoded_number() {
    let mut design = fresh_design();
    design.tiers.push(scale_reference_tier(0.5));
    design.tiers.push(meet_existing_tier("P1"));

    let (_, hint) = optimize_hint(&design, 750);
    assert!(hint.contains("750"), "expected the budget in: {hint}");
    assert!(!hint.contains("200-evaluation"));
}

#[test]
fn optimize_hint_names_the_canonical_light_pose() {
    let mut design = fresh_design();
    design.tiers.push(scale_reference_tier(0.5));
    design.tiers.push(meet_existing_tier("P1"));

    let (_, hint) = optimize_hint(&design, 200);
    assert!(hint.to_lowercase().contains("canonical light pose"));
}

fn design_with_named_tiers(names: &[&str]) -> Design {
    let mut design = fresh_design();
    for &name in names {
        let mut tier = meet_existing_tier(name);
        tier.indices = vec![0.0];
        design.tiers.push(tier);
    }
    design
}

#[test]
fn facet_count_from_solved_excludes_the_preform_s_own_planes() {
    // An EMPTY design (zero tiers, nothing cut yet) must show zero facets, not the
    // bare preform's own plane count.
    let design = fresh_design();
    let solved = design.solve().expect("a tierless design still solves");
    assert_eq!(
        facet_count_from_solved(&design, &solved),
        0,
        "an uncut preform has no facets of its own to count"
    );
}

#[test]
fn optimize_change_rows_formats_one_row_per_angle_change_with_the_tiers_own_name() {
    let design = design_with_named_tiers(&["G1", "P1"]);
    let mut outcome = sample_outcome(true, false);
    outcome.changes = vec![AngleChange {
        index: 1,
        from_deg: -40.0,
        to_deg: -41.5,
    }];
    let rows = optimize_change_rows(&outcome, &design);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tier_number.as_str(), "#2");
    assert_eq!(rows[0].name.as_str(), "P1");
    assert_eq!(rows[0].from_angle.as_str(), "-40.00\u{b0}");
    assert_eq!(rows[0].to_angle.as_str(), "-41.50\u{b0}");
    assert_eq!(rows[0].delta.as_str(), "-1.50\u{b0}");
}

#[test]
fn facet_count_from_solved_is_the_length_of_the_designs_expanded_planes() {
    let mut design = design_with_named_tiers(&["G1"]);
    design.tiers[0].constraint = MeetConstraint::ScaleReference(1.0);
    let solved = design
        .solve()
        .expect("a single scale-reference tier always solves");
    assert_eq!(
        facet_count_from_solved(&design, &solved),
        design.planes_from_solved(&solved).len()
            - design.preform.planes_offset(design.preform_y_offset).len()
    );
    assert!(
        facet_count_from_solved(&design, &solved) > 0,
        "a solved design always has at least one facet plane"
    );
}

#[test]
fn the_start_and_progress_status_lines_name_the_stage_and_the_defaulted_ri() {
    assert_eq!(
        optimize_start_status(None),
        "Optimizing... 0 evaluations, 0.0s elapsed"
    );
    assert!(optimize_start_status(Some(1.54)).contains("scored for n_d=1.5400"));
    let coordinate = optimize_progress_status(SearchStage::Coordinate, 12, 220, 1.26);
    assert_eq!(
        coordinate,
        "Optimizing... 12 of ~220 evaluations, 1.3s elapsed"
    );
    assert!(optimize_progress_status(SearchStage::Polish, 3, 10, 0.0).contains("(polish)"));
    assert!(
        optimize_progress_status(SearchStage::BaselineFull, 0, 10, 0.0).contains("starting point")
    );
    assert!(optimize_progress_status(SearchStage::FinalFull, 0, 10, 0.0).contains("the result"));
}
