//! The step list [`crate::edit::History`] keeps for a history panel: a label per step
//! that stays aligned through apply/undo/redo/new-apply/coalescing, [`History::entries`],
//! [`History::jump_to`] and [`History::design_at`].

use super::fixtures::{fresh_design, tier};
use crate::{
    design::Design,
    edit::{Edit, History, JumpError},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use std::time::Duration;

/// Three different edits: add a table, add a pavilion tier, then re-pin the table.
fn three_edits() -> [Edit; 3] {
    [
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
    ]
}

/// A history with the three edits applied, the design at the end, the labels the edits
/// were worded with (taken before each applied) and the design after every step
/// (`snapshots[0]` is the design before any).
fn three_steps() -> (Design, History, Vec<String>, Vec<Design>) {
    let mut design = fresh_design();
    let mut history = History::new();
    let mut labels = Vec::new();
    let mut snapshots = vec![design.clone()];
    for edit in three_edits() {
        labels.push(edit.describe(&design));
        history.apply(&mut design, edit).expect("edit must apply");
        snapshots.push(design.clone());
    }
    (design, history, labels, snapshots)
}

fn labels_of(history: &History) -> Vec<String> {
    history
        .entries()
        .into_iter()
        .map(|entry| entry.label)
        .collect()
}

#[test]
fn an_empty_history_has_no_entries_and_sits_at_position_zero() {
    let history = History::new();
    assert_eq!(history.entries(), Vec::new());
    assert_eq!(history.len_undo(), 0);
    assert_eq!(history.len_redo(), 0);
    assert_eq!(history.len_total(), 0);
}

#[test]
fn every_step_carries_the_words_of_its_forward_edit() {
    let (_, history, labels, _) = three_steps();
    let entries = history.entries();
    assert_eq!(labels_of(&history), labels);
    assert_eq!(
        entries.iter().map(|e| e.position).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "oldest first, counting from 1"
    );
    assert!(entries.iter().all(|entry| !entry.undone));
    assert_eq!(history.len_undo(), 3);
    assert_eq!(history.len_redo(), 0);
    assert_eq!(history.len_total(), 3);
    assert!(
        labels.iter().all(|label| !label.is_empty()),
        "a label is never empty"
    );
}

#[test]
fn labels_follow_their_steps_to_the_redo_side_and_back() {
    let (mut design, mut history, labels, _) = three_steps();
    let revisions: Vec<u64> = history.entries().iter().map(|e| e.revision).collect();

    history.undo(&mut design).unwrap();
    history.undo(&mut design).unwrap();
    assert_eq!(history.len_undo(), 1);
    assert_eq!(history.len_redo(), 2);
    let entries = history.entries();
    assert_eq!(
        labels_of(&history),
        labels,
        "undoing moves labels, it drops none"
    );
    assert_eq!(
        entries.iter().map(|e| e.undone).collect::<Vec<_>>(),
        vec![false, true, true]
    );
    assert_eq!(
        entries.iter().map(|e| e.revision).collect::<Vec<_>>(),
        revisions,
        "undoing does not change which design state a step names"
    );

    history.redo(&mut design).unwrap();
    assert_eq!(history.len_undo(), 2);
    assert_eq!(
        history
            .entries()
            .iter()
            .map(|e| e.undone)
            .collect::<Vec<_>>(),
        vec![false, false, true]
    );
    assert_eq!(labels_of(&history), labels);
}

#[test]
fn a_new_edit_after_undo_drops_the_undone_labels() {
    let (mut design, mut history, labels, _) = three_steps();
    history.undo(&mut design).unwrap();
    history.undo(&mut design).unwrap();

    let extra = Edit::AddTier {
        index: 1,
        tier: tier("P2", -35.0, MeetConstraint::ScaleReference(0.6), &[]),
    };
    let extra_label = extra.describe(&design);
    history.apply(&mut design, extra).expect("edit must apply");

    assert_eq!(history.len_redo(), 0, "the undone branch is abandoned");
    assert_eq!(labels_of(&history), vec![labels[0].clone(), extra_label]);
    assert!(history.entries().iter().all(|entry| !entry.undone));
}

#[test]
fn a_failed_edit_records_no_step() {
    let mut design = fresh_design();
    let mut history = History::new();
    assert!(
        history
            .apply(&mut design, Edit::RemoveTier { index: 3 })
            .is_err()
    );
    assert_eq!(history.entries(), Vec::new());
}

#[test]
fn merged_nudges_are_one_entry_worded_by_the_latest_change() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Duration::from_secs(10);
    let mut last_label = String::new();
    let mut revisions = Vec::new();
    for (n, angle) in [-40.1, -40.2, -40.3].into_iter().enumerate() {
        let edit = Edit::ModifyTier {
            index: 0,
            tier: tier("P1", angle, MeetConstraint::ScaleReference(0.5), &[]),
        };
        last_label = edit.describe(&design);
        history
            .apply_coalescing(
                &mut design,
                edit,
                7,
                t0 + Duration::from_millis(100 * n as u64),
            )
            .expect("nudge must apply");
        revisions.push(history.entries()[0].revision);
    }
    let entries = history.entries();
    assert_eq!(entries.len(), 1, "three nudges, one step");
    assert_eq!(entries[0].label, last_label);
    assert_eq!(history.len_undo(), 1);
    assert!(
        revisions[0] != revisions[1] && revisions[1] != revisions[2],
        "each merge changes the design at that position, so the revision moves: {revisions:?}"
    );
}

