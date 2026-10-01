//! Property test: `SetSchedule`, `RemapIndices` (and its `RestoreIndices`
//! inverse) and `RetargetAngles` all apply and invert exactly over generated
//! tier lists.

use super::fixtures::{fresh_design, tier};
use crate::{
    design::ConstraintTier,
    edit::{Edit, History, RemapRounding},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

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
        // A snapshot after EACH edit (starting with `before` itself), so undo/redo
        // can be checked one step at a time below -- not just "fully undone
        // matches the start, fully redone matches the end", which a bug in an
        // INTERMEDIATE step could satisfy by coincidence (e.g. two wrong undos that
        // happen to cancel out over the whole sequence).
        let mut snapshots = vec![before.clone()];
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
        snapshots.push(design.clone());
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
        snapshots.push(design.clone());
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
        snapshots.push(design.clone());
        let after_edits = design.clone();

        assert_ne!(
            design, before,
            "seed {seed}: the edits above must have changed something"
        );

        // Undo one step at a time, checking each intermediate state against the
        // matching forward snapshot (`snapshots` in reverse, skipping `after_edits`
        // itself since `design` already IS that).
        for expected in snapshots.iter().rev().skip(1) {
            assert!(
                history.undo(&mut design).unwrap(),
                "seed {seed}: undo must still have a step left"
            );
            assert_eq!(
                design, *expected,
                "seed {seed}: one undo step did not restore the matching intermediate state"
            );
        }
        assert!(
            !history.undo(&mut design).unwrap(),
            "seed {seed}: nothing should be left to undo"
        );
        assert_eq!(
            design, before,
            "seed {seed}: undoing all the way back must restore the original design exactly"
        );

        // Redo one step at a time, same per-step check in the forward direction.
        for expected in snapshots.iter().skip(1) {
            assert!(
                history.redo(&mut design).unwrap(),
                "seed {seed}: redo must still have a step left"
            );
            assert_eq!(
                design, *expected,
                "seed {seed}: one redo step did not restore the matching intermediate state"
            );
        }
        assert!(
            !history.redo(&mut design).unwrap(),
            "seed {seed}: nothing should be left to redo"
        );
        assert_eq!(
            design, after_edits,
            "seed {seed}: redoing all the way forward must reproduce the fully-edited design exactly"
        );
    }
}
