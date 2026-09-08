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
