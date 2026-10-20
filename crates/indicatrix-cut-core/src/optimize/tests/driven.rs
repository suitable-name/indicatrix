//! Tests for tiers that follow a relation: they are never free, a candidate that moves
//! a tier they read moves them too (so the score describes a design the user can get),
//! their angle never appears in a change list (their depth may: followers keep depth
//! freedom), and applying a candidate leaves the relations satisfied in the same undo step.
//!
//! Every test uses RBC-445. Tier 8 is "B" (29.0 degrees) and tier 9 is "C" (28.1 degrees);
//! C is made to follow B with `B - 0.9`.

use super::{
    super::{
        AngleChange, ObjectiveComponents, ObjectiveWeights, OptimizeCandidate, OptimizeConfig,
        SearchHooks, apply_optimize_candidate, apply_optimize_outcome, apply_optimize_result,
        candidate, free_tier_indices, free_tier_indices_with, optimize_design_with,
        space::settle_relations,
    },
    anchored::{anchored_options, space_for},
    fixtures::{imported_rbc, rbc_445},
};
use crate::{design::Design, edit::History};
use indicatrix::{geometry::meet_solver::MeetConstraint, optics::materials::GemMaterial};

const DRIVER: usize = 8;
const FOLLOWER: usize = 9;

/// Makes tier `follower` follow the relation `text` and moves it to the angle that gives.
fn follow(design: &mut Design, follower: usize, text: &str) {
    design.ensure_tier_ids();
    let relation = design.parse_relation(text).expect("the relation reads");
    let id = design.tier_ids[follower];
    design.tier_relations.insert(id, relation);
    let updates = design.evaluate_relations().expect("the relation evaluates");
    for (position, angle_deg) in updates {
        design.tiers[position].angle_deg = angle_deg;
    }
}

fn imported_with_follower() -> Design {
    let mut design = imported_rbc();
    follow(&mut design, FOLLOWER, "B - 0.9");
    design
}

fn yield_only() -> OptimizeConfig {
    OptimizeConfig {
        weights: ObjectiveWeights {
            windowing: 0.0,
            extinction: 0.0,
            tilt_brilliance: 0.0,
            yield_weight: 1.0,
            ..ObjectiveWeights::default()
        },
        seed: 7,
        max_evaluations: 24,
        polish_start_step_deg: None,
        ..OptimizeConfig::default()
    }
}

fn candidate_of(changes: Vec<AngleChange>) -> OptimizeCandidate {
    OptimizeCandidate {
        provenance: None,
        preservation_baseline: None,
        changes,
        mast_changes: Vec::new(),
        after: ObjectiveComponents {
            windowing_pct: 0.0,
            extinction_pct: 0.0,
            tilt_brilliance_pct: 0.0,
        },
        score: 0.0,
        yield_loss_pct: 0.0,
        tone: None,
    }
}

// --- which tiers are free ---

#[test]
fn a_tier_that_follows_a_relation_is_never_free() {
    let mut design = rbc_445();
    assert!(free_tier_indices(&design).contains(&FOLLOWER));
    follow(&mut design, FOLLOWER, "B - 0.9");
    let free = free_tier_indices(&design);
    assert!(!free.contains(&FOLLOWER), "the follower is not a variable");
    assert!(free.contains(&DRIVER), "the tier it reads still is");
}

#[test]
fn a_tier_that_follows_a_relation_is_not_an_anchored_variable_either() {
    let design = imported_with_follower();
    let options = anchored_options(&design);
    assert!(
        options.anchor_hinges.contains_key(&FOLLOWER),
        "the hinge is there; only the relation keeps the tier out"
    );
    let free = free_tier_indices_with(&design, &options);
    assert!(free.contains(&DRIVER));
    assert!(!free.contains(&FOLLOWER));
}

// --- the candidate that gets scored is the design the user gets ---

#[test]
fn writing_the_angle_of_a_tier_moves_the_tiers_that_follow_it() {
    let design = imported_with_follower();
    let options = anchored_options(&design);
    let (space, _free) = space_for(&design, &options);

    let mut moved = design;
    assert!(space.write_angle(&mut moved, DRIVER, 30.0));
    assert_eq!(moved.tiers[DRIVER].angle_deg, 30.0);
    assert!(
        (moved.tiers[FOLLOWER].angle_deg - 29.1).abs() < 1e-9,
        "the follower must be 0.9 below the tier it follows, got {}",
        moved.tiers[FOLLOWER].angle_deg
    );
}

