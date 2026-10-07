//! Edits where the flat and concave tier lists meet: the shared-name rule on
//! flat adds/modifies, and gear remaps of concave indices.

use super::fixtures::{fresh_design, tier};
use crate::{
    design::{ConcaveTier, ConcaveTool, Design, ToolMotion},
    edit::{Edit, History, RemapRounding, ScheduleState},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn groove(indices: &[f64]) -> ConcaveTier {
    ConcaveTier {
        name: "Groove".to_owned(),
        angle_deg: -40.0,
        indices: indices.to_vec(),
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.0, 0.1],
        diameter_ratio: 0.5,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

#[test]
fn a_flat_tier_cannot_take_a_concave_tiers_name() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddTier {
            index: 0,
            tier: tier("P1", -41.0, MeetConstraint::ScaleReference(0.5), &[0.0]),
        })
        .unwrap();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[0.0, 24.0]),
        })
        .unwrap();
    let before = design.clone();

    // Adding, and renaming (also as one of several `/`-joined names), are refused
    // and leave the design untouched.
    for name in ["Groove", "P2/Groove"] {
        let clashing = tier(name, -30.0, MeetConstraint::MeetExisting, &[0.0]);
        assert!(
            design
                .apply_edit(Edit::AddTier {
                    index: 1,
                    tier: clashing.clone()
                })
                .is_err()
        );
        assert!(
            design
                .apply_edit(Edit::ModifyTier {
                    index: 0,
                    tier: clashing
                })
                .is_err()
        );
    }
    assert_eq!(design, before);
    design.validate_concave_tiers().expect("still valid");

    // A different name is fine.
    design
        .apply_edit(Edit::ModifyTier {
            index: 0,
            tier: tier("P9", -41.0, MeetConstraint::ScaleReference(0.5), &[0.0]),
        })
        .unwrap();
}

#[test]
fn remapping_wraps_a_concave_index_that_rounds_up_to_the_gear() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[0.0, 95.0]),
        })
        .unwrap();
    // 95 * 80 / 96 = 79.17, which Ceil rounds to 80: position 0 of the 80 gear.
    let inverse = design
        .apply_edit(Edit::RemapIndices {
            from_gear: 96,
            to_gear: 80,
            rounding: RemapRounding::Ceil,
        })
        .unwrap();
    assert_eq!(design.concave_tiers[0].indices, vec![0.0, 0.0]);
    design.meta.gear_teeth = 80;
    design
        .validate_concave_tiers()
        .expect("remapped list is valid");
    design.meta.gear_teeth = 96;
    design.apply_edit(inverse).unwrap();
    assert_eq!(design.concave_tiers[0].indices, vec![0.0, 95.0]);
}

fn schedule(gear_teeth: i32) -> Edit {
    Edit::SetSchedule {
        gear_teeth,
        symmetry_order: 4,
        mirror: false,
    }
}

fn remap(to_gear: i32) -> Edit {
    Edit::RemapIndices {
        from_gear: 96,
        to_gear,
        rounding: RemapRounding::Nearest,
    }
}

/// A smaller gear may not leave a concave tier with an index past the new wheel; the
/// refusal names the concave tier and changes nothing. A gear the indices fit (the wheel's
/// own tooth count counts as position 0) and any larger one apply.
#[test]
fn a_smaller_gear_is_refused_while_a_concave_index_is_off_the_new_wheel() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[0.0, 90.0]),
        })
        .unwrap();
    let before = design.clone();

    let refused = design
        .apply_edit(schedule(80))
        .expect_err("90 is off an 80 wheel");
    assert_eq!((refused.index, refused.tier_count), (0, 1));
    assert_eq!(design, before);

    design
        .apply_edit(schedule(90))
        .expect("90 is the wheel's own tooth count");
    design
        .apply_edit(schedule(120))
        .expect("a larger gear always fits");
    design
        .apply_edit(schedule(96))
        .expect("and back down to where the indices fit");
    assert_eq!(design.concave_tiers, before.concave_tiers);
}

/// The editor's gear change is a batch of the remap and the schedule: applying it works,
/// whichever order its two edits are written in, and undoing it (the inverse batch grows
/// the gear back before the indices are restored) works too.
#[test]
fn a_remap_and_a_smaller_gear_in_one_batch_apply_and_undo() {
    for edits in [vec![remap(80), schedule(80)], vec![schedule(80), remap(80)]] {
        let mut design = fresh_design();
        design
            .apply_edit(Edit::AddConcaveTier {
                index: 0,
                tier: groove(&[0.0, 90.0]),
            })
            .unwrap();
        let before = design.clone();

        let inverse = design
            .apply_edit(Edit::Batch(edits))
            .expect("the batch fits the new wheel");
        assert_eq!(design.meta.gear_teeth, 80);
        assert_eq!(design.concave_tiers[0].indices, vec![0.0, 75.0]);
        design
            .validate_concave_tiers()
            .expect("valid on the new gear");

        design.apply_edit(inverse).expect("the undo applies");
        assert_eq!(design, before);
    }
}

