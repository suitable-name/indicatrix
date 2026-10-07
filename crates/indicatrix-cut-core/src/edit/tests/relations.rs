//! `Edit::SetTierRelation`: apply, exact undo and redo, how a relation follows its tier
//! through add, remove and move, and the sentences `describe` gives it.

use super::fixtures::{fresh_design, tier};
use crate::{
    design::{Design, TierRelation},
    edit::{Edit, EditError, History},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Three crown tiers C1 (40), C2 (38) and C3 (30), ids 0, 1 and 2.
fn crown_design() -> Design {
    let mut design = fresh_design();
    for (name, angle) in [("C1", 40.0), ("C2", 38.0), ("C3", 30.0)] {
        design
            .tiers
            .push(tier(name, angle, MeetConstraint::MeetExisting, &[0.0]));
    }
    design.ensure_tier_ids();
    design
}

fn relation(design: &Design, text: &str) -> TierRelation {
    design.parse_relation(text).expect("the relation reads")
}

fn set(index: usize, relation: Option<TierRelation>) -> Edit {
    Edit::SetTierRelation { index, relation }
}

#[test]
fn setting_a_relation_then_undo_and_redo_is_exact() {
    let mut design = crown_design();
    let before = design.clone();
    let c2_follows_c1 = relation(&design, "C1 - 2");
    let mut history = History::new();

    history
        .apply(&mut design, set(1, Some(c2_follows_c1.clone())))
        .expect("the relation applies");
    assert_eq!(design.tier_relation(1), Some(&c2_follows_c1));
    assert_eq!(design.tier_relation(0), None);
    assert_ne!(design, before);
    // A relation edit moves no angle by itself; a session folds the angle update in.
    assert_eq!(design.tiers[1].angle_deg, 38.0);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(design.tier_relations.is_empty());

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.tier_relation(1), Some(&c2_follows_c1));
}

#[test]
fn replacing_and_clearing_a_relation_undo_to_the_previous_one() {
    let mut design = crown_design();
    let first = relation(&design, "C1 - 2");
    let second = relation(&design, "C1 - 3");
    let mut history = History::new();
    history
        .apply(&mut design, set(1, Some(first.clone())))
        .unwrap();
    history
        .apply(&mut design, set(1, Some(second.clone())))
        .unwrap();
    assert_eq!(design.tier_relation(1), Some(&second));
    history.apply(&mut design, set(1, None)).unwrap();
    assert_eq!(design.tier_relation(1), None);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tier_relation(1), Some(&second));
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tier_relation(1), Some(&first));
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tier_relation(1), None);
}

#[test]
fn a_tier_that_does_not_exist_is_refused_and_nothing_changes() {
    let mut design = crown_design();
    let before = design.clone();
    let reads_c1 = relation(&design, "C1 + 1");
    let error = design
        .apply_edit(set(9, Some(reads_c1)))
        .expect_err("tier 9 does not exist");
    assert_eq!(
        error,
        EditError {
            index: 9,
            tier_count: 3
        }
    );
    assert_eq!(design, before);
}

#[test]
fn removing_a_driven_tier_takes_its_relation_and_undo_brings_it_back() {
    let mut design = crown_design();
    let c3_follows_c2 = relation(&design, "C2 - 3");
    let mut history = History::new();
    history
        .apply(&mut design, set(2, Some(c3_follows_c2.clone())))
        .unwrap();
    let before = design.clone();
    let driven_id = design.tier_id_at(2).expect("C3 has an id");

    history
        .apply(&mut design, Edit::RemoveTier { index: 2 })
        .unwrap();
    assert!(design.tier_relations.is_empty());
    assert_eq!(design.tier_relation_for_id(driven_id), None);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(design.tier_relation(2), Some(&c3_follows_c2));
    assert_eq!(design.tier_id_at(2), Some(driven_id));
}

