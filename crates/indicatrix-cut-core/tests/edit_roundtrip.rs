//! Property tests for the edit vocabulary: every [`Edit`] variant, applied
//! through [`Design::apply_edit`] and then undone by applying the [`Edit`] it
//! returns, must reproduce the exact original [`Design`] -- `#[derive(PartialEq)]`
//! on [`Design`] itself is the oracle, so there is nothing to hand-roll per
//! field.
//!
//! An integration test (`tests/`), not a unit test inside the crate: every
//! type and function used here is `indicatrix_cut_core`'s own public API, the
//! same surface `apps/indicatrix-cut` itself is limited to -- see the crate's
//! own doc comment. `crates/indicatrix-cut-core/src/edit/tests.rs` already
//! covers `AddTier`/`ModifyTier`/`MoveTier`/`RemoveTier`/`SetConstraint`/
//! `SetGirdleDiameterMm`/`SetMaterial`/`SetMeta`/`SetPreform`/`SetSchedule`/
//! `SetCheaterOffset`/`SetTierNote`/`RemapIndices`/`RestoreIndices`/
//! `RetargetAngles`/`Batch` with hand-picked cases (grepped before writing this
//! file, per this item's own instruction) -- this file is NOT a duplicate of
//! that coverage: it drives every variant, including the two this item names
//! by name (`RemapIndices`/`RetargetAngles`) and the two with no dedicated
//! round-trip test anywhere in the crate (`SetIndices`/`SetPreformYOffset`/
//! `SetTierTarget`), through a small deterministic generator over a fixed
//! multi-tier fixture, at several seeds each, so a regression that only shows
//! up for a PARTICULAR combination of indices/values has more chances to be
//! caught than one hand-picked example gives it.
//!
//! No `proptest` dependency: not in this workspace already (checked
//! `Cargo.toml`/`Cargo.lock` before writing this), and the task's own
//! instruction allows a seeded LCG instead. [`Lcg`] below is that generator --
//! deterministic (a fixed seed reproduces the exact same sequence every run,
//! so a failure is always reproducible without needing a printed seed) and
//! dependency-free.

use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, History, MaterialSelection, PreformSpec, RemapRounding,
    TierTarget,
    design::{ConcaveTier, ConcaveTool, RelationExpr, TierRelation, ToolMotion},
};

/// A tiny, dependency-free seeded PRNG (xorshift64*) -- enough to drive this
/// file's own generators deterministically. Not cryptographic, not a general
/// utility: local to this file, matching the task's own "a seeded LCG ... is
/// fine" allowance (xorshift64* has better output distribution than a bare LCG
/// for the same one-line implementation cost, but plays the identical role
/// here: a deterministic, seedable stream of numbers).
struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        // `0` would stay `0` forever under xorshift -- never a seed this file
        // actually passes, but folded in defensively so a future caller can't
        // silently get a degenerate all-zero stream.
        Self(seed | 1)
    }

    const fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A `usize` in `0..bound`. `bound == 0` always returns `0` (the caller's
    /// own responsibility to skip an empty range beforehand).
    const fn index(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }

    /// A finite `f64` in `[lo, hi)`.
    fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        unit.mul_add(hi - lo, lo)
    }

    const fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }
}

