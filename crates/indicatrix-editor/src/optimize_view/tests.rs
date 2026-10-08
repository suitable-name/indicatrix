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
    let weights = parse_optimize_weights("1.0", "2.5", "0", 0.0, 0.0).unwrap();
    assert_eq!(weights.windowing, 1.0);
    assert_eq!(weights.extinction, 2.5);
    assert_eq!(weights.tilt_brilliance, 0.0);
    assert_eq!(weights.yield_weight, 0.0);
}

#[test]
fn parse_optimize_weights_rejects_a_non_numeric_field() {
    let err = parse_optimize_weights("not-a-number", "1.0", "1.0", 0.0, 0.0).unwrap_err();
    assert!(err.contains("Windowing"));
}

#[test]
fn parse_optimize_weights_rejects_a_negative_weight() {
    // A negative weight is not merely out of range -- it would invert that
    // component's polarity -- so this is checked separately from finiteness.
    let err = parse_optimize_weights("1.0", "-0.5", "1.0", 0.0, 0.0).unwrap_err();
    assert!(err.contains("Extinction"));
}

#[test]
fn parse_optimize_weights_rejects_non_finite_values() {
    assert!(parse_optimize_weights("NaN", "1.0", "1.0", 0.0, 0.0).is_err());
    assert!(parse_optimize_weights("1.0", "inf", "1.0", 0.0, 0.0).is_err());
}

/// `yield_weight` comes
/// straight from the Optimize tab's `0..1` slider, not a parsed text field --
/// it passes through into `ObjectiveWeights` untouched, whatever value it is
/// (the slider itself is what keeps it in range).
#[test]
fn parse_optimize_weights_carries_the_yield_slider_value_through_untouched() {
    let weights = parse_optimize_weights("1.0", "1.0", "1.0", 0.4, 0.0).unwrap();
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
    // The tier is stored at -40 and moves to -41.5: a person reads 40.00 and 41.50 (the
    // pavilion side comes from the block), and the tier got 1.5 degrees steeper.
    assert_eq!(rows[0].from_angle.as_str(), "40.00\u{b0}");
    assert_eq!(rows[0].to_angle.as_str(), "41.50\u{b0}");
    assert_eq!(rows[0].delta.as_str(), "+1.50\u{b0}");
}

#[test]
fn optimize_change_rows_reads_a_flatter_pavilion_move_as_a_negative_change() {
    let design = design_with_named_tiers(&["G1", "P1"]);
    let mut outcome = sample_outcome(true, false);
    outcome.changes = vec![AngleChange {
        index: 1,
        from_deg: -41.5,
        to_deg: -40.0,
    }];
    let rows = optimize_change_rows(&outcome, &design);
    // Stored -41.5 to -40: flatter by 1.5, so the change keeps its explicit minus sign while
    // both angles are positive.
    assert_eq!(rows[0].from_angle.as_str(), "41.50\u{b0}");
    assert_eq!(rows[0].to_angle.as_str(), "40.00\u{b0}");
    assert_eq!(rows[0].delta.as_str(), "-1.50\u{b0}");
}

#[test]
fn optimize_change_rows_leaves_a_crown_angle_as_it_is() {
    let design = design_with_named_tiers(&["C1"]);
    let mut outcome = sample_outcome(true, false);
    outcome.changes = vec![AngleChange {
        index: 0,
        from_deg: 34.0,
        to_deg: 35.25,
    }];
    let rows = optimize_change_rows(&outcome, &design);
    assert_eq!(rows[0].from_angle.as_str(), "34.00\u{b0}");
    assert_eq!(rows[0].to_angle.as_str(), "35.25\u{b0}");
    assert_eq!(rows[0].delta.as_str(), "+1.25\u{b0}");
}

