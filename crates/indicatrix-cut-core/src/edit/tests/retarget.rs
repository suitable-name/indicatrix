//! `Edit::RetargetAngles`: applying/undoing a batch of angle changes as one
//! step, the same-tier-named-twice edge case, and out-of-range rejection.

use super::fixtures::{fresh_design, tier};
use crate::edit::{Edit, EditError, History};
use indicatrix::geometry::meet_solver::MeetConstraint;

#[test]
fn retarget_angles_applies_and_undoes_as_one_step() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    design
        .tiers
        .push(tier("P2", -35.0, MeetConstraint::ScaleReference(0.6), &[]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -42.0), (1, -35.0, -38.0)],
            },
        )
        .expect("retarget must apply");
    assert_eq!(design.tiers[0].angle_deg, -42.0);
    assert_eq!(design.tiers[1].angle_deg, -38.0);
    assert!(history.can_undo());

    // One atomic undo step reverts BOTH tiers at once.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(!history.can_undo());
}

/// An edit naming the SAME tier twice must undo exactly back to the
/// original angle, not an intermediate value. Applying `[(0,-40,-41),(0,-41,-42)]`
/// in order leaves tier 0 at -42; the built inverse must replay in REVERSE order
/// (`[(_,-42,-41),(_,-41,-40)]`) so undo actually lands back on -40, not -41.
#[test]
fn retarget_angles_naming_the_same_tier_twice_undoes_exactly() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -41.0), (0, -41.0, -42.0)],
            },
        )
        .expect("retarget must apply");
    assert_eq!(design.tiers[0].angle_deg, -42.0);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design, before,
        "undo must restore the true original angle, not an intermediate one"
    );
}

#[test]
fn retarget_angles_rejects_an_out_of_range_index_without_mutating_the_design() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();

    let err = design
        .apply_edit(Edit::RetargetAngles {
            changes: vec![(0, -40.0, -42.0), (5, -10.0, -12.0)],
        })
        .expect_err("index 5 is out of range");
    assert_eq!(
        err,
        EditError {
            index: 5,
            tier_count: 1
        }
    );
    assert_eq!(
        design, before,
        "a rejected multi-tier edit must not partially apply"
    );
}