/// A small "round brilliant"-shaped fixture: one girdle tier (the block
/// anchor) plus two pavilion and two crown tiers, each independently
/// scale-referenced (every tier `Design::from_asc_schedule` imports pins to
/// `MeetConstraint::ScaleReference` regardless of the file's own `G` field --
/// see that method's own doc comment) so every block already solves and no
/// generated edit needs to special-case a `MissingAnchor` design. Five tiers
/// (not one or two) so `MoveTier`/`RemapIndices`/`RetargetAngles`'s
/// multi-tier behaviour has real interior positions to exercise, not just
/// edge cases.
fn round_brilliant_fixture() -> Design {
    const ASC: &str = "GemCad 5.0\n\
         g 16 0.0\n\
         y 4 n\n\
         I 1.54\n\
         a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
         a -42.000000 0.60000000 0 4 8 12 G Set stone size\n\
         a -38.000000 0.55000000 2 6 10 14 G Set stone size\n\
         a 32.000000 0.45000000 0 4 8 12 G Set stone size\n\
         a 40.000000 0.40000000 2 6 10 14 G Set stone size\n";
    let schedule = indicatrix_formats::asc::parse_asc(ASC).expect("fixture .asc must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.54),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design
}

/// Applies `edit`, then applies the [`Edit`] it returns (its own computed
/// inverse) -- asserting `design` ends up exactly equal (`PartialEq`) to how
/// it started, tier identities included ([`Design::tier_ids_eq`]). `label` is folded
/// into every panic message so a failing seed names which variant/seed
/// combination broke, not just "assertion failed."
fn assert_round_trips(mut design: Design, edit: &Edit, label: &str) {
    let before = design.clone();
    let inverse = design
        .apply_edit(edit.clone())
        .unwrap_or_else(|e| panic!("{label}: apply failed for {edit:?}: {e}"));
    // Every case this file drives is generated specifically to change
    // something -- a passing round trip on a no-op edit would never have
    // caught a broken apply (e.g. an inverse computed from the wrong field).
    assert_ne!(design, before, "{label}: edit was a no-op");
    design
        .apply_edit(inverse.clone())
        .unwrap_or_else(|e| panic!("{label}: undo failed for {inverse:?} (from {edit:?}): {e}"));
    assert_eq!(
        design, before,
        "{label}: {edit:?} then its own inverse {inverse:?} did not reproduce the original design"
    );
    assert!(
        design.tier_ids_eq(&before),
        "{label}: {edit:?} then its own inverse {inverse:?} changed which TierId sits in a slot"
    );
}

/// A fresh [`ConstraintTier`] for [`Edit::AddTier`]/[`Edit::ModifyTier`] --
/// varied per seed so a run touches different angles/indices, not the exact
/// same literal tier every time.
fn generated_tier(rng: &mut Lcg) -> ConstraintTier {
    let mast = rng.range_f64(0.2, 0.9);
    ConstraintTier {
        angle_deg: rng.range_f64(-60.0, 60.0),
        name: format!("G{}", rng.index(99)),
        indices: vec![f64::from(rng.index(16) as u32)],
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

const SEEDS: [u64; 5] = [1, 7, 42, 1_000_003, 0xDEAD_BEEF];

#[test]
fn add_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len() + 1); // insertion point, inclusive of the end
        let tier = generated_tier(&mut rng);
        assert_round_trips(design, &Edit::AddTier { index, tier }, "AddTier");
    }
}

#[test]
fn remove_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        assert_round_trips(design, &Edit::RemoveTier { index }, "RemoveTier");
    }
}

/// Also exercises the "removed tier carried a cheater offset/note" branch of
/// [`Edit::RemoveTier`]'s own inverse (a multi-step `Batch`, per
/// `Design::apply_edit`'s own doc comment on `apply_remove_tier`) on every
/// other seed, rather than only the plain single-`AddTier` inverse.
#[test]
fn remove_tier_with_cheater_offset_and_note_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        design
            .cheater_offsets_deg
            .insert(index, rng.range_f64(-5.0, 5.0));
        design
            .tier_notes
            .insert(index, "polish carefully".to_string());
        assert_round_trips(
            design,
            &Edit::RemoveTier { index },
            "RemoveTier (with offset+note)",
        );
    }
}

/// Undoing a removal brings back the removed tier's EXACT `TierId` (a plain
/// `AddTier` inverse would allocate a fresh one), with the target keyed by that
/// id attached to it again and every other tier's id untouched.
#[test]
fn undoing_a_tier_removal_restores_the_exact_tier_id() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let removed_id = design
            .tier_id_at(index)
            .expect("a design imported from .asc gives every tier an id");
        let target = TierTarget::DepthMm(1.25);
        design
            .apply_edit(Edit::SetTierTarget {
                index,
                target: Some(target),
            })
            .expect("the fixture has a tier at this index");
        let before = design.clone();

        let inverse = design
            .apply_edit(Edit::RemoveTier { index })
            .expect("the index is in range");
        assert_eq!(
            design.index_of_tier_id(removed_id),
            None,
            "seed {seed}: the removed id must leave the design"
        );
        design.apply_edit(inverse).expect("undo of a removal");

        assert_eq!(
            design.tier_id_at(index),
            Some(removed_id),
            "seed {seed}: undo must put the same TierId back in the same slot"
        );
        assert!(
            design.tier_ids_eq(&before),
            "seed {seed}: every other tier keeps its id"
        );
        assert_eq!(
            design.tier_target(index),
            Some(target),
            "seed {seed}: the target keyed by the id is attached again"
        );
    }
}

