//! Tests for the overall verdict: the levels for constructed inputs, what [`gather()`] reads off
//! real designs, and that every fix gives the edit it promises as ONE undo step.

use super::{
    BRILLIANCE_CHECK_BELOW_PCT, BRILLIANCE_PROBLEM_BELOW_PCT, EXTINCTION_CHECK_PCT,
    EXTINCTION_PROBLEM_PCT, FixAction, Level, MAX_LISTED_WINDOWING_TIERS, ProportionFact,
    QUICK_ADD_TABLE_MAST, ReasonKind, SolveFacts, TierWindowing, VerdictInputs,
    WINDOWING_CHECK_PCT, WINDOWING_PROBLEM_PCT, evaluate,
    fix::{partial_removal_edit, remove_tier_edit, unique_table_name, vanished_positions_in},
    gather, plan_fix, steep_target_deg,
};
use crate::{
    retarget::{metrics::MetricColumn, validity::analyze},
    session::EditorSession,
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, History, ManufacturabilityWarning, PreformSpec, ScheduleMeta,
    TierId, design::hinge::tier_hinge_points, manufacturability::check_cut_order,
};
use std::{collections::BTreeSet, ops::Range};

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint, indices: &[f64]) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn session_of(design: Design) -> EditorSession {
    EditorSession::with_history(design, History::new())
}

/// Applies `edit` through the session, the way the desktop applies a fix.
fn apply(session: &mut EditorSession, edit: Edit) {
    session
        .try_apply(edit)
        .map_err(|error| error.to_string())
        .expect("the planned edit applies");
}

/// A block-preform design with the given tiers (tier ids allocated).
fn design_with(tiers: Vec<ConstraintTier>) -> Design {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers = tiers;
    design.ensure_tier_ids();
    design
}

/// The standard round brilliant in a rough deep enough that the preform never touches it.
fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// The standard round brilliant keeping only the tiers named in `keep`.
fn brilliant_keeping(keep: &[&str]) -> Design {
    let tiers = ConstraintTier::standard_round_brilliant()
        .into_iter()
        .filter(|tier| keep.contains(&tier.name.as_str()))
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        tiers,
    )
}

fn position_of(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .unwrap_or_else(|| panic!("no tier named {name}"))
}

/// Inputs for a stone that solves and closes with nothing else to say.
fn closed() -> VerdictInputs {
    VerdictInputs {
        solve: SolveFacts::Closed,
        ..VerdictInputs::default()
    }
}

fn column(windowing: f32, brilliance: f32, extinction: f32) -> MetricColumn {
    MetricColumn {
        windowing_pct: windowing,
        brilliance_pct: brilliance,
        extinction_pct: extinction,
    }
}

/// A sound stone's optics: little windowing, good brilliance, moderate extinction.
fn sound_optics() -> MetricColumn {
    column(4.0, 70.0, 12.0)
}

fn fractional(tier_index: usize, name: &str, requested: f64) -> ManufacturabilityWarning {
    ManufacturabilityWarning::FractionalIndex {
        tier_index,
        tier_id: TierId(tier_index as u64),
        tier_name: name.to_string(),
        requested,
        achievable: requested.round(),
        azimuth_error_deg: 0.1,
    }
}

fn out_of_order(tier_index: usize, target: usize) -> ManufacturabilityWarning {
    ManufacturabilityWarning::OutOfOrderMeet {
        tier_index,
        tier_id: TierId(tier_index as u64),
        tier_name: format!("T{tier_index}"),
        target_tier_index: target,
        target_tier_name: format!("T{target}"),
    }
}

fn vanishing(
    tier_index: usize,
    name: &str,
    vanished: usize,
    total: usize,
) -> ManufacturabilityWarning {
    ManufacturabilityWarning::VanishingFacet {
        tier_index,
        tier_id: TierId(tier_index as u64),
        tier_name: name.to_string(),
        vanished,
        total,
    }
}

fn windowing_entry(tier: usize, margin: f64, crown: bool, fixable: bool) -> TierWindowing {
    TierWindowing {
        tier,
        name: format!("P{tier}"),
        angle_deg: 38.0,
        critical_deg: 40.5,
        margin_deg: margin,
        crown_estimate: crown,
        fixable,
    }
}

// ---------------------------------------------------------------------------------------
// Levels and sentences for constructed inputs
// ---------------------------------------------------------------------------------------

#[test]
fn a_design_without_tiers_has_nothing_to_check() {
    let verdict = evaluate(&VerdictInputs::default());
    assert_eq!(verdict.level, Level::Good);
    assert!(verdict.reasons.is_empty(), "{:?}", verdict.reasons);
    assert!(
        verdict.headline.contains("Nothing to check"),
        "{}",
        verdict.headline
    );
}

