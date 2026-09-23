//! Re-solve only what an edit could actually change, instead of
//! [`Design::solve`](crate::design::Design::solve)'s whole-design re-solve every time.
//!
//! A full re-solve costs 0.01 ms (2 tiers) to 5.9 s (a real 103-tier catalogue
//! design) non-monotonically -- cost tracks how badly `meet_solver`'s refinement
//! sweeps thrash, not tier count -- which is why `indicatrix-cut`'s Edit tab has an
//! explicit "Solve" button instead of solving on every keystroke.
//!
//! # No changes to the solver itself
//!
//! `solve_meet_points` takes every tier and returns every tier; there is no
//! subgraph-only entry point, and this module does not add one.
//! [`Design::resolve_dirty`](crate::design::Design::resolve_dirty) instead
//! temporarily replaces every tier [`affected_tiers`] does *not* mark as possibly
//! changed with [`MeetConstraint::ScaleReference`] at its last known mast, then calls
//! the ordinary `solve_meet_points` over the whole (partially substituted) list --
//! reusing the real solver rather than a second, subgraph-only implementation that
//! could drift out of sync with `meet_solver`'s tie-breaking rules.
//!
//! # Rejected: a per-tier dependency graph
//!
//! The natural approach -- `MeetNamed`/`MeetExisting` depend only on the tiers they
//! name or precede, affected = transitive closure from the edit -- is wrong:
//! `meet_solver::solve`'s phase 3 rebuilds the entire plane arrangement from every
//! non-anchor tier's *current* mast on every sweep, ignoring file order and named
//! references, so any non-anchor tier can move any other. Measured on real fixtures
//! ("Six Main Hilite LB", "RBC-445", "Briolette of India Replica"): editing a
//! design's last tier moved earlier tiers' masts by up to 3.8x10<sup>-2</sup>
//! relative, including edits crossing crown and pavilion entirely -- cases a
//! "preceding tiers only" rule marks as unaffected (silently wrong). Filtering by a
//! tier's previous `SolveStrategy` doesn't help either: every non-anchor tier in all
//! three fixtures reported `DependencyOrder`, yet phase 3 still moved several of them
//! regardless.
//!
//! # The one guarantee that holds
//!
//! `run_pipeline`'s `is_anchor[i]` guard is unconditional in every mutating loop in
//! `solve.rs`: a [`MeetConstraint::ScaleReference`] tier's mast is *never* written by
//! anything but its own authored value. So [`affected_tiers`] is exactly: the edited
//! tier(s), plus every tier whose current constraint is not `ScaleReference` -- no
//! named-reference resolution, no file-order edges, no strategy inspection, since
//! anything narrower risks the silent-stale-mast failure measured above.
//!
//! # What this buys
//!
//! Immediately after import (which pins every tier to `ScaleReference` -- see
//! [`Design::from_asc_schedule`](crate::design::Design::from_asc_schedule)), editing
//! one tier's angle affects only that tier. It buys nothing once a design has more
//! than one non-`ScaleReference` tier anywhere: [`affected_tiers`] then includes
//! essentially the whole meet-derived remainder regardless of which tier was edited
//! -- see `resolve_dirty_speed_on_a_large_real_design` (ignored by default) for the
//! measured extreme.

use crate::{
    design::{Design, DesignSolveError},
    edit::Edit,
};
use indicatrix::geometry::meet_solver::{MeetConstraint, MeetTierInput, SolvedTier};
use std::collections::BTreeSet;

/// Every tier index that could possibly need re-solving: `dirty` itself, plus every
/// tier whose CURRENT constraint is not [`MeetConstraint::ScaleReference`] -- see
/// this module's doc comment for why this is the provably-safe rule rather than a
/// narrower, edge-counting one.
///
/// `dirty` is included explicitly because the edited tier itself might BE a
/// `ScaleReference` whose own value changed (e.g. `Edit::SetConstraint`) --
/// otherwise invisible to the "not `ScaleReference`" filter.
pub(crate) fn affected_tiers(inputs: &[MeetTierInput], dirty: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut affected = dirty.clone();
    for (i, tier) in inputs.iter().enumerate() {
        if !matches!(tier.constraint, MeetConstraint::ScaleReference(_)) {
            affected.insert(i);
        }
    }
    affected
}

