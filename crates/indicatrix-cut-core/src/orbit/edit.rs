//! [`Design`]'s orbit-editing methods: membership (add/remove, expanding or
//! collapsing a whole orbit unit at once) and detach/reattach (the escape
//! hatch for a design's genuine, deliberate asymmetry) -- see the parent
//! module's doc comment for why membership edits always operate on whole
//! orbit units while detach/reattach operate per occurrence.

use super::model::{
    INDEX_TOLERANCE, OrbitUnit, expected_orbit, mirror_indices, orbit_units, ring_distance,
    rotate_indices,
};
use crate::{
    design::Design,
    edit::{Edit, EditError},
};

/// `true` iff `a` and `b` are the same index-wheel occurrence, within
/// [`INDEX_TOLERANCE`]. Index values live on the gear ring (size
/// `gear_teeth_abs`), so this compares them with [`ring_distance`] --
/// a linear `(a - b).abs()` would miss the wrap where a position pinned
/// at (or near) `0` and one at `gear_teeth_abs - epsilon` are the same
/// physical tooth (see [`expected_orbit`]'s use of `rem_euclid`).
/// `gear_teeth_abs == 0` means the schedule states no usable gear (see
/// [`orbit_units`]'s degenerate case), so there is no ring to wrap
/// around and this falls back to the plain linear comparison.
fn same_index(a: f64, b: f64, gear_teeth_abs: u32) -> bool {
    let gear = f64::from(gear_teeth_abs);
    if gear > 0.0 {
        ring_distance(a, b, gear) < INDEX_TOLERANCE
    } else {
        (a - b).abs() < INDEX_TOLERANCE
    }
}