#[test]
fn a_clean_closed_stone_is_good() {
    let verdict = evaluate(&closed());
    assert_eq!(verdict.level, Level::Good);
    assert!(verdict.reasons.is_empty(), "{:?}", verdict.reasons);
    assert_eq!(verdict.headline, "Looks good: closes, no warnings.");
    // With the optics measured the headline says they were looked at too.
    let measured = evaluate(&closed().with_optics(Some(sound_optics())));
    assert_eq!(measured.level, Level::Good);
    assert_eq!(
        measured.headline,
        "Looks good: closes, no warnings, little windowing."
    );
}

#[test]
fn a_design_that_does_not_solve_is_a_problem_and_the_headline_names_the_cause() {
    let inputs = VerdictInputs {
        solve: SolveFacts::DoesNotSolve("P2 needs a meet".to_string()),
        ..VerdictInputs::default()
    };
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Problem);
    assert_eq!(verdict.headline, "Does not solve: P2 needs a meet.");
    assert_eq!(verdict.reasons.len(), 1);
    assert_eq!(verdict.reasons[0].kind, ReasonKind::DoesNotSolve);
    assert_eq!(verdict.reasons[0].fix, None);
}

#[test]
fn facets_that_do_not_close_a_stone_are_a_problem() {
    let inputs = VerdictInputs {
        solve: SolveFacts::NotClosed("3 of the facets run off without closing the stone.".into()),
        ..VerdictInputs::default()
    };
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Problem);
    assert!(
        verdict.headline.starts_with("Does not close: "),
        "{}",
        verdict.headline
    );
}

#[test]
fn warnings_make_it_check_and_the_headline_counts_them() {
    let mut inputs = closed();
    inputs.warnings = vec![fractional(1, "P1", 24.5), out_of_order(0, 2)];
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Check);
    assert_eq!(verdict.headline, "Check 2 things.");
    assert!(verdict.reasons.iter().all(|r| r.level == Level::Check));
    // One thing reads singular.
    let mut single = closed();
    single.warnings = vec![fractional(1, "P1", 24.5)];
    assert_eq!(evaluate(&single).headline, "Check 1 thing.");
}

#[test]
fn the_worst_reason_comes_first_and_leads_the_headline() {
    let mut inputs = VerdictInputs {
        solve: SolveFacts::DoesNotSolve("the girdle needs a mast".to_string()),
        ..VerdictInputs::default()
    };
    inputs.warnings = vec![fractional(0, "P1", 3.5)];
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Problem);
    assert_eq!(verdict.reasons[0].kind, ReasonKind::DoesNotSolve);
    assert_eq!(verdict.reasons[1].kind, ReasonKind::OffGear);
    assert!(verdict.headline.starts_with("Does not solve"));
}

#[test]
fn optical_thresholds_sit_on_the_named_constants() {
    let level_of = |optics: MetricColumn| evaluate(&closed().with_optics(Some(optics))).level;
    let base = sound_optics();

    // Windowing: more than 15 % is a Check, more than 30 % a Problem.
    let at = |windowing: f32| MetricColumn {
        windowing_pct: windowing,
        ..base
    };
    assert_eq!(level_of(at(WINDOWING_CHECK_PCT)), Level::Good);
    assert_eq!(level_of(at(WINDOWING_CHECK_PCT + 0.01)), Level::Check);
    assert_eq!(level_of(at(WINDOWING_PROBLEM_PCT)), Level::Check);
    assert_eq!(level_of(at(WINDOWING_PROBLEM_PCT + 0.01)), Level::Problem);
    assert!((WINDOWING_CHECK_PCT - 15.0).abs() < f32::EPSILON);
    assert!((WINDOWING_PROBLEM_PCT - 30.0).abs() < f32::EPSILON);

    // Extinction.
    let at = |extinction: f32| MetricColumn {
        extinction_pct: extinction,
        ..base
    };
    assert_eq!(level_of(at(EXTINCTION_CHECK_PCT)), Level::Good);
    assert_eq!(level_of(at(EXTINCTION_CHECK_PCT + 0.01)), Level::Check);
    assert_eq!(level_of(at(EXTINCTION_PROBLEM_PCT)), Level::Check);
    assert_eq!(level_of(at(EXTINCTION_PROBLEM_PCT + 0.01)), Level::Problem);

    // Brilliance: too LITTLE light is the problem.
    let at = |brilliance: f32| MetricColumn {
        brilliance_pct: brilliance,
        ..base
    };
    assert_eq!(level_of(at(BRILLIANCE_CHECK_BELOW_PCT)), Level::Good);
    assert_eq!(
        level_of(at(BRILLIANCE_CHECK_BELOW_PCT - 0.01)),
        Level::Check
    );
    assert_eq!(level_of(at(BRILLIANCE_PROBLEM_BELOW_PCT)), Level::Check);
    assert_eq!(
        level_of(at(BRILLIANCE_PROBLEM_BELOW_PCT - 0.01)),
        Level::Problem
    );
}