/// Re-solves a design after `edit` was applied to it, as cheaply as the edit
/// allows.
///
/// `apps/indicatrix-cut` does not call this function directly (grep: zero call
/// sites) -- it re-solves through its own `auto_solve` wrapper around
/// [`Design::resolve_dirty`]/[`Design::solve`] instead. This function is the
/// library-level building block that wrapper's reasoning is built from, and
/// this crate's own tests exercise it directly.
///
/// `design` is the design *after* `edit` was applied (via
/// [`crate::edit::History::apply`], or replayed by
/// [`crate::edit::History::undo`]/[`crate::edit::History::redo`] -- this
/// function does not care which, only that `edit` is the exact value that was
/// just applied to reach `design`'s current state). `previous` must be the
/// design's own last valid solve, from immediately before `edit` -- see
/// [`Design::resolve_dirty`]'s "Panics" section for the alignment invariant
/// this relies on for the two variants that actually use it.
///
/// # Errors
///
/// Propagates [`Design::solve`]'s/[`Design::resolve_dirty`]'s
/// [`DesignSolveError`].
pub fn resolve_after_edit(
    design: &Design,
    previous: &[SolvedTier],
    edit: &Edit,
) -> Result<Vec<SolvedTier>, DesignSolveError> {
    match edit {
        // None of these touch a tier's angle/indices/constraint -- no MAST could
        // possibly have changed, so `previous`'s masts are still valid and this
        // returns them unchanged. `SetMeta`'s `gear_reference_angle` is the one
        // exception worth flagging here: it rotates the solved PLANE arrangement
        // downstream of this function (see `Edit::SetMeta`'s own doc comment), so
        // a caller that also needs the rotated planes -- not just the masts --
        // still gets them correctly, since `Design::planes_from_solved` reads
        // `gear_reference_angle` fresh on every call; this function's job is
        // masts only.
        Edit::SetPreform { .. }
        | Edit::SetPreformYOffset { .. }
        | Edit::SetGirdleDiameterMm { .. }
        | Edit::SetMaterial { .. }
        | Edit::SetMeta { .. }
        | Edit::SetCheaterOffset { .. }
        | Edit::SetTierNote { .. }
        // A `TierTarget` only ever becomes a mast inside `Design::solve_with`/
        // `resolve_dirty_with`'s own resolution pre-pass (see
        // `crate::design::targets`'s module docs), never retroactively -- so
        // editing one alone changes no mast `resolve_after_edit` should already
        // know about; a caller that wants the target actually applied re-solves
        // through those entry points next, same as every edit here.
        | Edit::SetTierTarget { .. } => Ok(previous.to_vec()),
        // Shifts every later tier's index against `previous`, which is keyed by
        // position -- always fully re-solve rather than reason about a moving index space.
        // `MoveTier` renumbers every tier strictly between its two positions the same way.
        Edit::AddTier { .. } | Edit::RemoveTier { .. } | Edit::MoveTier { .. } => design.solve(),
        // All three replace exactly one tier's fields in place; every other tier
        // keeps its index, so `previous` stays aligned.
        Edit::ModifyTier { index, .. }
        | Edit::SetConstraint { index, .. }
        | Edit::SetIndices { index, .. } => {
            design.resolve_dirty(previous, &BTreeSet::from([*index]))
        }
        // Gear/symmetry/mirror feed `solve_meet_points` itself, and
        // `RemapIndices`/`RestoreIndices` can rewrite every tier's indices/detached
        // at once -- the same "index-wheel position could have moved" case as
        // `AddTier`/`RemoveTier`, so always fully re-solve.
        Edit::SetSchedule { .. } | Edit::RemapIndices { .. } | Edit::RestoreIndices { .. } => {
            design.solve()
        }
        // Same shape as `ModifyTier`, generalized to many indices at once.
        Edit::RetargetAngles { changes } => {
            let dirty: BTreeSet<usize> = changes.iter().map(|&(index, _, _)| index).collect();
            design.resolve_dirty(previous, &dirty)
        }
        // A batch's own effect is the union of its sub-edits': full re-solve if ANY
        // sub-edit would need one on its own, else `resolve_dirty` over the union of
        // every sub-edit's own dirty tier set -- see `batch_needs_full_resolve`.
        Edit::Batch(edits) => {
            let mut dirty = BTreeSet::new();
            if batch_needs_full_resolve(edits, &mut dirty) {
                design.solve()
            } else {
                design.resolve_dirty(previous, &dirty)
            }
        }
    }
}

