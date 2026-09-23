//! [`Design::apply_edit`]: turning one [`Edit`] into its own inverse while
//! mutating `self` in place -- the one primitive [`super::History`] is built
//! on.

use super::edit_type::{Edit, EditError, RemapRounding};
use crate::design::Design;

/// The factor an index-wheel position is scaled by when moving from `from_gear`
/// teeth to `to_gear` teeth.
///
/// Both tooth counts are taken by MAGNITUDE. An `.asc` header may carry a negative
/// `g` (a convention for a reversed wheel), and the orbit math, the solver and
/// [`crate::design::Design`]'s own `gear_teeth_abs` all read it that way; scaling by
/// a signed ratio instead would flip every index to a negative position no wheel
/// has. `from_gear == 0` (never a valid real gear, but not rejected by this crate's
/// own types either) is treated as a no-op ratio (`1.0`) rather than dividing by
/// zero, so this is always finite.
#[must_use]
pub fn remap_ratio(from_gear: i32, to_gear: i32) -> f64 {
    let from_abs = f64::from(from_gear.unsigned_abs());
    if from_abs == 0.0 {
        1.0
    } else {
        f64::from(to_gear.unsigned_abs()) / from_abs
    }
}

/// Converts one index-wheel position from `from_gear` teeth to `to_gear` teeth,
/// rounding per `rounding` -- see [`Edit::RemapIndices`]'s own doc comment.
fn remap_index(index: f64, from_gear: i32, to_gear: i32, rounding: RemapRounding) -> f64 {
    let scaled = index * remap_ratio(from_gear, to_gear);
    match rounding {
        RemapRounding::Nearest => scaled.round(),
        RemapRounding::Floor => scaled.floor(),
        RemapRounding::Ceil => scaled.ceil(),
    }
}

