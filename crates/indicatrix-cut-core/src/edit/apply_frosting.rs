//! [`Edit::SetTierFrosting`]'s apply/inverse half, and the gear-remap step that moves the
//! facet positions of a tier frosted facet by facet. Split from `apply` to keep that file
//! readable.

use super::{
    apply::remap_index,
    edit_type::{Edit, EditError, RemapRounding},
};
use crate::design::{Design, TierFrosting};

impl Design {
    /// [`Edit::SetTierFrosting`]'s own apply/inverse half. Keyed by the current occupant's
    /// [`crate::design::TierId`] (via [`Design::tier_id_at`]), so -- like
    /// [`Design::tier_targets`] -- it never needs renumbering on `AddTier`/`RemoveTier`/
    /// `MoveTier`. Changes only the frosting map; the frosting is normalised first, so a
    /// frosting of no facet is a clear.
    pub(super) fn apply_set_tier_frosting(
        &mut self,
        index: usize,
        frosting: Option<TierFrosting>,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        if index >= tier_count {
            return Err(EditError { index, tier_count });
        }
        // Self-heals `tier_ids`, only after the bounds check -- see `sync_tier_ids`.
        self.sync_tier_ids();
        let Some(id) = self.tier_id_at(index) else {
            return Err(EditError { index, tier_count });
        };
        let previous = match frosting.and_then(TierFrosting::normalised) {
            Some(frosting) => self.tier_frosting.insert(id, frosting),
            None => self.tier_frosting.remove(&id),
        };
        Ok(Edit::SetTierFrosting {
            index,
            frosting: previous,
        })
    }

    /// Moves the facet positions of every tier frosted facet by facet from `from_gear` to
    /// `to_gear` teeth, with the same rounding [`Edit::RemapIndices`] gives the tiers'
    /// own `indices`, so a frosted facet stays on the facet it names.
    ///
    /// Returns one [`Edit::SetTierFrosting`] per tier it changed, each holding the frosting
    /// the tier had, to be part of the remap's inverse. Empty (and nothing is touched) for a
    /// design without per-facet frosting, which is every design that does not use it.
    pub(super) fn remap_frosted_facets(
        &mut self,
        from_gear: i32,
        to_gear: i32,
        rounding: RemapRounding,
    ) -> Vec<Edit> {
        let mut inverse = Vec::new();
        let ids: Vec<_> = self.tier_frosting.keys().copied().collect();
        for id in ids {
            let Some(old @ TierFrosting::Facets(_)) = self.tier_frosting.get(&id).cloned() else {
                continue;
            };
            let Some(index) = self.index_of_tier_id(id) else {
                continue;
            };
            match old.with_positions_mapped(|position| {
                remap_index(position, from_gear, to_gear, rounding)
            }) {
                Some(moved) => {
                    self.tier_frosting.insert(id, moved);
                }
                None => {
                    self.tier_frosting.remove(&id);
                }
            }
            inverse.push(Edit::SetTierFrosting {
                index,
                frosting: Some(old),
            });
        }
        inverse
    }
}