#[test]
fn a_nudge_after_the_window_is_a_second_entry() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Duration::from_secs(10);
    for (n, angle) in [-40.1, -40.2].into_iter().enumerate() {
        history
            .apply_coalescing(
                &mut design,
                Edit::ModifyTier {
                    index: 0,
                    tier: tier("P1", angle, MeetConstraint::ScaleReference(0.5), &[]),
                },
                7,
                t0 + Duration::from_millis(600 * n as u64),
            )
            .expect("nudge must apply");
    }
    assert_eq!(history.entries().len(), 2);
}

#[test]
fn every_step_gets_its_own_revision() {
    let (_, history, _, _) = three_steps();
    let mut revisions: Vec<u64> = history.entries().iter().map(|e| e.revision).collect();
    revisions.sort_unstable();
    revisions.dedup();
    assert_eq!(revisions.len(), 3);
}

#[test]
fn jump_to_reaches_every_position_with_the_design_of_that_step() {
    let (mut design, mut history, labels, snapshots) = three_steps();
    // Every route: from wherever the history is to every position, in a scrambled order.
    for target in [0, 3, 1, 2, 2, 0, 1, 3] {
        let before = history.len_undo();
        let moved = history
            .jump_to(&mut design, target)
            .expect("jump must succeed");
        assert_eq!(moved, before.abs_diff(target));
        assert_eq!(history.len_undo(), target);
        assert_eq!(history.len_total(), 3, "a jump never loses a step");
        assert_eq!(design, snapshots[target], "the design at position {target}");
        assert_eq!(labels_of(&history), labels, "labels survive the jump");
    }
}

#[test]
fn jumping_back_leaves_the_later_steps_to_redo() {
    let (mut design, mut history, _, snapshots) = three_steps();
    history.jump_to(&mut design, 1).unwrap();
    assert!(history.can_redo());
    assert_eq!(history.len_redo(), 2);
    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design, snapshots[2]);
}

#[test]
fn jumping_to_where_the_design_already_is_does_nothing() {
    let (mut design, mut history, _, snapshots) = three_steps();
    let before = history.clone();
    assert_eq!(history.jump_to(&mut design, 3), Ok(0));
    assert_eq!(history, before);
    assert_eq!(design, snapshots[3]);
}

#[test]
fn jumping_past_the_last_step_is_refused_before_anything_moves() {
    let (mut design, mut history, _, snapshots) = three_steps();
    let before = history.clone();
    assert_eq!(
        history.jump_to(&mut design, 4),
        Err(JumpError::OutOfRange {
            position: 4,
            steps: 3
        })
    );
    assert_eq!(history, before);
    assert_eq!(design, snapshots[3]);
    let message = history.jump_to(&mut design, 9).unwrap_err().to_string();
    assert!(message.contains("no step 9"), "{message}");
}

#[test]
fn a_failed_replay_leaves_the_design_at_the_last_good_step() {
    let (mut design, mut history, _, snapshots) = three_steps();
    // Something changes the design behind the history's back: one tier too few. Undoing the
    // re-pin (step 3, edits tier 0) still works, undoing the pavilion add (step 2, removes
    // tier 1) does not.
    design.tiers.truncate(1);
    let error = history
        .jump_to(&mut design, 0)
        .expect_err("the replay of step 2 must fail");
    assert!(
        matches!(error, JumpError::Replay { at: 2, .. }),
        "the design stands at position 2: {error:?}"
    );
    assert_eq!(history.len_undo(), 2, "the history agrees with the design");
    assert_eq!(history.len_redo(), 1);
    assert_eq!(design.tiers[0].constraint, snapshots[2].tiers[0].constraint);
    assert!(error.to_string().contains("step 2"), "{error}");
}

#[test]
fn design_at_matches_the_design_real_undos_and_redos_reach() {
    let (design, history, _, snapshots) = three_steps();
    for (position, snapshot) in snapshots.iter().enumerate() {
        let copy = history
            .design_at(&design, position)
            .expect("position is in range");
        assert_eq!(&copy, snapshot, "position {position}");
    }
    // From the middle of the history, in both directions.
    let (mut middle, mut history, _, snapshots) = three_steps();
    history.jump_to(&mut middle, 1).unwrap();
    let before = (middle.clone(), history.clone());
    for (position, snapshot) in snapshots.iter().enumerate() {
        let copy = history
            .design_at(&middle, position)
            .expect("position is in range");
        assert_eq!(&copy, snapshot, "position {position} from position 1");
    }
    assert_eq!((middle, history), before, "design_at changes neither input");
}

#[test]
fn design_at_refuses_a_position_past_the_last_step() {
    let (design, history, _, _) = three_steps();
    assert_eq!(
        history.design_at(&design, 4),
        Err(JumpError::OutOfRange {
            position: 4,
            steps: 3
        })
    );
}

#[test]
fn design_at_reports_a_step_that_cannot_be_replayed() {
    let (mut design, history, _, _) = three_steps();
    design.tiers.truncate(1);
    let error = history
        .design_at(&design, 0)
        .expect_err("step 2 cannot be taken back");
    assert!(
        matches!(error, JumpError::Replay { at: 2, .. }),
        "{error:?}"
    );
}
