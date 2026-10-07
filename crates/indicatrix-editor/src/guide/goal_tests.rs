//! Every [`Goal`] kind against designs built the way the tier form builds them, plus the
//! goal checks a guide relies on.

use super::{
    EIGHT_FOLD_INDICES, Goal, GoalContext, MeetKind, goal_met,
    tests::{design_with, tier},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design};

fn named(mut tier: ConstraintTier, targets: &[&str]) -> ConstraintTier {
    tier.constraint = MeetConstraint::MeetNamed(targets.iter().map(|t| (*t).to_owned()).collect());
    tier
}

fn unspecified(mut tier: ConstraintTier) -> ConstraintTier {
    tier.constraint = MeetConstraint::MeetExisting;
    tier
}

/// The worked example's four tiers with the angles and indices of its steps, and a variety of
/// meets so that the goal tests can tell the three kinds apart: the girdle states a scale
/// value, the pavilion and the crown name a facet, the table is unspecified. (The values the
/// walkthrough really tells the learner to type, and whether they solve, are pinned in
/// `tests.rs`; this design is for the goal predicates, not for solving.)
fn brilliant() -> Design {
    design_with(&[
        tier("G1", 90.0, &EIGHT_FOLD_INDICES),
        named(tier("P1", -40.0, &EIGHT_FOLD_INDICES), &["G1"]),
        named(tier("C1", 34.5, &EIGHT_FOLD_INDICES), &["G1"]),
        unspecified(tier("T", 0.0, &[])),
    ])
}

fn met(goal: &Goal, design: &Design) -> bool {
    goal_met(goal, &GoalContext::new(design))
}

#[test]
fn a_reading_step_is_never_met_from_state() {
    let design = brilliant();
    let ctx = GoalContext::new(&design)
        .solved_closed(true)
        .view_mode(0)
        .inspector_tab(0);
    assert!(!goal_met(&Goal::Manual, &ctx));
    assert!(Goal::Manual.is_manual());
    assert!(!Goal::SolvedClosed.is_manual());
}

#[test]
fn an_event_goal_needs_that_event_since_the_step_began() {
    let design = brilliant();
    let goal = Goal::Event("palette_opened".to_owned());
    assert!(!goal_met(&goal, &GoalContext::new(&design)));
    let other = ["tutorials_opened".to_owned()];
    assert!(!goal_met(&goal, &GoalContext::new(&design).events(&other)));
    let seen = ["tutorials_opened".to_owned(), "palette_opened".to_owned()];
    assert!(goal_met(&goal, &GoalContext::new(&design).events(&seen)));
}

#[test]
fn a_tier_goal_needs_the_right_name_angle_and_indices() {
    let goal = Goal::tier("G1", 90.0).with_indices(&EIGHT_FOLD_INDICES);
    assert!(met(&goal, &brilliant()));
    // Name: case and spaces do not matter, the name must.
    assert!(met(
        &goal,
        &design_with(&[tier(" g1 ", 90.0, &EIGHT_FOLD_INDICES)])
    ));
    assert!(!met(
        &goal,
        &design_with(&[tier("Girdle", 90.0, &EIGHT_FOLD_INDICES)])
    ));
    // Angle: the step's 0.05 degree allowance, no more.
    assert!(met(
        &goal,
        &design_with(&[tier("G1", 90.04, &EIGHT_FOLD_INDICES)])
    ));
    assert!(!met(
        &goal,
        &design_with(&[tier("G1", 89.9, &EIGHT_FOLD_INDICES)])
    ));
    // Indices: one short, or one too many, is a different tier.
    assert!(!met(
        &goal,
        &design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES[..7])])
    ));
    let mut nine = EIGHT_FOLD_INDICES.to_vec();
    nine.push(6.0);
    assert!(!met(&goal, &design_with(&[tier("G1", 90.0, &nine)])));
}