#[test]
fn a_candidate_whose_relations_cannot_be_satisfied_is_dropped() {
    // 29 + 80 is not a facet angle at all.
    let mut design = imported_rbc();
    design.ensure_tier_ids();
    let relation = design.parse_relation("B + 80").expect("the text reads");
    let id = design.tier_ids[FOLLOWER];
    design.tier_relations.insert(id, relation);
    assert!(!settle_relations(&mut design));

    // 29 + 60.6 is a facet angle, but past the 89.5 degrees the search keeps clear of.
    let mut near_vertical = imported_rbc();
    near_vertical.ensure_tier_ids();
    let relation = near_vertical
        .parse_relation("B + 60.6")
        .expect("the text reads");
    let id = near_vertical.tier_ids[FOLLOWER];
    near_vertical.tier_relations.insert(id, relation);
    assert!(!settle_relations(&mut near_vertical));
}

#[test]
fn a_design_without_relations_is_settled_untouched() {
    let mut design = imported_rbc();
    let before = design.clone();
    assert!(settle_relations(&mut design));
    assert_eq!(design, before);
}

// --- a whole run ---

/// Optimize with only the tier that "C" reads free: every candidate keeps `C = B - 0.9`,
/// "C" is in no angle change list, and the candidate's yield is the yield of the design the
/// relation produces.
///
/// "C" keeps its depth freedom: `movable_masts` lets a pinned relation follower with a hinge
/// move its mast, and the meets "C" shares with the turned crown facet need it (item 2 of
/// `docs/meet-preserving-optimizers-implementation-report.md`: followers contribute depth
/// variables, only independent follower-angle edits are refused). So a depth change of "C"
/// is listed with the rest of the certified state; it starts from the mast the design
/// pinned, applying it reproduces the listed depth bit for bit (owner decision Q1,
/// 2026-10-19), and applying moves "C"'s angle only through the relation. The check is never
/// vacuous: a list that does not name "C" must leave its pinned mast bit for bit, and at
/// least one list (the result's or a candidate's) names it, since the turned crown facet
/// moves the meets "C" shares with it.
#[test]
fn every_candidate_keeps_the_relation_and_never_lists_the_followers_angle() {
    let design = imported_with_follower();
    let MeetConstraint::ScaleReference(pinned_follower_mast) = design.tiers[FOLLOWER].constraint
    else {
        panic!("the imported follower is pinned");
    };
    let mut options = anchored_options(&design);
    options
        .anchor_hinges
        .retain(|index, _| *index == DRIVER || *index == FOLLOWER);
    options.angle_bounds.insert(DRIVER, (27.0, 31.0));
    options.keep_candidates = 3;
    options.candidate_separation_deg = Some(0.25);
    assert_eq!(free_tier_indices_with(&design, &options), vec![DRIVER]);

    let result = optimize_design_with(
        &design,
        &GemMaterial::diamond(),
        &yield_only(),
        &options,
        &SearchHooks::default(),
    )
    .expect("the design solves");
    assert!(
        !result.candidates.is_empty(),
        "turning the crown facet changes how much of the rough is kept"
    );
    assert!(result.outcome.changes.iter().all(|c| c.index != FOLLOWER));

    // The follower's pinned depth after `applied` took a change list: bit for bit the
    // depth that list named, or, when the list does not name it, bit for bit the pinned
    // mast. Returns whether the list named it, so the exact branch is known to have run.
    let follower_depth_reproduced = |applied: &Design, listed: &[super::super::MastChange]| {
        let MeetConstraint::ScaleReference(now) = applied.tiers[FOLLOWER].constraint else {
            panic!("the follower stays pinned");
        };
        let mut listed_follower = false;
        for mast in listed.iter().filter(|m| m.index == FOLLOWER) {
            listed_follower = true;
            assert_eq!(mast.from_mast.to_bits(), pinned_follower_mast.to_bits());
            assert_eq!(
                now.to_bits(),
                mast.to_mast.to_bits(),
                "applying must reproduce the listed follower depth exactly"
            );
        }
        if !listed_follower {
            assert_eq!(
                now.to_bits(),
                pinned_follower_mast.to_bits(),
                "a list that does not name the follower's depth must leave it pinned as it was"
            );
        }
        listed_follower
    };
    let mut follower_depth_listed = false;

    // Applying the result itself (one undo step) reproduces every listed follower depth,
    // through both apply APIs: the outcome alone (rebuilt from its provenance) and the
    // result (its `changes` together with its `mast_changes`).
    let mut applied = design.clone();
    let mut history = History::new();
    apply_optimize_outcome(&mut history, &mut applied, &result.outcome)
        .expect("the result applies to the design it was computed from");
    follower_depth_listed |= follower_depth_reproduced(&applied, &result.mast_changes);
    assert!(history.undo(&mut applied).expect("one undo step"));
    assert_eq!(applied, design);
    let mut applied = design.clone();
    let mut history = History::new();
    apply_optimize_result(&mut history, &mut applied, &result)
        .expect("the result applies through its own API");
    follower_depth_listed |= follower_depth_reproduced(&applied, &result.mast_changes);
    assert!(history.undo(&mut applied).expect("one undo step"));
    assert_eq!(applied, design);

    for candidate_found in &result.candidates {
        assert!(candidate_found.changes.iter().all(|c| c.index != FOLLOWER));

        let mut applied = design.clone();
        let mut history = History::new();
        apply_optimize_candidate(&mut history, &mut applied, candidate_found)
            .expect("a candidate applies to the design it was computed from");
        // A listed depth of the follower starts from the pinned mast and is what applying
        // leaves on the tier.
        follower_depth_listed |= follower_depth_reproduced(&applied, &candidate_found.mast_changes);
        let driver_deg = applied.tiers[DRIVER].angle_deg;
        assert!(
            (applied.tiers[FOLLOWER].angle_deg - (driver_deg - 0.9)).abs() < 1e-9,
            "C must stay 0.9 below B: B {driver_deg}, C {}",
            applied.tiers[FOLLOWER].angle_deg
        );
        for (position, angle_deg) in applied.evaluate_relations().expect("evaluates") {
            assert_eq!(applied.tiers[position].angle_deg, angle_deg);
        }

        let solved = applied.solve().expect("the applied design solves");
        let planes = applied.planes_from_solved(&solved);
        let applied_loss = candidate::yield_loss_pct(
            &applied,
            indicatrix::geometry::stone_metrics::measure_solid(&planes).as_ref(),
        );
        assert!(
            (applied_loss - candidate_found.yield_loss_pct).abs() < 1e-4,
            "the scored design must be the applied one: scored {}, applied {applied_loss}",
            candidate_found.yield_loss_pct
        );
    }
    assert!(
        follower_depth_listed,
        "the turned crown facet moves the meets C shares with it, so some certified state \
         lists C's depth; otherwise the exact check above never ran"
    );
}