#[test]
fn a_measured_problem_names_the_figure() {
    let verdict = evaluate(&closed().with_optics(Some(column(45.0, 70.0, 12.0))));
    assert_eq!(verdict.level, Level::Problem);
    assert_eq!(verdict.reasons[0].kind, ReasonKind::OpticalWindowing);
    assert!(
        verdict.reasons[0].text.contains("45 %"),
        "{}",
        verdict.reasons[0].text
    );
    assert_eq!(verdict.headline, "1 problem to fix.");

    // A problem plus a check says both.
    let mut inputs = closed().with_optics(Some(column(45.0, 70.0, 12.0)));
    inputs.warnings = vec![fractional(0, "P1", 3.5)];
    assert_eq!(
        evaluate(&inputs).headline,
        "1 problem to fix, 1 more thing to check."
    );
}

#[test]
fn the_windowing_list_is_capped_worst_first_with_a_summary_line() {
    let mut inputs = closed();
    inputs.windowing = (0..MAX_LISTED_WINDOWING_TIERS + 2)
        .map(|i| windowing_entry(i, -(i as f64) - 1.0, false, true))
        .collect();
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.reasons.len(), MAX_LISTED_WINDOWING_TIERS + 1);
    // The worst margin (-6 degrees, tier 5) is the first line, then -5, ...
    assert_eq!(
        verdict.reasons[0].tier,
        Some(MAX_LISTED_WINDOWING_TIERS + 1)
    );
    assert_eq!(
        verdict.reasons[0].fix,
        Some(FixAction::SteepenPavilion {
            tier: MAX_LISTED_WINDOWING_TIERS + 1
        })
    );
    let summary = verdict.reasons.last().expect("a summary line");
    assert_eq!(summary.tier, None);
    assert_eq!(summary.fix, None);
    assert!(summary.text.starts_with("2 more tiers"), "{}", summary.text);
}

#[test]
fn a_crown_estimate_has_no_fix_and_follows_the_pavilion_tiers() {
    let mut inputs = closed();
    inputs.windowing = vec![
        windowing_entry(1, -9.0, true, false),
        windowing_entry(5, -0.5, false, true),
    ];
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.reasons[0].tier, Some(5));
    assert!(verdict.reasons[0].fix.is_some());
    assert_eq!(verdict.reasons[1].tier, Some(1));
    assert_eq!(verdict.reasons[1].fix, None);
    assert!(verdict.reasons[1].text.contains("estimated"));
    // The text also says plainly what the estimate does and does not look at.
    assert!(verdict.reasons[1].text.contains("same side"));
    assert!(verdict.reasons[1].text.contains("not the whole stone"));
    // A plain pavilion reason makes no such claim.
    assert!(!verdict.reasons[0].text.contains("same side"));
}

#[test]
fn a_tier_with_an_old_style_name_is_called_by_its_standard_code() {
    let mut entry = windowing_entry(2, -2.5, false, true);
    entry.name = "3".to_string();
    let mut inputs = closed();
    inputs.windowing = vec![entry];
    inputs.warnings = vec![fractional(1, "2", 24.5)];
    // The names a person reads for tiers 0, 1 and 2.
    inputs.tier_names = vec!["G1".to_string(), "P1".to_string(), "P2".to_string()];
    let verdict = evaluate(&inputs);
    let texts: Vec<&str> = verdict
        .reasons
        .iter()
        .map(|reason| reason.text.as_str())
        .collect();
    assert!(
        texts.iter().any(|text| text.starts_with(
            "P2: at 38.0\u{b0} it is below the critical angle (40.5\u{b0} for this material)"
        )),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("P1: an index position sits between gear teeth")),
        "{texts:?}"
    );
    // Without the display names the reasons fall back to the names the findings carry.
    inputs.tier_names.clear();
    let fallback = evaluate(&inputs);
    assert!(
        fallback
            .reasons
            .iter()
            .any(|reason| reason.text.starts_with("3: at 38.0\u{b0}")),
        "{:?}",
        fallback.reasons
    );
}

#[test]
fn a_crown_without_a_table_offers_to_add_one() {
    let mut inputs = closed();
    inputs.missing_table = true;
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Check);
    assert_eq!(verdict.reasons[0].kind, ReasonKind::MissingTable);
    assert_eq!(verdict.reasons[0].fix, Some(FixAction::AddTable));
}

#[test]
fn a_meet_with_a_later_tier_offers_to_move_the_tier_behind_the_latest_target() {
    let mut inputs = closed();
    inputs.warnings = vec![out_of_order(0, 1), out_of_order(0, 3), out_of_order(2, 2)];
    let verdict = evaluate(&inputs);
    let moves: Vec<_> = verdict
        .reasons
        .iter()
        .filter(|r| r.kind == ReasonKind::OutOfOrder)
        .collect();
    assert_eq!(moves.len(), 2, "one reason per tier");
    assert_eq!(
        moves[0].fix,
        Some(FixAction::MoveAfter { tier: 0, after: 3 })
    );
    assert!(moves[0].text.contains("2 tiers"), "{}", moves[0].text);
    // A tier naming itself cannot be moved behind itself.
    assert_eq!(moves[1].tier, Some(2));
    assert_eq!(moves[1].fix, None);
}