#[test]
fn a_tier_goal_without_indices_accepts_any_and_a_blank_list_means_blank() {
    let anything = Goal::tier("T", 0.0);
    assert!(met(&anything, &design_with(&[tier("T", 0.0, &[])])));
    assert!(met(&anything, &design_with(&[tier("T", 0.0, &[12.0])])));

    let blank = Goal::tier("T", 0.0).with_indices(&[]);
    assert!(met(&blank, &design_with(&[tier("T", 0.0, &[])])));
    assert!(met(&blank, &design_with(&[tier("T", 0.0, &[0.0])])));
    assert!(!met(&blank, &design_with(&[tier("T", 0.0, &[12.0])])));
}

#[test]
fn a_tier_goal_can_demand_the_kind_of_meets_setting() {
    let design = brilliant();
    let on_p1 = |kind| Goal::tier("P1", -40.0).with_meet(kind);
    assert!(met(&on_p1(MeetKind::Named), &design));
    assert!(!met(&on_p1(MeetKind::ExactScale), &design));
    assert!(!met(&on_p1(MeetKind::Unspecified), &design));
    assert!(met(
        &Goal::tier("G1", 90.0).with_meet(MeetKind::ExactScale),
        &design
    ));
    assert!(met(
        &Goal::tier("T", 0.0).with_meet(MeetKind::Unspecified),
        &design
    ));
}

#[test]
fn the_builders_leave_other_goals_alone() {
    assert!(matches!(
        Goal::SolvedClosed
            .with_indices(&[1.0])
            .with_meet(MeetKind::Named),
        Goal::SolvedClosed
    ));
}

#[test]
fn the_tier_form_words_name_the_three_meet_kinds() {
    assert_eq!(MeetKind::Unspecified.label(), "Unspecified vertex");
    assert_eq!(MeetKind::Named.label(), "Named facet(s)");
    assert_eq!(MeetKind::ExactScale.label(), "Exact scale value");
    assert_eq!(
        MeetKind::of(&MeetConstraint::MeetExisting),
        MeetKind::Unspecified
    );
    assert_eq!(
        MeetKind::of(&MeetConstraint::MeetNamed(Vec::new())),
        MeetKind::Named
    );
    assert_eq!(
        MeetKind::of(&MeetConstraint::ScaleReference(1.0)),
        MeetKind::ExactScale
    );
}

#[test]
fn a_tier_exists_by_any_of_its_names() {
    let design = design_with(&[tier("P1/P2", -40.0, &[0.0])]);
    assert!(met(&Goal::TierExists("p2".to_owned()), &design));
    assert!(met(&Goal::TierExists("P1".to_owned()), &design));
    assert!(!met(&Goal::TierExists("P3".to_owned()), &design));
    assert!(!met(&Goal::TierExists("P1/P2".to_owned()), &design));
}

#[test]
fn a_tier_count_goal_counts_tiers() {
    let design = brilliant();
    assert!(met(&Goal::TierCountAtLeast(4), &design));
    assert!(!met(&Goal::TierCountAtLeast(5), &design));
    assert!(!met(&Goal::TierCountAtLeast(1), &design_with(&[])));
}

#[test]
fn a_material_goal_reads_the_design_unless_the_context_names_one() {
    let mut design = brilliant();
    let goal = Goal::Material("Diamond".to_owned());
    assert!(!met(&goal, &design));
    design.material.name = Some(" diamond ".to_owned());
    assert!(met(&goal, &design));
    // The context's material wins when the UI knows better.
    assert!(!goal_met(
        &goal,
        &GoalContext::new(&design).material("Quartz")
    ));
    design.material.name = Some("Quartz".to_owned());
    assert!(goal_met(
        &goal,
        &GoalContext::new(&design).material("Diamond")
    ));
}

#[test]
fn a_solved_goal_needs_the_verdict_and_at_least_one_tier() {
    let design = brilliant();
    assert!(!met(&Goal::SolvedClosed, &design));
    assert!(goal_met(
        &Goal::SolvedClosed,
        &GoalContext::new(&design).solved_closed(true)
    ));
    // A zero-tier design "solves" to its bare preform, which is not the stone.
    assert!(!goal_met(
        &Goal::SolvedClosed,
        &GoalContext::new(&design_with(&[])).solved_closed(true)
    ));
}

