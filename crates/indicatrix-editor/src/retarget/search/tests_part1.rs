use super::*;
use crate::{
    retarget::{
        CrownShift, apply_with_anchors, build_plan,
        check::best_attempt,
        validity::ValidityStatus,
        view::{FOLLOWS_LABEL, plan_row_view},
    },
    session::EditorSession,
};
use indicatrix_cut_core::{
    BuiltinMaterials, ConstraintTier, History, MaterialSelection, PreformSpec, ResolvedMaterial,
    ScheduleMeta,
};

fn selection_at(n_d: f64) -> MaterialSelection {
    MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(n_d),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
}

fn target_at(n_d: f64) -> ResolvedMaterial {
    selection_at(n_d).resolve(&BuiltinMaterials)
}

/// The standard brilliant as a 1.72 stone: the owner's starting point for the sapphire and
/// emerald cases. Every tier is a pinned (`ScaleReference`) tier, like an imported design.
fn brilliant_at_172() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.material = selection_at(1.72);
    design.ensure_tier_ids();
    design
}

const SAPPHIRE_CROWN: CrownShift = CrownShift {
    fraction: 0.33,
    scale_by_ratio: false,
    follow_pavilion: false,
};

const EMERALD_CROWN: CrownShift = CrownShift {
    fraction: 0.0,
    scale_by_ratio: true,
    follow_pavilion: false,
};

/// A small budget so the tests stay quick.
fn quick() -> SearchSettings {
    SearchSettings {
        evaluations: 24,
        keep: 2,
        // The allowance has its own tests; the rest pin the allowance-free behaviour.
        girdle: None,
        ..SearchSettings::default()
    }
}

fn tier_named(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .unwrap_or_else(|| panic!("no tier named {name}"))
}

fn run(
    design: &Design,
    plan: &RetargetPlan,
    settings: &SearchSettings,
) -> Result<SearchReport, SearchError> {
    let inputs = SearchInputs {
        design,
        plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
        settings,
    };
    run_search(&inputs, &AtomicBool::new(false), &|_, _| {})
}

/// Makes `driven` follow `text`.
fn follow(design: &mut Design, driven: usize, text: &str) {
    design.ensure_tier_ids();
    let relation = design.parse_relation(text).expect("the relation reads");
    let id = design.tier_ids[driven];
    design.tier_relations.insert(id, relation);
}

/// The brilliant with its lower-girdle angle following the pavilion main: 1.5 degrees
/// steeper, which is how the standard schedule already is (41 and 42.5).
fn brilliant_with_a_follower() -> (Design, usize, usize) {
    let mut design = brilliant_at_172();
    let main = tier_named(&design, "Pavilion Main");
    let lower = tier_named(&design, "Lower Girdle");
    follow(&mut design, lower, "[Pavilion Main] + 1.5");
    (design, main, lower)
}

// --- Settings, estimate and bounds ---

#[test]
fn the_default_settings_are_balanced_six_degrees_three_hundred_steps_three_options() {
    let settings = SearchSettings::default();
    assert_eq!(settings.preset, ObjectivePreset::Balanced);
    assert!((settings.range_deg - 6.0).abs() < 1e-12);
    assert_eq!(settings.evaluations, 300);
    assert_eq!(settings.keep, 3);
    assert_eq!(settings.seed, 0);
}

#[test]
fn a_choice_past_the_end_of_a_list_is_handled() {
    let settings = SearchSettings::from_choices(99, 99, 99);
    assert_eq!(settings.preset, ObjectivePreset::Balanced);
    assert!((settings.range_deg - 15.0).abs() < 1e-12);
    assert_eq!(settings.evaluations, 800);
    let third = SearchSettings::from_choices(2, 0, 0);
    assert_eq!(third.preset, ObjectivePreset::LowWindowing);
    assert!((third.range_deg - 3.0).abs() < 1e-12);
    assert_eq!(third.evaluations, 100);
}

#[test]
fn the_polish_stage_adds_to_the_budget() {
    let settings = SearchSettings::default();
    assert_eq!(settings.total_steps(5), 300 + 3 * 5 + 20);
    assert!(settings.total_steps(10) > settings.total_steps(5));
}

