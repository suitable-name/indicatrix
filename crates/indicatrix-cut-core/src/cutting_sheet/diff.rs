//! [`TierDelta`] and [`diff_tiers`]: a positional before/after tier
//! comparison (e.g. a saved snapshot against a design's current state) for a
//! "compare two designs" view.

use crate::design::{ConcaveTier, ConstraintTier};
use indicatrix::geometry::meet_solver::SolvedTier;

/// One tier's before/after comparison, position by position -- see
/// [`diff_tiers`].
#[derive(Debug, Clone, PartialEq)]
pub struct TierDelta {
    /// Position in both tier lists (see [`diff_tiers`]'s positional-only
    /// contract).
    pub index: usize,
    /// The tier's name in the "after" list, or the "before" list's if the
    /// tier was removed (see [`Self::removed`]).
    pub name: String,
    /// `before`'s angle, or `None` if `index` is past the end of `before`
    /// (this tier was added).
    pub angle_before: Option<f64>,
    /// `after`'s angle, or `None` if `index` is past the end of `after`
    /// (this tier was removed).
    pub angle_after: Option<f64>,
    /// `before`'s index-wheel positions, when this tier existed in `before`.
    pub indices_before: Option<Vec<f64>>,
    /// `after`'s index-wheel positions, when this tier still exists in
    /// `after`.
    pub indices_after: Option<Vec<f64>>,
    /// `before`'s solved mast, when a `before_solved` list was supplied and
    /// this tier existed in `before`.
    pub mast_before: Option<f64>,
    /// `after`'s solved mast, when an `after_solved` list was supplied and
    /// this tier still exists in `after`.
    pub mast_after: Option<f64>,
}

impl TierDelta {
    /// `true` iff this tier exists in both lists and its angle actually
    /// changed.
    #[must_use]
    pub fn angle_changed(&self) -> bool {
        match (self.angle_before, self.angle_after) {
            (Some(before), Some(after)) => (before - after).abs() > f64::EPSILON,
            _ => false,
        }
    }

    /// `true` iff this tier exists in both lists and its index-wheel
    /// positions actually changed.
    #[must_use]
    pub fn indices_changed(&self) -> bool {
        matches!((&self.indices_before, &self.indices_after), (Some(b), Some(a)) if b != a)
    }

    /// `true` iff both masts are known and differ by more than `tolerance`
    /// (model units) -- a caller compares against a tolerance rather than
    /// exact equality since a solved mast is a floating-point result, not an
    /// authored value.
    #[must_use]
    pub fn mast_changed(&self, tolerance: f64) -> bool {
        match (self.mast_before, self.mast_after) {
            (Some(before), Some(after)) => (before - after).abs() > tolerance,
            _ => false,
        }
    }

    /// `true` iff `index` is past the end of the "before" list -- this tier
    /// was added.
    #[must_use]
    pub const fn added(&self) -> bool {
        self.angle_before.is_none()
    }

    /// `true` iff `index` is past the end of the "after" list -- this tier
    /// was removed.
    #[must_use]
    pub const fn removed(&self) -> bool {
        self.angle_after.is_none()
    }
}

/// Compares two tier lists position by position (e.g. a saved snapshot's
/// `before` against a design's current `after`), producing one [`TierDelta`]
/// per position either list has a tier at.
///
/// **Positional only**: a tier inserted or removed partway through shifts
/// every later position, so this reports every tier from that point on as
/// "changed" rather than following the renamed/moved tier -- the same
/// trade-off a plain `zip` over two `Vec`s always makes. A caller that wants
/// insert/delete-aware alignment (matching tiers by name first) builds that
/// on top of this; nothing here hides the limitation, since `TierDelta` names
/// exactly which positions were compared.
///
/// `before_solved`/`after_solved`, when supplied, must have one entry per
/// tier in `before`/`after` respectively -- the same alignment contract
/// [`crate::design::Design::planes_from_solved`] documents; a mismatched length is treated
/// as "no solved masts" (`mast_before`/`mast_after` stay `None`) rather than
/// panicking, since a diff is a read-only report and should degrade
/// gracefully rather than crash on stale solved state.
#[must_use]
pub fn diff_tiers(
    before: &[ConstraintTier],
    before_solved: Option<&[SolvedTier]>,
    after: &[ConstraintTier],
    after_solved: Option<&[SolvedTier]>,
) -> Vec<TierDelta> {
    let before_masts = before_solved.filter(|s| s.len() == before.len());
    let after_masts = after_solved.filter(|s| s.len() == after.len());
    let len = before.len().max(after.len());
    (0..len)
        .map(|index| {
            let b = before.get(index);
            let a = after.get(index);
            TierDelta {
                index,
                name: a.or(b).map_or_else(String::new, |t| t.name.clone()),
                angle_before: b.map(|t| t.angle_deg),
                angle_after: a.map(|t| t.angle_deg),
                indices_before: b.map(|t| t.indices.clone()),
                indices_after: a.map(|t| t.indices.clone()),
                mast_before: before_masts.and_then(|s| s.get(index)).map(|s| s.mast),
                mast_after: after_masts.and_then(|s| s.get(index)).map(|s| s.mast),
            }
        })
        .collect()
}

/// One concave tier's before/after comparison, position by position -- see
/// [`diff_concave_tiers`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveTierDelta {
    /// Position in both [`crate::design::Design::concave_tiers`] lists.
    pub position: usize,
    /// The tier in `before`, or `None` when it was added.
    pub before: Option<ConcaveTier>,
    /// The tier in `after`, or `None` when it was removed.
    pub after: Option<ConcaveTier>,
}

impl ConcaveTierDelta {
    /// `true` iff the tier exists on both sides and differs in any field
    /// (tool code, θ, displacement, diameter, angle, motion, facet angle,
    /// indices, name).
    #[must_use]
    pub fn changed(&self) -> bool {
        matches!((&self.before, &self.after), (Some(b), Some(a)) if b != a)
    }
}

/// Compares two concave tier lists position by position and reports only the
/// positions that differ.
///
/// A tier present on one side only (added or removed) or present on both with
/// any field changed is reported. Unchanged positions are left out, so two
/// designs that agree on every concave tier compare as an empty list.
///
/// Positional for the same reason [`diff_tiers`] is: inserting a tier shifts
/// every later position. Without this, a compare between two snapshots that
/// differ only in a concave tier would read as "no change".
#[must_use]
pub fn diff_concave_tiers(before: &[ConcaveTier], after: &[ConcaveTier]) -> Vec<ConcaveTierDelta> {
    (0..before.len().max(after.len()))
        .filter_map(|position| {
            let (b, a) = (before.get(position), after.get(position));
            (b != a).then(|| ConcaveTierDelta {
                position,
                before: b.cloned(),
                after: a.cloned(),
            })
        })
        .collect()
}
