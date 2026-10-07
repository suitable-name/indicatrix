//! Tests for [`super`].

use super::{
    TierRef::{Concave as C, Flat as F},
    *,
};
use crate::{
    design::{ConcaveTier, ConcaveTool, ScheduleMeta, ToolMotion},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn pinned(name: &str, angle_deg: f64) -> ConstraintTier {
    meeting(name, angle_deg, MeetConstraint::ScaleReference(0.5))
}

fn meets(name: &str, angle_deg: f64, targets: &[&str]) -> ConstraintTier {
    meeting(
        name,
        angle_deg,
        MeetConstraint::MeetNamed(targets.iter().map(|target| (*target).to_owned()).collect()),
    )
}

fn meeting(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_owned(),
        indices: vec![0.0],
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn design(tiers: Vec<ConstraintTier>) -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        tiers,
    )
}

fn concave(angle_deg: f64) -> ConcaveTier {
    ConcaveTier {
        name: String::new(),
        angle_deg,
        indices: vec![0.0, 4.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.0, 0.1],
        diameter_ratio: 0.5,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

/// The fixture is stored top-down (table, star, crown main, upper girdle, girdle, pavilion
/// main, lower girdle, culet). Cut in order: the pavilion section with its girdle in stored
/// order, then the crown section, then the table.
#[test]
fn the_standard_round_brilliant_is_cut_pavilion_first_and_table_last() {
    let tiers = ConstraintTier::standard_round_brilliant();
    assert_eq!(flat_cutting_order(&tiers), [4, 5, 6, 7, 1, 2, 3, 0]);
    assert_eq!(
        design(tiers).cutting_order(),
        [F(4), F(5), F(6), F(7), F(1), F(2), F(3), F(0)]
    );
}

#[test]
fn a_design_stored_in_cutting_order_keeps_it() {
    let tiers = vec![
        pinned("Pavilion Main", -41.0),
        pinned("Girdle", 90.0),
        pinned("Lower Girdle", -42.5),
        pinned("Culet", -0.0),
        pinned("Crown Main", 34.5),
        pinned("Star", 15.0),
        pinned("Upper Girdle", 41.0),
        pinned("Table", 0.0),
    ];
    assert_eq!(flat_cutting_order(&tiers), [0, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn a_facet_that_meets_a_later_facet_moves_to_just_after_it() {
    let tiers = vec![
        meets("Alpha", -41.0, &["Gamma"]),
        pinned("Beta", -42.0),
        pinned("Gamma", -43.0),
        pinned("Delta", -44.0),
    ];
    // Alpha waits for Gamma; Delta, stored after Gamma, stays behind Alpha.
    assert_eq!(flat_cutting_order(&tiers), [1, 2, 0, 3]);
}

#[test]
fn a_facet_that_meets_several_later_facets_follows_the_last_of_them() {
    let tiers = vec![
        meets("Alpha", -41.0, &["Beta", "Gamma"]),
        pinned("Beta", -42.0),
        pinned("Delta", -44.0),
        pinned("Gamma", -43.0),
    ];
    assert_eq!(flat_cutting_order(&tiers), [1, 2, 3, 0]);
}

#[test]
fn a_chain_of_forward_meets_is_cut_from_its_far_end() {
    let tiers = vec![
        meets("Alpha", -41.0, &["Beta"]),
        meets("Beta", -42.0, &["Gamma"]),
        pinned("Gamma", -43.0),
    ];
    assert_eq!(flat_cutting_order(&tiers), [2, 1, 0]);
}

#[test]
fn a_meet_with_an_earlier_facet_changes_nothing() {
    let tiers = vec![pinned("Beta", -42.0), meets("Alpha", -41.0, &["Beta"])];
    assert_eq!(flat_cutting_order(&tiers), [0, 1]);
}

/// Two facets that name each other cannot both come first; both keep their stored place, and
/// an unrelated facet stays where it is too.
#[test]
fn a_cycle_keeps_the_stored_order() {
    let tiers = vec![
        meets("Xray", -41.0, &["Yankee"]),
        meets("Yankee", -42.0, &["Xray"]),
        pinned("Zulu", -43.0),
    ];
    assert_eq!(flat_cutting_order(&tiers), [0, 1, 2]);
}

/// A meet that names a facet of another section is not an ordering constraint: the sections
/// already say which comes first.
#[test]
fn a_target_in_another_section_does_not_move_a_facet() {
    let tiers = vec![
        meets("Crown A", 35.0, &["Table"]),
        pinned("Crown B", 30.0),
        pinned("Table", 0.0),
        meets("Pavilion A", -41.0, &["Crown B"]),
    ];
    assert_eq!(flat_cutting_order(&tiers), [3, 0, 1, 2]);
}

/// `G` is not a tier name here; the solver's resolver (the one the sheet uses) reads it as
/// the girdle, so the lower girdle waits for the tier at 90 degrees.
#[test]
fn names_resolve_through_the_solvers_resolver() {
    let tiers = vec![meets("Lower", -42.0, &["G"]), pinned("Edge", 90.0)];
    assert_eq!(flat_cutting_order(&tiers), [1, 0]);
}

#[test]
fn concave_tiers_stay_at_the_end_of_their_section_with_the_dependency_pass() {
    let mut design = design(vec![
        meets("Alpha", -41.0, &["Gamma"]),
        pinned("Beta", -42.0),
        pinned("Gamma", -43.0),
        pinned("Crown", 35.0),
        pinned("Table", 0.0),
    ]);
    design.concave_tiers = vec![concave(-30.0), concave(36.0)];
    assert_eq!(
        design.cutting_order(),
        [F(1), F(2), F(0), C(0), F(3), C(1), F(4)]
    );
}

#[test]
fn the_order_is_the_same_every_time() {
    let tiers = vec![
        meets("Alpha", -41.0, &["Gamma"]),
        meets("Beta", -42.0, &["Gamma", "Delta"]),
        pinned("Gamma", -43.0),
        pinned("Delta", -44.0),
        meets("Crown A", 35.0, &["Crown B"]),
        pinned("Crown B", 30.0),
    ];
    let first = flat_cutting_order(&tiers);
    for _ in 0..8 {
        assert_eq!(flat_cutting_order(&tiers), first);
    }
    // Alpha waits only for Gamma, so it lands right after it; Beta also waits for Delta, so it
    // lands right after that; in the crown section Crown A waits for Crown B.
    assert_eq!(first, [2, 0, 3, 1, 5, 4]);
}