#[test]
fn optimize_change_rows_names_an_old_style_tier_by_its_standard_code() {
    // Tiers named the old way (`1`, `2`) show the same code the tier table shows.
    let design = design_with_named_tiers(&["1", "2"]);
    let codes = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    let mut outcome = sample_outcome(true, false);
    outcome.changes = vec![AngleChange {
        index: 1,
        from_deg: -40.0,
        to_deg: -41.0,
    }];
    let rows = optimize_change_rows(&outcome, &design);
    assert_eq!(rows[0].name.as_str(), codes[1].code.as_str());
    assert_ne!(rows[0].name.as_str(), "2");
    assert_eq!(tier_name_for_row(&design, 1), codes[1].code);
    assert_eq!(tier_name_for_row(&design, 9), "");
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
    let coordinate = optimize_progress_status(SearchStage::Coordinate, 12, 220, None, 1.26);
    assert_eq!(
        coordinate,
        "Optimizing... 12 of ~220 evaluations, 1.3s elapsed"
    );
    assert!(optimize_progress_status(SearchStage::Polish, 3, 10, None, 0.0).contains("(polish)"));
    assert!(
        optimize_progress_status(SearchStage::BaselineFull, 0, 10, None, 0.0)
            .contains("starting point")
    );
    assert!(
        optimize_progress_status(SearchStage::FinalFull, 0, 10, None, 0.0).contains("the result")
    );
}

#[test]
fn the_progress_line_names_the_start_and_the_best_score_of_a_multi_start_run() {
    let start = StartProgress {
        index: 2,
        count: 8,
        best_fast_score: 12.411,
    };
    assert_eq!(
        optimize_progress_status(SearchStage::Coordinate, 412, 983, Some(start), 6.24),
        "Optimizing... start 3 of 8, best 12.41, 412 of ~983 evaluations, 6.2s elapsed"
    );
    assert_eq!(
        optimize_progress_status(SearchStage::Polish, 900, 983, Some(start), 9.0),
        "Optimizing (polish)... start 3 of 8, best 12.41, 900 of ~983 evaluations, 9.0s elapsed"
    );
    assert_eq!(
        optimize_progress_status(SearchStage::Screening, 24, 983, Some(start), 0.44),
        "Optimizing... screening 24 of 28 starting points, 0.4s elapsed"
    );
    assert_eq!(
        optimize_progress_status(SearchStage::Screening, 24, 983, None, 0.44),
        "Optimizing... screening starting points, 24 drawn, 0.4s elapsed"
    );
    // The full-fidelity stages say nothing about starts.
    assert!(
        !optimize_progress_status(SearchStage::FinalFull, 0, 983, Some(start), 1.0)
            .contains("start 3")
    );
}

#[test]
fn optimize_hint_mentions_the_starts() {
    let mut design = fresh_design();
    design.tiers.push(scale_reference_tier(0.5));
    design.tiers.push(meet_existing_tier("P1"));
    let (_, hint) = optimize_hint(&design, 800);
    assert!(hint.contains("starting arrangements"), "{hint}");
}

#[test]
fn parse_optimize_weights_accepts_arithmetic_in_a_weight_box() {
    let weights = parse_optimize_weights("1 + 1", "3 / 2", " 0 ", 0.5, 0.0).unwrap();
    assert_eq!(weights.windowing, 2.0);
    assert_eq!(weights.extinction, 1.5);
    assert_eq!(weights.tilt_brilliance, 0.0);
    assert_eq!(weights.yield_weight, 0.5);
}

#[test]
fn parse_optimize_weights_reads_a_plain_number_exactly_as_the_f32_parse_does() {
    // Weights feed the score directly, so a run with plain boxes must stay bit-identical
    // to what it was before the boxes took arithmetic.
    let weights = parse_optimize_weights("0.1", "0.3", "1e0", 0.0, 0.0).unwrap();
    assert_eq!(
        weights.windowing.to_bits(),
        "0.1".parse::<f32>().unwrap().to_bits()
    );
    assert_eq!(
        weights.extinction.to_bits(),
        "0.3".parse::<f32>().unwrap().to_bits()
    );
    assert_eq!(weights.tilt_brilliance, 1.0);
}

#[test]
fn parse_optimize_weights_explains_a_calculation_it_cannot_do() {
    let err = parse_optimize_weights("1 +", "1.0", "1.0", 0.0, 0.0).unwrap_err();
    assert!(
        err.starts_with("Windowing weight '1 +' cannot be calculated"),
        "{err}"
    );
    let err = parse_optimize_weights("1.0", "1.0", "2 * -3", 0.0, 0.0).unwrap_err();
    assert!(err.contains("Tilt brilliance"), "{err}");
    assert!(err.contains("non-negative"), "{err}");
}

#[test]
fn direction_of_follows_the_metrics_own_polarity() {
    assert_eq!(direction_of(-1.0, false), 1);
    assert_eq!(direction_of(1.0, false), -1);
    assert_eq!(direction_of(1.0, true), 1);
    assert_eq!(direction_of(-1.0, true), -1);
    assert_eq!(direction_of(0.0, true), 0);
    assert_eq!(direction_of(f32::EPSILON / 2.0, false), 0);
}