impl Design {
    /// The orbit units the tier at `tier_index` currently decomposes into -- see
    /// the module docs for what a unit is and how common each shape is across the
    /// real catalogue.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn orbit_units(&self, tier_index: usize) -> Result<Vec<OrbitUnit>, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        Ok(orbit_units(&tier.indices, &self.meta))
    }

    /// Builds the [`Edit`] that adds one facet occurrence at `position` to the
    /// tier at `tier_index`, **expanded to its complete orbit**: every position the
    /// symmetry model says that facet's orbit needs (see [`expected_orbit`]),
    /// skipping whatever the tier already lists. An addition can therefore never
    /// leave a half-populated orbit unit behind. Positions already present (and
    /// their `detached` status) are left untouched.
    ///
    /// Only computes the [`Edit`]; apply it via [`crate::edit::History::apply`]
    /// like any other edit (`History` stays the sole mutator of [`Design`]).
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn add_orbit_member(&self, tier_index: usize, position: f64) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let wanted = expected_orbit(
            position,
            self.meta.symmetry_order,
            self.meta.mirror,
            self.meta.gear_teeth_abs(),
        );
        let gear_teeth_abs = self.meta.gear_teeth_abs();
        let mut indices = tier.indices.clone();
        for w in wanted {
            if !indices.iter().any(|&v| same_index(v, w, gear_teeth_abs)) {
                indices.push(w);
            }
        }
        indices.sort_by(f64::total_cmp);
        Ok(Edit::SetIndices {
            index: tier_index,
            indices,
            detached: tier.detached.clone(),
        })
    }

    /// Builds the [`Edit`] that removes the occurrence at `position` from the tier
    /// at `tier_index`. If `position` belongs to a unit whose ATTACHED members
    /// alone are complete (every [`Design::detach_orbit_member`]d occurrence in
    /// that unit excluded from both the count and the sweep), every attached
    /// member of that unit is removed with it: deleting "one facet" out of a
    /// clean orbit deletes the facet, not one arbitrary copy, so `symmetry_order`
    /// never silently becomes a lie about what `indices` holds. A detached
    /// `position`, or one belonging to a unit that is not attached-complete (either
    /// genuinely partial, or complete only by counting a detached sibling that
    /// must never be swept), is removed alone.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range or `position` is not
    /// currently among that tier's `indices`.
    pub fn remove_orbit_member(&self, tier_index: usize, position: f64) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let gear_teeth_abs = self.meta.gear_teeth_abs();
        if !tier
            .indices
            .iter()
            .any(|&v| same_index(v, position, gear_teeth_abs))
        {
            return Err(EditError {
                index: tier_index,
                tier_count,
            });
        }
        let is_detached = tier
            .detached
            .iter()
            .any(|&v| same_index(v, position, gear_teeth_abs));
        let to_remove: Vec<f64> = if is_detached {
            vec![position]
        } else {
            let units = orbit_units(&tier.indices, &self.meta);
            units
                .into_iter()
                .find(|u| {
                    u.members
                        .iter()
                        .any(|&m| same_index(m, position, gear_teeth_abs))
                })
                .map_or_else(
                    || vec![position],
                    |u| {
                        // A detached occurrence has already left orbit-wide
                        // propagation (that is the whole point of detaching it), so it
                        // must never be swept along with the rest of the unit -- and
                        // its absence means the unit is no longer "clean" for the
                        // completeness check either: completeness is judged over the
                        // ATTACHED members alone, not `u.expected_len` against the raw
                        // (detached-included) member count `OrbitUnit::is_complete`
                        // would use.
                        let OrbitUnit {
                            members,
                            expected_len,
                        } = u;
                        let attached: Vec<f64> = members
                            .into_iter()
                            .filter(|&m| {
                                !tier
                                    .detached
                                    .iter()
                                    .any(|&d| same_index(d, m, gear_teeth_abs))
                            })
                            .collect();
                        if attached.len() == expected_len {
                            attached
                        } else {
                            vec![position]
                        }
                    },
                )
        };
        let indices: Vec<f64> = tier
            .indices
            .iter()
            .copied()
            .filter(|&v| !to_remove.iter().any(|&r| same_index(r, v, gear_teeth_abs)))
            .collect();
        let detached: Vec<f64> = tier
            .detached
            .iter()
            .copied()
            .filter(|&v| !to_remove.iter().any(|&r| same_index(r, v, gear_teeth_abs)))
            .collect();
        Ok(Edit::SetIndices {
            index: tier_index,
            indices,
            detached,
        })
    }

    /// Builds the [`Edit`] that marks `position` (an occurrence in the tier at
    /// `tier_index`) detached from orbit-wide propagation -- the visible,
    /// deliberate act needed before a real design's intentional asymmetry (see the
    /// module docs' `partial`/`mixed_fold` cases) can be edited one occurrence at a
    /// time without [`Design::remove_orbit_member`] sweeping its former orbit-mates
    /// along. A no-op edit if `position` is already detached.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range or `position` is not
    /// currently among that tier's `indices`.
    pub fn detach_orbit_member(&self, tier_index: usize, position: f64) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let gear_teeth_abs = self.meta.gear_teeth_abs();
        if !tier
            .indices
            .iter()
            .any(|&v| same_index(v, position, gear_teeth_abs))
        {
            return Err(EditError {
                index: tier_index,
                tier_count,
            });
        }
        let mut detached = tier.detached.clone();
        if !detached
            .iter()
            .any(|&v| same_index(v, position, gear_teeth_abs))
        {
            detached.push(position);
            detached.sort_by(f64::total_cmp);
        }
        Ok(Edit::SetIndices {
            index: tier_index,
            indices: tier.indices.clone(),
            detached,
        })
    }

    /// The inverse of [`Design::detach_orbit_member`]: rejoins `position`
    /// to orbit-wide propagation. A no-op edit if `position` was not
    /// detached.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn reattach_orbit_member(
        &self,
        tier_index: usize,
        position: f64,
    ) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let gear_teeth_abs = self.meta.gear_teeth_abs();
        let detached: Vec<f64> = tier
            .detached
            .iter()
            .copied()
            .filter(|&v| !same_index(v, position, gear_teeth_abs))
            .collect();
        Ok(Edit::SetIndices {
            index: tier_index,
            indices: tier.indices.clone(),
            detached,
        })
    }

    /// Builds the [`Edit`] that detaches every occurrence currently in the tier at
    /// `tier_index` at once -- the tier-wide convenience `indicatrix-cut`'s single
    /// "Detach" button per row applies. A no-op if every occurrence is already
    /// detached.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn detach_all_in_tier(&self, tier_index: usize) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        Ok(Edit::SetIndices {
            index: tier_index,
            indices: tier.indices.clone(),
            detached: tier.indices.clone(),
        })
    }

    /// The inverse of [`Design::detach_all_in_tier`]: clears the tier at
    /// `tier_index`'s entire detached set, rejoining every occurrence to
    /// orbit-wide propagation.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn reattach_all_in_tier(&self, tier_index: usize) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        Ok(Edit::SetIndices {
            index: tier_index,
            indices: tier.indices.clone(),
            detached: Vec::new(),
        })
    }

    /// Builds the [`Edit`] that rotates every index-wheel position of the
    /// tier at `tier_index` -- both `indices` and `detached` -- by `k_teeth`
    /// around the gear (see [`rotate_indices`]). The routine "move this
    /// break tier half a step" operation a `GemCad`/GCS user expects a single
    /// button for, rather than sixteen numbers computed by hand.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn rotate_indices(&self, tier_index: usize, k_teeth: f64) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let gear_teeth_abs = self.meta.gear_teeth_abs();
        let mut indices = rotate_indices(&tier.indices, k_teeth, gear_teeth_abs);
        let mut detached = rotate_indices(&tier.detached, k_teeth, gear_teeth_abs);
        indices.sort_by(f64::total_cmp);
        detached.sort_by(f64::total_cmp);
        Ok(Edit::SetIndices {
            index: tier_index,
            indices,
            detached,
        })
    }

    /// Builds the [`Edit`] that mirrors every index-wheel position of the
    /// tier at `tier_index` -- both `indices` and `detached` -- to the other
    /// side of the symmetry axis (see [`mirror_indices`]).
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn mirror_indices(&self, tier_index: usize) -> Result<Edit, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let gear_teeth_abs = self.meta.gear_teeth_abs();
        let mut indices = mirror_indices(&tier.indices, gear_teeth_abs);
        let mut detached = mirror_indices(&tier.detached, gear_teeth_abs);
        indices.sort_by(f64::total_cmp);
        detached.sort_by(f64::total_cmp);
        Ok(Edit::SetIndices {
            index: tier_index,
            indices,
            detached,
        })
    }
}
