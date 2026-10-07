//! Tests of Optimize's hint and time estimate.

use super::*;
use crate::EditorSession;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    CANONICAL_LIGHTING_PRESET, ConstraintTier, PreformSpec, ScheduleMeta, ToneGoal,
};

fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0],
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// Every tier pinned, like a design fresh off the catalogue.
fn imported() -> Design {
    let mut design = EditorSession::fresh().design;
    design
        .tiers
        .push(tier("C1", 40.0, MeetConstraint::ScaleReference(0.6)));
    design
        .tiers
        .push(tier("P1", -41.0, MeetConstraint::ScaleReference(0.7)));
    design
        .tiers
        .push(tier("G", -90.0, MeetConstraint::ScaleReference(0.9)));
    design
}

/// One free pavilion tier next to a pinned crown tier.
fn adopted() -> Design {
    let mut design = EditorSession::fresh().design;
    design
        .tiers
        .push(tier("C1", 40.0, MeetConstraint::ScaleReference(0.6)));
    design
        .tiers
        .push(tier("P1", -41.0, MeetConstraint::MeetExisting));
    design
}

fn form<'a>() -> RunForm<'a> {
    RunForm {
        preset_index: 0,
        weight_windowing: "1.0",
        weight_extinction: "1.0",
        weight_tilt_brilliance: "1.0",
        yield_weight: 0.0,
        tone: 0.0,
        vary_anchored: false,
        keep_girdle: true,
        budget_text: "200",
        starts: 1,
        seed_text: "0",
        polish: true,
        candidates: DEFAULT_CANDIDATES,
        ranges: &[],
    }
}

fn plan(design: &Design, form: &RunForm<'_>) -> Result<RunPlan, String> {
    build_run_plan(design, form, CANONICAL_LIGHTING_PRESET)
}

// --- the objective ---

#[test]
fn the_combo_lists_the_seven_presets_then_custom() {
    assert_eq!(
        preset_labels(),
        vec![
            "Balanced",
            "Brilliance",
            "Low windowing",
            "Low extinction",
            "Keep weight",
            "Lighten dark rough",
            "Intensify pale rough",
            "Custom"
        ]
    );
    assert_eq!(preset_labels().len(), 8);
    assert_eq!(CUSTOM_PRESET_INDEX, 7);
    assert!(preset_description(1).starts_with("Brilliance"));
    assert!(preset_description(CUSTOM_PRESET_INDEX).starts_with("Custom"));
    assert!(preset_description(99).starts_with("Custom"));
}

#[test]
fn a_preset_gives_its_weights_and_custom_gives_none() {
    for (index, preset) in ObjectivePreset::ALL.into_iter().enumerate() {
        assert_eq!(weights_for_preset(index), Some(preset.weights()));
    }
    assert_eq!(weights_for_preset(CUSTOM_PRESET_INDEX), None);
}

#[test]
fn a_preset_beats_the_weight_boxes_which_are_not_even_read() {
    let design = adopted();
    for (index, preset) in ObjectivePreset::ALL.into_iter().enumerate() {
        let run = plan(
            &design,
            &RunForm {
                preset_index: index,
                weight_windowing: "not a number",
                weight_extinction: "-3",
                weight_tilt_brilliance: "",
                yield_weight: 0.7,
                ..form()
            },
        )
        .expect("the preset needs no valid boxes");
        assert_eq!(run.config.weights, preset.weights());
    }
}

#[test]
fn custom_reads_the_boxes_and_the_slider_and_allows_arithmetic() {
    let design = adopted();
    let run = plan(
        &design,
        &RunForm {
            preset_index: CUSTOM_PRESET_INDEX,
            weight_windowing: "1 + 1",
            weight_extinction: "0.5",
            weight_tilt_brilliance: "4 / 2",
            yield_weight: 0.25,
            ..form()
        },
    )
    .expect("custom weights read");
    assert_eq!(
        run.config.weights,
        ObjectiveWeights {
            windowing: 2.0,
            extinction: 0.5,
            tilt_brilliance: 2.0,
            yield_weight: 0.25,
            ..ObjectiveWeights::default()
        }
    );
}

