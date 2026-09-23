//! A design: a preform plus an editable cutting schedule of *authored
//! constraints*, and the derivation from that state down to a renderable solid.
//!
//! # Constraints, not masts
//!
//! A faceter does not think "cut this facet to depth 0.6499" -- they think "this
//! facet meets the junction of those three." The mast number a `.asc` file records
//! is the *output* of that intent, not the intent itself. [`ConstraintTier`] runs
//! that forwards instead: the constraint
//! ([`indicatrix::geometry::meet_solver::MeetConstraint`], the exact three-case enum
//! the solver classifies real schedules into) is the field a tier's own mutation
//! methods touch; the mast is never stored on a tier at all, only produced by
//! [`Design::solve`] on demand. This is what makes the editor CAD instead of a text
//! form over `.asc` fields.
//!
//! # Scale anchoring
//!
//! Meet-vertex incidence has a real, corpus-measured blind spot: it cannot recover a
//! block's (crown/pavilion/girdle) own translation along its normal, because a
//! coherent shift of every plane in a block preserves every vertex the block's own
//! facets pass through (see `indicatrix::geometry::meet_solver`'s module docs, "What
//! meets alone cannot determine"). A block therefore needs exactly one
//! [`MeetConstraint::ScaleReference`] tier -- a real authored dimension, matching a
//! real schedule's own "Set stone size"/"Level girdle" instructions -- or
//! [`Design::solve`] cannot produce a mast for anything in that block at all.
//!
//! This crate deliberately does not use
//! [`indicatrix::geometry::meet_solver::apply_ratio_anchors`] (the corpus tool's
//! estimate-from-printed-ratios fallback, 10-40% error): an in-progress editor design
//! has no printed ratios to estimate from either, so the only input it could be fed
//! here would itself be another guess. [`Design::from_asc_schedule`] instead
//! synthesizes a missing block anchor from its own first tier's real recorded mast
//! (not a fitted ratio) while that mast is still available on import -- see that
//! function's doc comment. A block with no tiers at all gets no anchor manufactured
//! either: [`Design::solve`] reports exactly which block(s) are missing one via
//! [`MissingAnchor`], and the editor must ask the user for a real
//! `MeetConstraint::ScaleReference` tier rather than silently picking a number.

mod construct;
mod export;
mod missing_anchor;
mod solve;
mod solve_error;
mod targets;
#[cfg(test)]
mod tests;
mod tier;
mod tier_id;

pub use construct::FreshDesignSpec;
pub(crate) use export::meet_name_is_asc_safe;
pub use missing_anchor::MissingAnchor;
pub use solve_error::{DesignSolveError, SolveMismatch};
pub use targets::{TargetResolveError, TierTarget};
pub use tier::{ConstraintTier, ScheduleMeta};
pub use tier_id::TierId;

use crate::{material::MaterialSelection, preform::PreformSpec};
use std::collections::BTreeMap;
use targets::TierTargetMap;