#[test]
fn the_estimate_says_how_long_in_plain_words() {
    assert!(estimate_text(300, Some(40.0)).contains("about 12 s"));
    assert!(estimate_text(300, Some(0.5)).contains("under a second"));
    assert!(estimate_text(800, Some(500.0)).contains("min"));
    assert!(estimate_text(300, None).contains("measured"));
    assert!(estimate_text(300, Some(f64::NAN)).contains("measured"));
    assert!(estimate_text(300, Some(0.0)).contains("measured"));
}

#[test]
fn bounds_are_the_range_either_side_on_the_angles_own_side() {
    assert_eq!(angle_bounds(41.0, 6.0), (35.0, 47.0));
    assert_eq!(angle_bounds(-41.0, 6.0), (-47.0, -35.0));
}

#[test]
fn bounds_never_cross_the_horizontal_or_pass_the_steep_limit() {
    let (low, high) = angle_bounds(3.0, 6.0);
    assert!((low - 1.0).abs() < 1e-12 && (high - 9.0).abs() < 1e-12);
    let (low, high) = angle_bounds(88.0, 6.0);
    assert!((low - 82.0).abs() < 1e-12 && (high - 89.0).abs() < 1e-12);
    for angle in [0.4, 1.0, 2.0, 15.0, 45.0, 89.5, -0.4, -2.0, -41.0, -80.0] {
        let (a, b) = angle_bounds(angle, 15.0);
        assert!(a <= angle && angle <= b, "{angle}: ({a}, {b})");
        assert_eq!(a.is_sign_negative(), angle.is_sign_negative(), "{angle}");
        assert_eq!(b.is_sign_negative(), angle.is_sign_negative(), "{angle}");
        assert!(a != 0.0 && b != 0.0, "{angle}");
    }
}

#[test]
fn the_girdle_floor_is_half_of_the_live_girdle_measured_against_the_start() {
    let mut original = analyze(&brilliant_at_172(), false).expect("the brilliant solves");
    let mut start = original.clone();
    original.girdle_percent = Some(2.0);
    start.girdle_percent = Some(2.0);
    assert!((girdle_fraction(&original, &start) - 0.5).abs() < 1e-12);
    start.girdle_percent = Some(4.0);
    assert!((girdle_fraction(&original, &start) - 0.25).abs() < 1e-12);
    start.girdle_percent = Some(1.0);
    assert!((girdle_fraction(&original, &start) - 1.0).abs() < 1e-12);
    start.girdle_percent = None;
    assert!((girdle_fraction(&original, &start) - 0.5).abs() < 1e-12);
    original.girdle_percent = None;
    start.girdle_percent = Some(2.0);
    assert!((girdle_fraction(&original, &start) - 0.5).abs() < 1e-12);
}

