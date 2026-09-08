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
/// One ordinary [`Edit::ModifyTier`] per changed tier, in tier-index order -- see the
/// module doc comment's "`History` remains the sole mutator" section. Returns the
/// number of edits actually applied (equal to `outcome.changes.len()` unless `design`
/// has since diverged from the design `outcome` was computed against and an index no
/// longer exists, at which point this stops and returns an error rather than applying
/// a partial, silently-mismatched set of the remaining changes).
///
/// # Errors
///
/// Propagates [`Design::apply_edit`]'s [`crate::edit::EditError`] the first time an
/// [`super::AngleChange`]'s `index` no longer names a real tier.
pub fn apply_optimize_outcome(
    history: &mut History,
    design: &mut Design,
    outcome: &OptimizeOutcome,
) -> Result<usize, EditError> {
    let mut applied = 0usize;
    for change in &outcome.changes {
        let Some(tier) = design.tiers.get(change.index) else {
            return Err(EditError {
                index: change.index,
                tier_count: design.tiers.len(),
            });
        };
        let mut new_tier = tier.clone();
        new_tier.angle_deg = change.to_deg;
        history.apply(
            design,
            Edit::ModifyTier {
                index: change.index,
                tier: new_tier,
            },
        )?;
        applied += 1;
    }
    Ok(applied)
}