/// A preform plus the cutting schedule being edited against it.
///
/// `tiers` is authored constraint state (see the module docs); `meta` is every
/// other `.asc` schedule field. There is no single `AscSchedule` field --
/// [`Design::to_asc_schedule`] builds one on demand (solving every tier's mast
/// first), for [`Design::planes`] and for a caller that wants to write a real
/// `.asc` file via `indicatrix_formats::asc::to_asc_string`. [`Design::from_asc_schedule`] is
/// the reverse direction, for loading one.
#[derive(Debug, Clone)]
pub struct Design {
    pub preform: PreformSpec,
    pub meta: ScheduleMeta,
    pub tiers: Vec<ConstraintTier>,
    /// The design's real-world girdle diameter, in millimetres -- the one dimension
    /// a lapidary actually measures with calipers on a finished stone, and this
    /// crate's sole anchor from the otherwise-dimensionless "mast unit" scale every
    /// plane offset uses (see `crate::preform`'s own module doc comment) to a real
    /// physical size. `None` means "not anchored yet": every figure
    /// `crate::yield_metrics` derives from it (mm-scale volume, carat weight) is
    /// then unavailable, while [`crate::yield_metrics::volumetric_yield`] stays
    /// available regardless -- it needs no mm scale at all.
    ///
    /// `crate::yield_metrics::mm_per_unit` is `girdle_diameter_mm` divided by the
    /// SOLVED design's own measured `width_axis`
    /// (`indicatrix::geometry::stone_metrics::SolidMetrics::width_axis`) -- the
    /// finished solid's own widest extent, which is NOT always the girdle band's
    /// width specifically: when the authored facets leave the preform's own side
    /// walls standing (a stone not yet cut out to its rough), those surviving
    /// walls are part of the finished solid too, so `width_axis` reads as the
    /// PREFORM's width, not the actual (narrower) girdle -- see
    /// [`crate::yield_metrics::YieldReport::mm_per_unit`]'s own doc comment for
    /// the resulting understated carat weight this is a real, unfixed binding
    /// for, not a bug in this field.
    ///
    /// # This does NOT round-trip through `.asc`
    ///
    /// `indicatrix_formats::asc::AscSchedule` has no field for a real-world dimension anywhere
    /// -- every number in a `.asc` file is a dimensionless mast. Exporting a design
    /// to `.asc` drops `girdle_diameter_mm` silently, and importing never populates
    /// it either. It lives only in this in-memory `Design` for now; the native
    /// format (`crate::native`) is what carries it to disk.
    pub girdle_diameter_mm: Option<f64>,
    /// How far the preform's own vertical span is shifted from centred, in the
    /// same mast-unit convention as every other plane offset -- see
    /// [`crate::preform::PreformSpec::planes_offset`], which this feeds on every
    /// [`Self::planes`]/[`Self::planes_from_solved`] call. `0.0` (the default)
    /// reproduces [`PreformSpec::planes`]'s own always-centred span exactly.
    ///
    /// A real piece of rough is rarely centred on the girdle a cutter plans to
    /// cut into it; this is the design-level anchor
    /// `PreformSpec` deliberately does not carry itself (see that type's own
    /// doc comment for why widening it would be a breaking change to several
    /// struct-literal call sites this crate does not solely own).
    ///
    /// # Round-tripping through the native format
    ///
    /// Carried as `indicatrix_formats::native::PreformTable::y_offset` (a
    /// `#[serde(default)]` field, so an older sidecar with no such column loads as
    /// `0.0` -- the same always-centred span it actually had); `crate::native`'s
    /// `to_native_file`/`load_paired` mirror it alongside `girdle_diameter_mm`. Like
    /// `girdle_diameter_mm`, it still has no `.asc` counterpart at all -- every
    /// number in a `.asc` file is a dimensionless mast, so exporting to `.asc`
    /// drops this silently and re-importing never populates it.
    pub preform_y_offset: f64,
    /// A per-tier "cheater"/azimuth-offset annotation (degrees), keyed by the
    /// tier's CURRENT position in [`Self::tiers`] -- the same positional
    /// addressing [`crate::edit::Edit::ModifyTier`]/`SetConstraint`/`SetIndices`
    /// already use. Needed when cutting an off-tooth index or compensating a
    /// specific machine's error; `GemCutStudio` calls the
    /// same concept a "cheater" angle.
    ///
    /// # Why this is a `Design`-level map, not a [`ConstraintTier`] field
    ///
    /// The natural home is a field on [`ConstraintTier`] itself (every other
    /// per-facet annotation lives there, and [`Edit::MoveTier`]'s own doc
    /// comment leans on "nothing outside `tiers` stores a tier's positional
    /// index" as an invariant). That field would be the right design, but
    /// widening [`ConstraintTier`] would be a breaking change to roughly a
    /// dozen struct-literal construction sites across `apps/indicatrix-cut` --
    /// exactly the blast radius
    /// [`crate::preform::PreformSpec::planes_offset`]'s own doc comment
    /// already declined for the same reason. A `Design`-level map keeps this
    /// entirely inside this crate; [`Design::apply_edit`]'s
    /// `AddTier`/`RemoveTier`/`MoveTier` arms renumber this map's keys
    /// alongside the tier list itself, so the "nothing else stores a
    /// positional index" invariant still holds functionally (the map always
    /// tracks the SAME tier a plain field on it would), just not
    /// structurally. A future pass that widens `ConstraintTier` directly can
    /// fold this map into it and drop the renumbering logic.
    ///
    /// **Not yet wired into geometry**: an offset recorded here does not yet
    /// shift the corresponding facet's plane azimuth -- that needs a matching
    /// change in `apps/indicatrix-cut/src/gui/solid_preview/facet_map.rs` and
    /// `indicatrix::geometry::cuts::StandardGemCuts` (both outside this
    /// crate), which the module doc comment on `crate::cutting_sheet` calls
    /// out as needing to move together. Until then this is authored, undoable
    /// display/export data only -- see
    /// [`crate::cutting_sheet::CutSheetRow::cheater_offset_deg`].
    ///
    /// # Round-tripping through the native format
    ///
    /// Carried as `indicatrix_formats::native::TierTable::cheater_offset_deg`, per
    /// tier, by array position -- see `crate::native::to_native_file` and
    /// [`crate::native::load_paired`]. Exactly like
    /// [`Self::tier_notes`], an offset changes no geometry at all (see the
    /// "Not yet wired into geometry" note above), so it is applied on load
    /// whenever the tier overlay itself is, not gated on anything additional.
    /// It survives a save/reopen round trip like every other authored
    /// annotation here.
    pub cheater_offsets_deg: BTreeMap<usize, f64>,
    /// A per-tier cutter-authored free-text note (e.g. "check meet here", "grind
    /// slowly"), keyed by the tier's CURRENT position in [`Self::tiers`] -- the
    /// same positional addressing [`Self::cheater_offsets_deg`] already uses (the
    /// history-trail counterpart lives separately as
    /// [`crate::native::SaveExtras::history_entries`]).
    ///
    /// # Why this is a `Design`-level map, not a [`ConstraintTier`] field
    ///
    /// Exactly [`Self::cheater_offsets_deg`]'s own reasoning, reapplied: the natural
    /// home is a field on [`ConstraintTier`] itself, but widening that struct would
    /// be a breaking change to roughly 83 struct-literal construction sites across
    /// 24 files. A `Design`-level map keeps this entirely
    /// inside this crate; [`Design::apply_edit`]'s `AddTier`/`RemoveTier`/`MoveTier`
    /// arms renumber this map's keys alongside [`Self::cheater_offsets_deg`]'s own,
    /// so the "nothing else stores a positional index" invariant still holds
    /// functionally. A future pass that widens `ConstraintTier` directly can fold
    /// this map into it and drop the renumbering logic.
    ///
    /// # Round-tripping through the native format
    ///
    /// Carried as `indicatrix_formats::native::TierTable::note`, per tier, by
    /// array position -- see `crate::native::to_native_file` and
    /// [`crate::native::load_paired`]. Unlike `cheater_offsets_deg`, a note
    /// changes no geometry at all, so it is applied on load whenever the tier
    /// overlay itself is (see [`crate::native::TierOverlay`]) rather than
    /// gated on anything additional.
    pub tier_notes: BTreeMap<usize, String>,
    /// Which material's specific gravity to estimate this design's carat weight
    /// from, plus any per-design override -- see
    /// [`crate::material::MaterialSelection`]'s own doc comment. A design input,
    /// exactly like `preform` above: mutated only through `crate::edit::History`,
    /// and (like `girdle_diameter_mm`) not preserved across an `.asc` round trip for
    /// the same reason.
    pub material: MaterialSelection,
    /// A stable identifier for the tier CURRENTLY at each position, parallel to
    /// [`Self::tiers`] (same length, same order) -- see [`TierId`]'s own doc
    /// comment for why this is a parallel `Vec` rather than a field on
    /// [`ConstraintTier`] itself, and for the "never reused" guarantee every
    /// [`crate::edit::Edit`] arm that touches `tiers` upholds by moving this `Vec`
    /// in lockstep. `#[must_use]` accessors [`Self::tier_id_at`]/
    /// [`Self::index_of_tier_id`] are the intended read path; this field stays
    /// `pub` only so `crate::edit`/`crate::native` (a sibling module, not a
    /// different crate) can maintain it directly on `AddTier`/`RemoveTier`/
    /// `MoveTier`.
    pub tier_ids: Vec<TierId>,
    /// The next value [`Self::allocate_tier_id`] will hand out -- monotonic,
    /// never rewound even when a tier (and its id) is removed, so an id is never
    /// reused within one `Design`'s lifetime (including across undo/redo, since
    /// undo never decrements this counter either -- see [`TierId`]'s own doc
    /// comment for why that matters).
    pub next_tier_id: u64,
    /// Per-tier authoring-level targets ("cut to 3.20 mm", "girdle 2% of width",
    /// "table 4.10 mm wide"), keyed by [`TierId`] rather than position -- see
    /// [`TierTarget`]'s own module doc comment for why this lives here rather
    /// than on [`ConstraintTier`], and [`Self::resolved_meet_tier_inputs`] for
    /// where a target actually becomes a mast.
    pub tier_targets: TierTargetMap,
}

