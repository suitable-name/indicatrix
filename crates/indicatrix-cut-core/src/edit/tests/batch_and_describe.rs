//! `Edit::Batch`: applying/undoing every sub-edit as one step, rejecting a
//! partially-invalid batch without mutating the design, and `describe`'s
//! cutter-facing sentences across representative variants.

use super::fixtures::{fresh_design, tier};
use crate::{
    edit::{Edit, EditError, History, RemapRounding},
    material::MaterialSelection,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

#[test]
fn batch_applies_every_sub_edit_and_undoes_them_all_in_one_step() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::Batch(vec![
                Edit::RetargetAngles {
                    changes: vec![(0, -40.0, -42.0)],
                },
                Edit::SetMaterial {
                    material: MaterialSelection {
                        name: Some("Quartz".to_string()),
                        specific_gravity_override: None,
                        refractive_index_override: None,
                    },
                },
            ]),
        )
        .expect("batch must apply");
    assert_eq!(design.tiers[0].angle_deg, -42.0);
    assert_eq!(design.material.name.as_deref(), Some("Quartz"));
    assert!(history.can_undo());

    // One atomic undo step reverts BOTH sub-edits at once.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(!history.can_undo());

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -42.0);
    assert_eq!(design.material.name.as_deref(), Some("Quartz"));
}

#[test]
fn batch_rejects_a_partially_invalid_batch_without_mutating_the_design() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();

    // The first sub-edit is valid on its own; the second names a tier index the
    // design doesn't have. A naive "apply each in turn against `self`" implementation
    // would leave the first sub-edit's effect in place -- this must not happen.
    let err = design
        .apply_edit(Edit::Batch(vec![
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -42.0)],
            },
            Edit::RemoveTier { index: 5 },
        ]))
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
        "a batch that fails partway must not leave the design half-applied"
    );
}

#[test]
fn describe_reads_as_a_cutters_sentence_for_representative_variants() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    design
        .tiers
        .push(tier("C1", 30.0, MeetConstraint::ScaleReference(0.5), &[]));

    assert_eq!(
        Edit::RetargetAngles {
            changes: vec![(0, -40.0, -41.0)],
        }
        .describe(&design),
        "Set P1 angle to -41.0 degrees"
    );
    assert_eq!(
        Edit::RemapIndices {
            from_gear: 96,
            to_gear: 80,
            rounding: RemapRounding::Nearest,
        }
        .describe(&design),
        "Remap gear 96 to 80"
    );
    assert_eq!(
        Edit::RemoveTier { index: 1 }.describe(&design),
        "Remove tier C1"
    );
    let optimize_changes: Vec<(usize, f64, f64)> = (0..12).map(|i| (i, 0.0, 1.0)).collect();
    assert_eq!(
        Edit::RetargetAngles {
            changes: optimize_changes,
        }
        .describe(&design),
        "Optimize 12 tiers"
    );
    assert_eq!(
        Edit::Batch(vec![
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -41.0)],
            },
            Edit::SetMaterial {
                material: MaterialSelection {
                    name: Some("Quartz".to_string()),
                    specific_gravity_override: None,
                    refractive_index_override: None,
                },
            },
        ])
        .describe(&design),
        "Retarget for Quartz"
    );
}