#[test]
fn the_guard_applies_each_floor_to_its_own_figure() {
    let design = brilliant_at_172();
    let mut original = analyze(&design, false).expect("the brilliant solves");
    let mut start = original.clone();
    original.girdle_percent = Some(6.0);
    original.girdle_thinnest_percent = Some(0.5);
    start.girdle_percent = Some(6.0);

    // A start whose corners are thinner than the live design's (0.3 against 0.5): the thinnest
    // floor rises to 0.25 / 0.3 of the start's figure, and the overall floor stays at a half.
    start.girdle_thinnest_percent = Some(0.3);
    let fractions = guard_fractions(&original, &start);
    assert!((fractions.overall - 0.5).abs() < 1e-12, "{fractions:?}");
    assert!(
        (fractions.thinnest - 0.25 / 0.3).abs() < 1e-12,
        "{fractions:?}"
    );

    // The options carry both: one floor is not held to the other's value.
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let settings = SearchSettings::default();
    let inputs = SearchInputs {
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
        settings: &settings,
    };
    let options = search_options(
        &inputs,
        &original,
        &design,
        &[],
        &BTreeMap::new(),
        fractions,
    );
    assert_eq!(options.min_girdle_fraction, Some(fractions.overall));
    assert_eq!(
        options.min_girdle_thinnest_fraction,
        Some(fractions.thinnest)
    );

    // Corners thicker than the live design's lower their floor; the overall floor does not move.
    start.girdle_thinnest_percent = Some(1.0);
    let fractions = guard_fractions(&original, &start);
    assert!((fractions.overall - 0.5).abs() < 1e-12, "{fractions:?}");
    assert!((fractions.thinnest - 0.25).abs() < 1e-12, "{fractions:?}");

    // And the overall figure moves its own floor alone.
    start.girdle_percent = Some(12.0);
    let fractions = guard_fractions(&original, &start);
    assert!((fractions.overall - 0.25).abs() < 1e-12, "{fractions:?}");
    assert!((fractions.thinnest - 0.25).abs() < 1e-12, "{fractions:?}");

    // A start whose corners are a knife edge already, or not measured, has half to keep.
    start.girdle_thinnest_percent = Some(0.0);
    assert!((guard_fractions(&original, &start).thinnest - 0.5).abs() < 1e-12);
    start.girdle_thinnest_percent = None;
    assert!((guard_fractions(&original, &start).thinnest - 0.5).abs() < 1e-12);
}

// --- Errors ---

#[test]
fn a_design_that_does_not_solve_cannot_be_searched() {
    let mut design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    for tier in &mut design.tiers {
        tier.constraint = MeetConstraint::MeetExisting;
    }
    let error = run(&design, &plan, &quick()).expect_err("nothing to compare with");
    assert!(matches!(error, SearchError::Unusable(_)), "{error:?}");
    assert!(!probe_evaluation(
        &design,
        &GemMaterial::diamond(),
        LightingPreset::RingLights
    ));
}

#[test]
fn the_errors_read_in_plain_english() {
    let does_not_solve =
        SearchError::Unusable(InvalidReason::DoesNotSolve("no anchor".to_string()));
    assert!(does_not_solve.to_string().contains("solves first"));
    assert!(does_not_solve.to_string().contains("no anchor"));
    let not_closed = SearchError::Unusable(InvalidReason::NotClosed);
    assert!(not_closed.to_string().contains("enclose a stone"));
    assert!(
        SearchError::Solve("no anchor".to_string())
            .to_string()
            .contains("no anchor")
    );
    assert!(SearchError::Cancelled.to_string().contains("cancelled"));
}

#[test]
fn a_raised_cancel_flag_stops_the_search_before_it_starts() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let settings = quick();
    let inputs = SearchInputs {
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
        settings: &settings,
    };
    let cancel = AtomicBool::new(true);
    assert_eq!(
        run_search(&inputs, &cancel, &|_, _| {}),
        Err(SearchError::Cancelled)
    );
}

#[test]
fn the_probe_evaluates_a_good_design() {
    assert!(probe_evaluation(
        &brilliant_at_172(),
        &GemMaterial::sapphire(),
        LightingPreset::RingLights
    ));
}

#[test]
fn the_probe_times_one_projected_proposal_not_a_bare_solve() {
    let probe = probe::run_probe(
        &brilliant_at_172(),
        &GemMaterial::sapphire(),
        LightingPreset::RingLights,
    );
    assert!(probe.solved);
    let diagnostics = probe
        .diagnostics
        .expect("the design's meet network was captured and a proposal built");
    // Phase 1 counts projections: a bare solve would leave it at zero.
    assert!(diagnostics.phase_calls[1] >= 1, "{diagnostics:?}");
}

#[test]
fn the_probe_of_a_design_that_does_not_solve_projects_nothing() {
    let mut design = brilliant_at_172();
    for tier in &mut design.tiers {
        tier.constraint = MeetConstraint::MeetExisting;
    }
    let probe = probe::run_probe(&design, &GemMaterial::diamond(), LightingPreset::RingLights);
    assert!(!probe.solved);
    assert_eq!(probe.diagnostics, None);
}

// --- The search on a fully pinned design ---