#[test]
fn off_gear_indices_group_per_tier_and_offer_snapping() {
    let mut inputs = closed();
    inputs.warnings = vec![
        fractional(1, "P1", 24.5),
        fractional(1, "P1", 48.4),
        fractional(2, "P2", 12.2),
    ];
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.reasons.len(), 2);
    assert_eq!(
        verdict.reasons[0].fix,
        Some(FixAction::SnapToTeeth { tier: 1 })
    );
    assert!(
        verdict.reasons[0].text.contains("2 index positions"),
        "{}",
        verdict.reasons[0].text
    );
    assert!(
        verdict.reasons[1].text.contains("nearest tooth is 12"),
        "{}",
        verdict.reasons[1].text
    );
}

#[test]
fn a_fully_cut_away_tier_offers_removal_after_a_question() {
    let mut inputs = closed();
    inputs.warnings = vec![vanishing(3, "Deep", 4, 4)];
    let verdict = evaluate(&inputs);
    let reason = &verdict.reasons[0];
    assert_eq!(reason.kind, ReasonKind::Vanished);
    assert_eq!(reason.fix, Some(FixAction::RemoveVanished { tier: 3 }));
    let question = reason.confirm.as_deref().expect("a removal asks first");
    assert!(question.starts_with("Remove Deep"), "{question}");
}

#[test]
fn a_partly_cut_away_tier_offers_removal_only_when_the_facets_can_be_told_apart() {
    let mut inputs = closed();
    inputs.warnings = vec![vanishing(3, "Deep", 1, 4)];
    assert_eq!(evaluate(&inputs).reasons[0].fix, None);
    inputs.partial_vanish_fixable = BTreeSet::from([3]);
    let verdict = evaluate(&inputs);
    assert_eq!(
        verdict.reasons[0].fix,
        Some(FixAction::RemoveVanished { tier: 3 })
    );
    let question = verdict.reasons[0].confirm.as_deref().expect("asks first");
    assert!(question.contains("other 3 stay"), "{question}");
}

#[test]
fn only_proportions_outside_the_window_count() {
    let fact = |label: &'static str, level: i32| ProportionFact {
        label,
        level,
        reason: "Typical range 53 to 62 %".to_string(),
    };
    let mut inputs = closed();
    inputs.proportions = vec![
        fact("The table width", 0),
        fact("The crown angle", 1),
        fact("The pavilion angle", -1),
        fact("The total depth", 2),
    ];
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.reasons.len(), 1);
    assert_eq!(verdict.reasons[0].kind, ReasonKind::Proportion);
    assert!(
        verdict.reasons[0]
            .text
            .starts_with("The total depth is outside the usual range")
    );
    assert!(
        verdict.reasons[0].text.ends_with("62 %."),
        "{}",
        verdict.reasons[0].text
    );
}

#[test]
fn an_unnamed_tier_reads_as_its_number() {
    let mut inputs = closed();
    inputs.warnings = vec![fractional(4, "  ", 2.5)];
    assert!(evaluate(&inputs).reasons[0].text.starts_with("Tier 5:"));
}

// ---------------------------------------------------------------------------------------
// gather(): what the verdict reads off a real design
// ---------------------------------------------------------------------------------------

#[test]
fn the_standard_brilliant_gathers_as_a_closed_stone_with_no_windowing() {
    let design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let inputs = gather(&design, Some(&solved), 1.54, None);
    assert_eq!(inputs.solve, SolveFacts::Closed);
    assert!(!inputs.missing_table);
    assert!(inputs.windowing.is_empty(), "{:?}", inputs.windowing);
    assert_eq!(inputs.proportions.len(), 5);
    // Whatever small findings the template has, it is not a Problem.
    assert_ne!(evaluate(&inputs).level, Level::Problem);
}

#[test]
fn no_solve_reports_the_callers_sentence_and_still_runs_the_mast_free_checks() {
    let design = design_with(vec![tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.5, 48.0, 72.0],
    )]);
    let inputs = gather(&design, None, 1.62, Some("P2 needs a meet."));
    assert_eq!(
        inputs.solve,
        SolveFacts::DoesNotSolve("P2 needs a meet.".to_string())
    );
    assert!(
        inputs
            .warnings
            .iter()
            .any(|w| matches!(w, ManufacturabilityWarning::FractionalIndex { .. })),
        "{:?}",
        inputs.warnings
    );
    let verdict = evaluate(&inputs);
    assert_eq!(verdict.level, Level::Problem);
    assert!(
        verdict
            .reasons
            .iter()
            .any(|r| r.fix == Some(FixAction::SnapToTeeth { tier: 0 }))
    );
}