#[test]
fn a_yield_goal_needs_a_girdle_diameter() {
    let mut design = brilliant();
    assert!(!met(&Goal::YieldApplied, &design));
    design.girdle_diameter_mm = Some(6.5);
    assert!(met(&Goal::YieldApplied, &design));
}

#[test]
fn view_mode_and_inspector_tab_goals_need_the_ui_to_say() {
    let design = brilliant();
    assert!(!met(&Goal::ViewMode(0), &design), "unknown is not a match");
    assert!(!met(&Goal::InspectorTab(0), &design));
    let ctx = GoalContext::new(&design).view_mode(3).inspector_tab(1);
    assert!(goal_met(&Goal::ViewMode(3), &ctx));
    assert!(!goal_met(&Goal::ViewMode(2), &ctx));
    assert!(goal_met(&Goal::InspectorTab(1), &ctx));
    assert!(!goal_met(&Goal::InspectorTab(0), &ctx));
}

#[test]
fn all_and_any_combine_goals() {
    let design = brilliant();
    let yes = Goal::TierCountAtLeast(1);
    let no = Goal::TierCountAtLeast(99);
    assert!(met(&Goal::All(vec![yes.clone(), yes.clone()]), &design));
    assert!(!met(&Goal::All(vec![yes.clone(), no.clone()]), &design));
    assert!(met(&Goal::Any(vec![no.clone(), yes]), &design));
    assert!(!met(&Goal::Any(vec![no.clone(), no]), &design));
    // The empty cases: All is vacuously met, Any never is.
    assert!(met(&Goal::All(Vec::new()), &design));
    assert!(!met(&Goal::Any(Vec::new()), &design));
}

#[test]
fn only_view_and_tab_goals_make_a_ui_poll() {
    assert!(Goal::ViewMode(1).watches_ui());
    assert!(Goal::InspectorTab(2).watches_ui());
    assert!(Goal::All(vec![Goal::SolvedClosed, Goal::ViewMode(1)]).watches_ui());
    assert!(Goal::Any(vec![Goal::All(vec![Goal::InspectorTab(0)])]).watches_ui());
    assert!(!Goal::SolvedClosed.watches_ui());
    assert!(!Goal::Event("palette_opened".into()).watches_ui());
    assert!(!Goal::Manual.watches_ui());
}

#[test]
fn a_goal_lists_the_events_it_waits_for() {
    let goal = Goal::All(vec![
        Goal::Event("palette_opened".into()),
        Goal::Any(vec![
            Goal::Event("tutorials_opened".into()),
            Goal::SolvedClosed,
        ]),
    ]);
    assert_eq!(goal.events(), ["palette_opened", "tutorials_opened"]);
    assert!(
        Goal::Manual.events().is_empty(),
        "a reading step waits for no event"
    );
}

#[test]
fn a_malformed_goal_names_its_problem() {
    let good = [
        Goal::Manual,
        Goal::SolvedClosed,
        Goal::YieldApplied,
        Goal::Event("palette_opened".into()),
        Goal::tier("G1", 90.0),
        Goal::TierExists("G1".into()),
        Goal::TierCountAtLeast(1),
        Goal::Material("Diamond".into()),
        Goal::ViewMode(3),
        Goal::InspectorTab(0),
        Goal::All(vec![Goal::SolvedClosed, Goal::ViewMode(0)]),
    ];
    for goal in &good {
        assert_eq!(goal.problem(), None, "{goal:?}");
    }
    let bad = [
        Goal::Event("typo_event".into()),
        Goal::tier(" ", 90.0),
        Goal::tier("G1", f64::NAN),
        Goal::TierExists(String::new()),
        Goal::Material("  ".into()),
        Goal::TierCountAtLeast(0),
        Goal::ViewMode(4),
        Goal::ViewMode(-1),
        Goal::InspectorTab(9),
        Goal::All(Vec::new()),
        Goal::Any(Vec::new()),
        Goal::All(vec![Goal::Manual]),
        Goal::Any(vec![Goal::SolvedClosed, Goal::Event("typo_event".into())]),
    ];
    for goal in &bad {
        assert!(goal.problem().is_some(), "{goal:?} should be refused");
    }
}