#[test]
fn custom_reads_the_signed_tone_slider() {
    let design = adopted();
    for (slider, weight, goal) in [
        (-2.0, 2.0, ToneGoal::Lighter),
        (3.0, 3.0, ToneGoal::Deeper),
        (0.0, 0.0, ToneGoal::Lighter),
    ] {
        let run = plan(
            &design,
            &RunForm {
                preset_index: CUSTOM_PRESET_INDEX,
                tone: slider,
                ..form()
            },
        )
        .expect("custom weights read");
        assert_eq!(run.config.weights.tone_weight, weight);
        assert_eq!(run.config.weights.tone_goal, goal);
    }
    let preset = plan(
        &design,
        &RunForm {
            preset_index: ObjectivePreset::IntensifyPale.index(),
            tone: -3.0,
            ..form()
        },
    )
    .expect("a preset needs no slider");
    assert_eq!(
        preset.config.weights,
        ObjectivePreset::IntensifyPale.weights()
    );
}

#[test]
fn custom_weights_that_cannot_be_read_are_refused_by_name() {
    let design = adopted();
    let error = plan(
        &design,
        &RunForm {
            preset_index: CUSTOM_PRESET_INDEX,
            weight_extinction: "-1",
            ..form()
        },
    )
    .unwrap_err();
    assert!(error.contains("Extinction"), "{error}");
    let error = plan(
        &design,
        &RunForm {
            preset_index: CUSTOM_PRESET_INDEX,
            weight_windowing: "1 +",
            ..form()
        },
    )
    .unwrap_err();
    assert!(error.contains("Windowing"), "{error}");
}

// --- what may change ---

#[test]
fn anchored_tiers_are_varied_by_default_only_when_nothing_else_is_free() {
    assert!(default_vary_anchored(&imported()));
    assert!(!default_vary_anchored(&adopted()));
    assert!(default_vary_anchored(&EditorSession::fresh().design));
}

#[test]
fn the_plan_carries_the_what_may_change_switches() {
    let design = imported();
    let run = plan(
        &design,
        &RunForm {
            vary_anchored: true,
            keep_girdle: true,
            candidates: 4,
            ..form()
        },
    )
    .expect("builds");
    assert!(run.options.vary_anchored);
    assert_eq!(run.options.min_girdle_fraction, Some(0.5));
    assert_eq!(run.options.keep_candidates, 4);
    assert_eq!(run.options.candidate_separation_deg, Some(0.5));
    assert!(run.options.anchor_hinges.is_empty(), "hinges come later");

    let run = plan(
        &design,
        &RunForm {
            keep_girdle: false,
            ..form()
        },
    )
    .expect("builds");
    assert!(!run.options.vary_anchored);
    assert_eq!(run.options.min_girdle_fraction, None);
}

#[test]
fn the_number_of_candidates_stays_between_one_and_five() {
    let design = adopted();
    for (asked, kept) in [(0, 1), (1, 1), (3, 3), (5, 5), (9, 5)] {
        let run = plan(
            &design,
            &RunForm {
                candidates: asked,
                ..form()
            },
        )
        .expect("builds");
        assert_eq!(run.options.keep_candidates, kept);
    }
}

#[test]
fn every_movable_tier_gets_the_default_range_and_the_pinned_ones_wait_for_the_switch() {
    let design = imported();
    let off = plan(&design, &form()).expect("builds");
    assert!(off.options.angle_bounds.is_empty());
    assert_eq!(off.movable_tiers, 0);

    let on = plan(
        &design,
        &RunForm {
            vary_anchored: true,
            ..form()
        },
    )
    .expect("builds");
    assert_eq!(on.movable_tiers, 2);
    assert_eq!(on.options.angle_bounds.get(&0), Some(&(35.0, 45.0)));
    assert_eq!(on.options.angle_bounds.get(&1), Some(&(-46.0, -36.0)));
    assert_eq!(on.options.angle_bounds.len(), 2, "the girdle tier has none");
}

