//! Frosted tiers and facets: which parts of a design are authored as frosted (a diffusely
//! scattering finish instead of a polished one) rather than polished.
//!
//! # Where it lives
//!
//! A [`TierFrosting`] sits in [`Design::tier_frosting`], keyed by the tier's [`TierId`] exactly
//! like [`Design::tier_targets`] and [`Design::tier_relations`], so it survives add, remove,
//! move and undo, and widening [`super::ConstraintTier`] is not needed. A tier absent from the
//! map is polished. A design without any frosting has an empty map, which keeps its behavior
//! and the bytes of its file and its scenes exactly as they were.
//!
//! Frosting is authored content: it participates in `PartialEq` (by tier position, like the
//! other id-keyed annotations) and changes only through the undoable
//! [`crate::edit::Edit::SetTierFrosting`]. It changes no geometry, so it never needs a solve.
//!
//! # Whole tiers and single facets
//!
//! A tier is frosted as a whole ([`TierFrosting::Tier`], the "Frosted" toggle of a tier, and
//! what Gem Cut Studio's tier-level `frosting` states) or only at some of its facets
//! ([`TierFrosting::Facets`], the index-wheel positions of the frosted ones, which is what a
//! per-facet `frosting` attribute of a `.gcs` file states). A facet is named by its index-wheel
//! position, the same way [`super::ConstraintTier::detached`] names a detached facet.
//!
//! # From the design to the renderer
//!
//! The renderer wants one finish per facet plane. [`Design::frosted_planes_from_solved`] gives
//! the indices of the frosted planes in [`Design::planes_from_solved`]'s arrangement, and
//! [`Design::frosted_planes_in`] the same for any arrangement built from the same schedule (the
//! Cut slider's partial stones). `.asc` has no notation for frosting, so none of this reaches
//! an exported `.asc` schedule.

use super::{Design, TierId};
use glam::DVec3;
use indicatrix::geometry::{cuts::StandardGemCuts, meet_solver::SolvedTier};
use indicatrix_formats::gcs::GcsTierFrosting;
use std::collections::{BTreeMap, BTreeSet};

/// Two index-wheel positions this close are the same facet.
const SAME_INDEX_EPSILON: f64 = 1e-6;

/// How one tier is frosted. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub enum TierFrosting {
    /// Every facet of the tier.
    Tier,
    /// Only the facets at these index-wheel positions: ascending, without repeats, never
    /// empty ([`Self::of_facets`] keeps that true).
    Facets(Vec<f64>),
}

/// Every frosted tier of a design, keyed by the tier's [`TierId`].
pub type TierFrostingMap = BTreeMap<TierId, TierFrosting>;

impl TierFrosting {
    /// The frosting of the facets at `positions`, `None` when no position is left.
    ///
    /// Finite values only, ascending, repeats (within [`SAME_INDEX_EPSILON`]) merged. A tier
    /// with no frosted facet is simply polished.
    #[must_use]
    pub fn of_facets(positions: impl IntoIterator<Item = f64>) -> Option<Self> {
        let mut list: Vec<f64> = positions.into_iter().filter(|p| p.is_finite()).collect();
        list.sort_by(f64::total_cmp);
        list.dedup_by(|later, earlier| (*later - *earlier).abs() < SAME_INDEX_EPSILON);
        (!list.is_empty()).then_some(Self::Facets(list))
    }

    /// This frosting in canonical form (see [`Self::of_facets`]); `None` when it frosts
    /// nothing.
    #[must_use]
    pub fn normalised(self) -> Option<Self> {
        match self {
            Self::Tier => Some(Self::Tier),
            Self::Facets(list) => Self::of_facets(list),
        }
    }

    /// Whether the facet at index-wheel position `index` is frosted.
    #[must_use]
    pub fn covers(&self, index: f64) -> bool {
        match self {
            Self::Tier => true,
            Self::Facets(list) => list.iter().any(|p| (p - index).abs() < SAME_INDEX_EPSILON),
        }
    }

    /// This frosting with every facet position mapped through `map` (a gear change moves the
    /// positions); a whole-tier frosting is unchanged.
    #[must_use]
    pub fn with_positions_mapped(&self, map: impl Fn(f64) -> f64) -> Option<Self> {
        match self {
            Self::Tier => Some(Self::Tier),
            Self::Facets(list) => Self::of_facets(list.iter().map(|&p| map(p))),
        }
    }
}

impl Design {
    /// How the tier CURRENTLY at `index` is frosted, `None` when it is polished.
    #[must_use]
    pub fn tier_frosting(&self, index: usize) -> Option<&TierFrosting> {
        self.tier_ids
            .get(index)
            .and_then(|id| self.tier_frosting.get(id))
    }

