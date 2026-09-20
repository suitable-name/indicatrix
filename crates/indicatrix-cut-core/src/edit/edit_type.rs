//! [`Edit`] itself (every reversible change a caller can describe against a
//! [`crate::design::Design`]) and [`EditError`], the one way applying one can
//! fail. See the parent module's doc comment for the command/inverse model
//! this whole crate's undo/redo rests on.

use crate::{
    design::{ConstraintTier, Design},
    material::MaterialSelection,
    preform::PreformSpec,
};
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
    /// Moves the tier currently at position `from` to position `to`, renumbering
    /// every tier strictly between the two positions by one slot -- exactly
    /// `Vec::remove(from)` followed by `Vec::insert(to)`, so `to` is a position in
    /// the ORIGINAL (pre-move) tier list, not a "gap" index in a shortened one.
    /// One undo step, unlike the reorder-by-two-content-swaps an editor's own
    /// "move up"/"move down" buttons used to apply.
    ///
    /// A tier is renumbered, never renamed, by this: [`MeetConstraint::MeetNamed`]
    /// targets a tier by NAME (untouched), and [`ConstraintTier::detached`] is a
    /// field of the tier struct itself, so it travels with the tier automatically.
    /// Nothing in [`crate::design::Design`] outside `tiers` itself stores a tier's
    /// positional index, so a plain remove-then-insert is already fully consistent.
    MoveTier { from: usize, to: usize },
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
    /// Replaces the design's authorable, non-geometric header fields wholesale --
    /// [`crate::design::ScheduleMeta::headers`] (the `.asc` `H` lines, the first of
    /// which the app treats as the design's title -- see the catalogue round-trip
    /// convention), [`crate::design::ScheduleMeta::footnotes`] and
    /// [`crate::design::ScheduleMeta::gear_reference_angle`] (the index wheel's
    /// zero-tooth offset). Deliberately its own variant rather than folded into
    /// `SetSchedule`: `SetSchedule`'s fields feed `solve_meet_points` and always
    /// force a full re-solve (see `crate::resolve`), while none of these three
    /// affect geometry at all -- same "no mast could possibly have changed"
    /// reasoning as `SetPreform`/`SetGirdleDiameterMm`/`SetMaterial`.
    SetMeta {
        headers: Vec<String>,
        footnotes: Vec<String>,
        gear_reference_angle: f64,
    },
    /// Sets (or, when `offset_deg` is `None`, clears) the tier currently at
    /// `index`'s cheater/azimuth offset -- see
    /// [`crate::design::Design::cheater_offsets_deg`]'s own doc comment for
    /// why this lives on `Design`, keyed by position, rather than on
    /// [`ConstraintTier`] itself. Like `ModifyTier`/`SetConstraint`, replaces
    /// exactly one entry in place; `AddTier`/`RemoveTier`/`MoveTier` renumber
    /// every OTHER entry so this one never needs to (see
    /// `crate::design::Design::apply_edit`'s own handling of those three).
    SetCheaterOffset {
        index: usize,
        offset_deg: Option<f64>,
    },
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
    /// Several sub-edits applied as ONE undo step: [`Design::apply_edit`] validates
    /// every sub-edit against a scratch clone of the design before writing anything
    /// back, so a sub-edit that fails partway through never leaves the design
    /// half-applied -- then commits the clone and returns the sub-edits' own inverses,
    /// reversed, as the exact undo (replayed in reverse order, matching the standard
    /// "undo a batch by unwinding it back to front" rule). What the editor uses
    /// wherever two edits are conceptually one user action -- e.g. "Retarget for
    /// material" (an angle retarget plus the material change it implies) -- so
    /// undoing it takes one press, not two.
    Batch(Vec<Self>),
}