/// Whether `edits` (an [`Edit::Batch`]'s payload) needs a full [`Design::solve`]
/// rather than [`Design::resolve_dirty`] -- generalizes [`resolve_after_edit`]'s own
/// per-variant reasoning over every sub-edit, recursing into a nested [`Edit::Batch`]
/// (never constructed today, but handled rather than assumed away). Collects every
/// directly-dirtied tier index into `dirty` along the way, used by the caller when the
/// answer turns out to be `false`.
fn batch_needs_full_resolve(edits: &[Edit], dirty: &mut BTreeSet<usize>) -> bool {
    let mut needs_full = false;
    for edit in edits {
        match edit {
            Edit::SetPreform { .. }
            | Edit::SetPreformYOffset { .. }
            | Edit::SetGirdleDiameterMm { .. }
            | Edit::SetMaterial { .. }
            | Edit::SetMeta { .. }
            | Edit::SetCheaterOffset { .. }
            | Edit::SetTierNote { .. }
            | Edit::SetTierTarget { .. } => {}
            Edit::AddTier { .. }
            | Edit::RemoveTier { .. }
            | Edit::MoveTier { .. }
            | Edit::SetSchedule { .. }
            | Edit::RemapIndices { .. }
            | Edit::RestoreIndices { .. } => needs_full = true,
            Edit::ModifyTier { index, .. }
            | Edit::SetConstraint { index, .. }
            | Edit::SetIndices { index, .. } => {
                dirty.insert(*index);
            }
            Edit::RetargetAngles { changes } => {
                dirty.extend(changes.iter().map(|&(index, _, _)| index));
            }
            Edit::Batch(inner) => needs_full |= batch_needs_full_resolve(inner, dirty),
        }
    }
    needs_full
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(constraint: MeetConstraint) -> MeetTierInput {
        MeetTierInput {
            angle_deg: 30.0,
            indices: vec![0.0],
            constraint,
            names: Vec::new(),
        }
    }

    /// Editing a `ScaleReference` tier in a design where every OTHER tier is also
    /// `ScaleReference` affects only itself -- the "freshly imported" case this
    /// module's whole value rests on.
    #[test]
    fn an_all_scale_reference_design_affects_only_the_edited_tier() {
        let inputs = vec![
            input(MeetConstraint::ScaleReference(1.0)),
            input(MeetConstraint::ScaleReference(0.5)),
            input(MeetConstraint::ScaleReference(0.4)),
        ];
        assert_eq!(
            affected_tiers(&inputs, &BTreeSet::from([1])),
            BTreeSet::from([1])
        );
    }

    /// A single `MeetExisting`/`MeetNamed` tier anywhere in the design pulls EVERY
    /// non-`ScaleReference` tier into the affected set, regardless of which tier was
    /// actually edited or where either sits in file order.
    #[test]
    fn every_non_scale_reference_tier_is_affected_regardless_of_which_one_was_edited() {
        let inputs = vec![
            input(MeetConstraint::ScaleReference(1.0)),
            input(MeetConstraint::MeetExisting),
            input(MeetConstraint::ScaleReference(0.4)),
            input(MeetConstraint::MeetNamed(vec!["whatever".to_string()])),
        ];
        assert_eq!(
            affected_tiers(&inputs, &BTreeSet::from([0])),
            BTreeSet::from([0, 1, 3])
        );
        assert_eq!(
            affected_tiers(&inputs, &BTreeSet::from([1])),
            BTreeSet::from([1, 3])
        );
    }

    /// `dirty` itself is always included even when it names a `ScaleReference`
    /// tier -- otherwise editing an anchor's OWN value (not just tiers that
    /// depend on it) would be invisible to the affected set.
    #[test]
    fn the_edited_tier_is_always_affected_even_if_it_is_a_scale_reference() {
        let inputs = vec![input(MeetConstraint::ScaleReference(1.0))];
        assert_eq!(
            affected_tiers(&inputs, &BTreeSet::from([0])),
            BTreeSet::from([0])
        );
    }

    /// Undo, composed with [`resolve_after_edit`], must restore byte-identical
    /// geometry AND byte-identical re-derived masts -- the exact `SolvedTier`s
    /// [`crate::edit::History::apply`]/[`crate::edit::History::undo`] compose to,
    /// not merely numbers `Design::solve()` happens to reproduce. Uses
    /// [`crate::edit::History::peek_undo`] to learn which `Edit` an upcoming
    /// `undo()` will replay, the real caller pattern.
    #[test]
    fn undo_composed_with_resolve_after_edit_restores_the_original_masts_and_design_exactly() {
        use crate::{
            design::{ConstraintTier, Design},
            edit::History,
            preform::PreformSpec,
        };

        let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
        design.tiers.push(ConstraintTier {
            angle_deg: 30.0,
            name: "A".to_string(),
            indices: vec![0.0, 24.0, 48.0, 72.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });
        design.tiers.push(ConstraintTier {
            angle_deg: 45.0,
            name: "B".to_string(),
            indices: vec![0.0, 24.0, 48.0, 72.0],
            constraint: MeetConstraint::MeetNamed(vec!["A".to_string()]),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });
        let original = design.clone();
        let baseline = design.solve().expect("must solve");

        let mut history = History::new();
        let edit = Edit::SetConstraint {
            index: 0,
            constraint: MeetConstraint::ScaleReference(0.7),
        };
        history
            .apply(&mut design, edit.clone())
            .expect("apply must succeed");
        let after_edit = resolve_after_edit(&design, &baseline, &edit).expect("resolve after edit");
        assert_ne!(
            design, original,
            "the edit must have actually changed something"
        );

        let undo_edit = history
            .peek_undo()
            .cloned()
            .expect("the edit above must be undoable");
        assert!(history.undo(&mut design).unwrap(), "undo must succeed");
        let restored =
            resolve_after_edit(&design, &after_edit, &undo_edit).expect("resolve after undo");

        assert_eq!(
            design, original,
            "undo must restore byte-identical geometry"
        );
        assert_eq!(restored.len(), baseline.len());
        for (r, b) in restored.iter().zip(&baseline) {
            assert_eq!(
                r.mast.to_bits(),
                b.mast.to_bits(),
                "undo must restore byte-identical masts too, not just design state"
            );
        }
    }
}