    /// Whether the tier CURRENTLY at `index` is frosted as a whole.
    #[must_use]
    pub fn tier_is_frosted(&self, index: usize) -> bool {
        matches!(self.tier_frosting(index), Some(TierFrosting::Tier))
    }

    /// Whether the tier CURRENTLY at `index` has any frosted facet (the whole tier or some
    /// of its facets).
    #[must_use]
    pub fn tier_has_frosting(&self, index: usize) -> bool {
        self.tier_frosting(index).is_some()
    }

    /// The frosting the tier at `index` would have with one facet flipped.
    ///
    /// The facet at index-wheel position `position` goes between frosted and polished, the other
    /// facets as they are now. `None` when that leaves nothing frosted; the whole-tier frosting
    /// ([`TierFrosting::Tier`]) when it leaves every facet of the tier frosted. A position that
    /// is not one of the tier's facets changes nothing.
    ///
    /// What a per-facet chip's toggle applies (through [`crate::edit::Edit::SetTierFrosting`]).
    #[must_use]
    pub fn frosting_with_facet_toggled(&self, index: usize, position: f64) -> Option<TierFrosting> {
        let current = self.tier_frosting(index).cloned();
        let Some(tier) = self.tiers.get(index) else {
            return current;
        };
        let covered = |at: f64| current.as_ref().is_some_and(|frosting| frosting.covers(at));
        let is_target = |at: f64| (at - position).abs() < SAME_INDEX_EPSILON;
        if !tier.indices.iter().copied().any(is_target) {
            return current;
        }
        let frosted_now = covered(position);
        let after: Vec<f64> = tier
            .indices
            .iter()
            .copied()
            .filter(|&at| {
                if is_target(at) {
                    !frosted_now
                } else {
                    covered(at)
                }
            })
            .collect();
        if after.len() == tier.indices.len() {
            return Some(TierFrosting::Tier);
        }
        TierFrosting::of_facets(after)
    }

    /// What the tier table says about the frosting of the tier at `index`.
    ///
    /// `""` for a polished tier, `"Frosted"` for a whole frosted tier and `"Frosted: 2 of 8"`
    /// (frosted facets of the tier's facets) for a tier frosted facet by facet.
    #[must_use]
    pub fn tier_frosting_text(&self, index: usize) -> String {
        match self.tier_frosting(index) {
            None => String::new(),
            Some(TierFrosting::Tier) => "Frosted".to_owned(),
            Some(frosting) => {
                let facets = self.tiers.get(index).map_or(0, |tier| tier.indices.len());
                let frosted = self.tiers.get(index).map_or(0, |tier| {
                    tier.indices
                        .iter()
                        .filter(|&&at| frosting.covers(at))
                        .count()
                });
                format!("Frosted: {frosted} of {facets}")
            }
        }
    }

    /// Whether any tier of the design is frosted.
    #[must_use]
    pub fn has_frosting(&self) -> bool {
        !self.tier_frosting.is_empty()
    }

    /// [`PartialEq`]'s check for the frosting: equal by tier position, so two designs that
    /// frost the same tiers compare equal whatever their id numbers.
    pub(super) fn tier_frosting_eq(&self, other: &Self) -> bool {
        if self.tier_frosting.is_empty() && other.tier_frosting.is_empty() {
            return true;
        }
        self.tiers.len() == other.tiers.len()
            && (0..self.tiers.len())
                .all(|index| self.tier_frosting(index) == other.tier_frosting(index))
    }

    /// The frosted facet planes of this design, as indices into
    /// [`Self::planes_from_solved`]'s arrangement (the preform's planes come first), ascending.
    ///
    /// Empty when the design has no frosting, and when `solved` does not fit the tiers. A
    /// tier's facets are matched to planes through the schedule's own plane provenance, so a
    /// facet whose plane was deduplicated against an earlier tier's identical plane frosts
    /// that one retained plane.
    #[must_use]
    pub fn frosted_planes_from_solved(&self, solved: &[SolvedTier]) -> Vec<u32> {
        if self.tier_frosting.is_empty() {
            return Vec::new();
        }
        let Ok(schedule) = self.try_to_asc_schedule_from_solved(solved) else {
            return Vec::new();
        };
        let provenance = StandardGemCuts::asc_plane_provenance(&schedule);
        let preform_len = self.preform.planes().len();
        let mut frosted = BTreeSet::new();
        for (index, tier) in self.tiers.iter().enumerate() {
            let (Some(frosting), Some(planes)) = (self.tier_frosting(index), provenance.get(index))
            else {
                continue;
            };
            for (slot, &plane) in planes.iter().enumerate() {
                let covered = match frosting {
                    TierFrosting::Tier => true,
                    TierFrosting::Facets(_) => tier
                        .indices
                        .get(slot)
                        .is_some_and(|&position| frosting.covers(position)),
                };
                if covered && let Ok(combined) = u32::try_from(preform_len + plane) {
                    frosted.insert(combined);
                }
            }
        }
        frosted.into_iter().collect()
    }