#[test]
fn a_mast_list_that_does_not_belong_to_the_design_counts_as_no_solve() {
    let design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let inputs = gather(&design, Some(&solved[..solved.len() - 1]), 1.54, None);
    assert!(
        matches!(&inputs.solve, SolveFacts::DoesNotSolve(why) if why.contains("changed")),
        "{:?}",
        inputs.solve
    );
}

#[test]
fn a_pavilion_below_the_critical_angle_is_listed_and_fixable() {
    // Index 1.50: critical angle 41.8 degrees, so the 41-degree main windows and the
    // 42.5-degree lower girdle does not.
    let design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let inputs = gather(&design, Some(&solved), 1.5, None);
    let pavilion: Vec<_> = inputs
        .windowing
        .iter()
        .filter(|w| !w.crown_estimate)
        .collect();
    assert_eq!(pavilion.len(), 1, "{:?}", inputs.windowing);
    assert_eq!(pavilion[0].name, "Pavilion Main");
    assert!(pavilion[0].fixable);
    assert!(pavilion[0].margin_deg < 0.0);
    let verdict = evaluate(&inputs);
    assert!(
        verdict
            .reasons
            .iter()
            .any(|r| r.kind == ReasonKind::Windowing
                && r.fix
                    == Some(FixAction::SteepenPavilion {
                        tier: pavilion[0].tier
                    }))
    );
}

#[test]
fn a_crown_without_a_table_is_found() {
    let with_table = brilliant();
    let without = brilliant_keeping(&[
        "Star",
        "Crown Main",
        "Upper Girdle",
        "Girdle",
        "Pavilion Main",
        "Lower Girdle",
        "Culet",
    ]);
    let solved = without.solve().expect("every tier is pinned");
    let inputs = gather(&without, Some(&solved), 1.54, None);
    assert!(inputs.missing_table);
    assert_eq!(inputs.solve, SolveFacts::Closed);
    let solved = with_table.solve().expect("every tier is pinned");
    assert!(!gather(&with_table, Some(&solved), 1.54, None).missing_table);
}

#[test]
fn a_fully_cut_away_tier_is_found_with_its_removal_offered() {
    let design = deep_and_table();
    let solved = design.solve().expect("must solve");
    let inputs = gather(&design, Some(&solved), 1.62, None);
    let verdict = evaluate(&inputs);
    let reason = verdict
        .reasons
        .iter()
        .find(|r| r.kind == ReasonKind::Vanished)
        .expect("the Deep tier vanishes");
    assert_eq!(reason.fix, Some(FixAction::RemoveVanished { tier: 0 }));
    assert!(
        reason
            .confirm
            .as_deref()
            .is_some_and(|q| q.contains("Deep"))
    );
}

// ---------------------------------------------------------------------------------------
// Fixes
// ---------------------------------------------------------------------------------------

/// A crown facet "Deep" that a later, shallower table cuts away completely.
fn deep_and_table() -> Design {
    design_with(vec![
        tier(
            "Deep",
            30.0,
            MeetConstraint::ScaleReference(0.95),
            &[0.0, 24.0, 48.0, 72.0],
        ),
        tier("T", 0.0, MeetConstraint::ScaleReference(0.2), &[]),
    ])
}

#[test]
fn snapping_moves_each_off_gear_position_to_its_tooth_as_one_undo_step() {
    let design = design_with(vec![tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.4, 47.6, 72.0],
    )]);
    let original = design.clone();
    let plan = plan_fix(&design, &FixAction::SnapToTeeth { tier: 0 }, 1.62).expect("plans");
    assert_eq!(
        plan.edit,
        Edit::SetIndices {
            index: 0,
            indices: vec![0.0, 24.0, 48.0, 72.0],
            detached: Vec::new(),
        }
    );
    assert!(
        plan.message.contains("Snapped 2 index positions of P1"),
        "{}",
        plan.message
    );

    let mut session = session_of(design);
    apply(&mut session, plan.edit);
    assert_eq!(session.design.tiers[0].indices, vec![0.0, 24.0, 48.0, 72.0]);
    assert_eq!(session.history.len_undo(), 1, "one undo step");
    session.undo().expect("undoes");
    assert_eq!(session.design.tiers, original.tiers);
    assert!(!session.history.can_undo());
}

#[test]
fn snapping_leaves_the_order_and_merges_positions_that_land_on_one_tooth() {
    let design = design_with(vec![tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[48.0, 24.2, 24.4, 0.0],
    )]);
    let plan = plan_fix(&design, &FixAction::SnapToTeeth { tier: 0 }, 1.62).expect("plans");
    let Edit::SetIndices { indices, .. } = plan.edit else {
        panic!("a snap is one SetIndices");
    };
    assert_eq!(indices, vec![48.0, 24.0, 0.0]);
}