impl Design {
    /// The cheater/azimuth offset (degrees) authored for the tier CURRENTLY at
    /// `index`, if any -- see [`Self::cheater_offsets_deg`]'s own doc comment.
    #[must_use]
    pub fn cheater_offset_deg(&self, index: usize) -> Option<f64> {
        self.cheater_offsets_deg.get(&index).copied()
    }

    /// The cutter-authored note attached to the tier CURRENTLY at `index`, if
    /// any -- see [`Self::tier_notes`]'s own doc comment.
    #[must_use]
    pub fn tier_note(&self, index: usize) -> Option<&str> {
        self.tier_notes.get(&index).map(String::as_str)
    }

    /// The stable [`TierId`] of the tier CURRENTLY at `index`, if `index` is in
    /// range -- see [`Self::tier_ids`]'s own doc comment.
    #[must_use]
    pub fn tier_id_at(&self, index: usize) -> Option<TierId> {
        self.tier_ids.get(index).copied()
    }

    /// [`Self::tier_id_at`], falling back to a synthetic `TierId(index as u64)`
    /// when `self.tier_ids` is out of sync with `self.tiers` (a caller that
    /// mutated `self.tiers` directly, bypassing `Edit`, without yet applying an
    /// `Edit` that would self-heal it -- see [`crate::edit::Design::apply_edit`]'s
    /// own doc comment). Used by [`crate::manufacturability`]'s checks, which
    /// take `&Design` (no way to self-heal by mutating) and must never panic
    /// over a tier id the same way indexing [`Self::tier_ids`] directly would.
    #[must_use]
    pub(crate) fn tier_id_at_or_synthetic(&self, index: usize) -> TierId {
        self.tier_id_at(index).unwrap_or(TierId(index as u64))
    }