/// A tier pushed straight onto `tiers` has no `TierId` (so a target, which is keyed by
/// id, cannot be set on it); `ensure_tier_ids` gives it one without disturbing the
/// ids already there, and a second call changes nothing.
#[test]
fn ensure_tier_ids_seeds_missing_ids_and_keeps_existing_ones() {
    let mut design = round_brilliant_fixture();
    let existing: Vec<_> = (0..design.tiers.len())
        .map(|index| design.tier_id_at(index))
        .collect();
    let seeded = generated_tier(&mut Lcg::new(3));
    design.tiers.push(seeded);
    let new_index = design.tiers.len() - 1;
    assert_eq!(design.tier_id_at(new_index), None, "pushed without an id");
    assert!(
        design
            .apply_edit(Edit::SetTierTarget {
                index: new_index,
                target: Some(TierTarget::DepthMm(1.0)),
            })
            .is_err(),
        "a target needs the tier's id"
    );

    design.ensure_tier_ids();
    for (index, id) in existing.iter().enumerate() {
        assert_eq!(design.tier_id_at(index), *id, "tier {index} keeps its id");
    }
    let new_id = design
        .tier_id_at(new_index)
        .expect("the new tier has an id");
    assert!(
        existing.iter().all(|id| *id != Some(new_id)),
        "and it is not one already in use"
    );
    design
        .apply_edit(Edit::SetTierTarget {
            index: new_index,
            target: Some(TierTarget::DepthMm(1.0)),
        })
        .expect("the seeded tier can now carry a target");

    let ids_after: Vec<_> = (0..design.tiers.len())
        .map(|index| design.tier_id_at(index))
        .collect();
    design.ensure_tier_ids();
    let ids_again: Vec<_> = (0..design.tiers.len())
        .map(|index| design.tier_id_at(index))
        .collect();
    assert_eq!(ids_after, ids_again, "a second call is a no-op");
}

/// The same identity guarantee through `History`: undo, redo and a second undo of
/// a removal each leave the exact ids the matching point in time had.
#[test]
fn history_undo_and_redo_of_a_removal_keep_the_tier_ids() {
    let mut design = round_brilliant_fixture();
    let mut history = History::new();
    let before = design.clone();

    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("the index is in range");
    let after_removal = design.clone();

    assert!(history.undo(&mut design).expect("undo replays"));
    assert!(design.tier_ids_eq(&before), "undo restores the ids");
    assert!(history.redo(&mut design).expect("redo replays"));
    assert!(
        design.tier_ids_eq(&after_removal),
        "redo removes the same tier again"
    );
    assert!(history.undo(&mut design).expect("second undo replays"));
    assert!(
        design.tier_ids_eq(&before),
        "a second undo still restores the original ids"
    );
}

#[test]
fn move_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let tier_count = design.tiers.len();
        let from = rng.index(tier_count);
        // A move to itself is a no-op (`assert_round_trips` requires a real
        // change) -- resample until `to != from`, bounded by `tier_count`
        // itself so this can never loop forever on a design with only one tier.
        let mut to = rng.index(tier_count);
        for _ in 0..tier_count {
            if to != from {
                break;
            }
            to = rng.index(tier_count);
        }
        if to == from {
            continue; // exhausted the resample budget above; try the next seed.
        }
        assert_round_trips(design, &Edit::MoveTier { from, to }, "MoveTier");
    }
}

#[test]
fn modify_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let tier = generated_tier(&mut rng);
        assert_round_trips(design, &Edit::ModifyTier { index, tier }, "ModifyTier");
    }
}

#[test]
fn set_constraint_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let constraint = if rng.bool() {
            MeetConstraint::MeetExisting
        } else {
            MeetConstraint::ScaleReference(rng.range_f64(0.1, 1.0))
        };
        assert_round_trips(
            design,
            &Edit::SetConstraint { index, constraint },
            "SetConstraint",
        );
    }
}