#[test]
fn snapping_refuses_when_every_position_is_on_a_tooth() {
    let design = design_with(vec![tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0],
    )]);
    let refusal =
        plan_fix(&design, &FixAction::SnapToTeeth { tier: 0 }, 1.62).expect_err("nothing to snap");
    assert!(refusal.contains("already on a gear tooth"), "{refusal}");
    // A tier that is gone is a stale verdict, not a panic.
    assert!(plan_fix(&design, &FixAction::SnapToTeeth { tier: 9 }, 1.62).is_err());
}

#[test]
fn removing_a_fully_cut_away_tier_is_one_undo_step_and_leaves_the_stone_alone() {
    let design = deep_and_table();
    let before = analyze(&design, false).expect("closes");
    let plan = plan_fix(&design, &FixAction::RemoveVanished { tier: 0 }, 1.62).expect("plans");
    assert_eq!(plan.edit, Edit::RemoveTier { index: 0 });
    assert!(plan.message.starts_with("Removed Deep"), "{}", plan.message);

    let mut session = session_of(design);
    apply(&mut session, plan.edit);
    assert_eq!(session.design.tiers.len(), 1);
    assert_eq!(session.design.tiers[0].name, "T");
    assert_eq!(session.history.len_undo(), 1, "one undo step");
    // The stone is the same.
    let after = analyze(&session.design, false).expect("still closes");
    match (before.table_percent, after.table_percent) {
        (Some(was), Some(now)) => assert!((was - now).abs() < 1e-6, "table {was} % became {now} %"),
        (None, None) => {}
        other => panic!("the table figure changed: {other:?}"),
    }
    session.undo().expect("undoes");
    assert_eq!(session.design.tiers.len(), 2);
    assert_eq!(session.design.tiers[0].name, "Deep");
}

#[test]
fn removing_a_tier_other_tiers_meet_clears_their_reference_in_the_same_step() {
    let mut design = deep_and_table();
    design.tiers.push(tier(
        "Z",
        20.0,
        MeetConstraint::MeetNamed(vec!["Deep".to_string()]),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    design.ensure_tier_ids();
    let edit = remove_tier_edit(&design, 0);
    let Edit::Batch(steps) = &edit else {
        panic!("a tier with dependants is removed in a batch: {edit:?}");
    };
    assert_eq!(steps.last(), Some(&Edit::RemoveTier { index: 0 }));
    let mut session = session_of(design);
    apply(&mut session, edit);
    assert_eq!(session.history.len_undo(), 1, "one undo step");
    assert_eq!(session.design.tiers.len(), 2);
    let z = session
        .design
        .tiers
        .iter()
        .find(|tier| tier.name == "Z")
        .expect("Z stays");
    assert_ne!(
        z.constraint,
        MeetConstraint::MeetNamed(vec!["Deep".to_string()])
    );
    session.undo().expect("undoes");
    assert_eq!(session.design.tiers.len(), 3);
}

#[test]
fn removing_a_tier_nothing_meets_is_a_plain_removal() {
    assert_eq!(
        remove_tier_edit(&deep_and_table(), 0),
        Edit::RemoveTier { index: 0 }
    );
}

#[test]
fn removing_does_nothing_for_a_tier_that_is_not_cut_away() {
    let design = deep_and_table();
    let refusal = plan_fix(&design, &FixAction::RemoveVanished { tier: 1 }, 1.62)
        .expect_err("the table is fine");
    assert!(refusal.contains("None of the facets"), "{refusal}");
}

#[test]
fn partly_cut_away_entries_are_found_from_the_plane_ranges() {
    let design = design_with(vec![tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0, 48.0, 72.0],
    )]);
    // Spelled out: `[5..9]` reads as an array of nine numbers to clippy.
    let ranges = [Range { start: 5, end: 9 }];
    // Plane 6 (the second entry) is not on the stone.
    let live = BTreeSet::from([5, 7, 8]);
    assert_eq!(
        vanished_positions_in(&design, &ranges, &live, 0),
        Some(vec![1])
    );
    // Everything live, or everything gone: not a partial removal.
    assert_eq!(
        vanished_positions_in(&design, &ranges, &BTreeSet::from([5, 6, 7, 8]), 0),
        None
    );
    assert_eq!(
        vanished_positions_in(&design, &ranges, &BTreeSet::new(), 0),
        None
    );
    // A range that does not match the list (a duplicated plane) cannot be mapped.
    let short = [Range { start: 5, end: 8 }];
    assert_eq!(vanished_positions_in(&design, &short, &live, 0), None);
}