#[test]
fn a_relation_follows_its_tier_through_add_move_and_remove() {
    let mut design = crown_design();
    let c3_follows_c2 = relation(&design, "C2 - 3");
    let mut history = History::new();
    history
        .apply(&mut design, set(2, Some(c3_follows_c2.clone())))
        .unwrap();
    let before = design.clone();

    // A tier added in front pushes C3 to row 3; the relation stays on C3.
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("New", 20.0, MeetConstraint::MeetExisting, &[0.0]),
            },
        )
        .unwrap();
    assert_eq!(design.tiers[3].name, "C3");
    assert_eq!(design.tier_relation(3), Some(&c3_follows_c2));
    assert_eq!(design.tier_relation(2), None);
    assert_eq!(design.relation_text(3).as_deref(), Some("C2 - 3"));
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);

    // Moving C3 to the front takes the relation along.
    history
        .apply(&mut design, Edit::MoveTier { from: 2, to: 0 })
        .unwrap();
    assert_eq!(design.tiers[0].name, "C3");
    assert_eq!(design.tier_relation(0), Some(&c3_follows_c2));
    assert_eq!(design.relation_drivers(0), vec![2]);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);

    // Removing the tier it READS leaves the relation in the map (a session clears it
    // in the same step); the core edit does not decide that.
    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .unwrap();
    assert!(design.tier_relation(1).is_some());
    assert!(design.evaluate_relations().is_err());
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

#[test]
fn a_relation_in_a_batch_is_all_or_nothing() {
    let mut design = crown_design();
    let before = design.clone();
    let c2_follows_c1 = relation(&design, "C1 - 2");
    let error = design
        .apply_edit(Edit::Batch(vec![
            set(1, Some(c2_follows_c1)),
            Edit::RemoveTier { index: 7 },
        ]))
        .expect_err("index 7 is out of range");
    assert_eq!(error.index, 7);
    assert_eq!(design, before, "the first sub-edit must not stay applied");
}

#[test]
fn describe_names_the_tier_and_the_relation() {
    let design = crown_design();
    let c2_follows_c1 = relation(&design, "C1 - 2");
    assert_eq!(
        set(1, Some(c2_follows_c1)).describe(&design),
        "Set C2 = C1 - 2"
    );
    assert_eq!(set(1, None).describe(&design), "Clear relation for C2");
}

#[test]
fn a_batch_of_an_edit_and_the_angles_that_follow_it_reads_as_the_edit() {
    let design = crown_design();
    let c2_follows_c1 = relation(&design, "C1 - 2");
    let set_it = set(1, Some(c2_follows_c1));
    let retarget = Edit::RetargetAngles {
        changes: vec![(1, 38.0, 37.0)],
    };
    let label = set_it.describe(&design);
    assert_eq!(
        Edit::Batch(vec![set_it.clone(), retarget.clone()]).describe(&design),
        label
    );
    // The undo entry holds the pair the other way round.
    assert_eq!(
        Edit::Batch(vec![retarget.clone(), set_it]).describe(&design),
        label
    );

    // A removal that also frees the relations reading the removed tier reads as the
    // removal.
    let remove = Edit::RemoveTier { index: 0 };
    assert_eq!(
        Edit::Batch(vec![remove.clone(), set(1, None), retarget]).describe(&design),
        remove.describe(&design)
    );
}

fn crown_tier(name: &str, angle: f64) -> crate::design::ConstraintTier {
    tier(name, angle, MeetConstraint::MeetExisting, &[0.0])
}

