//! `Edit::MoveTier`: reordering up/down, the no-op self-move, out-of-range
//! rejection, and its `describe` text.

use super::fixtures::{fresh_design, tier};
use crate::{
    design::Design,
    edit::{Edit, EditError, History},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn four_tier_design() -> Design {
    let mut design = fresh_design();
    for name in ["A", "B", "C", "D"] {
        design
            .tiers
            .push(tier(name, 0.0, MeetConstraint::ScaleReference(0.5), &[]));
    }
    design
}

fn tier_names(design: &Design) -> Vec<&str> {
    design.tiers.iter().map(|t| t.name.as_str()).collect()
}

/// Moving a tier toward the front (`to < from`) must renumber every tier strictly
/// between the two positions, and undo must restore the original order exactly.
#[test]
fn move_tier_up_reorders_and_undoes_cleanly() {
    let mut design = four_tier_design();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::MoveTier { from: 2, to: 0 })
        .expect("move must apply");
    assert_eq!(tier_names(&design), vec!["C", "A", "B", "D"]);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(tier_names(&design), vec!["A", "B", "C", "D"]);
}

/// Moving a tier toward the back (`to > from`) is the same operation in the other
/// direction -- checked separately since `apply_move_tier`'s remove-then-insert
/// shifts indices differently depending on which side `to` sits on relative to
/// `from`.
#[test]
fn move_tier_down_reorders_and_undoes_cleanly() {
    let mut design = four_tier_design();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::MoveTier { from: 0, to: 2 })
        .expect("move must apply");
    assert_eq!(tier_names(&design), vec!["B", "C", "A", "D"]);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(tier_names(&design), vec!["A", "B", "C", "D"]);
}

/// Moving a tier to its own current position must be a true no-op -- the design is
/// byte-identical afterward, and it still undoes cleanly (as itself).
#[test]
fn move_tier_to_its_own_position_is_a_no_op() {
    let mut design = four_tier_design();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::MoveTier { from: 1, to: 1 })
        .expect("no-op move must apply");
    assert_eq!(design, before);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

/// A `from`/`to` naming a tier index the design doesn't have must be rejected
/// without mutating the design, exactly like every other index-bearing `Edit`.
#[test]
fn move_tier_out_of_range_is_rejected_without_mutating_the_design() {
    let mut design = four_tier_design();
    let before = design.clone();

    let err = design
        .apply_edit(Edit::MoveTier { from: 1, to: 9 })
        .expect_err("to=9 is out of range for a 4-tier design");
    assert_eq!(
        err,
        EditError {
            index: 9,
            tier_count: 4
        }
    );
    assert_eq!(design, before);

    let err = design
        .apply_edit(Edit::MoveTier { from: 9, to: 1 })
        .expect_err("from=9 is out of range for a 4-tier design");
    assert_eq!(
        err,
        EditError {
            index: 9,
            tier_count: 4
        }
    );
    assert_eq!(design, before);
}

/// `describe` must read as a cutter's sentence for a move in each direction.
#[test]
fn describe_move_tier_names_the_tier_and_direction() {
    let design = four_tier_design();
    assert_eq!(
        Edit::MoveTier { from: 2, to: 0 }.describe(&design),
        "Move tier C up"
    );
    assert_eq!(
        Edit::MoveTier { from: 0, to: 2 }.describe(&design),
        "Move tier A down"
    );
}