impl Design {
    /// Applies `edit` to `self` and returns the [`Edit`] that undoes exactly
    /// what was just done -- e.g. applying `AddTier` returns `RemoveTier` at
    /// the same index, and vice versa. Used directly by
    /// [`super::History::apply`]; also usable standalone by a caller that
    /// wants edit/undo without a [`super::History`] stack (e.g. a scripted
    /// test).
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] (without modifying `self`) when `edit` names a
    /// tier index the current schedule doesn't have -- `SetPreform` can
    /// never fail, since it names no index.
    pub fn apply_edit(&mut self, edit: Edit) -> Result<Edit, EditError> {
        // Self-heals `tier_ids` against a caller that mutated `self.tiers`
        // directly (`Vec::push`/etc, bypassing `Edit` entirely) -- a real,
        // pervasive pattern this crate's OWN test fixtures use throughout (build
        // a `Design` via `Design::fresh`, then `design.tiers.push(tier(...))`
        // straight onto the public field, never through `History`). Every
        // `tier_ids`-touching arm below assumes `self.tier_ids.len() ==
        // self.tiers.len()`; without this, the very first `AddTier`/
        // `RemoveTier`/`MoveTier` after such a direct push would index or pop
        // past the end of a `tier_ids` that never grew to match.
        self.sync_tier_ids();
        let tier_count = self.tiers.len();
        match edit {
            Edit::AddTier { index, tier } => {
                if index > tier_count {
                    return Err(EditError { index, tier_count });
                }
                self.tiers.insert(index, tier);
                let id = self.allocate_tier_id();
                self.tier_ids.insert(index, id);
                self.shift_cheater_offsets_for_insert(index);
                self.shift_tier_notes_for_insert(index);
                Ok(Edit::RemoveTier { index })
            }
            Edit::RemoveTier { index } => self.apply_remove_tier(index, tier_count),
            Edit::MoveTier { from, to } => self.apply_move_tier(from, to, tier_count),
            Edit::ModifyTier { index, tier } => {
                let Some(slot) = self.tiers.get_mut(index) else {
                    return Err(EditError { index, tier_count });
                };
                let previous = std::mem::replace(slot, tier);
                Ok(Edit::ModifyTier {
                    index,
                    tier: previous,
                })
            }
            Edit::SetConstraint { index, constraint } => {
                let Some(slot) = self.tiers.get_mut(index) else {
                    return Err(EditError { index, tier_count });
                };
                let previous = std::mem::replace(&mut slot.constraint, constraint);
                Ok(Edit::SetConstraint {
                    index,
                    constraint: previous,
                })
            }
            Edit::SetIndices {
                index,
                indices,
                detached,
            } => {
                let Some(slot) = self.tiers.get_mut(index) else {
                    return Err(EditError { index, tier_count });
                };
                let previous_indices = std::mem::replace(&mut slot.indices, indices);
                let previous_detached = std::mem::replace(&mut slot.detached, detached);
                Ok(Edit::SetIndices {
                    index,
                    indices: previous_indices,
                    detached: previous_detached,
                })
            }
            Edit::SetPreform { preform } => {
                let previous = std::mem::replace(&mut self.preform, preform);
                Ok(Edit::SetPreform { preform: previous })
            }
            Edit::SetPreformYOffset { y_offset } => {
                let previous = std::mem::replace(&mut self.preform_y_offset, y_offset);
                Ok(Edit::SetPreformYOffset { y_offset: previous })
            }
            Edit::SetGirdleDiameterMm { girdle_diameter_mm } => {
                let previous = std::mem::replace(&mut self.girdle_diameter_mm, girdle_diameter_mm);
                Ok(Edit::SetGirdleDiameterMm {
                    girdle_diameter_mm: previous,
                })
            }
            Edit::SetMaterial { material } => {
                let previous = std::mem::replace(&mut self.material, material);
                Ok(Edit::SetMaterial { material: previous })
            }
            Edit::SetMeta {
                headers,
                footnotes,
                gear_reference_angle,
            } => Ok(self.apply_set_meta(headers, footnotes, gear_reference_angle)),
            Edit::SetSchedule {
                gear_teeth,
                symmetry_order,
                mirror,
            } => Ok(self.apply_set_schedule(gear_teeth, symmetry_order, mirror)),
            Edit::RemapIndices {
                from_gear,
                to_gear,
                rounding,
            } => Ok(self.apply_remap_indices(from_gear, to_gear, rounding)),
            Edit::RestoreIndices { tiers } => self.apply_restore_indices(tiers, tier_count),
            Edit::RetargetAngles { changes } => self.apply_retarget_angles(changes, tier_count),
            Edit::SetCheaterOffset { index, offset_deg } => {
                self.apply_set_cheater_offset(index, offset_deg, tier_count)
            }
            Edit::SetTierNote { index, note } => self.apply_set_tier_note(index, note, tier_count),
            Edit::SetTierTarget { index, target } => {
                self.apply_set_tier_target(index, target, tier_count)
            }
            Edit::Batch(edits) => self.apply_batch(edits),
        }
    }

    /// Grows [`Design::tier_ids`] with freshly allocated ids until it matches
    /// [`Design::tiers`]' current length, or truncates it if `tiers` somehow
    /// got SHORTER than `tier_ids` (defensive only -- nothing in this crate
    /// removes a tier without also removing its id via [`Self::apply_edit`]
    /// itself; only a direct `tiers.truncate()`/`tiers.pop()` outside `Edit`
    /// could cause this side). See [`Self::apply_edit`]'s own doc comment for
    /// why this exists at all: a caller that pushes onto `self.tiers` directly
    /// (this crate's own test fixtures do, throughout) leaves `tier_ids` behind,
    /// and every `Edit` arm that indexes or pops `tier_ids` by position needs
    /// the two back in lockstep before it runs.
    fn sync_tier_ids(&mut self) {
        match self.tier_ids.len().cmp(&self.tiers.len()) {
            std::cmp::Ordering::Less => {
                while self.tier_ids.len() < self.tiers.len() {
                    let id = self.allocate_tier_id();
                    self.tier_ids.push(id);
                }
            }
            std::cmp::Ordering::Greater => self.tier_ids.truncate(self.tiers.len()),
            std::cmp::Ordering::Equal => {}
        }
    }

    /// [`Edit::RemoveTier`]'s own apply/inverse half, split out of
    /// [`Self::apply_edit`] purely to stay under clippy's `too_many_lines` (same
    /// reasoning as [`Self::apply_set_schedule`]). Restoring the removed tier's own
    /// cheater offset and/or note (if either was set) each needs a further step
    /// beyond a plain `AddTier` inverse -- see the inline comment below -- so the
    /// inverse is a multi-step [`Edit::Batch`] whenever either was recorded, else
    /// the plain `AddTier` every other tier removal already used.
    fn apply_remove_tier(&mut self, index: usize, tier_count: usize) -> Result<Edit, EditError> {
        if index >= tier_count {
            return Err(EditError { index, tier_count });
        }
        let removed = self.tiers.remove(index);
        let removed_id = self.tier_ids.remove(index);
        let removed_target = self.tier_targets.remove(&removed_id);
        let removed_offset = self.shift_cheater_offsets_for_remove(index);
        let removed_note = self.shift_tier_notes_for_remove(index);
        let mut steps = vec![Edit::AddTier {
            index,
            tier: removed,
        }];
        // Restore the removed tier's own cheater offset/note/target (if any) as
        // further steps: `AddTier`'s own apply allocates a FRESH `TierId` (never
        // the removed one, since a `TierId` is never reused -- see that type's own
        // doc comment) and only reindexes every OTHER positional entry (see
        // `Self::shift_cheater_offsets_for_insert`/`Self::shift_tier_notes_for_
        // insert`), so the specific values have to be set back explicitly,
        // resolved against whichever id now occupies `index`, to reproduce exact
        // pre-removal state on undo.
        if let Some(offset_deg) = removed_offset {
            steps.push(Edit::SetCheaterOffset {
                index,
                offset_deg: Some(offset_deg),
            });
        }
        if let Some(note) = removed_note {
            steps.push(Edit::SetTierNote {
                index,
                note: Some(note),
            });
        }
        if let Some(target) = removed_target {
            steps.push(Edit::SetTierTarget {
                index,
                target: Some(target),
            });
        }
        Ok(if steps.len() == 1 {
            steps.remove(0)
        } else {
            Edit::Batch(steps)
        })
    }

    /// [`Edit::SetMeta`]'s own apply/inverse half, split out of
    /// [`Self::apply_edit`] purely to stay under clippy's `too_many_lines`. Never
    /// fails (no tier index involved), so this returns the inverse [`Edit`]
    /// directly rather than a `Result`.
    const fn apply_set_meta(
        &mut self,
        headers: Vec<String>,
        footnotes: Vec<String>,
        gear_reference_angle: f64,
    ) -> Edit {
        let previous_headers = std::mem::replace(&mut self.meta.headers, headers);
        let previous_footnotes = std::mem::replace(&mut self.meta.footnotes, footnotes);
        let previous_gear_reference_angle =
            std::mem::replace(&mut self.meta.gear_reference_angle, gear_reference_angle);
        Edit::SetMeta {
            headers: previous_headers,
            footnotes: previous_footnotes,
            gear_reference_angle: previous_gear_reference_angle,
        }
    }

    /// [`Edit::SetCheaterOffset`]'s own apply/inverse half, split out of
    /// [`Self::apply_edit`] purely to stay under clippy's `too_many_lines`.
    fn apply_set_cheater_offset(
        &mut self,
        index: usize,
        offset_deg: Option<f64>,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        if index >= tier_count {
            return Err(EditError { index, tier_count });
        }
        let previous = match offset_deg {
            Some(deg) => self.cheater_offsets_deg.insert(index, deg),
            None => self.cheater_offsets_deg.remove(&index),
        };
        Ok(Edit::SetCheaterOffset {
            index,
            offset_deg: previous,
        })
    }

    /// [`Edit::SetTierNote`]'s own apply/inverse half, split out of
    /// [`Self::apply_edit`] purely to stay under clippy's `too_many_lines`.
    fn apply_set_tier_note(
        &mut self,
        index: usize,
        note: Option<String>,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        if index >= tier_count {
            return Err(EditError { index, tier_count });
        }
        let previous = match note {
            Some(text) => self.tier_notes.insert(index, text),
            None => self.tier_notes.remove(&index),
        };
        Ok(Edit::SetTierNote {
            index,
            note: previous,
        })
    }

    /// [`Edit::SetTierTarget`]'s own apply/inverse half, split out of
    /// [`Self::apply_edit`] purely to stay under clippy's `too_many_lines`. Keyed
    /// by the current occupant's [`crate::design::TierId`] (via
    /// [`Design::tier_id_at`]), so -- unlike [`Self::apply_set_cheater_offset`]/
    /// [`Self::apply_set_tier_note`] -- this never needs renumbering on
    /// `AddTier`/`RemoveTier`/`MoveTier`.
    fn apply_set_tier_target(
        &mut self,
        index: usize,
        target: Option<crate::design::TierTarget>,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        let Some(id) = self.tier_id_at(index) else {
            return Err(EditError { index, tier_count });
        };
        let previous = match target {
            Some(t) => self.tier_targets.insert(id, t),
            None => self.tier_targets.remove(&id),
        };
        Ok(Edit::SetTierTarget {
            index,
            target: previous,
        })
    }

    /// Renumbers [`Design::cheater_offsets_deg`] for a tier just INSERTED at
    /// `index`: every entry at `index` or later moves up one position, exactly
    /// matching `self.tiers.insert(index, ..)`'s own effect on positions.
    /// Iterates from the highest key down so no entry overwrites another
    /// before it is itself moved.
    fn shift_cheater_offsets_for_insert(&mut self, index: usize) {
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
    fn shift_cheater_offsets_for_remove(&mut self, index: usize) -> Option<f64> {
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
    fn shift_cheater_offsets_for_move(&mut self, from: usize, to: usize) {
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
    fn shift_tier_notes_for_insert(&mut self, index: usize) {
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
    fn shift_tier_notes_for_remove(&mut self, index: usize) -> Option<String> {
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
    fn shift_tier_notes_for_move(&mut self, from: usize, to: usize) {
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

    /// [`Edit::Batch`]'s own apply/inverse half. Validates the WHOLE batch against a
    /// scratch clone of `self` -- never `self` itself -- before writing anything back,
    /// so a sub-edit that fails partway through (an out-of-range tier index) leaves
    /// `self` completely untouched, matching every other variant's "without modifying
    /// `self`" error contract. The inverse is each sub-edit's own real inverse (as
    /// [`Design::apply_edit`] computed it against the trial clone, so it undoes
    /// exactly what was actually done, not what the caller asked for), collected in
    /// REVERSE order -- undoing a batch unwinds it back to front, the standard rule.
    fn apply_batch(&mut self, edits: Vec<Edit>) -> Result<Edit, EditError> {
        let mut trial = self.clone();
        let mut inverses = Vec::with_capacity(edits.len());
        for edit in edits {
            inverses.push(trial.apply_edit(edit)?);
        }
        *self = trial;
        inverses.reverse();
        Ok(Edit::Batch(inverses))
    }

    /// [`Edit::MoveTier`]'s own apply/inverse half. `to == tier_count` is rejected
    /// exactly like `RemoveTier`'s own out-of-range check: `to` names a position in
    /// the CURRENT (pre-move) list, and `tier_count - 1` is already the last valid
    /// one -- see the variant's own doc comment. Moving a tier to its own current
    /// position (`from == to`) is accepted as a no-op rather than special-cased: a
    /// plain remove-then-insert already reproduces the original list unchanged in
    /// that case.
    fn apply_move_tier(
        &mut self,
        from: usize,
        to: usize,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        if from >= tier_count {
            return Err(EditError {
                index: from,
                tier_count,
            });
        }
        if to >= tier_count {
            return Err(EditError {
                index: to,
                tier_count,
            });
        }
        let tier = self.tiers.remove(from);
        self.tiers.insert(to, tier);
        let id = self.tier_ids.remove(from);
        self.tier_ids.insert(to, id);
        self.shift_cheater_offsets_for_move(from, to);
        self.shift_tier_notes_for_move(from, to);
        Ok(Edit::MoveTier { from: to, to: from })
    }

    /// [`Edit::SetSchedule`]'s own apply/inverse half, split out of
    /// [`Self::apply_edit`] purely to stay under clippy's `too_many_lines`. Never
    /// fails (no tier index involved), so this returns the inverse [`Edit`]
    /// directly rather than a `Result`.
    const fn apply_set_schedule(
        &mut self,
        gear_teeth: i32,
        symmetry_order: u32,
        mirror: bool,
    ) -> Edit {
        let previous = (
            self.meta.gear_teeth,
            self.meta.symmetry_order,
            self.meta.mirror,
        );
        self.meta.gear_teeth = gear_teeth;
        self.meta.symmetry_order = symmetry_order;
        self.meta.mirror = mirror;
        Edit::SetSchedule {
            gear_teeth: previous.0,
            symmetry_order: previous.1,
            mirror: previous.2,
        }
    }

    /// [`Edit::RemapIndices`]'s own apply half -- see that variant's own doc
    /// comment for why the inverse is [`Edit::RestoreIndices`] (a verbatim
    /// snapshot) rather than a reverse remap. Never fails (touches every current
    /// tier uniformly, names no specific index), so this returns the inverse
    /// [`Edit`] directly.
    fn apply_remap_indices(
        &mut self,
        from_gear: i32,
        to_gear: i32,
        rounding: RemapRounding,
    ) -> Edit {
        let tiers = self
            .tiers
            .iter()
            .enumerate()
            .map(|(index, tier)| (index, tier.indices.clone(), tier.detached.clone()))
            .collect();
        for tier in &mut self.tiers {
            tier.indices = tier
                .indices
                .iter()
                .map(|&idx| remap_index(idx, from_gear, to_gear, rounding))
                .collect();
            tier.detached = tier
                .detached
                .iter()
                .map(|&idx| remap_index(idx, from_gear, to_gear, rounding))
                .collect();
        }
        Edit::RestoreIndices { tiers }
    }

    /// [`Edit::RestoreIndices`]'s own apply/inverse half. Validates every named
    /// index BEFORE mutating anything, matching [`Self::apply_edit`]'s own
    /// contract ("without modifying `self`" on error) for every other
    /// index-bearing variant.
    fn apply_restore_indices(
        &mut self,
        tiers: Vec<(usize, Vec<f64>, Vec<f64>)>,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        for &(index, _, _) in &tiers {
            if index >= tier_count {
                return Err(EditError { index, tier_count });
            }
        }
        let mut previous = Vec::with_capacity(tiers.len());
        for (index, indices, detached) in tiers {
            let slot = &mut self.tiers[index];
            let previous_indices = std::mem::replace(&mut slot.indices, indices);
            let previous_detached = std::mem::replace(&mut slot.detached, detached);
            previous.push((index, previous_indices, previous_detached));
        }
        // When an entry list names the same tier twice (latent today -- every real
        // caller dedupes by construction, but `Edit` is public), the undo must
        // reverse the exact order of application, same as `Edit::Batch`'s own
        // sub-inverses -- otherwise replaying `previous` forward re-derives the wrong
        // intermediate value for a repeated index.
        previous.reverse();
        Ok(Edit::RestoreIndices { tiers: previous })
    }

    /// [`Edit::RetargetAngles`]'s own apply/inverse half. See that variant's own
    /// doc comment for why the inverse is built from each tier's ACTUAL previous
    /// `angle_deg`, not the caller-supplied `old_deg`.
    fn apply_retarget_angles(
        &mut self,
        changes: Vec<(usize, f64, f64)>,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        for &(index, _, _) in &changes {
            if index >= tier_count {
                return Err(EditError { index, tier_count });
            }
        }
        let mut inverse = Vec::with_capacity(changes.len());
        for (index, _old_deg, new_deg) in changes {
            let slot = &mut self.tiers[index];
            let actual_previous = std::mem::replace(&mut slot.angle_deg, new_deg);
            inverse.push((index, new_deg, actual_previous));
        }
        // Same as `apply_restore_indices` -- when `changes` names the same tier
        // index twice, undo in reverse order, or replaying `inverse` forward
        // re-applies an intermediate value instead of the tier's true original
        // angle.
        inverse.reverse();
        Ok(Edit::RetargetAngles { changes: inverse })
    }
}