/// [`Edit::SetIndices`] -- no dedicated round-trip test existed anywhere in
/// the crate before this file (grepped first, per this item's own
/// instruction: `edit/tests.rs` exercises `SetConstraint`/`SetCheaterOffset`/
/// `SetTierNote` but never `SetIndices`).
#[test]
fn set_indices_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let indices = vec![
            f64::from(rng.index(16) as u32),
            f64::from(rng.index(16) as u32),
        ];
        let detached = vec![f64::from(rng.index(16) as u32)];
        assert_round_trips(
            design,
            &Edit::SetIndices {
                index,
                indices,
                detached,
            },
            "SetIndices",
        );
    }
}

#[test]
fn set_preform_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let preform = PreformSpec::block(
            rng.range_f64(1.0, 3.0),
            rng.range_f64(0.5, 2.0),
            rng.range_f64(1.0, 3.0),
        );
        assert_round_trips(design, &Edit::SetPreform { preform }, "SetPreform");
    }
}

/// [`Edit::SetPreformYOffset`] -- no dedicated round-trip test existed
/// anywhere in the crate before this file (`native/tests.rs`'s
/// `preform_y_offset_round_trips_through_save_and_load` covers the NATIVE
/// FILE round trip, a different thing entirely from this edit's own
/// apply/undo).
#[test]
fn set_preform_y_offset_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let y_offset = rng.range_f64(-1.0, 1.0);
        assert_round_trips(
            design,
            &Edit::SetPreformYOffset { y_offset },
            "SetPreformYOffset",
        );
    }
}

#[test]
fn set_girdle_diameter_mm_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let girdle_diameter_mm = Some(rng.range_f64(3.0, 12.0));
        assert_round_trips(
            design,
            &Edit::SetGirdleDiameterMm { girdle_diameter_mm },
            "SetGirdleDiameterMm",
        );
    }
}

#[test]
fn set_material_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let material = MaterialSelection {
            name: Some(if rng.bool() { "Sapphire" } else { "Ruby" }.to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(rng.range_f64(1.5, 1.9)),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        };
        assert_round_trips(design, &Edit::SetMaterial { material }, "SetMaterial");
    }
}

#[test]
fn set_meta_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let edit = Edit::SetMeta {
            headers: vec![format!("Design #{}", rng.index(9999))],
            footnotes: vec!["cut by a property test".to_string()],
            gear_reference_angle: rng.range_f64(0.0, 22.5),
        };
        assert_round_trips(design, &edit, "SetMeta");
    }
}

#[test]
fn set_cheater_offset_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let offset_deg = Some(rng.range_f64(-5.0, 5.0));
        assert_round_trips(
            design,
            &Edit::SetCheaterOffset { index, offset_deg },
            "SetCheaterOffset",
        );
    }
}

#[test]
fn set_tier_note_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let note = Some(format!("note #{}", rng.index(9999)));
        assert_round_trips(design, &Edit::SetTierNote { index, note }, "SetTierNote");
    }
}

/// [`Edit::SetTierTarget`] -- no dedicated round-trip test existed anywhere in
/// the crate before this file.
#[test]
fn set_tier_target_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let target = Some(match rng.index(3) {
            0 => TierTarget::DepthMm(rng.range_f64(0.1, 2.0)),
            1 => TierTarget::GirdleThicknessMm(rng.range_f64(0.05, 0.5)),
            _ => TierTarget::TableWidthMm(rng.range_f64(1.0, 5.0)),
        });
        assert_round_trips(
            design,
            &Edit::SetTierTarget { index, target },
            "SetTierTarget",
        );
    }
}

/// A relation that makes the tier at `index` follow another tier of `design` by a
/// generated offset, for the [`Edit::SetTierRelation`] tests.
fn generated_relation(design: &Design, index: usize, rng: &mut Lcg) -> TierRelation {
    let count = design.tiers.len();
    // Always a DIFFERENT tier, so the relation never reads its own tier.
    let reader = (index + 1 + rng.index(count - 1)) % count;
    let reference = design
        .tier_id_at(reader)
        .expect("a design imported from .asc gives every tier an id");
    TierRelation::new(RelationExpr::offset_from(
        reference,
        rng.range_f64(-5.0, 5.0),
    ))
}

/// [`Edit::SetTierRelation`] -- setting a relation and undoing it.
#[test]
fn set_tier_relation_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let relation = Some(generated_relation(&design, index, &mut rng));
        assert_round_trips(
            design,
            &Edit::SetTierRelation { index, relation },
            "SetTierRelation",
        );
    }
}