// --- the face-up tone ---

fn tone_of(l_star: f32, chroma: f32) -> indicatrix::color::metrics::FaceUpTone {
    indicatrix::color::metrics::FaceUpTone {
        l_star,
        chroma,
        ..indicatrix::color::metrics::FaceUpTone::NONE
    }
}

/// A result whose best candidate is lighter (L* 50 to 60) and weaker (C* 20 to 10).
fn toned_result(goal: Option<ToneGoal>) -> (OptimizeResult, OptimizeCandidate) {
    let candidate = OptimizeCandidate {
        changes: Vec::new(),
        mast_changes: Vec::new(),
        after: sample_outcome(false, false).after,
        score: 15.0,
        yield_loss_pct: 25.0,
        tone: Some(tone_of(60.0, 10.0)),
    };
    let result = OptimizeResult {
        outcome: sample_outcome(false, false),
        mast_changes: Vec::new(),
        candidates: vec![candidate.clone()],
        tone_before: Some(tone_of(50.0, 20.0)),
        tone_goal: goal,
        lighting: indicatrix_cut_core::CANONICAL_LIGHTING_PRESET,
        starts_run: 1,
        best_start: 0,
    };
    (result, candidate)
}

#[test]
fn the_tone_slider_is_signed_lighter_negative_deeper_positive() {
    let lighter = parse_optimize_weights("1", "1", "1", 0.0, -2.5).unwrap();
    assert_eq!(
        (lighter.tone_weight, lighter.tone_goal),
        (2.5, ToneGoal::Lighter)
    );
    let deeper = parse_optimize_weights("1", "1", "1", 0.0, 3.0).unwrap();
    assert_eq!(
        (deeper.tone_weight, deeper.tone_goal),
        (3.0, ToneGoal::Deeper)
    );
    let off = parse_optimize_weights("1", "1", "1", 0.0, 0.0).unwrap();
    assert_eq!(off.tone_weight, 0.0);
    assert_eq!(
        parse_optimize_weights("1", "1", "1", 0.0, f32::NAN).unwrap_err(),
        "Tone must be a number."
    );
    assert!(parse_optimize_weights("1", "1", "1", 0.0, f32::INFINITY).is_err());
}

#[test]
fn signed_tone_is_the_inverse_of_the_slider() {
    for slider in [-3.0f32, -1.5, 0.0, 0.5, 3.0] {
        let weights = parse_optimize_weights("1", "1", "1", 0.0, slider).unwrap();
        assert_eq!(signed_tone(&weights), slider);
    }
    assert_eq!(
        signed_tone(&indicatrix_cut_core::ObjectivePreset::LightenDark.weights()),
        -3.0
    );
    assert_eq!(
        signed_tone(&indicatrix_cut_core::ObjectivePreset::IntensifyPale.weights()),
        3.0
    );
    assert_eq!(signed_tone(&ObjectiveWeights::default()), 0.0);
}

#[test]
fn tone_rows_judge_only_the_figure_the_goal_pulls_on() {
    let (result, candidate) = toned_result(Some(ToneGoal::Lighter));
    let rows = tone_result_rows(&result, &candidate);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].label, "Face-up lightness L*");
    assert_eq!(rows[1].label, "Face-up colour strength C*");
    assert_eq!(rows[0].direction, 1, "L* rose under a lighter goal");
    assert_eq!(
        rows[1].direction, 0,
        "chroma is not judged by a lighter goal"
    );

    let (result, candidate) = toned_result(Some(ToneGoal::Deeper));
    let rows = tone_result_rows(&result, &candidate);
    assert_eq!(rows[0].direction, 0);
    assert_eq!(rows[1].direction, -1, "chroma fell under a deeper goal");

    let (result, candidate) = toned_result(None);
    let rows = tone_result_rows(&result, &candidate);
    assert_eq!(rows.len(), 2, "an unweighted tone is still reported");
    assert!(rows.iter().all(|row| row.direction == 0));
    assert_eq!(rows[0].before, "50.00");
    assert!(
        rows[0].after.starts_with("60.00 (+10.00)"),
        "{}",
        rows[0].after
    );
}

