//! The concave tiers' structural edits, as [`EditorSession`] methods: Duplicate, Remove and
//! Move.

use super::{DuplicateOutcome, MovedTier, RemovedTier, display_name};
use crate::{loading::unique_duplicate_name, session::EditorSession};
use indicatrix_cut_core::{Edit, EditError};

impl EditorSession {
    /// Duplicate for a concave tier: inserts a copy right after tier `index` of
    /// `design.concave_tiers` as one `Edit::AddConcaveTier`, named by
    /// [`unique_duplicate_name`] against every flat AND concave name (a concave tier may
    /// not reuse a flat tier's name).
    ///
    /// `Ok(None)` when the tier does not exist.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn duplicate_concave_tier(
        &mut self,
        index: usize,
    ) -> Result<Option<DuplicateOutcome>, EditError> {
        let Some(source) = self.design.concave_tiers.get(index) else {
            return Ok(None);
        };
        let mut duplicate = source.clone();
        let source_label = display_name(&source.name);
        let existing_names: Vec<String> = self
            .design
            .tiers
            .iter()
            .map(|t| t.name.clone())
            .chain(self.design.concave_tiers.iter().map(|t| t.name.clone()))
            .collect();
        duplicate.name = unique_duplicate_name(&source.name, &existing_names);
        let duplicate_label = duplicate.name.clone();
        let new_index = index + 1;
        let change = self.apply(Edit::AddConcaveTier {
            index: new_index,
            tier: duplicate,
        })?;
        Ok(Some(DuplicateOutcome {
            change,
            new_index,
            source_label,
            duplicate_label,
        }))
    }

    /// Remove for a concave tier, as one `Edit::RemoveConcaveTier`. Nothing can meet a
    /// concave tier by name, so unlike [`Self::remove_tier`] there is no dependant check.
    ///
    /// `Ok(None)` when the tier does not exist.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn remove_concave_tier(&mut self, index: usize) -> Result<Option<RemovedTier>, EditError> {
        let Some(tier) = self.design.concave_tiers.get(index) else {
            return Ok(None);
        };
        let (name, facet_count) = (display_name(&tier.name), tier.indices.len());
        let change = self.apply(Edit::RemoveConcaveTier { index })?;
        Ok(Some(RemovedTier {
            change,
            name,
            facet_count,
        }))
    }

    /// Row reorder for a concave tier: one place up (`direction < 0`) or down as one
    /// `Edit::MoveConcaveTier`. The order matters, since it is the cutting order inside
    /// a section's concave group.
    ///
    /// `Ok(None)` at either end of the list.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn move_concave_tier(
        &mut self,
        index: usize,
        direction: i32,
    ) -> Result<Option<MovedTier>, EditError> {
        let tier_count = self.design.concave_tiers.len();
        let target = if direction < 0 {
            index.checked_sub(1)
        } else {
            index.checked_add(1).filter(|&t| t < tier_count)
        };
        let Some(target) = target else {
            return Ok(None);
        };
        let change = self.apply(Edit::MoveConcaveTier {
            from: index,
            to: target,
        })?;
        Ok(Some(MovedTier { change, target }))
    }
}