/// Replacing a relation, and clearing one, each undo back to the previous relation.
#[test]
fn replacing_and_clearing_a_tier_relation_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let first = generated_relation(&design, index, &mut rng);
        design
            .apply_edit(Edit::SetTierRelation {
                index,
                relation: Some(first),
            })
            .expect("the index is in range");
        let second = generated_relation(&design, index, &mut rng);
        assert_round_trips(
            design.clone(),
            &Edit::SetTierRelation {
                index,
                relation: Some(second),
            },
            "SetTierRelation (replace)",
        );
        assert_round_trips(
            design,
            &Edit::SetTierRelation {
                index,
                relation: None,
            },
            "SetTierRelation (clear)",
        );
    }
}

/// Removing a tier that follows a relation takes the relation with it, and undoing
/// the removal puts both back -- the relation stays attached to the same `TierId`.
#[test]
fn removing_a_driven_tier_round_trips_with_its_relation() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let relation = generated_relation(&design, index, &mut rng);
        design
            .apply_edit(Edit::SetTierRelation {
                index,
                relation: Some(relation.clone()),
            })
            .expect("the index is in range");
        let driven_id = design.tier_id_at(index).expect("the tier has an id");

        let mut removed = design.clone();
        let inverse = removed
            .apply_edit(Edit::RemoveTier { index })
            .expect("the index is in range");
        assert!(
            removed.tier_relation_for_id(driven_id).is_none(),
            "seed {seed}: the removed tier's relation leaves with it"
        );
        removed.apply_edit(inverse).expect("undo of a removal");
        assert_eq!(removed, design, "seed {seed}: undo restores the design");
        assert_eq!(
            removed.tier_relation_for_id(driven_id),
            Some(&relation),
            "seed {seed}: the relation is attached to the same tier again"
        );
        assert_round_trips(design, &Edit::RemoveTier { index }, "RemoveTier (driven)");
    }
}

#[test]
fn set_schedule_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let edit = Edit::SetSchedule {
            gear_teeth: 8 + i32::try_from(rng.index(56)).unwrap_or(8),
            symmetry_order: 2 + u32::try_from(rng.index(6)).unwrap_or(2),
            mirror: rng.bool(),
        };
        assert_round_trips(design, &edit, "SetSchedule");
    }
}

/// This item's own named target #1: [`Edit::RemapIndices`]'s inverse is
/// [`Edit::RestoreIndices`] (a verbatim snapshot, not a reverse remap -- see
/// that variant's own doc comment for why a lossy ratio makes a reverse remap
/// unsound), so this is the one variant in this file where `edit` and its own
/// inverse are two DIFFERENT variants, not the same one played backward.
#[test]
fn remap_indices_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let from_gear = design.meta.gear_teeth;
        let to_gear = [8, 12, 20, 24, 32][rng.index(5)];
        let rounding = [
            RemapRounding::Nearest,
            RemapRounding::Floor,
            RemapRounding::Ceil,
        ][rng.index(3)];
        assert_round_trips(
            design,
            &Edit::RemapIndices {
                from_gear,
                to_gear,
                rounding,
            },
            "RemapIndices",
        );
    }
}

/// [`Edit::RestoreIndices`] played directly (not only reached via
/// `RemapIndices`'s own inverse above) -- its own apply/inverse pair is
/// symmetric (applying it twice, with the two `tiers` snapshots swapped,
/// round-trips), unlike `RemapIndices`.
#[test]
fn restore_indices_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let tiers = vec![(index, vec![f64::from(rng.index(16) as u32)], Vec::new())];
        assert_round_trips(design, &Edit::RestoreIndices { tiers }, "RestoreIndices");
    }
}

/// This item's own named target #2: [`Edit::RetargetAngles`], single- and
/// multi-tier.
#[test]
fn retarget_angles_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let old_deg = design.tiers[index].angle_deg;
        let new_deg = old_deg + rng.range_f64(1.0, 10.0);
        assert_round_trips(
            design,
            &Edit::RetargetAngles {
                changes: vec![(index, old_deg, new_deg)],
            },
            "RetargetAngles (single)",
        );
    }
}

