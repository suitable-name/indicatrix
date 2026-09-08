//! [`Design::apply_edit`]: turning one [`Edit`] into its own inverse while
//! mutating `self` in place -- the one primitive [`super::History`] is built
//! on.

use super::edit_type::{Edit, EditError, RemapRounding};
use crate::design::Design;

/// Converts one index-wheel position from `from_gear` teeth to `to_gear` teeth,
/// rounding per `rounding` -- see [`Edit::RemapIndices`]'s own doc comment.
/// `from_gear == 0` (never a valid real gear, but not rejected by this crate's own
/// types either) is treated as a no-op ratio (`1.0`) rather than dividing by zero,
/// so this always returns a finite number for any finite input.
fn remap_index(index: f64, from_gear: i32, to_gear: i32, rounding: RemapRounding) -> f64 {
    let ratio = if from_gear == 0 {
        1.0
    } else {
        f64::from(to_gear) / f64::from(from_gear)
    };
    let scaled = index * ratio;
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
        let tier_count = self.tiers.len();
        match edit {
            Edit::AddTier { index, tier } => {
                if index > tier_count {
                    return Err(EditError { index, tier_count });
                }
                self.tiers.insert(index, tier);
                Ok(Edit::RemoveTier { index })
            }
            Edit::RemoveTier { index } => {
                if index >= tier_count {
                    return Err(EditError { index, tier_count });
                }
                let removed = self.tiers.remove(index);
                Ok(Edit::AddTier {
                    index,
                    tier: removed,
                })
            }
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
        }
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
        Ok(Edit::RetargetAngles { changes: inverse })
    }
}
