//! Tests for the verdict's fixes: every fix gives the edit it promises as ONE undo step.
//! (Split from `tests.rs` to keep both files under 800 lines; the fixtures it shares live there.)

use super::{brilliant, brilliant_keeping, deep_and_table, design_with, tier};
use crate::{
    retarget::validity::analyze,
    session::EditorSession,
    verdict::{
        FixAction, QUICK_ADD_TABLE_MAST, SolveFacts,
        fix::{partial_removal_edit, remove_tier_edit, unique_table_name, vanished_positions_in},
        gather, plan_fix, steep_target_deg,
    },
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    Design, Edit, History,
    design::hinge::tier_hinge_points,
    manufacturability::check_cut_order,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
};
use std::{collections::BTreeSet, ops::Range};

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

fn position_of(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .unwrap_or_else(|| panic!("no tier named {name}"))
}

// ---------------------------------------------------------------------------------------
// Fixes
// ---------------------------------------------------------------------------------------

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

/// The girdle OUTLINE is the contract (owner decision, 2026-10-19): the steepened facet
/// turns about its girdle corners, the product's own gate accepts the result, and the
/// measured band moves by at most half a percentage point.
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

    // The girdle OUTLINE is the contract (owner decision, 2026-10-19): the facet turns about
    // its girdle corners, so the pivot has not moved, the fix passes the product's own
    // meet-preservation gate, and the measured band (read at the ridge points, which move
    // when the corners stay) changes by no more than half a percentage point.
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
    // (a) The product's own gate accepts the fixed stone: the outline junctions stay.
    MeetPreservationBaseline::capture(&design)
        .expect("the original captures")
        .validate(&session.design, PreservationPolicy::OPTIMIZE)
        .expect("the steepened stone keeps the girdle outline and every meet");
    // (b) The measured band may move a little: the ridge points are not outline corners.
    assert!(
        (girdle_after - girdle_before).abs() <= 0.5,
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