#[test]
fn retarget_angles_multi_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        // Two DISTINCT indices -- the repeated-index case is covered separately
        // below; this is the ordinary multi-tier case.
        let first = rng.index(design.tiers.len());
        let second = (first + 1) % design.tiers.len();
        if first == second {
            continue; // a one-tier fixture: nothing else to retarget alongside it.
        }
        let changes = vec![
            (
                first,
                design.tiers[first].angle_deg,
                design.tiers[first].angle_deg + rng.range_f64(1.0, 5.0),
            ),
            (
                second,
                design.tiers[second].angle_deg,
                design.tiers[second].angle_deg - rng.range_f64(1.0, 5.0),
            ),
        ];
        assert_round_trips(
            design,
            &Edit::RetargetAngles { changes },
            "RetargetAngles (multi)",
        );
    }
}

/// The SAME tier index named twice in one `RetargetAngles` must still
/// round-trip -- the inverse has to unwind in the exact reverse order it was
/// applied in, not the order the caller wrote it in.
#[test]
fn retarget_angles_same_tier_twice_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let start = design.tiers[index].angle_deg;
        let mid = start + rng.range_f64(1.0, 5.0);
        let end = mid + rng.range_f64(1.0, 5.0);
        let changes = vec![(index, start, mid), (index, mid, end)];
        assert_round_trips(
            design,
            &Edit::RetargetAngles { changes },
            "RetargetAngles (repeated index)",
        );
    }
}

/// [`Edit::Batch`]: this crate's own only real-world composite (a retarget's
/// angle changes plus the material change it implies -- see
/// `Edit::describe`'s own `describe_batch` special case) exercised directly,
/// on top of `edit/tests.rs`'s own existing `Batch` coverage for the
/// error/rollback half.
#[test]
fn batch_retarget_and_material_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let old_deg = design.tiers[index].angle_deg;
        let new_deg = old_deg + rng.range_f64(1.0, 10.0);
        let edit = Edit::Batch(vec![
            Edit::RetargetAngles {
                changes: vec![(index, old_deg, new_deg)],
            },
            Edit::SetMaterial {
                material: MaterialSelection {
                    name: Some("Sapphire".to_string()),
                    specific_gravity_override: None,
                    refractive_index_override: None,
                    body_color_override: None,
                    body_color_bands_override: None,
                    absorption_path_scale_override: None,
                },
            },
        ]);
        assert_round_trips(design, &edit, "Batch(RetargetAngles, SetMaterial)");
    }
}

/// A concave tier for the concave edits -- varied per seed. Indices stay inside
/// the fixture's 16-tooth gear and the name never clashes with a flat tier
/// (`generated_tier` names those `G<n>`).
fn generated_concave_tier(rng: &mut Lcg) -> ConcaveTier {
    let count = 1 + rng.index(4);
    ConcaveTier {
        name: format!("Groove {}", rng.index(99)),
        angle_deg: rng.range_f64(-60.0, -10.0),
        indices: (0..count)
            .map(|_| f64::from(rng.index(16) as u32))
            .collect(),
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: rng.range_f64(0.0, 90.0),
        displacement: [0.0, 0.0, rng.range_f64(0.0, 0.2)],
        diameter_ratio: rng.range_f64(0.2, 0.8),
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

/// The round-brilliant fixture plus three concave tiers, added through the edit
/// vocabulary so they carry real ids.
fn concave_fixture(rng: &mut Lcg) -> Design {
    let mut design = round_brilliant_fixture();
    for position in 0..3 {
        let mut tier = generated_concave_tier(rng);
        tier.name = format!("Groove {position}");
        design
            .apply_edit(Edit::AddConcaveTier {
                index: position,
                tier,
            })
            .expect("a valid concave tier appends");
    }
    design
}

#[test]
fn add_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len() + 1);
        let tier = generated_concave_tier(&mut rng);
        assert_round_trips(
            design,
            &Edit::AddConcaveTier { index, tier },
            "AddConcaveTier",
        );
    }
}

#[test]
fn remove_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len());
        assert_round_trips(
            design,
            &Edit::RemoveConcaveTier { index },
            "RemoveConcaveTier",
        );
    }
}

#[test]
fn modify_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len());
        let mut tier = generated_concave_tier(&mut rng);
        tier.name = "Renamed".to_owned();
        assert_round_trips(
            design,
            &Edit::ModifyConcaveTier { index, tier },
            "ModifyConcaveTier",
        );
    }
}

