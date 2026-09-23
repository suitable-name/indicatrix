use super::*;
use crate::{
    design::{ConstraintTier, Design},
    material::MaterialSelection,
    preform::PreformSpec,
};
use indicatrix::geometry::{meet_solver::MeetConstraint, stone_metrics::SolidStatus};
use std::time::{Duration, Instant};

fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint, indices: &[f64]) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn fresh_design() -> Design {
    Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62)
}

#[test]
fn add_tier_then_undo_round_trips() {
    let mut design = fresh_design();
    let mut history = History::new();
    let before = design.clone();

    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("T", 0.0, MeetConstraint::ScaleReference(0.32), &[]),
            },
        )
        .expect("add must apply");
    assert_eq!(design.tiers.len(), 1);
    assert!(history.can_undo());
    assert!(!history.can_redo());

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(!history.can_undo());
    assert!(history.can_redo());

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.tiers.len(), 1);
    assert!(!history.can_redo());
}

#[test]
fn remove_tier_then_undo_restores_it_at_the_same_position() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("A", 10.0, MeetConstraint::ScaleReference(0.5), &[1.0]));
    design
        .tiers
        .push(tier("B", 20.0, MeetConstraint::MeetExisting, &[2.0]));
    design
        .tiers
        .push(tier("C", 30.0, MeetConstraint::MeetExisting, &[3.0]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("remove must apply");
    assert_eq!(
        design
            .tiers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "C"]
    );

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(
        design
            .tiers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B", "C"]
    );
}

#[test]
fn modify_tier_then_undo_restores_the_old_values() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("T", 0.0, MeetConstraint::ScaleReference(0.30), &[]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("T", 0.0, MeetConstraint::ScaleReference(0.45), &[]),
            },
        )
        .expect("modify must apply");
    assert_eq!(
        design.tiers[0].constraint,
        MeetConstraint::ScaleReference(0.45)
    );

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(
        design.tiers[0].constraint,
        MeetConstraint::ScaleReference(0.30)
    );
}

/// `SetConstraint` changes only the constraint, leaving angle/name/indices
/// alone -- the property that distinguishes it from `ModifyTier`.
#[test]
fn set_constraint_then_undo_touches_only_the_constraint_field() {
    let mut design = fresh_design();
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0],
    ));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetConstraint {
                index: 0,
                constraint: MeetConstraint::MeetNamed(vec!["G1".to_string()]),
            },
        )
        .expect("set constraint must apply");
    assert_eq!(
        design.tiers[0].constraint,
        MeetConstraint::MeetNamed(vec!["G1".to_string()])
    );
    assert_eq!(design.tiers[0].angle_deg, -41.0);
    assert_eq!(design.tiers[0].name, "P1");
    assert_eq!(design.tiers[0].indices, vec![0.0, 24.0]);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

#[test]
fn set_preform_then_undo_restores_the_old_preform() {
    let mut design = fresh_design();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetPreform {
                preform: PreformSpec::cylinder(96, 1.2, 1.0, 0.9),
            },
        )
        .expect("set preform must apply");
    assert_ne!(design.preform, before.preform);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

/// `SetGirdleDiameterMm` then undo must restore the previous scale
/// anchor (`None`, for a fresh design) exactly, the same round-trip
/// `set_preform_then_undo_restores_the_old_preform` above already checks for
/// `SetPreform`.
#[test]
fn set_girdle_diameter_mm_then_undo_restores_the_old_value() {
    let mut design = fresh_design();
    assert_eq!(design.girdle_diameter_mm, None);
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(8.2),
            },
        )
        .expect("set girdle diameter must apply");
    assert_eq!(design.girdle_diameter_mm, Some(8.2));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(design.girdle_diameter_mm, None);
}

/// `SetMaterial` then undo must restore the previous material
/// selection wholesale.
#[test]
fn set_material_then_undo_restores_the_old_selection() {
    let mut design = fresh_design();
    assert_eq!(design.material, MaterialSelection::none());
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetMaterial {
                material: MaterialSelection {
                    name: Some("Diamond".to_string()),
                    specific_gravity_override: None,
                    refractive_index_override: None,
                },
            },
        )
        .expect("set material must apply");
    assert_eq!(design.material.name.as_deref(), Some("Diamond"));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