    /// The frosted planes among `planes`, as indices into `planes`, ascending.
    ///
    /// `planes` is an arrangement built from the same schedule as [`Self::planes_from_solved`]
    /// (the whole stone, or a prefix or subset of its tiers the Cut slider shows).
    ///
    /// A plane is frosted when it is bit for bit a frosted plane of the whole stone: every
    /// builder of a partial arrangement resolves a shown tier's plane exactly as the finished
    /// stone does (cheater offsets included), so the match is exact.
    #[must_use]
    pub fn frosted_planes_in(&self, solved: &[SolvedTier], planes: &[(DVec3, f64)]) -> Vec<u32> {
        let whole_frosted = self.frosted_planes_from_solved(solved);
        if whole_frosted.is_empty() {
            return Vec::new();
        }
        let whole = self.planes_from_solved(solved);
        let key = |plane: &(DVec3, f64)| {
            [
                plane.0.x.to_bits(),
                plane.0.y.to_bits(),
                plane.0.z.to_bits(),
                plane.1.to_bits(),
            ]
        };
        let frosted_keys: BTreeSet<[u64; 4]> = whole_frosted
            .iter()
            .filter_map(|&index| usize::try_from(index).ok().and_then(|i| whole.get(i)))
            .map(key)
            .collect();
        planes
            .iter()
            .enumerate()
            .filter(|(_, plane)| frosted_keys.contains(&key(plane)))
            .filter_map(|(index, _)| u32::try_from(index).ok())
            .collect()
    }

    /// The sentence a `.asc` export shows when this design is frosted: `.asc` has no notation
    /// for frosting, so the file is written polished. `None` for a design without frosting.
    #[must_use]
    pub fn asc_frosting_note(&self) -> Option<String> {
        if self.tier_frosting.is_empty() {
            return None;
        }
        let count = self.tier_frosting.len();
        Some(format!(
            "The .asc format has no notation for frosting, so the {count} frosted {} will be \
             written polished. A Gem Cut Studio (.gcs) export or the design file keeps it.",
            if count == 1 { "tier" } else { "tiers" }
        ))
    }

    /// Applies the frosting a `.gcs` file states to this design, replacing any it had.
    ///
    /// The design was built from that file's converted schedule; `frosting` (see
    /// `indicatrix_formats::gcs::gcs_frosting`) names tiers by position and facets by slot in
    /// the tier's `indices`. A tier position or slot past the design is ignored.
    pub fn apply_imported_frosting(&mut self, frosting: &[(usize, GcsTierFrosting)]) {
        self.ensure_tier_ids();
        self.tier_frosting.clear();
        for (position, how) in frosting {
            let (Some(tier), Some(&id)) = (self.tiers.get(*position), self.tier_ids.get(*position))
            else {
                continue;
            };
            let frosted = match how {
                GcsTierFrosting::Whole => Some(TierFrosting::Tier),
                GcsTierFrosting::Facets(slots) => TierFrosting::of_facets(
                    slots
                        .iter()
                        .filter_map(|&slot| tier.indices.get(slot).copied()),
                ),
            };
            if let Some(frosted) = frosted {
                self.tier_frosting.insert(id, frosted);
            }
        }
    }

    /// This design's frosting in the terms the `.gcs` writer takes.
    ///
    /// By tier position, and for a facet-by-facet frosting by slot in the tier's `indices` (see
    /// `indicatrix_formats::gcs::to_gcs_string_with_frosting`). Empty for a design without
    /// frosting, which makes the writer's output exactly what it was before frosting existed.
    #[must_use]
    pub fn gcs_frosting_spec(&self) -> Vec<(usize, GcsTierFrosting)> {
        if self.tier_frosting.is_empty() {
            return Vec::new();
        }
        self.tiers
            .iter()
            .enumerate()
            .filter_map(|(position, tier)| {
                let how = match self.tier_frosting(position)? {
                    TierFrosting::Tier => GcsTierFrosting::Whole,
                    frosting @ TierFrosting::Facets(_) => {
                        let slots: Vec<usize> = tier
                            .indices
                            .iter()
                            .enumerate()
                            .filter(|&(_, &index)| frosting.covers(index))
                            .map(|(slot, _)| slot)
                            .collect();
                        if slots.is_empty() {
                            return None;
                        }
                        GcsTierFrosting::Facets(slots)
                    }
                };
                Some((position, how))
            })
            .collect()
    }
}
