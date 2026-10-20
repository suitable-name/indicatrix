//! [`apply_optimize_outcome`], [`apply_optimize_result`] and
//! [`apply_optimize_candidate`] -- turning an [`super::OptimizeOutcome`] (or a ranked
//! [`super::OptimizeCandidate`]) into real, undoable `History` edits. See the parent
//! module's doc comment ("`History` remains the sole mutator") for why
//! [`super::optimize_design`] itself never mutates a [`Design`].

use super::{
    options::{OptimizeCandidate, OptimizeResult},
    search::OptimizeOutcome,
};
use crate::{
    design::Design,
    edit::{EditError, History},
};

#[cfg(doc)]
use crate::edit::Edit;

/// Apply the exact accepted state in one undo step, including pinned depths and
/// relation followers.
///
/// Source identity, parameter freshness and the production geometry gate are checked
/// before History is touched. Returns changed tier count.
///
/// # Errors
/// Missing/stale provenance or an invalid geometric change returns `EditError`.
/// Use `materialize_outcome` when detailed preservation diagnostics are needed.
pub fn apply_optimize_outcome(
    history: &mut History,
    design: &mut Design,
    outcome: &OptimizeOutcome,
) -> Result<usize, EditError> {
    let (_, edit, count) = super::materialize_outcome(design, outcome)?;
    if count > 0 {
        history.apply(design, edit)?;
    }
    Ok(count)
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
    super::materialize::preflight(design, &result.outcome.changes, &result.mast_changes)?;
    let Some(source) = &result.outcome.provenance else {
        return Err(EditError {
            index: 0,
            tier_count: design.tiers.len(),
        });
    };
    if source.masts
        != result
            .mast_changes
            .iter()
            .map(|m| (m.index, m.from_mast, m.to_mast))
            .collect::<Vec<_>>()
    {
        return Err(EditError {
            index: 0,
            tier_count: design.tiers.len(),
        });
    }
    if let Some(candidate) = result.candidates.first() {
        if candidate.provenance.as_ref() != Some(source)
            || candidate.changes != result.outcome.changes
            || candidate.mast_changes != result.mast_changes
        {
            return Err(EditError {
                index: 0,
                tier_count: design.tiers.len(),
            });
        }
        return apply_optimize_candidate(history, design, candidate);
    }
    apply_optimize_outcome(history, design, &result.outcome)
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
    let (_, edit, count) = super::materialize_candidate(design, candidate)?;
    if count > 0 {
        history.apply(design, edit)?;
    }
    Ok(count)
}
