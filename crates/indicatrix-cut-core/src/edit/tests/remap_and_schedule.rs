//! `SetSchedule` (gear/symmetry/mirror), `RemapIndices`/`RestoreIndices`, and
//! [`crate::edit::remap_ratio`] itself.

use super::fixtures::{fresh_design, tier};
use crate::edit::{Edit, History, RemapRounding, remap_ratio};
use indicatrix::geometry::meet_solver::MeetConstraint;

// --- SetSchedule ---

#[test]
fn set_schedule_then_undo_restores_the_old_gear_symmetry_mirror() {
    let mut design = fresh_design();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetSchedule {
                gear_teeth: 80,
                symmetry_order: 8,
                mirror: false,
            },
        )
        .expect("set schedule must apply");
    assert_eq!(design.meta.gear_teeth, 80);
    assert_eq!(design.meta.symmetry_order, 8);
    assert!(!design.meta.mirror);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

// --- RemapIndices / RestoreIndices ---

/// The plan's own 96 -> 80 example: a non-integer ratio (5/6), so the forward
/// remap is genuinely lossy -- undo must restore the ORIGINAL vectors verbatim
/// rather than attempt (and get wrong) a reverse 80 -> 96 remap.
#[test]
fn remap_indices_then_undo_restores_original_indices_verbatim_even_when_lossy() {
    let mut design = fresh_design();
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 8.0, 24.0, 40.0, 56.0, 72.0, 88.0],
    ));
    design.tiers[0].detached = vec![8.0, 88.0];
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::RemapIndices {
                from_gear: 96,
                to_gear: 80,
                rounding: RemapRounding::Nearest,
            },
        )
        .expect("remap must apply");
    // 96 -> 80 must have actually changed something -- otherwise this test
    // would not be exercising real lossiness at all.
    assert_ne!(design.tiers[0].indices, before.tiers[0].indices);
    assert_ne!(design.tiers[0].detached, before.tiers[0].detached);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design, before,
        "undo must restore the exact original indices/detached, not a lossy reverse remap"
    );
}

#[test]
fn remap_indices_redo_reproduces_the_exact_same_remapped_values() {
    let mut design = fresh_design();
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::RemapIndices {
                from_gear: 96,
                to_gear: 80,
                rounding: RemapRounding::Nearest,
            },
        )
        .expect("remap must apply");
    let after_remap = design.clone();

    assert!(history.undo(&mut design).unwrap());
    assert!(history.redo(&mut design).unwrap());
    assert_eq!(
        design, after_remap,
        "redo must reproduce the exact same remapped values, not recompute a fresh remap"
    );
}

/// A gear remap that never actually changes an index (`from_gear == to_gear`)
/// must still round-trip cleanly through undo -- the degenerate, non-lossy case.
#[test]
fn remap_indices_with_equal_gears_is_a_no_op_and_still_undoes_cleanly() {
    let mut design = fresh_design();
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::RemapIndices {
                from_gear: 96,
                to_gear: 96,
                rounding: RemapRounding::Nearest,
            },
        )
        .expect("remap must apply");
    assert_eq!(design, before);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

/// A negative `g` header (a reversed index wheel, which real `.asc` files carry)
/// must remap by MAGNITUDE. Scaling by the signed ratio moved every position to a
/// negative index no wheel has, silently ruining the whole schedule.
#[test]
fn remap_indices_ignores_the_sign_of_either_gear() {
    let mut design = fresh_design();
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    let mut from_negative = design.clone();
    let mut both_positive = design.clone();

    for (target, from_gear) in [(&mut from_negative, -96), (&mut both_positive, 96)] {
        target
            .apply_edit(Edit::RemapIndices {
                from_gear,
                to_gear: 48,
                rounding: RemapRounding::Nearest,
            })
            .expect("remap must apply");
    }

    assert_eq!(from_negative, both_positive);
    assert_eq!(from_negative.tiers[0].indices, vec![0.0, 12.0, 24.0, 36.0]);
}

/// An [`Edit::RestoreIndices`] naming the SAME tier twice (latent
/// today -- `RemapIndices`'s own inverse never repeats an index, but the variant
/// is public) must undo back to the true original `indices`/`detached`, not an
/// intermediate value along the way. Same shape as
/// `retarget_angles_naming_the_same_tier_twice_undoes_exactly`.
#[test]
fn restore_indices_naming_the_same_tier_twice_undoes_exactly() {
    let mut design = fresh_design();
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::RestoreIndices {
                tiers: vec![(0, vec![0.0, 12.0], vec![]), (0, vec![0.0], vec![])],
            },
        )
        .expect("restore indices must apply");
    assert_eq!(design.tiers[0].indices, vec![0.0]);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design, before,
        "undo must restore the true original indices, not an intermediate value"
    );
}

/// The ratio itself, at the boundaries the tier-table preview shares with the edit.
#[test]
fn remap_ratio_is_magnitude_only_and_finite_at_zero() {
    assert!((remap_ratio(96, 48) - 0.5).abs() < f64::EPSILON);
    assert!((remap_ratio(-96, 48) - 0.5).abs() < f64::EPSILON);
    assert!((remap_ratio(96, -48) - 0.5).abs() < f64::EPSILON);
    assert!((remap_ratio(-96, -48) - 0.5).abs() < f64::EPSILON);
    // Not a real gear, but reachable through this crate's own types.
    assert!((remap_ratio(0, 48) - 1.0).abs() < f64::EPSILON);
}