/// A batch that shrinks the gear and leaves the concave indices where they were is refused
/// as a whole: nothing of it is applied.
#[test]
fn a_batch_ending_with_a_concave_index_off_the_wheel_changes_nothing() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[0.0, 90.0]),
        })
        .unwrap();
    let before = design.clone();
    let result = design.apply_edit(Edit::Batch(vec![schedule(80)]));
    assert!(result.is_err());
    assert_eq!(design, before);
}

#[test]
fn a_remap_that_would_invalidate_a_concave_tier_changes_nothing() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddTier {
            index: 0,
            tier: tier("P1", -41.0, MeetConstraint::ScaleReference(0.5), &[8.0]),
        })
        .unwrap();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[8.0]),
        })
        .unwrap();
    let before = design.clone();
    assert!(
        design
            .apply_edit(Edit::RemapIndices {
                from_gear: 96,
                to_gear: 0,
                rounding: RemapRounding::Nearest,
            })
            .is_err()
    );
    assert_eq!(design, before);
}

/// A design on the 96 wheel with one concave tier holding indices `[0, 90]`.
fn design_with_a_groove_at_90() -> Design {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[0.0, 90.0]),
        })
        .unwrap();
    design
}

/// The replacement a text apply builds: the design's own schedule state on `gear_teeth`.
fn replacement(design: &Design, gear_teeth: i32) -> Edit {
    let mut state = ScheduleState::of(design);
    state.meta.gear_teeth = gear_teeth;
    Edit::ReplaceSchedule(Box::new(state))
}

/// `Edit::ReplaceSchedule` obeys the same gear rule as `SetSchedule`: a smaller gear that
/// leaves a concave index off the wheel is refused, naming the concave tier, and nothing
/// changes (no history step either). A gear the indices fit, and any larger one, apply.
#[test]
fn a_replacement_that_shrinks_the_gear_under_a_concave_index_is_refused_and_changes_nothing() {
    let mut design = design_with_a_groove_at_90();
    let before = design.clone();

    let refused = design
        .apply_edit(replacement(&before, 80))
        .expect_err("90 is off an 80 wheel");
    assert_eq!((refused.index, refused.tier_count), (0, 1));
    assert_eq!(design, before);

    let mut history = History::new();
    assert!(
        history
            .apply(&mut design, replacement(&before, 80))
            .is_err()
    );
    assert_eq!(design, before);
    assert!(
        !history.undo(&mut design).unwrap(),
        "a refused edit leaves nothing to undo"
    );

    design
        .apply_edit(replacement(&before, 90))
        .expect("90 is the wheel's own tooth count");
    design
        .apply_edit(replacement(&before, 120))
        .expect("a larger gear always fits");
    assert_eq!(design.meta.gear_teeth, 120);
    assert_eq!(design.concave_tiers, before.concave_tiers);
}

/// The inverse of a replacement that grows the gear is a replacement that shrinks it back, and
/// it applies: the indices fitted the smaller wheel before.
#[test]
fn undoing_a_replacement_that_grew_the_gear_shrinks_it_back_without_a_refusal() {
    let mut design = design_with_a_groove_at_90();
    let before = design.clone();
    let mut history = History::new();
    history
        .apply(&mut design, replacement(&before, 120))
        .expect("a larger gear fits");
    assert_eq!(design.meta.gear_teeth, 120);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.meta.gear_teeth, 120);
}

/// A replacement that comes with the remap of the concave indices is checked at the end of the
/// batch, so it applies whichever order its two edits are written in, and one undo takes the
/// whole batch back exactly.
#[test]
fn a_replacement_with_a_remap_in_one_batch_applies_and_undoes_exactly() {
    let base = design_with_a_groove_at_90();
    for remap_first in [true, false] {
        let mut design = base.clone();
        let edits = if remap_first {
            vec![remap(80), replacement(&base, 80)]
        } else {
            vec![replacement(&base, 80), remap(80)]
        };
        let mut history = History::new();
        history
            .apply(&mut design, Edit::Batch(edits))
            .expect("the batch ends on a wheel the indices fit");
        assert_eq!(design.meta.gear_teeth, 80);
        assert_eq!(design.concave_tiers[0].indices, vec![0.0, 75.0]);
        design
            .validate_concave_tiers()
            .expect("valid on the new gear");

        assert!(history.undo(&mut design).unwrap());
        assert_eq!(design, base);
        assert!(history.redo(&mut design).unwrap());
        assert_eq!(design.meta.gear_teeth, 80);
        assert_eq!(design.concave_tiers[0].indices, vec![0.0, 75.0]);
    }
}

/// The same batch without the remap ends with the index still off the wheel: refused as a
/// whole, and the design is exactly as it was.
#[test]
fn a_batch_with_a_replacement_and_no_remap_changes_nothing() {
    let mut design = design_with_a_groove_at_90();
    let before = design.clone();
    let result = design.apply_edit(Edit::Batch(vec![replacement(&before, 80)]));
    assert!(result.is_err());
    assert_eq!(design, before);
}
