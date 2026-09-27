//! Per-tier annotation edits (`SetCheaterOffset`, `SetTierNote`) and how they
//! renumber/relocate alongside `AddTier`/`RemoveTier`/`MoveTier`.

use super::fixtures::{fresh_design, tier};
use crate::edit::{Edit, History};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// `SetCheaterOffset` then undo must set/restore exactly one tier's own
/// offset, leaving every other tier's `None` untouched, and clearing it
/// (`offset_deg: None`) must undo back to a real previous value when one was
/// set.
#[test]
fn set_cheater_offset_then_undo_restores_the_previous_value() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("A", 10.0, MeetConstraint::ScaleReference(0.5), &[1.0]));
    design
        .tiers
        .push(tier("B", 20.0, MeetConstraint::MeetExisting, &[2.0]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetCheaterOffset {
                index: 1,
                offset_deg: Some(2.5),
            },
        )
        .expect("set cheater offset must apply");
    assert_eq!(design.cheater_offset_deg(0), None);
    assert_eq!(design.cheater_offset_deg(1), Some(2.5));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(design.cheater_offset_deg(1), None);

    // Clearing a real value, then undoing, must restore it.
    history
        .apply(
            &mut design,
            Edit::SetCheaterOffset {
                index: 1,
                offset_deg: Some(2.5),
            },
        )
        .expect("set cheater offset must apply");
    history
        .apply(
            &mut design,
            Edit::SetCheaterOffset {
                index: 1,
                offset_deg: None,
            },
        )
        .expect("clear cheater offset must apply");
    assert_eq!(design.cheater_offset_deg(1), None);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.cheater_offset_deg(1), Some(2.5));
}

/// `AddTier` before a tier with a recorded cheater offset must shift that
/// offset's key along with the tier itself, and `RemoveTier` on the OFFSET
/// tier must both remove the offset and restore it on undo (a `Batch` inverse
/// under the hood -- see `Design::apply_edit`'s own `RemoveTier` arm).
#[test]
fn add_and_remove_tier_renumber_cheater_offsets() {
    let mut design = fresh_design();
    for name in ["A", "B", "C"] {
        design
            .tiers
            .push(tier(name, 0.0, MeetConstraint::MeetExisting, &[]));
    }
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetCheaterOffset {
                index: 1, // "B"
                offset_deg: Some(4.0),
            },
        )
        .expect("set cheater offset must apply");
    let after_set = design.clone();

    // Insert a new tier before "A": "B"'s offset (and "B" itself) must both
    // move from index 1 to index 2.
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("Z", 0.0, MeetConstraint::MeetExisting, &[]),
            },
        )
        .expect("add must apply");
    assert_eq!(design.tiers[2].name, "B");
    assert_eq!(design.cheater_offset_deg(1), None);
    assert_eq!(design.cheater_offset_deg(2), Some(4.0));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, after_set);
    assert_eq!(design.cheater_offset_deg(1), Some(4.0));

    // Removing "B" itself (the offset tier) must drop the offset, and
    // undoing that removal must restore both the tier AND its own offset.
    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("remove must apply");
    assert_eq!(design.cheater_offset_deg(1), None);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, after_set);
    assert_eq!(design.cheater_offset_deg(1), Some(4.0));
}

/// `MoveTier` must relocate the moved tier's OWN cheater offset along with
/// it, not just shift everyone else's -- and undo (the exact inverse move)
/// must put it back.
#[test]
fn move_tier_relocates_its_own_cheater_offset() {
    let mut design = fresh_design();
    for name in ["A", "B", "C", "D"] {
        design
            .tiers
            .push(tier(name, 0.0, MeetConstraint::MeetExisting, &[]));
    }
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetCheaterOffset {
                index: 1, // "B"
                offset_deg: Some(3.0),
            },
        )
        .expect("set cheater offset must apply");
    let after_set = design.clone();

    // Move "B" (index 1) to the end (index 3): "C"/"D" shift down to fill the
    // gap, and "B"'s own offset must follow it to index 3.
    history
        .apply(&mut design, Edit::MoveTier { from: 1, to: 3 })
        .expect("move must apply");
    assert_eq!(
        design
            .tiers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "C", "D", "B"]
    );
    assert_eq!(design.cheater_offset_deg(1), None);
    assert_eq!(design.cheater_offset_deg(3), Some(3.0));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, after_set);
    assert_eq!(design.cheater_offset_deg(1), Some(3.0));
}