/// `SetMeta` then undo must restore the previous headers/footnotes/gear
/// reference angle wholesale, and applying it must not force a re-solve --
/// none of those three fields feed `solve_meet_points`.
#[test]
fn set_meta_then_undo_restores_the_old_value() {
    let mut design = fresh_design();
    assert_eq!(design.meta.headers, Vec::<String>::new());
    assert_eq!(design.meta.footnotes, Vec::<String>::new());
    assert_eq!(design.meta.gear_reference_angle, 0.0);
    let before = design.clone();
    let before_solved = design.solve().expect("fresh design must solve");
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetMeta {
                headers: vec!["My Design".to_string()],
                footnotes: vec!["Cut for a client".to_string()],
                gear_reference_angle: 1.5,
            },
        )
        .expect("set meta must apply");
    assert_eq!(design.meta.headers, vec!["My Design".to_string()]);
    assert_eq!(design.meta.footnotes, vec!["Cut for a client".to_string()]);
    assert_eq!(design.meta.gear_reference_angle, 1.5);
    // Purely metadata: the solved masts are untouched by the edit.
    let after_solved = design.solve().expect("design must still solve");
    let before_masts: Vec<f64> = before_solved.iter().map(|t| t.mast).collect();
    let after_masts: Vec<f64> = after_solved.iter().map(|t| t.mast).collect();
    assert_eq!(before_masts, after_masts);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

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

// --- RetargetAngles ---

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

// --- apply_coalescing ---

#[test]
fn apply_coalescing_merges_consecutive_calls_with_the_same_key_into_one_undo_step() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(100),
        )
        .expect("second nudge must apply");
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.3, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(200),
        )
        .expect("third nudge must apply");
    assert_eq!(design.tiers[0].angle_deg, -40.3);

    // Three coalesced calls, one undo step: this must revert all the way back to
    // BEFORE the first nudge, not just undo the third.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(!history.can_undo());
}

#[test]
fn apply_coalescing_starts_a_new_undo_step_once_the_window_elapses() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    // Past the 500ms window -- must NOT merge with the call above.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(600),
        )
        .expect("second nudge must apply");

    // Two separate undo steps: the first undo only reverts the second nudge.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.1);
    assert!(history.can_undo());
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.0);
    assert!(!history.can_undo());
}

#[test]
fn apply_coalescing_with_a_different_key_starts_a_new_undo_step() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    design
        .tiers
        .push(tier("P2", -35.0, MeetConstraint::ScaleReference(0.6), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first tier's nudge must apply");
    // A different tier's nudge (different key), well within the window -- must still
    // get its own undo step rather than merging with tier 0's.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 1,
                tier: tier("P2", -35.1, MeetConstraint::ScaleReference(0.6), &[]),
            },
            2,
            t0 + Duration::from_millis(50),
        )
        .expect("second tier's nudge must apply");

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[1].angle_deg, -35.0);
    assert_eq!(
        design.tiers[0].angle_deg, -40.1,
        "tier 0's nudge must still stand"
    );
    assert!(history.can_undo());
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.0);
}

#[test]
fn an_ordinary_apply_between_two_coalescing_calls_breaks_the_merge() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("nudge must apply");
    // An ordinary apply (e.g. a Save Tier click) in between, same design/instant.
    history
        .apply(
            &mut design,
            Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(8.0),
            },
        )
        .expect("ordinary apply must succeed");
    // A same-key coalescing call right after must NOT merge across the apply above.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(10),
        )
        .expect("nudge after an ordinary apply must apply");

    // Three real undo steps: the second nudge, the girdle-diameter change, then the
    // first nudge -- none of them merged together.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.1);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.girdle_diameter_mm, None);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.0);
    assert!(!history.can_undo());
}