#[test]
fn tone_rows_are_empty_without_both_tones() {
    let (mut result, candidate) = toned_result(None);
    result.tone_before = None;
    let none: [OptimizeResultLine; 0] = [];
    assert_eq!(tone_result_rows(&result, &candidate), none);
    let (result, mut candidate) = toned_result(None);
    candidate.tone = None;
    assert_eq!(tone_result_rows(&result, &candidate), none);
}

fn blue_sapphire() -> GemMaterial {
    GemMaterial::sapphire().with_body_color(
        indicatrix::optics::materials::body_color::BODY_COLOR_PRESETS[1].absorption_rgb,
    )
}

#[test]
fn a_material_has_body_colour_only_with_a_positive_band() {
    assert!(!material_has_body_color(&GemMaterial::diamond()));
    assert!(material_has_body_color(&blue_sapphire()));
}

#[test]
fn the_scale_note_has_a_branch_for_colour_size_and_light() {
    let mut design = crate::EditorSession::fresh().design;
    let none = tone_scale_note(&design, &GemMaterial::diamond(), LightingPreset::Daylight);
    assert!(none.contains("no body colour"), "{none}");
    assert!(none.contains("Design settings"), "{none}");

    let material = blue_sapphire();
    let unsized_note = tone_scale_note(&design, &material, LightingPreset::Daylight);
    assert!(
        unsized_note.starts_with("No stone size set"),
        "{unsized_note}"
    );
    assert!(unsized_note.contains("girdle diameter"), "{unsized_note}");

    design.girdle_diameter_mm = Some(6.5);
    let daylight = tone_scale_note(&design, &material, LightingPreset::Daylight);
    assert!(daylight.starts_with("Face-up colour is worked out for a 6.5 mm stone"));
    assert!(
        daylight.contains("under D65 Daylight (6500K)"),
        "{daylight}"
    );
    assert!(
        daylight.ends_with("the Live Render's lighting)."),
        "{daylight}"
    );
    let incandescent = tone_scale_note(&design, &material, LightingPreset::Incandescent);
    assert!(
        incandescent.contains("under Incandescent (3200K)"),
        "{incandescent}"
    );
    let uv = tone_scale_note(&design, &material, LightingPreset::UvLamp365);
    assert!(uv.contains("in daylight (D65)"), "{uv}");
    assert!(uv.contains("UV lamp has no visible white"), "{uv}");
}

#[test]
fn the_swatch_caption_names_the_light() {
    assert_eq!(
        tone_lighting_label(LightingPreset::Incandescent),
        "under Incandescent (3200K)"
    );
    assert_eq!(
        tone_lighting_label(LightingPreset::UvLamp395),
        "in daylight (D65) -- UV lamp"
    );
}

#[test]
fn a_stone_size_scales_the_material_to_the_design_width() {
    let planes = indicatrix::geometry::cuts::StandardGemCuts::standard_round_brilliant();
    let width = indicatrix::render_setup::measure_model_width(&planes).expect("SRB measures");
    let mut design = crate::EditorSession::fresh().design;

    // A per-model-unit colour (the triple of a body-colour preset) is tuned for 7 mm: the face-up
    // calibration `1 / K` while no size is set, `mm / 7 / K` once one is -- the renderer's own rule.
    let face_up = indicatrix::render_setup::MODEL_UNIT_FACE_UP_PATH;
    let unsized_material =
        crate::material_lookup::sized_material_for_optimize(blue_sapphire(), &design, &planes);
    assert_eq!(unsized_material.absorption_path_scale, 1.0 / face_up);

    design.girdle_diameter_mm = Some(6.5);
    let sized =
        crate::material_lookup::sized_material_for_optimize(blue_sapphire(), &design, &planes);
    assert_eq!(sized.absorption_path_scale, 6.5_f32 / 7.0 / face_up);

    // A per-millimetre colour (bands) scales to the model width, 7 mm while no size is set.
    let banded = || GemMaterial::sapphire().with_body_color_bands(&[[560.0, 80.0, 0.4]], 1.0);
    design.girdle_diameter_mm = None;
    let banded_unsized =
        crate::material_lookup::sized_material_for_optimize(banded(), &design, &planes);
    assert_eq!(banded_unsized.absorption_path_scale, (7.0 / width) as f32);
    design.girdle_diameter_mm = Some(6.5);
    let banded_sized =
        crate::material_lookup::sized_material_for_optimize(banded(), &design, &planes);
    assert_eq!(banded_sized.absorption_path_scale, (6.5 / width) as f32);
}