#[test]
fn a_tier_save_that_renames_and_sets_a_relation_reads_as_both() {
    let design = crown_design();
    let c2_follows_c1 = relation(&design, "C1 - 2");
    let rename = Edit::ModifyTier {
        index: 1,
        tier: crown_tier("C2b", 38.0),
    };
    let save = Edit::Batch(vec![rename.clone(), set(1, Some(c2_follows_c1.clone()))]);
    assert_eq!(
        save.describe(&design),
        "Rename C2 to C2b and set C2b = C1 - 2"
    );

    // An editor session folds the angles that follow into the same batch, around the save.
    let folded = Edit::Batch(vec![
        save,
        Edit::RetargetAngles {
            changes: vec![(1, 38.0, 38.0)],
        },
    ]);
    assert_eq!(
        folded.describe(&design),
        "Rename C2 to C2b and set C2b = C1 - 2"
    );

    // A save that keeps the name is a modification, and a depth target rides along.
    let modify = Edit::Batch(vec![
        Edit::ModifyTier {
            index: 1,
            tier: crown_tier("C2", 36.0),
        },
        Edit::SetTierTarget {
            index: 1,
            target: None,
        },
        set(1, Some(c2_follows_c1)),
    ]);
    assert_eq!(
        modify.describe(&design),
        "Modify tier C2 and set C2 = C1 - 2"
    );

    // Following a rename's meet-name rewrites is still the same save.
    let with_rewrite = Edit::Batch(vec![
        rename,
        Edit::SetConstraint {
            index: 2,
            constraint: MeetConstraint::MeetExisting,
        },
        set(1, None),
    ]);
    assert_eq!(
        with_rewrite.describe(&design),
        "Rename C2 to C2b and clear relation for C2b"
    );
}

#[test]
fn a_new_tier_with_a_relation_reads_as_both_and_its_undo_as_a_removal() {
    let mut design = crown_design();
    let c4_follows_c1 = relation(&design, "C1 - 5");
    let add = Edit::Batch(vec![
        Edit::AddTier {
            index: 3,
            tier: crown_tier("C4", 35.0),
        },
        set(3, Some(c4_follows_c1)),
    ]);
    assert_eq!(add.describe(&design), "Add tier C4 and set C4 = C1 - 5");

    // The undo of that save, as the history holds it: the relation goes, then the tier.
    design.tiers.push(crown_tier("C4", 35.0));
    design.ensure_tier_ids();
    let undo = Edit::Batch(vec![set(3, None), Edit::RemoveTier { index: 3 }]);
    assert_eq!(undo.describe(&design), "Remove tier C4");
}

#[test]
fn the_undo_and_redo_of_a_tier_save_with_a_relation_read_the_same_way() {
    let mut design = crown_design();
    let c2_follows_c1 = relation(&design, "C1 - 2");
    let save = Edit::Batch(vec![
        Edit::ModifyTier {
            index: 1,
            tier: crown_tier("C2b", 38.0),
        },
        set(1, Some(c2_follows_c1)),
    ]);
    let mut history = History::new();
    history.apply(&mut design, save).expect("the save applies");
    assert_eq!(design.tiers[1].name, "C2b");

    let undo = history.peek_undo().expect("there is a step to undo");
    assert_eq!(
        undo.describe(&design),
        "Rename C2b to C2 and clear relation for C2"
    );
    assert!(history.undo(&mut design).unwrap());
    let redo = history.peek_redo().expect("there is a step to redo");
    assert_eq!(
        redo.describe(&design),
        "Rename C2 to C2b and set C2b = C1 - 2"
    );
}

#[test]
fn a_modification_and_a_relation_of_two_different_tiers_keeps_the_generic_label() {
    let design = crown_design();
    let c2_follows_c1 = relation(&design, "C1 - 2");
    let unrelated = Edit::Batch(vec![
        Edit::ModifyTier {
            index: 0,
            tier: crown_tier("C1", 41.0),
        },
        set(1, Some(c2_follows_c1)),
    ]);
    assert_eq!(unrelated.describe(&design), "2 combined edits");
}

#[test]
fn angle_updates_alone_read_as_an_angle_change() {
    let design = crown_design();
    let one_tier = Edit::Batch(vec![
        Edit::RetargetAngles {
            changes: vec![(0, 40.0, 39.0)],
        },
        Edit::RetargetAngles {
            changes: vec![(0, 39.0, 38.0)],
        },
    ]);
    assert_eq!(one_tier.describe(&design), "Set C1 angle to 38.0 degrees");
    let two_tiers = Edit::Batch(vec![
        Edit::RetargetAngles {
            changes: vec![(0, 40.0, 39.0)],
        },
        Edit::RetargetAngles {
            changes: vec![(1, 38.0, 37.0)],
        },
    ]);
    assert_eq!(two_tiers.describe(&design), "Change angles of 2 tiers");
}