    /// The CURRENT position of the tier known by `id`, if it still exists --
    /// the reverse of [`Self::tier_id_at`]. Linear in tier count; fine for an
    /// editor-scale design (hundreds of tiers, not millions).
    #[must_use]
    pub fn index_of_tier_id(&self, id: TierId) -> Option<usize> {
        self.tier_ids.iter().position(|&existing| existing == id)
    }

    /// Hands out a fresh, never-before-used [`TierId`] and advances
    /// [`Self::next_tier_id`] -- the only way a [`TierId`] is ever created.
    pub(crate) const fn allocate_tier_id(&mut self) -> TierId {
        let id = TierId(self.next_tier_id);
        self.next_tier_id += 1;
        id
    }

    /// [`Self::tier_targets`], reindexed by CURRENT position rather than
    /// [`TierId`] -- [`Self::eq`]'s own building block, and generally useful
    /// for a caller comparing two designs' authored targets without caring
    /// about the arbitrary underlying id numbers.
    fn tier_targets_by_position(&self) -> Vec<Option<TierTarget>> {
        (0..self.tiers.len()).map(|i| self.tier_target(i)).collect()
    }
}

/// Manual, not derived: [`Self::tier_ids`]/[`Self::next_tier_id`] are pure
/// bookkeeping (a stable IDENTITY for a tier slot, never authored content),
/// deliberately excluded here so two designs that carry the exact same
/// authored/geometric content still compare equal regardless of what the
/// underlying id numbers happen to be -- in particular so `History::undo`
/// still restores a byte-for-byte-equal `Design` even from a starting design
/// whose `tier_ids` was never healed (e.g. built by pushing directly onto
/// `tiers`, bypassing `Edit` entirely, the pattern this crate's OWN test
/// fixtures use throughout) and gets healed by [`Self::apply_edit`]'s
/// self-sync on its first real edit -- see that method's own doc comment.
/// [`Self::tier_targets`] IS real authored content, so it still participates,
/// but through [`Self::tier_targets_by_position`] rather than raw map
/// equality, for the same id-numbering-should-not-matter reason.
impl PartialEq for Design {
    fn eq(&self, other: &Self) -> bool {
        self.preform == other.preform
            && self.meta == other.meta
            && self.tiers == other.tiers
            && self.girdle_diameter_mm == other.girdle_diameter_mm
            && self.preform_y_offset == other.preform_y_offset
            && self.cheater_offsets_deg == other.cheater_offsets_deg
            && self.tier_notes == other.tier_notes
            && self.material == other.material
            && self.tier_targets_by_position() == other.tier_targets_by_position()
    }
}