/// With several tiers free, the follower is still never a variable and never listed.
#[test]
fn a_run_with_many_free_tiers_never_changes_the_follower_directly() {
    let design = imported_with_follower();
    let mut options = anchored_options(&design);
    for index in free_tier_indices_with(&design, &options) {
        let angle = design.tiers[index].angle_deg;
        options
            .angle_bounds
            .insert(index, (angle - 1.0, angle + 1.0));
    }
    assert!(!options.angle_bounds.contains_key(&FOLLOWER));
    let result = optimize_design_with(
        &design,
        &GemMaterial::diamond(),
        &yield_only(),
        &options,
        &SearchHooks::default(),
    )
    .expect("the design solves");
    for candidate_found in &result.candidates {
        assert!(candidate_found.changes.iter().all(|c| c.index != FOLLOWER));
    }
    assert!(result.outcome.changes.iter().all(|c| c.index != FOLLOWER));
}

// --- applying ---

#[test]
fn uncertified_angle_changes_cannot_move_a_follower() {
    let original = imported_with_follower();
    let mut design = original.clone();
    let mut history = History::new();
    let found = candidate_of(vec![AngleChange {
        index: DRIVER,
        from_deg: 29.0,
        to_deg: 30.0,
    }]);

    assert!(apply_optimize_candidate(&mut history, &mut design, &found).is_err());
    assert_eq!(design, original);
    assert!(!history.can_undo());
}

#[test]
fn uncertified_angle_changes_cannot_bypass_the_network() {
    let original = imported_rbc();
    let mut design = original.clone();
    let mut history = History::new();
    let found = candidate_of(vec![AngleChange {
        index: DRIVER,
        from_deg: 29.0,
        to_deg: 30.0,
    }]);
    assert!(apply_optimize_candidate(&mut history, &mut design, &found).is_err());
    assert_eq!(design, original);
    assert!(!history.can_undo());
}