#[test]
fn the_owners_sapphire_case_gives_valid_options_no_worse_than_the_start() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let report = run(&design, &plan, &quick()).expect("the search runs");

    // A change this small is valid by Shift alone (the same case `check.rs` pins), so the search
    // starts from the Shift result. Asserted unconditionally: a Shift that turned invalid must
    // fail here, not slip into a branch that checks nothing.
    assert_eq!(
        report.shift_validity.status,
        ValidityStatus::Valid,
        "{:?}",
        report.shift_validity.reasons
    );
    assert_eq!(report.start, StartPoint::Shift);
    assert!(
        report.free_tiers > 0,
        "every pinned facet must be searchable"
    );
    assert_ne!(report.candidates, Vec::new(), "{:?}", report.shift_validity);
    for candidate in &report.candidates {
        assert_eq!(
            candidate.validity.status,
            ValidityStatus::Valid,
            "{:?}",
            candidate.validity
        );
        assert!(
            candidate.numbers.score <= report.start_numbers.score + 1e-3,
            "{} against the start {}",
            candidate.numbers.score,
            report.start_numbers.score
        );
    }
    let best = report.best().expect("a best option");
    assert!(best.numbers.score <= report.start_numbers.score + 1e-3);
    assert!(
        report
            .candidates
            .windows(2)
            .all(|pair| pair[0].numbers.score <= pair[1].numbers.score),
        "best first"
    );
}

#[test]
fn the_owners_emerald_case_ends_valid_in_one_way_or_another() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.5791), EMERALD_CROWN, &[]);
    let report = run(&design, &plan, &quick()).expect("the search runs");
    assert_ne!(
        report.candidates,
        Vec::new(),
        "{:?} / {:?}",
        report.shift_validity,
        report.notes()
    );
    for candidate in &report.candidates {
        assert_eq!(candidate.validity.status, ValidityStatus::Valid);
        // No option leaves the girdle thinner than half at its corners, or a knife edge there:
        // the overall girdle figure cannot show it, so the options' figures are read directly.
        let (was, now) = (
            candidate.validity.figures.thinnest_was,
            candidate.validity.figures.thinnest_now,
        );
        if let (Some(was), Some(now)) = (was, now) {
            assert!(
                now > KNIFE_EDGE_PERCENT && now >= MIN_GIRDLE_FRACTION * was,
                "{:?}: thinnest point {was} % became {now} %",
                candidate.kind
            );
        }
    }
}

#[test]
fn the_owners_emerald_shift_with_the_girdle_allowance_never_loses_its_corners_silently() {
    // With the allowance on (the dialog's default) the Shift may climb the girdle ladder. Under
    // the owner's girdle OUTLINE contract (2026-10-19) the corners are kept, so the Shift is
    // usually valid at rung 0 and nothing is thickened. Either a rung makes it valid, and then
    // the verdict names the rung, the band is thicker and the table keeps its size; or no rung
    // is enough and the old refusal stands.
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.5791), EMERALD_CROWN, &[]);
    let settings = SearchSettings {
        girdle: Some(GirdleAllowance::standard()),
        ..quick()
    };
    let report = run(&design, &plan, &settings).expect("the search runs");
    let figures = report.shift_validity.figures;
    if report.shift_validity.status == ValidityStatus::Valid {
        assert_eq!(report.start, StartPoint::Shift);
        // `None` means valid at rung 0: the outline hinge kept the corners and nothing was
        // thickened (owner's girdle OUTLINE contract, 2026-10-19).
        if let Some(percent) = figures.girdle_thickened_percent {
            assert!(
                (percent - 5.0).abs() < 1e-9 || (percent - 10.0).abs() < 1e-9,
                "{percent}"
            );
            let (was, now) = (figures.girdle_was.unwrap(), figures.girdle_now.unwrap());
            assert!(now > was, "girdle {was} % became {now} %");
            let (was, now) = (figures.table_was.unwrap(), figures.table_now.unwrap());
            assert!((now - was).abs() < 1.0, "table {was} % became {now} %");
            assert!(
                report
                    .shift_validity
                    .detail_lines()
                    .iter()
                    .any(|line| line.contains("The girdle was thickened")),
                "{:?}",
                report.shift_validity.detail_lines()
            );
        }
    } else {
        assert!(
            report
                .shift_validity
                .reasons
                .iter()
                .any(|reason| matches!(reason, InvalidReason::GirdleThinAtCorners { .. })),
            "{:?}",
            report.shift_validity.reasons
        );
    }
}

