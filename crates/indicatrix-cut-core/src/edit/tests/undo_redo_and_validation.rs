//! Multi-step undo/redo, out-of-range rejection, the redo-stack-clearing
//! rule, and how `Design::status`'s validation tracks edits through undo.

use super::fixtures::{fresh_design, tier};
use crate::edit::{Edit, EditError, History};
use indicatrix::geometry::{meet_solver::MeetConstraint, stone_metrics::SolidStatus};

/// A multi-step edit sequence, undone all the way back and redone all
/// the way forward, must reproduce the same design state at every step
/// -- not just the endpoints.
#[test]
fn multi_step_undo_redo_round_trips_at_every_step() {
    let mut design = fresh_design();
    let mut history = History::new();
    let mut snapshots = vec![design.clone()];

    for edit in [
        Edit::AddTier {
            index: 0,
            tier: tier("T", 0.0, MeetConstraint::ScaleReference(0.32), &[]),
        },
        Edit::AddTier {
            index: 1,
            tier: tier(
                "P1",
                -41.0,
                MeetConstraint::ScaleReference(0.65),
                &[0.0, 24.0, 48.0, 72.0],
            ),
        },
        Edit::ModifyTier {
            index: 0,
            tier: tier("T", 0.0, MeetConstraint::ScaleReference(0.30), &[]),
        },
    ] {
        history.apply(&mut design, edit).expect("edit must apply");
        snapshots.push(design.clone());
    }

    for snapshot in snapshots.iter().rev().skip(1) {
        assert!(history.undo(&mut design).unwrap());
        assert_eq!(&design, snapshot);
    }
    assert!(!history.can_undo());

    for snapshot in snapshots.iter().skip(1) {
        assert!(history.redo(&mut design).unwrap());
        assert_eq!(&design, snapshot);
    }
    assert!(!history.can_redo());
}

#[test]
fn out_of_range_index_is_rejected_without_mutating_the_design() {
    let mut design = fresh_design();
    let before = design.clone();
    let err = design
        .apply_edit(Edit::RemoveTier { index: 5 })
        .expect_err("must reject an out-of-range index");
    assert_eq!(
        err,
        EditError {
            index: 5,
            tier_count: 0
        }
    );
    assert_eq!(design, before);
}

/// A new edit after an undo must discard the abandoned redo branch --
/// the standard editor rule, checked here because a stale redo entry
/// would otherwise replay against tier positions that no longer mean
/// what they meant when it was recorded.
#[test]
fn a_new_edit_after_undo_clears_the_redo_stack() {
    let mut design = fresh_design();
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("A", 0.0, MeetConstraint::ScaleReference(0.3), &[]),
            },
        )
        .expect("add must apply");
    history.undo(&mut design).unwrap();
    assert!(history.can_redo());

    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("B", 0.0, MeetConstraint::ScaleReference(0.3), &[]),
            },
        )
        .expect("add must apply");
    assert!(!history.can_redo());
}

/// Edits that pinch the solid to zero volume must surface as
/// `Degenerate` via `Design::status`, and undoing the pinch must close
/// it again -- edit/undo and validation status composing the way an
/// editor actually needs them to.
///
/// A `Design`'s arrangement is always `preform.planes() ++ facet
/// planes`, and the preform alone already bounds a bunch of space around
/// the origin (see `crate::preform`'s tests) -- so in practice
/// `Unbounded` is essentially unreachable through `Design::status` as
/// long as the preform stays sane; the preform always caps every
/// direction, so a schedule missing a closing facet just shows more of
/// the rough there instead of leaving the arrangement open. `Degenerate`
/// (zero/negative volume, or too few vertices) is the status an editor
/// actually needs to watch for once a preform is in play, which is what
/// this test exercises: two zero-mast facets on opposite sides pinch the
/// preform's own vertical extent down to a single plane.
#[test]
fn validation_status_tracks_edits_through_undo() {
    let mut design = fresh_design();
    assert!(
        design.is_closed(),
        "a fresh design (preform alone) must close"
    );

    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("T", 0.0, MeetConstraint::ScaleReference(0.0), &[]),
            },
        )
        .expect("add must apply");
    assert!(
        design.is_closed(),
        "a single mast-0.0 table facet only halves the preform's own height, not to zero"
    );

    history
        .apply(
            &mut design,
            Edit::AddTier {
                // Negative zero forces the pavilion (not crown) side --
                // see `StandardGemCuts::from_asc_schedule`'s doc comment
                // on the zero-angle sign convention.
                index: 1,
                tier: tier("C", -0.0, MeetConstraint::ScaleReference(0.0), &[]),
            },
        )
        .expect("add must apply");
    match design
        .status()
        .expect("both tiers are anchored, must solve")
    {
        SolidStatus::Degenerate { volume, .. } => {
            assert_eq!(volume, Some(0.0));
        }
        other => panic!(
            "expected Degenerate once both zero-mast facets pinch the solid flat, got {other:?}"
        ),
    }

    assert!(history.undo(&mut design).unwrap());
    assert!(design.is_closed(), "undo must restore closure");
}
