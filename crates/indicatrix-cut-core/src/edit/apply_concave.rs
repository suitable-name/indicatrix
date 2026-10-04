//! The concave-tier halves of [`Design::apply_edit`], split from `apply.rs` so
//! that file stays focused on the flat-tier edits. Every method here validates
//! before it mutates, keeping `apply_edit`'s "without modifying `self` on error"
//! contract, and returns the exact inverse [`Edit`].

use super::edit_type::{Edit, EditError};
use crate::design::{ConcaveTier, Design, TierId};

impl Design {
    /// Whether `tier` may sit in [`Design::concave_tiers`]: it passes
    /// [`ConcaveTier::validate`] against this design's gear and does not reuse a
    /// flat tier's name.
    ///
    /// [`EditError`] has one shape, naming a position and a count, so a rejected
    /// tier reuses it with the concave list's length -- the same precedent as
    /// `SetSchedule`'s schedule-wide validation failure. The editor's own tier
    /// form reports the precise [`crate::design::ConcaveTierError`] before an edit
    /// is ever built; this is the backstop that keeps a hostile tier out of the
    /// design.
    fn check_concave_candidate(&self, index: usize, tier: &ConcaveTier) -> Result<(), EditError> {
        let reject = || EditError {
            index,
            tier_count: self.concave_tiers.len(),
        };
        tier.validate(self.meta.gear_teeth).map_err(|_| reject())?;
        let clashes = !tier.name.is_empty()
            && self
                .tiers
                .iter()
                .any(|flat| flat.names().contains(&tier.name.as_str()));
        if clashes { Err(reject()) } else { Ok(()) }
    }

    /// Whether any name of the flat `tier` equals a concave tier's name, the clash
    /// [`Design::validate_concave_tiers`] rejects on every load and resolve. The flat
    /// edits ([`Edit::AddTier`], [`Edit::ModifyTier`]) refuse such a tier so the
    /// clash is refused symmetrically, instead of producing a file that will not
    /// reopen.
    pub(super) fn flat_tier_clashes_with_concave(
        &self,
        tier: &crate::design::ConstraintTier,
    ) -> bool {
        self.concave_tiers.iter().any(|concave| {
            !concave.name.is_empty() && tier.names().contains(&concave.name.as_str())
        })
    }

    /// [`Edit::AddConcaveTier`]'s apply/inverse half; `index == len` appends.
    pub(super) fn apply_add_concave_tier(
        &mut self,
        index: usize,
        tier: ConcaveTier,
    ) -> Result<Edit, EditError> {
        let tier_count = self.concave_tiers.len();
        if index > tier_count {
            return Err(EditError { index, tier_count });
        }
        self.check_concave_candidate(index, &tier)?;
        // Heals ids a caller left short by pushing onto `concave_tiers` directly;
        // only after validation, so a rejected edit leaves `self` untouched.
        self.ensure_concave_tier_ids();
        self.concave_tiers.insert(index, tier);
        let id = self.allocate_tier_id();
        self.concave_tier_ids.insert(index, id);
        Ok(Edit::RemoveConcaveTier { index })
    }

    /// [`Edit::RemoveConcaveTier`]'s apply/inverse half. A plain
    /// [`Edit::AddConcaveTier`] inverse would allocate a fresh id, so the inverse
    /// also restores the removed one -- the same "ids are never reused" rule as
    /// `RemoveTier`.
    pub(super) fn apply_remove_concave_tier(&mut self, index: usize) -> Result<Edit, EditError> {
        let tier_count = self.concave_tiers.len();
        if index >= tier_count {
            return Err(EditError { index, tier_count });
        }
        self.ensure_concave_tier_ids();
        let tier = self.concave_tiers.remove(index);
        let id = self.concave_tier_ids.remove(index);
        Ok(Edit::Batch(vec![
            Edit::AddConcaveTier { index, tier },
            Edit::RestoreConcaveTierId { index, id },
        ]))
    }

    /// [`Edit::ModifyConcaveTier`]'s apply/inverse half.
    pub(super) fn apply_modify_concave_tier(
        &mut self,
        index: usize,
        tier: ConcaveTier,
    ) -> Result<Edit, EditError> {
        let tier_count = self.concave_tiers.len();
        if index >= tier_count {
            return Err(EditError { index, tier_count });
        }
        self.check_concave_candidate(index, &tier)?;
        let previous = std::mem::replace(&mut self.concave_tiers[index], tier);
        Ok(Edit::ModifyConcaveTier {
            index,
            tier: previous,
        })
    }

    /// [`Edit::MoveConcaveTier`]'s apply/inverse half, with the same
    /// remove-then-insert meaning of `to` as [`Edit::MoveTier`].
    pub(super) fn apply_move_concave_tier(
        &mut self,
        from: usize,
        to: usize,
    ) -> Result<Edit, EditError> {
        let tier_count = self.concave_tiers.len();
        for index in [from, to] {
            if index >= tier_count {
                return Err(EditError { index, tier_count });
            }
        }
        self.ensure_concave_tier_ids();
        let tier = self.concave_tiers.remove(from);
        self.concave_tiers.insert(to, tier);
        let id = self.concave_tier_ids.remove(from);
        self.concave_tier_ids.insert(to, id);
        Ok(Edit::MoveConcaveTier { from: to, to: from })
    }

    /// [`Edit::RestoreConcaveTierId`]'s apply/inverse half -- the concave twin of
    /// `RestoreTierId`, which writes `tier_ids` and so cannot be reused.
    pub(super) fn apply_restore_concave_tier_id(
        &mut self,
        index: usize,
        id: TierId,
    ) -> Result<Edit, EditError> {
        let tier_count = self.concave_tiers.len();
        let Some(slot) = self.concave_tier_ids.get_mut(index) else {
            return Err(EditError { index, tier_count });
        };
        let previous = std::mem::replace(slot, id);
        Ok(Edit::RestoreConcaveTierId {
            index,
            id: previous,
        })
    }

    /// [`Edit::RestoreConcaveIndices`]'s apply/inverse half: a verbatim snapshot
    /// restore, validated whole before any write.
    pub(super) fn apply_restore_concave_indices(
        &mut self,
        tiers: Vec<(usize, Vec<f64>)>,
    ) -> Result<Edit, EditError> {
        let tier_count = self.concave_tiers.len();
        if let Some(&(index, _)) = tiers.iter().find(|(index, _)| *index >= tier_count) {
            return Err(EditError { index, tier_count });
        }
        let mut previous = Vec::with_capacity(tiers.len());
        for (index, indices) in tiers {
            let old = std::mem::replace(&mut self.concave_tiers[index].indices, indices);
            previous.push((index, old));
        }
        // Reverse for a repeated index, exactly like `RestoreIndices`.
        previous.reverse();
        Ok(Edit::RestoreConcaveIndices { tiers: previous })
    }
}
