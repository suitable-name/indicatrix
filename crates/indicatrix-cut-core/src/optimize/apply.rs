//! [`apply_optimize_outcome`], [`apply_optimize_result`] and
//! [`apply_optimize_candidate`] -- turning an [`super::OptimizeOutcome`] (or a ranked
//! [`super::OptimizeCandidate`]) into real, undoable `History` edits. See the parent
//! module's doc comment ("`History` remains the sole mutator") for why
//! [`super::optimize_design`] itself never mutates a [`Design`].

use super::{
    options::{MastChange, OptimizeCandidate, OptimizeResult},
    search::{AngleChange, OptimizeOutcome},
};
use crate::{
    design::{ConstraintTier, Design},
    edit::{Edit, EditError, History},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Applies an [`OptimizeOutcome`]'s [`super::AngleChange`]s to `design` through `history`.
///
/// All changed tiers are folded into ONE [`Edit::Batch`] of [`Edit::ModifyTier`]
/// sub-edits, in tier-index order, so an Optimize apply is a single undo step
/// rather than one `Ctrl+Z` per tier. Every index is
/// validated up front against a scratch clone of `design.tiers` -- never
/// `design` itself -- so a change whose index no longer exists leaves `design`
/// completely untouched instead of applying a partial, silently-mismatched
/// prefix of the outcome (matching [`Edit::Batch`]'s own all-or-nothing
/// contract). Returns the number of tier changes folded into the batch (equal
/// to `outcome.changes.len()` on success).
///
/// An outcome of a run that varied anchored tiers also has masts to move; this
/// function applies angles only. [`apply_optimize_result`] applies both.
///
/// This function does not follow tier relations: the angles it writes are exactly the
/// outcome's. `indicatrix_editor::EditorSession::apply_optimize_outcome` is the entry
/// point for a design with relations; it works the relations out and applies them in the
/// same undo step. [`apply_optimize_result`] and [`apply_optimize_candidate`] fold the
/// relations in themselves.
///
/// # Stale indices
///
/// [`super::AngleChange::index`] is a POSITION, recorded when [`super::optimize_design`]
/// ran. If `design` was edited since (most plausibly [`Edit::MoveTier`], which
/// shifts every tier strictly between its two positions) `index` can now name a
/// completely different tier than the one the search actually decided on --
/// checking only that the index is IN RANGE (as this function used to) would
/// silently write the change to whatever tier happens to sit there now. So every
/// change is also checked against [`super::AngleChange::from_deg`]: the tier
/// currently at `index` must carry EXACTLY that angle (bit-for-bit, via
/// `f64::to_bits`, since this is an identity check, not a tolerance) before its
/// angle is overwritten -- the same all-or-nothing contract as an out-of-range
/// index, reported through the same [`crate::edit::EditError`] this function
/// already returns for that case (this crate has no narrower error variant to
/// spend on the distinction without changing `EditError` itself, which lives in
/// a different module).
///
/// # Errors
///
/// Returns [`crate::edit::EditError`] the first time an [`super::AngleChange`]'s
/// `index` no longer names a real tier, OR names a tier whose current angle no
/// longer matches [`super::AngleChange::from_deg`] (see "Stale indices" above),
/// before `design` is mutated at all.
pub fn apply_optimize_outcome(
    history: &mut History,
    design: &mut Design,
    outcome: &OptimizeOutcome,
) -> Result<usize, EditError> {
    apply_changes(history, design, &outcome.changes, &[], false)
}

/// Applies an [`OptimizeResult`]'s best result to `design` through `history`.
///
/// It applies the `outcome.changes` angle changes AND the `mast_changes` that go with them, as ONE
/// [`Edit::Batch`] (one undo step, exact undo).
///
/// Same all-or-nothing contract and stale guard as [`apply_optimize_outcome`], extended
/// to masts: a mast change is stale unless its tier is still a `ScaleReference` tier
/// whose mast is bit-for-bit `from_mast`. A tier with both an angle and a mast change
/// becomes one `ModifyTier` edit. Returns the number of tiers changed.
///
/// A design with tier relations is left satisfying them: every tier that follows a
/// relation moves to the angle its relation gives once the changes are in, inside the
/// same batch, so one undo restores everything. Those tiers are counted in the returned
/// number.
///
/// # Errors
///
/// Returns [`crate::edit::EditError`] for the first change whose tier is gone or no
/// longer carries the angle or mast the change started from, or (naming the first tier
/// that follows a relation) when the relations cannot be worked out after the changes,
/// before `design` is mutated at all.
pub fn apply_optimize_result(
    history: &mut History,
    design: &mut Design,
    result: &OptimizeResult,
) -> Result<usize, EditError> {
    apply_changes(
        history,
        design,
        &result.outcome.changes,
        &result.mast_changes,
        true,
    )
}

/// Applies one ranked [`OptimizeCandidate`] -- its angle changes and mast changes -- to
/// `design` through `history`, exactly like [`apply_optimize_result`] does for the best
/// one.
///
/// # Errors
///
/// As [`apply_optimize_result`].
pub fn apply_optimize_candidate(
    history: &mut History,
    design: &mut Design,
    candidate: &OptimizeCandidate,
) -> Result<usize, EditError> {
    apply_changes(
        history,
        design,
        &candidate.changes,
        &candidate.mast_changes,
        true,
    )
}

/// Moves every tier that follows a relation in `design` to the angle its relation gives
/// when `trial_tiers` are the tiers, and adds each tier it moves to `touched`.
///
/// `Err(index)` names the first tier that follows a relation when the relations cannot
/// be worked out (a loop, a missing tier, a result that is not an angle).
fn fold_relations(
    design: &Design,
    trial_tiers: &mut [ConstraintTier],
    touched: &mut Vec<usize>,
) -> Result<(), usize> {
    if design.tier_relations.is_empty() {
        return Ok(());
    }
    let mut trial = design.clone();
    trial.tiers = trial_tiers.to_vec();
    let updates = trial.evaluate_relations().map_err(|_| {
        (0..design.tiers.len())
            .find(|&index| design.is_tier_driven(index))
            .unwrap_or(0)
    })?;
    for (position, angle_deg) in updates {
        let Some(tier) = trial_tiers.get_mut(position) else {
            continue;
        };
        if tier.angle_deg.to_bits() != angle_deg.to_bits() {
            tier.angle_deg = angle_deg;
            if !touched.contains(&position) {
                touched.push(position);
            }
        }
    }
    Ok(())
}

/// The shared core: validates every change against a scratch copy of the tiers, then
/// applies one `ModifyTier` per touched tier (in first-touched order, angle changes
/// first) as a single batch. With `follow_relations`, the tiers that follow a relation
/// join the batch at the angles their relations give.
fn apply_changes(
    history: &mut History,
    design: &mut Design,
    angle_changes: &[AngleChange],
    mast_changes: &[MastChange],
    follow_relations: bool,
) -> Result<usize, EditError> {
    if angle_changes.is_empty() && mast_changes.is_empty() {
        return Ok(0);
    }
    let stale_or_out_of_range = |index: usize| EditError {
        index,
        tier_count: design.tiers.len(),
    };
    let mut trial_tiers = design.tiers.clone();
    let mut touched: Vec<usize> = Vec::with_capacity(angle_changes.len() + mast_changes.len());
    for change in angle_changes {
        let Some(tier) = trial_tiers.get_mut(change.index) else {
            return Err(stale_or_out_of_range(change.index));
        };
        if tier.angle_deg.to_bits() != change.from_deg.to_bits() {
            return Err(stale_or_out_of_range(change.index));
        }
        tier.angle_deg = change.to_deg;
        if !touched.contains(&change.index) {
            touched.push(change.index);
        }
    }
    for change in mast_changes {
        let Some(tier) = trial_tiers.get_mut(change.index) else {
            return Err(stale_or_out_of_range(change.index));
        };
        let MeetConstraint::ScaleReference(current_mast) = tier.constraint else {
            return Err(stale_or_out_of_range(change.index));
        };
        if current_mast.to_bits() != change.from_mast.to_bits() {
            return Err(stale_or_out_of_range(change.index));
        }
        tier.constraint = MeetConstraint::ScaleReference(change.to_mast);
        if !touched.contains(&change.index) {
            touched.push(change.index);
        }
    }
    if follow_relations {
        fold_relations(design, &mut trial_tiers, &mut touched).map_err(stale_or_out_of_range)?;
    }
    let edits: Vec<Edit> = touched
        .iter()
        .map(|&index| Edit::ModifyTier {
            index,
            tier: trial_tiers[index].clone(),
        })
        .collect();
    let applied = edits.len();
    history.apply(design, Edit::Batch(edits))?;
    Ok(applied)
}