/// [`History::with_coalesce_window`] lets a caller pick a
/// window other than the crate-wide 500ms default -- here, one short enough that
/// two calls 100ms apart (which the default-window test above merges) do NOT
/// merge.
#[test]
fn with_coalesce_window_uses_the_given_window_instead_of_the_default() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::with_coalesce_window(Duration::from_millis(50));
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    // 100ms > this history's own 50ms window -- must NOT merge.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(100),
        )
        .expect("second nudge must apply");

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design.tiers[0].angle_deg, -40.1,
        "only the second nudge undoes"
    );
    assert!(history.can_undo());
}

/// [`History::end_coalesce_run`] ends a coalescing run
/// explicitly, so a call with the SAME key that would otherwise merge (well
/// inside the window) starts a fresh undo step instead once a caller has named
/// its own interaction boundary (e.g. pointer release).
#[test]
fn end_coalesce_run_stops_the_next_same_key_call_from_merging() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    history.end_coalesce_run();
    // Same key, well inside the 500ms window -- would merge without the explicit
    // end_coalesce_run above.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(10),
        )
        .expect("second nudge must apply");

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design.tiers[0].angle_deg, -40.1,
        "only the second nudge undoes"
    );
    assert!(history.can_undo());
}

// --- property test: edits apply and invert exactly over generated tier lists ---

/// A tiny deterministic splitmix64-seeded generator for a handful of synthetic
/// tiers -- the same generator shape `crate::optimize::search`'s own
/// `seeded_permutation` uses internally (see that module's "Determinism"
/// section), reimplemented locally here since that one is private to a
/// different module and this crate's own rules forbid a non-deterministic
/// (`HashMap`/`HashSet`/wall-clock-seeded) alternative.
fn generate_tiers(seed: u64) -> Vec<ConstraintTier> {
    let mut state = seed;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let tier_count = 2 + (next() % 3) as usize; // 2..=4 tiers
    (0..tier_count)
        .map(|i| {
            let angle = -10.0 - (next() % 60) as f64; // pavilion, -10..-69
            let index_count = 1 + (next() % 4) as usize;
            let indices: Vec<f64> = (0..index_count).map(|_| (next() % 96) as f64).collect();
            tier(
                &format!("T{i}"),
                angle,
                MeetConstraint::ScaleReference((i as f64).mul_add(0.05, 0.3)),
                &indices,
            )
        })
        .collect()
}

/// Over several generated tier lists (deterministic seeds, no `HashMap`/`HashSet`
/// anywhere -- see this crate's own determinism contract), `SetSchedule`,
/// `RemapIndices` (and its `RestoreIndices` inverse) and `RetargetAngles` all apply
/// and undo back to byte-identical design state, and redo reproduces the
/// fully-edited state exactly.
#[test]
fn new_a2_edit_variants_apply_and_invert_exactly_over_generated_tier_lists() {
    for seed in [1u64, 2, 3, 4, 5] {
        let mut design = fresh_design();
        for generated in generate_tiers(seed) {
            design.tiers.push(generated);
        }
        let before = design.clone();

        let mut history = History::new();
        history
            .apply(
                &mut design,
                Edit::SetSchedule {
                    gear_teeth: 80,
                    symmetry_order: 8,
                    mirror: seed % 2 == 0,
                },
            )
            .expect("set schedule must apply");
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
        let retarget_changes: Vec<(usize, f64, f64)> = design
            .tiers
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.angle_deg, t.angle_deg - 1.5))
            .collect();
        history
            .apply(
                &mut design,
                Edit::RetargetAngles {
                    changes: retarget_changes,
                },
            )
            .expect("retarget must apply");
        let after_edits = design.clone();

        assert_ne!(
            design, before,
            "seed {seed}: the edits above must have changed something"
        );

        while history.undo(&mut design).unwrap() {}
        assert_eq!(
            design, before,
            "seed {seed}: undoing all the way back must restore the original design exactly"
        );

        while history.redo(&mut design).unwrap() {}
        assert_eq!(
            design, after_edits,
            "seed {seed}: redoing all the way forward must reproduce the fully-edited design exactly"
        );
    }
}

// --- Edit::MoveTier ---

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

// --- Edit::Batch ---

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