#[test]
fn an_edited_range_replaces_the_default_and_a_bad_one_is_refused() {
    let design = adopted();
    let edited = [RangeInput {
        tier: 1,
        min_text: "40".to_string(),
        max_text: "41 + 2".to_string(),
    }];
    let run = plan(
        &design,
        &RunForm {
            ranges: &edited,
            ..form()
        },
    )
    .expect("builds");
    assert_eq!(run.options.angle_bounds.get(&1), Some(&(-43.0, -40.0)));

    let broken = [RangeInput {
        tier: 1,
        min_text: "x".to_string(),
        max_text: "41".to_string(),
    }];
    let error = plan(
        &design,
        &RunForm {
            ranges: &broken,
            ..form()
        },
    )
    .unwrap_err();
    assert!(error.contains("Angle range of tier #2 (P1)"), "{error}");
}

#[test]
fn a_driven_tier_gets_no_range_and_is_not_counted_as_movable() {
    let mut design = adopted();
    design
        .tiers
        .push(tier("P2", -36.0, MeetConstraint::MeetExisting));
    design.ensure_tier_ids();
    let relation = design.parse_relation("P1 - 5").expect("reads");
    let id = design.tier_ids[2];
    design.tier_relations.insert(id, relation);

    let run = plan(&design, &form()).expect("builds");
    assert_eq!(run.movable_tiers, 1);
    assert!(run.options.angle_bounds.contains_key(&1));
    assert!(!run.options.angle_bounds.contains_key(&2));
}