#[test]
fn a_girdle_failure_climbs_the_ladder_and_other_stones_do_not() {
    let design = brilliant_at_172();
    let original = analyze(&design, true).expect("the design solves and closes");
    let plan = build_plan(&design, &target_at(1.5791), EMERALD_CROWN, &[]);
    let moves = plan.moving_angles();
    let unallowed = best_attempt(&design, &moves, &original, None, &|| false)
        .expect("relations hold")
        .expect("not stopped");
    let allowed = best_attempt(
        &design,
        &moves,
        &original,
        Some(GirdleAllowance::standard()),
        &|| false,
    )
    .expect("relations hold")
    .expect("not stopped");
    assert_eq!(unallowed.girdle_step, None);
    if unallowed.reasons.is_empty() {
        // Rung 0 wins when it is valid: the allowance changes nothing.
        assert_eq!(allowed.girdle_step, None);
        assert_eq!(allowed.anchors, unallowed.anchors);
    } else if allowed.girdle_step.is_some() {
        assert_eq!(allowed.reasons.len(), 0);
    }
    // A plan that moves nothing is valid at rung 0 and never thickens.
    let still = best_attempt(
        &design,
        &[],
        &original,
        Some(GirdleAllowance::standard()),
        &|| false,
    )
    .expect("relations hold")
    .expect("not stopped");
    assert_eq!(still.girdle_step, None);
}

#[test]
fn the_owners_emerald_shift_keeps_its_corners_and_the_search_starts_from_it_when_it_is_valid() {
    // The Shift turns both break facets steeper about their girdle OUTLINE corners (owner
    // decision, 2026-10-19): the corners keep their thickness, so the gate no longer refuses
    // the Shift for them. When the Shift is valid the search starts from it; otherwise
    // (another reason) the old note stands.
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.5791), EMERALD_CROWN, &[]);
    let report = run(&design, &plan, &quick()).expect("the search runs");
    assert!(
        !report
            .shift_validity
            .reasons
            .iter()
            .any(|reason| matches!(reason, InvalidReason::GirdleThinAtCorners { .. })),
        "the outline hinge keeps the corners: {:?}",
        report.shift_validity.reasons
    );
    if report.shift_validity.status == ValidityStatus::Valid {
        assert_eq!(report.start, StartPoint::Shift);
    } else {
        assert_ne!(report.start, StartPoint::Shift);
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("Shift alone is not valid here")),
            "{:?}",
            report.notes()
        );
    }
}

#[test]
fn the_same_inputs_give_the_same_candidates() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let first = run(&design, &plan, &quick()).expect("the search runs");
    let second = run(&design, &plan, &quick()).expect("the search runs");
    assert_eq!(first, second);
}

#[test]
fn table_culet_and_girdle_keep_their_angle_and_the_girdle_keeps_its_mast() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let report = run(&design, &plan, &quick()).expect("the search runs");
    assert_ne!(report.candidates, Vec::new());
    for candidate in &report.candidates {
        for name in ["Table", "Culet", "Girdle"] {
            let index = tier_named(&design, name);
            assert_eq!(
                candidate.design.tiers[index].angle_deg.to_bits(),
                design.tiers[index].angle_deg.to_bits(),
                "{name}"
            );
            assert!(
                candidate
                    .angles
                    .iter()
                    .all(|&(changed, _)| changed != index),
                "{name} must stay out of the proposal"
            );
        }
        let girdle = tier_named(&design, "Girdle");
        assert_eq!(candidate.design.tiers[girdle], design.tiers[girdle]);
    }
}

