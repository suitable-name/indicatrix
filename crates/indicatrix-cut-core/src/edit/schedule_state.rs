//! [`ScheduleState`] and [`Design::apply_replace_schedule`]: the apply/inverse half of
//! [`Edit::ReplaceSchedule`], which swaps a design's whole flat tier list in one step.

use super::edit_type::{Edit, EditError};
use crate::design::{ConstraintTier, Design, ScheduleMeta, TierId, TierRelation, TierTarget};
use std::collections::{BTreeMap, BTreeSet};

/// Everything about a design's flat tier list that [`Edit::ReplaceSchedule`] swaps.
///
/// That is the schedule metadata (gear, symmetry, header and footnote lines), the tiers
/// with their stable ids, and the per-tier annotations (cheater offsets and notes by
/// position; targets and relations by id), all in one undo step.
///
/// What it leaves alone is everything that is not part of the cutting instructions: the
/// preform, the girdle size, the material and the concave tiers. A caller builds the new
/// state from [`Self::of`] and changes the fields it means to change, so a field it does
/// not touch keeps its value exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleState {
    /// The schedule metadata ([`Design::meta`]).
    pub meta: ScheduleMeta,
    /// The tiers ([`Design::tiers`]).
    pub tiers: Vec<ConstraintTier>,
    /// One id per tier, in the same order ([`Design::tier_ids`]). Must be as long as
    /// `tiers` and hold no id twice.
    pub tier_ids: Vec<TierId>,
    /// The id counter ([`Design::next_tier_id`]). Applying the state never lowers the
    /// design's own counter, so an id handed out once is never handed out again.
    pub next_tier_id: u64,
    /// Cheater offsets by tier position ([`Design::cheater_offsets_deg`]).
    pub cheater_offsets_deg: BTreeMap<usize, f64>,
    /// Tier notes by tier position ([`Design::tier_notes`]).
    pub tier_notes: BTreeMap<usize, String>,
    /// Targets by tier id ([`Design::tier_targets`]).
    pub tier_targets: BTreeMap<TierId, TierTarget>,
    /// Relations by the driven tier's id ([`Design::tier_relations`]).
    pub tier_relations: BTreeMap<TierId, TierRelation>,
}

impl ScheduleState {
    /// The state `design` has right now.
    #[must_use]
    pub fn of(design: &Design) -> Self {
        Self {
            meta: design.meta.clone(),
            tiers: design.tiers.clone(),
            tier_ids: design.tier_ids.clone(),
            next_tier_id: design.next_tier_id,
            cheater_offsets_deg: design.cheater_offsets_deg.clone(),
            tier_notes: design.tier_notes.clone(),
            tier_targets: design.tier_targets.clone(),
            tier_relations: design.tier_relations.clone(),
        }
    }
}

impl Design {
    /// [`Edit::ReplaceSchedule`]'s own apply/inverse half, split out of
    /// [`Self::apply_edit`] to keep that function short.
    ///
    /// Everything is checked before anything is written, so a refused state leaves
    /// `self` exactly as it was. The inverse is the same variant holding the state this
    /// call replaced.
    ///
    /// # Errors
    ///
    /// [`EditError`] when the state is not usable: a zero gear tooth count or symmetry
    /// order (`index` 0, like [`Edit::SetSchedule`]); ids that do not match the tiers,
    /// repeat, or are missing for a target, relation or relation reference; an annotation
    /// keyed past the last tier; or a tier that shares a name with a concave tier
    /// (`index` is that tier's position).
    pub(super) fn apply_replace_schedule(
        &mut self,
        state: ScheduleState,
    ) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let new_count = state.tiers.len();
        let refuse = |index: usize| EditError { index, tier_count };
        if state.meta.gear_teeth == 0 || state.meta.symmetry_order == 0 {
            return Err(refuse(0));
        }
        if state.tier_ids.len() != new_count {
            return Err(refuse(new_count));
        }
        let ids: BTreeSet<TierId> = state.tier_ids.iter().copied().collect();
        if ids.len() != new_count {
            return Err(refuse(new_count));
        }
        if let Some(&key) = state
            .cheater_offsets_deg
            .keys()
            .chain(state.tier_notes.keys())
            .find(|&&key| key >= new_count)
        {
            return Err(refuse(key));
        }
        if state.tier_targets.keys().any(|id| !ids.contains(id)) {
            return Err(refuse(new_count));
        }
        let relations_are_whole = state.tier_relations.iter().all(|(id, relation)| {
            ids.contains(id) && relation.references().iter().all(|read| ids.contains(read))
        });
        if !relations_are_whole {
            return Err(refuse(new_count));
        }
        if let Some(position) = state
            .tiers
            .iter()
            .position(|tier| self.flat_tier_clashes_with_concave(tier))
        {
            return Err(refuse(position));
        }
        // Healing the id list is a write, so it waits until nothing can be refused.
        self.ensure_tier_ids();
        let previous = ScheduleState::of(self);
        self.meta = state.meta;
        self.tiers = state.tiers;
        self.tier_ids = state.tier_ids;
        self.next_tier_id = self.next_tier_id.max(state.next_tier_id);
        self.cheater_offsets_deg = state.cheater_offsets_deg;
        self.tier_notes = state.tier_notes;
        self.tier_targets = state.tier_targets;
        self.tier_relations = state.tier_relations;
        Ok(Edit::ReplaceSchedule(Box::new(previous)))
    }
}