#[test]
fn a_partial_removal_edit_keeps_the_other_positions_and_their_marks() {
    let mut design = design_with(vec![tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0, 48.0, 72.0],
    )]);
    design.tiers[0].detached = vec![24.0, 72.0];
    let edit = partial_removal_edit(&design, 0, &[1]);
    assert_eq!(
        edit,
        Edit::SetIndices {
            index: 0,
            indices: vec![0.0, 48.0, 72.0],
            detached: vec![72.0],
        }
    );
    let mut session = session_of(design);
    apply(&mut session, edit);
    assert_eq!(session.design.tiers[0].indices, vec![0.0, 48.0, 72.0]);
    assert_eq!(session.history.len_undo(), 1);
}

/// "A" meets "B", which is cut after it.
fn out_of_order_pair() -> Design {
    design_with(vec![
        tier(
            "A",
            30.0,
            MeetConstraint::MeetNamed(vec!["B".to_string()]),
            &[0.0, 24.0, 48.0, 72.0],
        ),
        tier(
            "B",
            45.0,
            MeetConstraint::ScaleReference(0.6),
            &[0.0, 24.0, 48.0, 72.0],
        ),
    ])
}

#[test]
fn moving_a_tier_behind_its_target_is_one_undo_step_and_clears_the_warning() {
    let design = out_of_order_pair();
    assert_eq!(check_cut_order(&design).len(), 1);
    let plan = plan_fix(&design, &FixAction::MoveAfter { tier: 0, after: 1 }, 1.62).expect("plans");
    assert_eq!(plan.edit, Edit::MoveTier { from: 0, to: 1 });
    assert_eq!(plan.select, Some(1));
    // The tiers carry old-style names (`A`, `B`), so the message calls them by the standard
    // codes the tier table shows.
    let codes = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    let expected = format!("Moved {} to just after {}", codes[0].code, codes[1].code);
    assert!(plan.message.contains(&expected), "{}", plan.message);

    let mut session = session_of(design);
    apply(&mut session, plan.edit);
    assert_eq!(session.design.tiers[0].name, "B");
    assert_eq!(session.design.tiers[1].name, "A");
    let out_of_order = check_cut_order(&session.design);
    assert!(out_of_order.is_empty(), "{out_of_order:?}");
    assert_eq!(session.history.len_undo(), 1, "one undo step");
    session.undo().expect("undoes");
    assert_eq!(session.design.tiers[0].name, "A");
}

#[test]
fn moving_refuses_a_move_that_does_not_help_or_a_stale_position() {
    let design = out_of_order_pair();
    // Moving the other way round is not a move behind the target.
    assert!(plan_fix(&design, &FixAction::MoveAfter { tier: 1, after: 0 }, 1.62).is_err());
    assert!(plan_fix(&design, &FixAction::MoveAfter { tier: 0, after: 7 }, 1.62).is_err());
}

#[test]
fn steepening_a_pavilion_keeps_the_girdle_and_removes_the_windowing() {
    // A small, easy-to-reason-about stone: table, crown mains, girdle, pavilion mains, culet.
    let design = brilliant_keeping(&["Table", "Crown Main", "Girdle", "Pavilion Main", "Culet"]);
    let n_d = 1.5;
    let main = position_of(&design, "Pavilion Main");
    let solved = design.solve().expect("every tier is pinned");
    let inputs = gather(&design, Some(&solved), n_d, None);
    assert!(
        inputs.windowing.iter().any(|w| w.tier == main && w.fixable),
        "{:?}",
        inputs.windowing
    );

    let plan = plan_fix(&design, &FixAction::SteepenPavilion { tier: main }, n_d)
        .expect("a steeper main is valid here");
    let target = steep_target_deg(n_d);
    assert!(
        (target - 43.82).abs() < 1e-9,
        "critical 41.81 + 2, rounded up: {target}"
    );
    let Edit::Batch(steps) = &plan.edit else {
        panic!(
            "the angle and its re-anchored mast travel together: {:?}",
            plan.edit
        );
    };
    assert_eq!(
        steps[0],
        Edit::RetargetAngles {
            changes: vec![(main, -41.0, -target)]
        }
    );
    assert!(
        steps[1..]
            .iter()
            .all(|step| matches!(step, Edit::SetConstraint { .. })),
        "{steps:?}"
    );

    let mut session = session_of(design.clone());
    apply(&mut session, plan.edit);
    assert_eq!(session.history.len_undo(), 1, "one undo step");
    assert!((session.design.tiers[main].angle_deg + target).abs() < 1e-9);

    // The girdle edge the facet turns about has not moved, and the girdle is as thick.
    let after_solved = session.design.solve().expect("still solves");
    let before_pivot = tier_hinge_points(&design, &solved)[&main];
    let after_pivot = tier_hinge_points(&session.design, &after_solved)[&main];
    assert!(
        (before_pivot - after_pivot).length() < 1e-6,
        "pivot moved from {before_pivot:?} to {after_pivot:?}"
    );
    let girdle_before = analyze(&design, false)
        .expect("closes")
        .girdle_percent
        .expect("a girdle");
    let girdle_after = analyze(&session.design, false)
        .expect("closes")
        .girdle_percent
        .expect("a girdle");
    assert!(
        (girdle_before - girdle_after).abs() < 1e-4,
        "girdle {girdle_before} % became {girdle_after} %"
    );

    // The verdict, recomputed from the new solve, no longer lists the facet.
    let after = gather(&session.design, Some(&after_solved), n_d, None);
    assert!(after.windowing.is_empty(), "{:?}", after.windowing);

    session.undo().expect("undoes");
    assert!((session.design.tiers[main].angle_deg + 41.0).abs() < 1e-12);
}