#[test]
fn every_option_keeps_the_table_and_culet_size() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let report = run(&design, &plan, &quick()).expect("the search runs");
    assert_ne!(report.candidates, Vec::new());
    let original = analyze(&design, true).expect("the brilliant solves");
    assert!(
        !original.flats.is_empty(),
        "the brilliant has a table and a culet"
    );
    for candidate in &report.candidates {
        let stone = analyze(&candidate.design, false).expect("an option solves");
        for flat in &original.flats {
            let now = stone
                .flats
                .iter()
                .find(|other| other.tier_index == flat.tier_index)
                .unwrap_or_else(|| panic!("tier {} lost its facet", flat.tier_index));
            let linear = (now.area / flat.area).sqrt();
            assert!(
                (linear - 1.0).abs() < 0.025,
                "tier {}: the facet is {linear} times its original size",
                flat.tier_index
            );
        }
    }
}

#[test]
fn a_pavilion_angle_never_drops_below_its_floor() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.9), SAPPHIRE_CROWN, &[]);
    let settings = SearchSettings {
        range_deg: 15.0,
        ..quick()
    };
    let report = run(&design, &plan, &settings).expect("the search runs");
    let (_, blocks) = retarget_scope(&design);
    let moves = plan.moving_angles();
    for candidate in &report.candidates {
        for (index, block) in blocks.iter().enumerate() {
            if *block != Block::Pavilion || design.is_tier_driven(index) {
                continue;
            }
            let live = design.tiers[index].angle_deg;
            // The start stone may already sit under the floor; a bound never forces it up.
            let start = moves
                .iter()
                .find(|(tier, _)| *tier == index)
                .map_or(live, |&(_, angle)| angle);
            let floor = pavilion_floor_deg(live, plan.n_from, plan.n_to).min(start.abs());
            assert!(
                candidate.design.tiers[index].angle_deg.abs() >= floor - 1e-9,
                "tier {index}: {} is under the floor {floor}",
                candidate.design.tiers[index].angle_deg
            );
        }
    }
}

#[test]
fn pavilion_floor_is_the_smaller_of_the_original_margin_and_two_degrees() {
    let n_from = 1.72;
    let n_to = 1.9;
    let crit_from = critical_angle_deg(n_from);
    let crit_to = critical_angle_deg(n_to);
    // A margin of 4 degrees is capped at 2.
    let floor = pavilion_floor_deg(crit_from + 4.0, n_from, n_to);
    assert!((floor - (crit_to + 2.0)).abs() < 1e-12, "{floor}");
    // A margin of 1 degree is kept as it is.
    let floor = pavilion_floor_deg(crit_from + 1.0, n_from, n_to);
    assert!((floor - (crit_to + 1.0)).abs() < 1e-12, "{floor}");
    // The sign of the angle does not matter (a pavilion angle is negative).
    let floor = pavilion_floor_deg(-(crit_from + 1.0), n_from, n_to);
    assert!((floor - (crit_to + 1.0)).abs() < 1e-12, "{floor}");
    // A pavilion that was already windowing has no margin to keep.
    let floor = pavilion_floor_deg(crit_from - 1.0, n_from, n_to);
    assert!((floor - (crit_to - 1.0)).abs() < 1e-12, "{floor}");
}

#[test]
fn a_floor_never_lifts_the_bounds_above_the_start_angle() {
    // Positive and negative angles, a floor under, at and over the start.
    assert_eq!(raise_floor((35.0, 47.0), 41.0, 38.0), (38.0, 47.0));
    assert_eq!(raise_floor((35.0, 47.0), 41.0, 44.0), (41.0, 47.0));
    assert_eq!(raise_floor((-47.0, -35.0), -41.0, 38.0), (-47.0, -38.0));
    assert_eq!(raise_floor((-47.0, -35.0), -41.0, 44.0), (-47.0, -41.0));
    // A floor under the low bound, or one that is not a number, changes nothing.
    assert_eq!(raise_floor((35.0, 47.0), 41.0, 30.0), (35.0, 47.0));
    assert_eq!(raise_floor((35.0, 47.0), 41.0, f64::NAN), (35.0, 47.0));
}