/// `SetTierNote` then undo must set/restore exactly one tier's own note,
/// leaving every other tier's `None` untouched, and clearing it (`note: None`)
/// must undo back to a real previous value when one was set -- the same
/// contract `set_cheater_offset_then_undo_restores_the_previous_value` already
/// covers for `SetCheaterOffset`.
#[test]
fn set_tier_note_then_undo_restores_the_previous_value() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("A", 10.0, MeetConstraint::ScaleReference(0.5), &[1.0]));
    design
        .tiers
        .push(tier("B", 20.0, MeetConstraint::MeetExisting, &[2.0]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1,
                note: Some("check meet here".to_string()),
            },
        )
        .expect("set tier note must apply");
    assert_eq!(design.tier_note(0), None);
    assert_eq!(design.tier_note(1), Some("check meet here"));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(design.tier_note(1), None);

    // Clearing a real value, then undoing, must restore it.
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1,
                note: Some("check meet here".to_string()),
            },
        )
        .expect("set tier note must apply");
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1,
                note: None,
            },
        )
        .expect("clear tier note must apply");
    assert_eq!(design.tier_note(1), None);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tier_note(1), Some("check meet here"));
}

/// `AddTier` before a tier with a recorded note must shift that note's key
/// along with the tier itself, and `RemoveTier` on the NOTE tier must both
/// remove the note and restore it on undo (a `Batch` inverse under the hood --
/// see `Design::apply_edit`'s own `RemoveTier` arm). Mirrors
/// `add_and_remove_tier_renumber_cheater_offsets` for `tier_notes`.
#[test]
fn add_and_remove_tier_renumber_tier_notes() {
    let mut design = fresh_design();
    for name in ["A", "B", "C"] {
        design
            .tiers
            .push(tier(name, 0.0, MeetConstraint::MeetExisting, &[]));
    }
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1, // "B"
                note: Some("grind slowly".to_string()),
            },
        )
        .expect("set tier note must apply");
    let after_set = design.clone();

    // Insert a new tier before "A": "B"'s note (and "B" itself) must both move
    // from index 1 to index 2.
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("Z", 0.0, MeetConstraint::MeetExisting, &[]),
            },
        )
        .expect("add must apply");
    assert_eq!(design.tiers[2].name, "B");
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(2), Some("grind slowly"));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, after_set);
    assert_eq!(design.tier_note(1), Some("grind slowly"));

    // Removing "B" itself (the note tier) must drop the note, and undoing that
    // removal must restore both the tier AND its own note.
    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("remove must apply");
    assert_eq!(design.tier_note(1), None);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, after_set);
    assert_eq!(design.tier_note(1), Some("grind slowly"));
}

/// A tier carrying BOTH a cheater offset AND a note, removed then undone, must
/// restore both in one undo step -- exercising `apply_remove_tier`'s
/// multi-step `Edit::Batch` inverse with more than one restored annotation at
/// once.
#[test]
fn remove_tier_undo_restores_both_cheater_offset_and_note_together() {
    let mut design = fresh_design();
    for name in ["A", "B", "C"] {
        design
            .tiers
            .push(tier(name, 0.0, MeetConstraint::MeetExisting, &[]));
    }
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetCheaterOffset {
                index: 1, // "B"
                offset_deg: Some(4.0),
            },
        )
        .expect("set cheater offset must apply");
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1, // "B"
                note: Some("grind slowly".to_string()),
            },
        )
        .expect("set tier note must apply");
    let after_set = design.clone();

    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("remove must apply");
    assert_eq!(design.cheater_offset_deg(1), None);
    assert_eq!(design.tier_note(1), None);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, after_set);
    assert_eq!(design.cheater_offset_deg(1), Some(4.0));
    assert_eq!(design.tier_note(1), Some("grind slowly"));
}

/// `MoveTier` must relocate the moved tier's OWN note along with it, not just
/// shift everyone else's -- and undo (the exact inverse move) must put it
/// back. Mirrors `move_tier_relocates_its_own_cheater_offset` for
/// `tier_notes`.
#[test]
fn move_tier_relocates_its_own_note() {
    let mut design = fresh_design();
    for name in ["A", "B", "C", "D"] {
        design
            .tiers
            .push(tier(name, 0.0, MeetConstraint::MeetExisting, &[]));
    }
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1, // "B"
                note: Some("check meet here".to_string()),
            },
        )
        .expect("set tier note must apply");
    let after_set = design.clone();

    // Move "B" (index 1) to the end (index 3): "C"/"D" shift down to fill the
    // gap, and "B"'s own note must follow it to index 3.
    history
        .apply(&mut design, Edit::MoveTier { from: 1, to: 3 })
        .expect("move must apply");
    assert_eq!(
        design
            .tiers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "C", "D", "B"]
    );
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(3), Some("check meet here"));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, after_set);
    assert_eq!(design.tier_note(1), Some("check meet here"));
}