impl Edit {
    /// A short, cutter-facing summary of what this [`Edit`] does -- e.g. "Set P1 angle
    /// to -41.0 degrees", "Remap gear 96 to 80", "Remove tier C1", "Optimize 12
    /// tiers" -- rather than a debug dump of the variant's fields. What the editor
    /// feeds [`super::History::peek_undo`]/[`super::History::peek_redo`] through to
    /// label the undo/redo hover hints and Edit menu items, so a cutter can tell what
    /// Ctrl+Z/Ctrl+Y will actually do before pressing it.
    ///
    /// `design` supplies tier names for the variants that only carry a tier's
    /// positional `index` (`RemoveTier`/`ModifyTier`/`SetConstraint`/`SetIndices`) --
    /// looked up against `design`'s CURRENT tier list, so this reads correctly when
    /// called against the same design state the `Edit` itself would apply to (exactly
    /// what `History::peek_undo`/`peek_redo` already promise -- see their own doc
    /// comments). A stale/out-of-range index (defensive only -- every real caller
    /// satisfies that promise) falls back to `"tier {index + 1}"`.
    #[must_use]
    pub fn describe(&self, design: &Design) -> String {
        match self {
            Self::AddTier { tier, .. } => format!("Add tier {}", tier_display_name(&tier.name)),
            Self::RemoveTier { index } => format!("Remove tier {}", tier_label_at(design, *index)),
            Self::MoveTier { from, to } => describe_move_tier(*from, *to, design),
            Self::ModifyTier { index, .. } => {
                format!("Modify tier {}", tier_label_at(design, *index))
            }
            Self::SetConstraint { index, .. } => {
                format!("Change meet target for {}", tier_label_at(design, *index))
            }
            Self::SetIndices { index, .. } => {
                format!(
                    "Change index positions for {}",
                    tier_label_at(design, *index)
                )
            }
            Self::SetPreform { .. } => "Change preform".to_string(),
            Self::SetGirdleDiameterMm { girdle_diameter_mm } => girdle_diameter_mm.map_or_else(
                || "Clear girdle diameter".to_string(),
                |mm| format!("Set girdle diameter to {mm:.2} mm"),
            ),
            Self::SetMaterial { material } => {
                format!("Set material to {}", material_display_name(material))
            }
            Self::SetMeta { .. } => "Change design details".to_string(),
            Self::SetCheaterOffset { index, offset_deg } => {
                let label = tier_label_at(design, *index);
                offset_deg.map_or_else(
                    || format!("Clear cheater offset for {label}"),
                    |deg| format!("Set cheater offset for {label} to {deg:.2} deg"),
                )
            }
            Self::SetSchedule { gear_teeth, .. } => format!("Set gear to {gear_teeth} teeth"),
            Self::RemapIndices {
                from_gear, to_gear, ..
            } => format!("Remap gear {from_gear} to {to_gear}"),
            Self::RestoreIndices { tiers } => {
                format!("Restore index positions for {} tier(s)", tiers.len())
            }
            Self::RetargetAngles { changes } => describe_retarget_angles(changes, design),
            Self::Batch(edits) => describe_batch(edits, design),
        }
    }
}

/// `name`, or a placeholder for an unnamed tier -- shared by every [`Edit::describe`]
/// arm that has a real [`ConstraintTier`] in hand (as opposed to only its index).
fn tier_display_name(name: &str) -> String {
    if name.is_empty() {
        "(unnamed tier)".to_string()
    } else {
        name.to_string()
    }
}

/// [`Edit::describe`]'s tier-name lookup for the variants that only carry a
/// positional `index` -- see that method's own doc comment.
fn tier_label_at(design: &Design, index: usize) -> String {
    design.tiers.get(index).map_or_else(
        || format!("tier {}", index + 1),
        |tier| tier_display_name(&tier.name),
    )
}

/// [`Edit::describe`]'s [`Edit::MoveTier`] arm: names the moved tier (looked up at
/// `from`, its position in the design this move is about to apply to -- see
/// [`Edit::describe`]'s own doc comment for why that's the correct list to read
/// against) and says which direction it travelled. `to == from` is a no-op move
/// (never constructed by a real caller, but not a panic either).
fn describe_move_tier(from: usize, to: usize, design: &Design) -> String {
    let label = tier_label_at(design, from);
    match to.cmp(&from) {
        std::cmp::Ordering::Less => format!("Move tier {label} up"),
        std::cmp::Ordering::Greater => format!("Move tier {label} down"),
        std::cmp::Ordering::Equal => format!("Move tier {label}"),
    }
}

/// [`Edit::describe`]'s label for a [`MaterialSelection`] -- its name when set, else a
/// placeholder (an RI-override-only selection has no catalogue name to show).
fn material_display_name(material: &MaterialSelection) -> String {
    material
        .name
        .clone()
        .unwrap_or_else(|| "(custom material)".to_string())
}

/// [`Edit::describe`]'s [`Edit::RetargetAngles`] arm: a single-tier retarget names
/// that tier and its new angle (the "Set P1 angle to -41.0 degrees" example); several
/// at once is always an Optimize/batch result in practice, so it reads as "Optimize N
/// tiers" instead of an unreadable per-tier list.
fn describe_retarget_angles(changes: &[(usize, f64, f64)], design: &Design) -> String {
    match changes {
        [] => "Retarget angles".to_string(),
        [(index, _, new_deg)] => {
            format!(
                "Set {} angle to {new_deg:.1} degrees",
                tier_label_at(design, *index)
            )
        }
        many => format!("Optimize {} tiers", many.len()),
    }
}

/// [`Edit::describe`]'s [`Edit::Batch`] arm. Special-cases the one composite this
/// crate builds today (a retarget's angle changes plus the material change it
/// implies, `callbacks::retarget_actions::setup_retarget_apply_callback`) so it reads
/// as one clear sentence rather than "2 combined edits"; anything else falls back to
/// delegating a single-edit batch, or a generic count otherwise.
fn describe_batch(edits: &[Edit], design: &Design) -> String {
    if let [Edit::RetargetAngles { .. }, Edit::SetMaterial { material }]
    | [Edit::SetMaterial { material }, Edit::RetargetAngles { .. }] = edits
    {
        return format!("Retarget for {}", material_display_name(material));
    }
    match edits {
        [] => "No-op".to_string(),
        [only] => only.describe(design),
        many => format!("{} combined edits", many.len()),
    }
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
