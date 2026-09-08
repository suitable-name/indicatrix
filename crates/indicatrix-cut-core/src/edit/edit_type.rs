//! [`Edit`] itself (every reversible change a caller can describe against a
//! [`crate::design::Design`]) and [`EditError`], the one way applying one can
//! fail. See the parent module's doc comment for the command/inverse model
//! this whole crate's undo/redo rests on.

use crate::{design::ConstraintTier, material::MaterialSelection, preform::PreformSpec};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// One reversible change to a [`crate::design::Design`].
///
/// Every variant names a tier by its position in `design.tiers` (`0`-based, file
/// order). A position is only meaningful against the schedule state the edit was
/// constructed against -- exactly like a text editor's line numbers -- so
/// [`crate::design::Design::apply_edit`] validates it fresh on every call rather than
/// trusting a stale index, and [`super::History::undo`]/[`super::History::redo`] only
/// ever replay an [`Edit`] this same module already validated and inverted once,
/// against the [`crate::design::Design`] that produced it.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Inserts `tier` at position `index` (`index == design.tiers.len()` appends).
    AddTier { index: usize, tier: ConstraintTier },
    /// Removes the tier currently at position `index`.
    RemoveTier { index: usize },
    /// Replaces the tier at position `index` wholesale with `tier` -- a caller
    /// wanting to change just one field reads the current tier first (e.g.
    /// `design.tiers[index].clone()`), mutates the clone, and passes that back.
    ModifyTier { index: usize, tier: ConstraintTier },
    /// Replaces the tier at position `index`'s [`MeetConstraint`] alone, leaving
    /// angle/name/indices untouched -- what "choose what this facet meets" in the
    /// editor UI applies.
    SetConstraint {
        index: usize,
        constraint: MeetConstraint,
    },
    /// Replaces the tier at `index`'s index-wheel positions and detached set
    /// together, leaving angle/name/constraint untouched -- what
    /// `crate::orbit`'s orbit-membership helpers build. Its own variant rather than
    /// reusing `ModifyTier` since membership changes only ever touch
    /// `indices`/`detached`.
    SetIndices {
        index: usize,
        indices: Vec<f64>,
        detached: Vec<f64>,
    },
    /// Replaces the design's preform wholesale.
    SetPreform { preform: PreformSpec },
    /// Replaces the design's real-world girdle diameter (millimetres) -- see
    /// [`crate::design::Design::girdle_diameter_mm`]. `None` clears the scale
    /// anchor entirely (back to "not anchored yet").
    SetGirdleDiameterMm { girdle_diameter_mm: Option<f64> },
    /// Replaces the design's material selection wholesale -- see
    /// [`crate::material::MaterialSelection`]. Wholesale, not per-field, matching
    /// `ModifyTier`'s own reasoning. The RI override rides inside
    /// `MaterialSelection` itself -- no separate variant.
    SetMaterial { material: MaterialSelection },
    /// Replaces the design's index-gear tooth count, symmetry order and mirror
    /// flag wholesale, the schedule-wide counterpart to `SetMaterial`. Every
    /// tier's own `indices`/`detached` stay numerically unchanged -- see
    /// [`Edit::RemapIndices`] for the edit that re-derives them for a NEW gear;
    /// the editor applies both as two separate `History` steps on a gear-combo
    /// change.
    SetSchedule {
        gear_teeth: i32,
        symmetry_order: u32,
        mirror: bool,
    },
    /// Re-derives every tier's `indices`/`detached` from `from_gear` teeth to
    /// `to_gear` teeth (`new = round(old * to_gear / from_gear)`, per `rounding` --
    /// see [`RemapRounding`]) -- what the editor's gear-remap dialog applies. Lossy
    /// whenever the ratio isn't integral (e.g. 96 -> 80): rather than a lossy
    /// reverse remap on undo, applying this records every tier's REAL previous
    /// `indices`/`detached` and returns [`Edit::RestoreIndices`] as the exact
    /// inverse.
    RemapIndices {
        from_gear: i32,
        to_gear: i32,
        rounding: RemapRounding,
    },
    /// Never constructed by a caller directly -- the exact inverse
    /// [`Edit::RemapIndices`] produces. Restores `indices`/`detached` verbatim for
    /// every tier named in `tiers` (`(index, indices, detached)`), in one atomic
    /// undo step. Public like every other [`Edit`] variant, since
    /// [`super::History`]'s undo/redo stacks hold plain, inspectable [`Edit`] values.
    RestoreIndices {
        tiers: Vec<(usize, Vec<f64>, Vec<f64>)>,
    },
    /// One undoable step retargeting several tiers' angles at once -- what the
    /// "Retarget for material" proposal applies after review, so undo reverts the
    /// whole retarget in one step rather than one tier at a time. Each tuple is
    /// `(index, old_deg, new_deg)`; `old_deg` is caller-facing/diagnostic only --
    /// applying this always sets `angle_deg` to `new_deg` and computes the real
    /// inverse from the tier's own actual previous value.
    RetargetAngles { changes: Vec<(usize, f64, f64)> },
}

/// How [`Edit::RemapIndices`] rounds a non-integral index-wheel position after
/// converting it from `from_gear` teeth to `to_gear` teeth.
///
/// See that variant's own doc comment. This crate never silently picks a rounding
/// mode itself; it is always caller-authored (the editor's own remap dialog shows
/// non-integral remaps in red, letting the user choose before applying).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemapRounding {
    /// Rounds to the nearest whole tooth (`f64::round`, ties away from zero).
    Nearest,
    /// Rounds down (`f64::floor`).
    Floor,
    /// Rounds up (`f64::ceil`).
    Ceil,
}

/// Why an [`Edit`] could not be applied. Always a position out of range for
/// the design's current tier list -- the only way one of these can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    pub index: usize,
    pub tier_count: usize,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tier index {} out of range (schedule has {} tier(s))",
            self.index, self.tier_count
        )
    }
}

impl std::error::Error for EditError {}