#[test]
fn the_hinges_skip_driven_tiers_and_follow_the_selection() {
    let mut design = adopted();
    design
        .tiers
        .push(tier("P2", -36.0, MeetConstraint::ScaleReference(0.5)));
    design.ensure_tier_ids();
    let relation = design.parse_relation("P1 - 5").expect("reads");
    let id = design.tier_ids[2];
    design.tier_relations.insert(id, relation);
    let hinges: BTreeMap<usize, DVec3> = (0..3)
        .map(|index| (index, DVec3::new(index as f64, 0.0, 1.0)))
        .collect();

    let mut options = OptimizeOptions::default();
    fill_anchor_hinges(&mut options, &design, &hinges, None);
    assert_eq!(
        options.anchor_hinges.keys().copied().collect::<Vec<_>>(),
        vec![0, 1]
    );

    let only = BTreeSet::from([1, 2]);
    fill_anchor_hinges(&mut options, &design, &hinges, Some(&only));
    assert_eq!(
        options.anchor_hinges.keys().copied().collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(options.anchor_hinges[&1], DVec3::new(1.0, 0.0, 1.0));
}

/// The standard round brilliant: every tier pinned, a real closed stone.
fn round_brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// A design that cannot solve: it has no scale-reference anchor at all.
fn unsolvable() -> Design {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    design
        .tiers
        .push(tier("A", 30.0, MeetConstraint::MeetExisting));
    design
}

/// The tier positions of the round brilliant's Crown Main and Pavilion Main.
const CROWN_MAIN: usize = 2;
const PAVILION_MAIN: usize = 5;

#[test]
fn options_that_do_not_vary_anchored_tiers_are_left_alone_without_a_solve() {
    let mut options = OptimizeOptions::default();
    prepare_anchor_hinges(&round_brilliant(), &mut options, None).expect("nothing to do");
    assert_eq!(options, OptimizeOptions::default());

    // A design that cannot solve is no problem either: nothing is solved.
    let design = unsolvable();
    assert!(design.solve().is_err(), "premise: the design cannot solve");
    prepare_anchor_hinges(&design, &mut options, None).expect("nothing to do");
    assert_eq!(options, OptimizeOptions::default());
}

#[test]
fn varying_anchored_tiers_measures_a_hinge_for_the_tiers_or_only_the_selected_ones() {
    let design = round_brilliant();
    let mut options = OptimizeOptions {
        vary_anchored: true,
        ..OptimizeOptions::default()
    };
    prepare_anchor_hinges(&design, &mut options, None).expect("the design solves");
    assert!(options.anchor_hinges.contains_key(&CROWN_MAIN));
    assert!(options.anchor_hinges.contains_key(&PAVILION_MAIN));
    assert!(
        options
            .anchor_hinges
            .keys()
            .all(|&index| index < design.tiers.len())
    );

    let only = BTreeSet::from([PAVILION_MAIN]);
    let mut limited = OptimizeOptions {
        vary_anchored: true,
        ..OptimizeOptions::default()
    };
    prepare_anchor_hinges(&design, &mut limited, Some(&only)).expect("the design solves");
    assert_eq!(
        limited.anchor_hinges.keys().copied().collect::<Vec<_>>(),
        vec![PAVILION_MAIN]
    );
}

#[test]
fn a_solve_the_caller_already_has_gives_the_same_hinges() {
    let design = round_brilliant();
    let solved = design.solve().expect("the design solves");
    let mut measured = OptimizeOptions::default();
    measure_anchor_hinges(&mut measured, &design, &solved, None);
    let mut prepared = OptimizeOptions {
        vary_anchored: true,
        ..OptimizeOptions::default()
    };
    prepare_anchor_hinges(&design, &mut prepared, None).expect("the design solves");
    assert_eq!(measured.anchor_hinges, prepared.anchor_hinges);
    assert!(!measured.anchor_hinges.is_empty());
}

#[test]
fn a_design_that_does_not_solve_is_refused_when_hinges_are_needed() {
    let mut options = OptimizeOptions {
        vary_anchored: true,
        ..OptimizeOptions::default()
    };
    assert!(prepare_anchor_hinges(&unsolvable(), &mut options, None).is_err());
    assert!(options.anchor_hinges.is_empty());
}

// --- the search knobs ---

#[test]
fn the_budget_reads_numbers_and_arithmetic_and_a_blank_means_the_default() {
    assert_eq!(parse_budget("200"), Ok(200));
    assert_eq!(parse_budget(" 50 * 4 "), Ok(200));
    assert_eq!(parse_budget("1e3"), Ok(1000));
    assert_eq!(parse_budget(""), Ok(DEFAULT_BUDGET));
    assert_eq!(parse_budget("12.6"), Ok(13));
}

#[test]
fn a_budget_that_makes_no_sense_is_refused() {
    assert!(parse_budget("0").unwrap_err().contains("at least 1"));
    assert!(parse_budget("-5").unwrap_err().contains("at least 1"));
    assert!(parse_budget("1000000").unwrap_err().contains("at most"));
    assert!(
        parse_budget("lots")
            .unwrap_err()
            .starts_with("Budget 'lots'")
    );
    assert!(parse_budget("inf").is_err());
}

#[test]
fn the_desktop_defaults_are_eight_starts_and_a_budget_of_eight_hundred() {
    assert_eq!(DEFAULT_BUDGET, 800);
    assert_eq!(DEFAULT_STARTS, 8);
    assert_eq!(parse_budget(""), Ok(800));
    assert_eq!(parse_starts(""), Ok(8));
}

#[test]
fn the_starts_read_whole_numbers_and_arithmetic() {
    assert_eq!(parse_starts("1"), Ok(1));
    assert_eq!(parse_starts(" 2 * 4 "), Ok(8));
    assert_eq!(parse_starts("32"), Ok(MAX_STARTS));
    assert_eq!(parse_starts("3.0"), Ok(3));
    assert_eq!(parse_starts("   "), Ok(DEFAULT_STARTS));
}

#[test]
fn starts_that_make_no_sense_are_refused() {
    assert!(parse_starts("0").unwrap_err().contains("1 or more"));
    assert!(parse_starts("-2").unwrap_err().contains("1 or more"));
    assert!(parse_starts("2.5").unwrap_err().contains("whole number"));
    assert!(parse_starts("33").unwrap_err().contains("at most 32"));
    assert!(
        parse_starts("many")
            .unwrap_err()
            .starts_with("Starts 'many'")
    );
    assert!(parse_starts("inf").is_err());
}

#[test]
fn the_starts_are_carried_into_the_config_and_clamped() {
    let design = adopted();
    let run = plan(
        &design,
        &RunForm {
            starts: 6,
            ..form()
        },
    )
    .expect("builds");
    assert_eq!(run.config.starts, 6);
    let run = plan(
        &design,
        &RunForm {
            starts: 0,
            ..form()
        },
    )
    .expect("builds");
    assert_eq!(run.config.starts, 1);
    let run = plan(
        &design,
        &RunForm {
            starts: 500,
            ..form()
        },
    )
    .expect("builds");
    assert_eq!(run.config.starts, MAX_STARTS);
}

#[test]
fn the_seed_reads_whole_numbers_and_arithmetic() {
    assert_eq!(parse_seed("0"), Ok(0));
    assert_eq!(parse_seed(" 41 + 1 "), Ok(42));
    assert_eq!(parse_seed(""), Ok(0));
    assert_eq!(parse_seed("7.0"), Ok(7));
}

#[test]
fn a_seed_that_is_not_a_whole_number_is_refused() {
    for bad in ["-1", "1.5", "x", "inf", "1e300"] {
        assert!(parse_seed(bad).is_err(), "{bad}");
    }
    assert!(parse_seed("x").unwrap_err().starts_with("Seed 'x'"));
}

#[test]
fn polish_off_removes_the_polish_stage_and_the_budget_and_seed_are_carried() {
    let design = adopted();
    let run = plan(
        &design,
        &RunForm {
            polish: false,
            budget_text: "80",
            seed_text: "5",
            ..form()
        },
    )
    .expect("builds");
    assert_eq!(run.config.polish_start_step_deg, None);
    assert_eq!(run.config.max_evaluations, 80);
    assert_eq!(run.config.seed, 5);
    assert_eq!(run.config.lighting, CANONICAL_LIGHTING_PRESET);

    let with_polish = plan(&design, &form()).expect("builds");
    assert_eq!(
        with_polish.config.polish_start_step_deg,
        OptimizeConfig::default().polish_start_step_deg
    );
}

#[test]
fn the_default_form_is_the_default_run() {
    let design = adopted();
    let run = plan(&design, &form()).expect("builds");
    assert_eq!(run.config.weights, ObjectiveWeights::default());
    assert_eq!(
        run.config.max_evaluations,
        OptimizeConfig::default().max_evaluations
    );
    assert_eq!(run.config.seed, 0);
}

// --- availability ---

#[test]
fn a_design_with_free_tiers_is_available_either_way() {
    let design = adopted();
    assert!(optimize_availability(&design, 200, false).0);
    assert!(optimize_availability(&design, 200, true).0);
}

#[test]
fn a_fully_pinned_design_is_available_once_anchored_tiers_may_vary() {
    let design = imported();
    let (available, hint) = optimize_availability(&design, 200, false);
    assert!(!available);
    assert!(hint.contains("pinned"), "{hint}");
    assert!(hint.contains("Vary anchored tiers"), "{hint}");

    let (available, hint) = optimize_availability(&design, 200, true);
    assert!(available);
    assert!(hint.contains("girdle"), "{hint}");
}

#[test]
fn a_design_with_nothing_that_can_move_is_unavailable_even_with_the_switch_on() {
    let mut design = EditorSession::fresh().design;
    design
        .tiers
        .push(tier("G", -90.0, MeetConstraint::ScaleReference(0.9)));
    assert!(!optimize_availability(&design, 200, true).0);
}

// --- the time estimate ---

#[test]
fn a_small_design_costs_seven_milliseconds_an_evaluation_plus_the_full_scorings() {
    // 200 evaluations at 7 ms = 1.4 s; the start and three candidates = 4 x 1.3 s.
    let seconds = estimate_run_seconds(12, 12, 200, 3, 1, None);
    assert!((seconds - (1.4 + 5.2)).abs() < 1e-9, "{seconds}");
}

#[test]
fn the_estimate_grows_with_the_budget_the_candidates_and_the_tier_count() {
    let base = estimate_run_seconds(12, 12, 200, 3, 1, None);
    assert!(estimate_run_seconds(12, 12, 400, 3, 1, None) > base);
    assert!(estimate_run_seconds(12, 12, 200, 5, 1, None) > base);
    assert!(estimate_run_seconds(36, 36, 200, 3, 1, None) > base);
    assert!(
        (estimate_run_seconds(6, 6, 200, 3, 1, None) - base).abs() < 1e-9,
        "never below the small design"
    );
}

#[test]
fn a_measured_rate_replaces_the_guess() {
    let seconds = estimate_run_seconds(12, 12, 100, 1, 1, Some(50.0));
    assert!((seconds - (5.0 + 2.6)).abs() < 1e-9, "{seconds}");
}

#[test]
fn several_starts_add_screening_and_polish_work_to_a_measured_estimate() {
    // A measured rate is a wall-clock rate, so no thread divisor applies and the extra
    // evaluations (screening draws, a polish per kept start) show in full.
    let one = estimate_run_seconds(12, 12, 800, 3, 1, Some(10.0));
    let eight = estimate_run_seconds(12, 12, 800, 3, 8, Some(10.0));
    assert!(eight > one, "{eight} vs {one}");
}

#[test]
fn a_budget_too_small_for_the_starts_is_estimated_as_one_start() {
    // Under eight sweeps per tier the search runs one start, and so does the estimate.
    let one = estimate_run_seconds(12, 12, 50, 3, 1, None);
    let eight = estimate_run_seconds(12, 12, 50, 3, 8, None);
    assert!((one - eight).abs() < 1e-9, "{one} vs {eight}");
}

#[test]
fn the_starts_rule_uses_the_free_tier_count_not_the_tier_count() {
    // Budget 100, eight starts asked: 12 free tiers leave one start (100 / 96), 2 free
    // tiers leave several (100 / 16), whatever the design's tier count is. A measured
    // rate carries no thread divisor, so the extra screening and polish work shows.
    let many_free = estimate_run_seconds(12, 12, 100, 3, 8, Some(10.0));
    let few_free = estimate_run_seconds(12, 2, 100, 3, 8, Some(10.0));
    assert!(few_free > many_free, "{few_free} vs {many_free}");
}

#[test]
fn more_threads_never_lengthen_the_guessed_estimate_of_many_starts() {
    // The guess divides the per-evaluation cost by min(starts, cores / 2) (at least one), so the
    // guess is never above the same evaluations run on a single thread.
    let guess = estimate_run_seconds(12, 12, 800, 3, 8, None);
    let config = OptimizeConfig {
        max_evaluations: 800,
        starts: 8,
        ..OptimizeConfig::default()
    };
    let serial_evaluations =
        indicatrix_cut_core::optimize::inclusive_max_evaluations_for(&config, 3, 12);
    let serial = (serial_evaluations as f64).mul_add(0.007, 4.0 * 1.3);
    assert!(guess <= serial + 1e-9, "{guess} vs {serial}");
}

#[test]
fn a_finished_run_gives_the_cost_of_one_evaluation() {
    // 100 evaluations, two candidates: 3 scorings = 3.9 s; 13.9 s total leaves 10 s.
    let rate = measured_ms_per_evaluation(13.9, 100, 2).expect("long enough");
    assert!((rate - 100.0).abs() < 1e-9, "{rate}");
    // No candidate still pays for the start and the one result.
    let rate = measured_ms_per_evaluation(2.6 + 5.0, 100, 0).expect("long enough");
    assert!((rate - 50.0).abs() < 1e-9, "{rate}");
}

#[test]
fn a_run_too_short_to_tell_gives_no_rate() {
    assert_eq!(measured_ms_per_evaluation(60.0, 19, 3), None);
    assert_eq!(measured_ms_per_evaluation(3.0, 100, 3), None, "all scoring");
    assert_eq!(measured_ms_per_evaluation(f64::NAN, 100, 3), None);
}

#[test]
fn durations_read_in_plain_words() {
    assert_eq!(format_duration(0.3), "a second or two");
    assert_eq!(format_duration(4.4), "about 4 seconds");
    assert_eq!(format_duration(41.0), "about 40 seconds");
    assert_eq!(format_duration(95.0), "about 2 minutes");
    assert_eq!(format_duration(70.0), "about 1 minute");
    assert_eq!(format_duration(3000.0), "about 50 minutes");
    assert_eq!(format_duration(7200.0), "about 2.0 hours");
    assert_eq!(format_duration(f64::NAN), "unknown");
    assert_eq!(format_duration(-1.0), "unknown");
}

#[test]
fn the_estimate_line_says_where_the_figure_comes_from() {
    assert!(estimate_text(40.0, true).contains("your last run"));
    assert!(estimate_text(40.0, false).contains("rough guess"));
    assert!(estimate_text(40.0, false).starts_with("Estimated time: about 40 seconds"));
}