#[test]
fn steepening_refuses_when_the_safe_angle_cannot_be_reached() {
    let design = brilliant_keeping(&["Table", "Crown Main", "Girdle", "Pavilion Main", "Culet"]);
    let main = position_of(&design, "Pavilion Main");
    // At an index of 1.0005 the critical angle is above 88 degrees: critical + 2 is steeper
    // than the editor allows.
    let refusal = plan_fix(&design, &FixAction::SteepenPavilion { tier: main }, 1.0005)
        .expect_err("not reachable");
    assert!(refusal.contains("steeper than"), "{refusal}");
}

#[test]
fn steepening_refuses_a_flat_facet_a_safe_facet_and_a_stale_tier() {
    let design = brilliant();
    let table = position_of(&design, "Table");
    let lower_girdle = position_of(&design, "Lower Girdle");
    let flat = plan_fix(&design, &FixAction::SteepenPavilion { tier: table }, 1.5)
        .expect_err("a table has no angle");
    assert!(flat.contains("flat facet"), "{flat}");
    // 42.5 degrees is already past critical + 2 at index 1.54 (40.5 + 2).
    let safe = plan_fix(
        &design,
        &FixAction::SteepenPavilion { tier: lower_girdle },
        1.54,
    )
    .expect_err("already safe");
    assert!(safe.contains("already at"), "{safe}");
    assert!(plan_fix(&design, &FixAction::SteepenPavilion { tier: 99 }, 1.54).is_err());
    assert!(
        plan_fix(
            &design,
            &FixAction::SteepenPavilion { tier: lower_girdle },
            f64::NAN
        )
        .is_err()
    );
}

#[test]
fn adding_a_table_uses_the_quick_add_height_and_is_one_undo_step() {
    let design = brilliant_keeping(&[
        "Star",
        "Crown Main",
        "Upper Girdle",
        "Girdle",
        "Pavilion Main",
        "Lower Girdle",
        "Culet",
    ]);
    let count = design.tiers.len();
    let solved = design.solve().expect("every tier is pinned");
    assert!(gather(&design, Some(&solved), 1.54, None).missing_table);

    let plan = plan_fix(&design, &FixAction::AddTable, 1.54).expect("plans");
    let Edit::AddTier { index, tier } = &plan.edit else {
        panic!("a table is one AddTier: {:?}", plan.edit);
    };
    assert_eq!(*index, count);
    assert_eq!(tier.name, "Table");
    assert!(tier.angle_deg.abs() < f64::EPSILON && tier.indices.is_empty());
    assert_eq!(
        tier.constraint,
        MeetConstraint::ScaleReference(QUICK_ADD_TABLE_MAST)
    );
    assert_eq!(plan.select, Some(count));

    let mut session = session_of(design);
    apply(&mut session, plan.edit);
    assert_eq!(session.design.tiers.len(), count + 1);
    assert_eq!(session.history.len_undo(), 1, "one undo step");
    // The verdict, recomputed from the new solve, no longer misses a table.
    let solved = session.design.solve().expect("still solves");
    let after = gather(&session.design, Some(&solved), 1.54, None);
    assert!(!after.missing_table);
    assert_eq!(after.solve, SolveFacts::Closed);
    session.undo().expect("undoes");
    assert_eq!(session.design.tiers.len(), count);
}

#[test]
fn a_design_that_has_a_table_gets_none_added() {
    let refusal = plan_fix(&brilliant(), &FixAction::AddTable, 1.54).expect_err("has a table");
    assert!(refusal.contains("already has a table"), "{refusal}");
}

#[test]
fn a_taken_table_name_gets_a_number() {
    // A sloping facet that happens to be called "Table" must not be duplicated.
    let mut design = brilliant_keeping(&["Star", "Crown Main", "Girdle", "Pavilion Main", "Culet"]);
    assert_eq!(unique_table_name(&design), "Table");
    design.tiers[0].name = "Table".to_string();
    assert_eq!(unique_table_name(&design), "Table 2");
    design.tiers[1].name = "table 2".to_string();
    assert_eq!(unique_table_name(&design), "Table 3");
}
