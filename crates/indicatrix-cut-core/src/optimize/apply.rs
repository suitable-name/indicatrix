//! [`apply_optimize_outcome`] -- turning an [`super::OptimizeOutcome`] into
//! real, undoable `History` edits. See the parent module's doc comment
//! ("`History` remains the sole mutator") for why [`super::optimize_design`]
//! itself never mutates a [`Design`].

use super::search::OptimizeOutcome;
use crate::{
    design::Design,
    edit::{Edit, EditError, History},
};

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
/// # Errors
///
/// Returns [`crate::edit::EditError`] the first time an [`super::AngleChange`]'s
/// `index` no longer names a real tier, before `design` is mutated at all.
pub fn apply_optimize_outcome(
    history: &mut History,
    design: &mut Design,
    outcome: &OptimizeOutcome,
) -> Result<usize, EditError> {
    if outcome.changes.is_empty() {
        return Ok(0);
    }
    let mut trial_tiers = design.tiers.clone();
    let mut edits = Vec::with_capacity(outcome.changes.len());
    for change in &outcome.changes {
        let Some(tier) = trial_tiers.get_mut(change.index) else {
            return Err(EditError {
                index: change.index,
                tier_count: design.tiers.len(),
            });
        };
        tier.angle_deg = change.to_deg;
        edits.push(Edit::ModifyTier {
            index: change.index,
            tier: tier.clone(),
        });
    }
    let applied = edits.len();
    history.apply(design, Edit::Batch(edits))?;
    Ok(applied)
}