#[test]
fn move_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let from = rng.index(design.concave_tiers.len());
        // Always a different slot, so the move is never a no-op.
        let to =
            (from + 1 + rng.index(design.concave_tiers.len() - 1)) % design.concave_tiers.len();
        assert_round_trips(
            design,
            &Edit::MoveConcaveTier { from, to },
            "MoveConcaveTier",
        );
    }
}

#[test]
fn undoing_a_concave_tier_removal_restores_the_exact_tier_id() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len());
        let ids_before = design.concave_tier_ids.clone();
        assert_eq!(ids_before.len(), design.concave_tiers.len());

        let inverse = design
            .apply_edit(Edit::RemoveConcaveTier { index })
            .expect("the index is in range");
        assert!(
            !design.concave_tier_ids.contains(&ids_before[index]),
            "seed {seed}: the removed id must leave the design"
        );
        design.apply_edit(inverse).expect("undo of a removal");

        assert_eq!(
            design.concave_tier_ids, ids_before,
            "seed {seed}: undo must put every concave TierId back in its slot"
        );
    }
}

#[test]
fn remap_indices_remaps_concave_indices_and_its_undo_restores_them_verbatim() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = concave_fixture(&mut rng);
        let before = design.clone();
        let from_gear = design.meta.gear_teeth;
        let to_gear = from_gear * 2;
        let inverse = design
            .apply_edit(Edit::RemapIndices {
                from_gear,
                to_gear,
                rounding: RemapRounding::Nearest,
            })
            .expect("a remap never fails");
        for (tier, original) in design.concave_tiers.iter().zip(&before.concave_tiers) {
            let doubled: Vec<f64> = original.indices.iter().map(|i| i * 2.0).collect();
            assert_eq!(
                tier.indices, doubled,
                "seed {seed}: concave indices follow the gear"
            );
        }
        assert!(
            matches!(&inverse, Edit::Batch(parts) if matches!(
                parts.as_slice(),
                [Edit::RestoreIndices { .. }, Edit::RestoreConcaveIndices { .. }]
            )),
            "seed {seed}: the inverse is Batch[RestoreIndices, RestoreConcaveIndices], got {inverse:?}"
        );
        design.apply_edit(inverse).expect("undo of a remap");
        assert_eq!(
            design, before,
            "seed {seed}: undo restores every index verbatim"
        );
    }
}

#[test]
fn remap_indices_inverse_is_unchanged_for_a_design_without_concave_tiers() {
    let mut design = round_brilliant_fixture();
    let inverse = design
        .apply_edit(Edit::RemapIndices {
            from_gear: design.meta.gear_teeth,
            to_gear: 32,
            rounding: RemapRounding::Nearest,
        })
        .expect("a remap never fails");
    assert!(
        matches!(inverse, Edit::RestoreIndices { .. }),
        "no concave tiers: the inverse stays the plain RestoreIndices, got {inverse:?}"
    );
}

#[test]
fn modify_concave_tier_rejects_an_invalid_tier_without_mutating() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = concave_fixture(&mut rng);
        // The fixture's flat tiers are unnamed, and empty names are exempt from
        // uniqueness, so give one a real name for the clash case to collide with.
        design.tiers[0].name = "Pavilion main".to_string();
        let before = design.clone();
        let index = rng.index(design.concave_tiers.len());

        let mut nan = generated_concave_tier(&mut rng);
        nan.diameter_ratio = f64::NAN;
        let mut out_of_gear = generated_concave_tier(&mut rng);
        out_of_gear.indices = vec![99.0];
        let mut clash = generated_concave_tier(&mut rng);
        clash.name = "Pavilion main".to_string();
        for bad in [nan, out_of_gear, clash] {
            assert!(
                design
                    .apply_edit(Edit::ModifyConcaveTier {
                        index,
                        tier: bad.clone()
                    })
                    .is_err(),
                "seed {seed}: {bad:?} must be rejected"
            );
            assert!(
                design
                    .apply_edit(Edit::AddConcaveTier { index, tier: bad })
                    .is_err()
            );
            assert_eq!(
                design, before,
                "seed {seed}: a rejected edit must not mutate"
            );
            assert_eq!(design.concave_tier_ids, before.concave_tier_ids);
        }
        let out_of_range = design.concave_tiers.len();
        assert!(
            design
                .apply_edit(Edit::RemoveConcaveTier {
                    index: out_of_range
                })
                .is_err()
        );
        assert_eq!(design, before);
    }
}
