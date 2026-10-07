//! Renumbering of the position-keyed side maps ([`Design::cheater_offsets_deg`] and
//! [`Design::tier_notes`]) when a tier is inserted, removed or moved. Split from
//! `apply.rs` so that file stays focused on the edits themselves; the methods are
//! unchanged.

use crate::design::Design;

#[cfg(doc)]
use super::Edit;

impl Design {
    /// Renumbers [`Design::cheater_offsets_deg`] for a tier just INSERTED at
    /// `index`: every entry at `index` or later moves up one position, exactly
    /// matching `self.tiers.insert(index, ..)`'s own effect on positions.
    /// Iterates from the highest key down so no entry overwrites another
    /// before it is itself moved.
    pub(super) fn shift_cheater_offsets_for_insert(&mut self, index: usize) {
        let to_shift: Vec<usize> = self
            .cheater_offsets_deg
            .range(index..)
            .map(|(&k, _)| k)
            .rev()
            .collect();
        for key in to_shift {
            if let Some(value) = self.cheater_offsets_deg.remove(&key) {
                self.cheater_offsets_deg.insert(key + 1, value);
            }
        }
    }

    /// Renumbers [`Design::cheater_offsets_deg`] for a tier just REMOVED from
    /// `index`: takes that entry out (returning it) and shifts every later
    /// entry down one position, exactly matching `self.tiers.remove(index)`'s
    /// own effect on positions.
    pub(super) fn shift_cheater_offsets_for_remove(&mut self, index: usize) -> Option<f64> {
        let removed = self.cheater_offsets_deg.remove(&index);
        let to_shift: Vec<usize> = self
            .cheater_offsets_deg
            .range(index + 1..)
            .map(|(&k, _)| k)
            .collect();
        for key in to_shift {
            if let Some(value) = self.cheater_offsets_deg.remove(&key) {
                self.cheater_offsets_deg.insert(key - 1, value);
            }
        }
        removed
    }

    /// Renumbers [`Design::cheater_offsets_deg`] for a tier moved from `from`
    /// to `to`, matching `Vec::remove(from)` then `Vec::insert(to, ..)`'s
    /// combined effect on every position -- including relocating `from`'s own
    /// entry (if any) to `to`, not just shifting everyone else. Used for both
    /// [`Edit::MoveTier`] and its own exact inverse (`from`/`to` swapped),
    /// since that reindexing is its own inverse the same way the tier-vector
    /// move already is.
    pub(super) fn shift_cheater_offsets_for_move(&mut self, from: usize, to: usize) {
        let moved = self.cheater_offsets_deg.remove(&from);
        let remainder = std::mem::take(&mut self.cheater_offsets_deg);
        self.cheater_offsets_deg = remainder
            .into_iter()
            .map(|(k, v)| (if k > from { k - 1 } else { k }, v))
            .map(|(k, v)| (if k >= to { k + 1 } else { k }, v))
            .collect();
        if let Some(value) = moved {
            self.cheater_offsets_deg.insert(to, value);
        }
    }

    /// Renumbers [`Design::tier_notes`] for a tier just INSERTED at `index` --
    /// exactly [`Self::shift_cheater_offsets_for_insert`]'s own logic, over the
    /// note map instead of the cheater-offset one.
    pub(super) fn shift_tier_notes_for_insert(&mut self, index: usize) {
        let to_shift: Vec<usize> = self
            .tier_notes
            .range(index..)
            .map(|(&k, _)| k)
            .rev()
            .collect();
        for key in to_shift {
            if let Some(value) = self.tier_notes.remove(&key) {
                self.tier_notes.insert(key + 1, value);
            }
        }
    }

    /// Renumbers [`Design::tier_notes`] for a tier just REMOVED from `index` --
    /// exactly [`Self::shift_cheater_offsets_for_remove`]'s own logic, over the
    /// note map instead of the cheater-offset one.
    pub(super) fn shift_tier_notes_for_remove(&mut self, index: usize) -> Option<String> {
        let removed = self.tier_notes.remove(&index);
        let to_shift: Vec<usize> = self
            .tier_notes
            .range(index + 1..)
            .map(|(&k, _)| k)
            .collect();
        for key in to_shift {
            if let Some(value) = self.tier_notes.remove(&key) {
                self.tier_notes.insert(key - 1, value);
            }
        }
        removed
    }

    /// Renumbers [`Design::tier_notes`] for a tier moved from `from` to `to` --
    /// exactly [`Self::shift_cheater_offsets_for_move`]'s own logic, over the note
    /// map instead of the cheater-offset one.
    pub(super) fn shift_tier_notes_for_move(&mut self, from: usize, to: usize) {
        let moved = self.tier_notes.remove(&from);
        let remainder = std::mem::take(&mut self.tier_notes);
        self.tier_notes = remainder
            .into_iter()
            .map(|(k, v)| (if k > from { k - 1 } else { k }, v))
            .map(|(k, v)| (if k >= to { k + 1 } else { k }, v))
            .collect();
        if let Some(value) = moved {
            self.tier_notes.insert(to, value);
        }
    }
}
