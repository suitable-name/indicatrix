//! [`Edit`] itself (every reversible change a caller can describe against a
//! [`crate::design::Design`]) and [`EditError`], the one way applying one can
//! fail. See the parent module's doc comment for the command/inverse model
//! this whole crate's undo/redo rests on.

use super::schedule_state::ScheduleState;
use crate::{
    design::{
        ConcaveTier, ConstraintTier, Design, TierId, TierRelation, TierTarget, compute_tier_labels,
        is_legacy_123_abc,
    },
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
    /// One undo step, rather than multiple steps as a reorder-by-two-content-swaps
    /// approach would require.
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
    /// Replaces the design's [`crate::design::Design::preform_y_offset`] -- how far
    /// the preform's own vertical span is shifted from centred. Its own variant
    /// rather than folded into `SetPreform`: the offset is a `Design`-level anchor
    /// `PreformSpec` itself does not carry (see that field's own doc comment for
    /// why), so it needs a mast-preserving edit of its own, the same reasoning
    /// `SetGirdleDiameterMm` already follows for the sibling real-world anchor.
    SetPreformYOffset { y_offset: f64 },
    /// Replaces the design's real-world girdle diameter (millimetres) -- see
    /// [`crate::design::Design::girdle_diameter_mm`]. `None` clears the scale
    /// anchor entirely (back to "not anchored yet").
    SetGirdleDiameterMm { girdle_diameter_mm: Option<f64> },
    /// Replaces the design's material selection wholesale -- see
    /// [`crate::material::MaterialSelection`]. Wholesale, not per-field, matching
    /// `ModifyTier`'s own reasoning. The RI override rides inside
    /// `MaterialSelection` itself -- no separate variant.
    SetMaterial { material: MaterialSelection },
    /// Replaces the design's authorable header fields wholesale --
    /// [`crate::design::ScheduleMeta::headers`] (the `.asc` `H` lines, the first of
    /// which the app treats as the design's title -- see the catalogue round-trip
    /// convention), [`crate::design::ScheduleMeta::footnotes`] and
    /// [`crate::design::ScheduleMeta::gear_reference_angle`] (the index wheel's
    /// zero-tooth offset). Deliberately its own variant rather than folded into
    /// `SetSchedule`: `SetSchedule`'s fields feed `solve_meet_points` and always
    /// force a full re-solve (see `crate::resolve`), while none of these three
    /// changes any tier's solved MAST -- same "no mast could possibly have
    /// changed" reasoning as `SetPreform`/`SetGirdleDiameterMm`/`SetMaterial`, so
    /// [`crate::resolve::resolve_after_edit`] never re-solves for this variant
    /// either.
    ///
    /// **Not geometry-inert, though**: `gear_reference_angle` rotates every
    /// solved plane's azimuth downstream of the solve itself
    /// (`indicatrix::geometry::cuts`), so for a preform that is not
    /// rotationally symmetric (a block, unlike a cylinder) the resulting mesh,
    /// yield and manufacturability warnings DO change even though no mast
    /// moved -- see [`crate::design::Design::planes_from_solved`].
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
    /// Sets (or, when `note` is `None`, clears) the tier currently at `index`'s
    /// cutter-authored free-text note -- see
    /// [`crate::design::Design::tier_notes`]'s own doc comment for why this lives
    /// on `Design`, keyed by position, rather than on [`ConstraintTier`] itself.
    /// Like `SetCheaterOffset`, replaces exactly one entry in place;
    /// `AddTier`/`RemoveTier`/`MoveTier` renumber every OTHER entry so this one
    /// never needs to.
    SetTierNote { index: usize, note: Option<String> },
    /// Sets (or, when `target` is `None`, clears) the tier currently at
    /// `index`'s [`TierTarget`] -- see [`crate::design::Design::tier_targets`]'s
    /// own doc comment for why this lives on `Design`, keyed by [`crate::design::TierId`],
    /// rather than on [`ConstraintTier`] itself. Like `SetCheaterOffset`/
    /// `SetTierNote`, replaces exactly one entry in place.
    SetTierTarget {
        index: usize,
        target: Option<TierTarget>,
    },
    /// Sets (or, when `relation` is `None`, clears) the [`TierRelation`] driving the
    /// angle of the tier currently at `index` -- see
    /// [`crate::design::Design::tier_relations`]. Keyed by [`TierId`] under the
    /// hood, like `SetTierTarget`. Inverse: the same variant holding the previous
    /// relation.
    ///
    /// This edit changes ONLY the relation. It does not move the angle: the tier
    /// keeps whatever angle it has until the relations are evaluated, which an
    /// `indicatrix_editor::session::EditorSession` does in the SAME undo step by
    /// batching this edit with an [`Edit::RetargetAngles`]. A bare `Design` that
    /// applies it alone can call [`crate::design::Design::evaluate_relations`]
    /// itself. [`crate::resolve::resolve_after_edit`] treats it like an angle edit
    /// of the driven tier.
    SetTierRelation {
        index: usize,
        relation: Option<TierRelation>,
    },
    /// Never constructed by a caller directly -- an internal bookkeeping step
    /// [`Design::apply_edit`]'s own [`Edit::RemoveTier`] inverse uses to restore
    /// the exact [`crate::design::TierId`] the removed tier held, since a plain
    /// [`Edit::AddTier`] always allocates a genuinely fresh one (see
    /// [`crate::design::TierId`]'s own "never reused" doc comment -- undoing a
    /// removal is the one case where reproducing the SAME id, not merely a
    /// distinct one, is the correct behaviour). Sets [`Design::tier_ids`] at
    /// `index` to `id` and returns the same variant carrying whatever id was
    /// there before, so applying this twice in a row (once to restore, once as
    /// its own recorded inverse) round-trips exactly like every other
    /// single-entry `Set*` variant.
    RestoreTierId { index: usize, id: TierId },
    /// Replaces the design's index-gear tooth count, symmetry order and mirror
    /// flag wholesale, the schedule-wide counterpart to `SetMaterial`. Every
    /// tier's own `indices`/`detached` stay numerically unchanged -- see
    /// [`Edit::RemapIndices`] for the edit that re-derives them for a NEW gear;
    /// the editor applies both as two separate `History` steps on a gear-combo
    /// change.
    ///
    /// [`Design::apply_edit`] rejects `gear_teeth == 0` or `symmetry_order == 0`
    /// with an [`EditError`] (without modifying `self`) rather than accepting
    /// them: either makes every tier's index-wheel position meaningless and
    /// produces a `.asc` `g`/`y` line [`indicatrix_formats::asc::parse_asc`]
    /// refuses to read back at all. It also rejects a SMALLER gear when a concave
    /// tier holds an index past the new wheel (the error names that concave tier):
    /// run the [`Edit::RemapIndices`] first, or put both in one [`Edit::Batch`]
    /// (only the batch's end state is checked), as the editor's gear change does.
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
    /// Inserts a concave tier at position `index` of
    /// [`Design::concave_tiers`] (`index == len` appends). Rejected, without
    /// mutation, when the tier fails [`ConcaveTier::validate`] or reuses a flat
    /// tier's name. Inverse: [`Edit::RemoveConcaveTier`].
    AddConcaveTier { index: usize, tier: ConcaveTier },
    /// Removes the concave tier at `index`. Inverse: an [`Edit::Batch`] of
    /// [`Edit::AddConcaveTier`] and [`Edit::RestoreConcaveTierId`], so the same
    /// [`TierId`] comes back (see [`Edit::RestoreTierId`] for why).
    RemoveConcaveTier { index: usize },
    /// Replaces the concave tier at `index` wholesale, validated like
    /// [`Edit::AddConcaveTier`]. Inverse: the same variant holding the previous
    /// tier.
    ModifyConcaveTier { index: usize, tier: ConcaveTier },
    /// Moves a concave tier from `from` to `to`, with [`Edit::MoveTier`]'s
    /// remove-then-insert meaning. The order matters: it is the cutting order
    /// inside a concave group. Inverse: the positions swapped.
    MoveConcaveTier { from: usize, to: usize },
    /// Never constructed by a caller directly -- the concave twin of
    /// [`Edit::RestoreTierId`], needed because that one writes `tier_ids`.
    RestoreConcaveTierId { index: usize, id: TierId },
    /// Never constructed by a caller directly -- the concave half of the inverse
    /// [`Edit::RemapIndices`] produces. Restores each named concave tier's
    /// `indices` verbatim, as `(index, indices)`.
    RestoreConcaveIndices { tiers: Vec<(usize, Vec<f64>)> },
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
    /// Swaps the design's whole flat tier list and schedule metadata for the held
    /// [`ScheduleState`] in one step -- what applying edited `.asc` text uses, where
    /// any number of tiers may have been added, removed, renamed or changed at once.
    /// The preform, girdle size, material and concave tiers are not touched.
    /// Inverse: the same variant holding the state this edit replaced.
    ///
    /// Rejected, without changing the design, when the state is not usable (see
    /// [`Design::apply_edit`]'s `ReplaceSchedule` arm and `ScheduleState`), and when the
    /// state's gear is smaller than the design's and a concave tier holds an index past
    /// the smaller wheel (the same rule as [`Edit::SetSchedule`]; a [`Edit::Batch`] is
    /// checked once, on its end state, so a replacement that comes with a remap of the
    /// concave indices applies).
    ReplaceSchedule(Box<ScheduleState>),
}

impl Edit {
    /// A short, cutter-facing summary of what this [`Edit`] does -- e.g. "Set P1 angle
    /// to 41.0 degrees", "Remap gear 96 to 80", "Remove tier C1", "Optimize 12
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
            Self::SetPreformYOffset { y_offset } => {
                format!("Set preform offset to {y_offset:.2}")
            }
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
            Self::SetTierNote { index, note } => {
                let label = tier_label_at(design, *index);
                note.as_ref().map_or_else(
                    || format!("Clear note for {label}"),
                    |_| format!("Set note for {label}"),
                )
            }
            Self::SetTierTarget { index, target } => {
                let label = tier_label_at(design, *index);
                target.map_or_else(
                    || format!("Clear target for {label}"),
                    |t| format!("Set target for {label} to {}", describe_tier_target(t)),
                )
            }
            Self::SetTierRelation { index, relation } => {
                describe_tier_relation(*index, relation.as_ref(), design)
            }
            // Internal bookkeeping only -- see this variant's own doc comment.
            // `describe_batch` filters it out of a `RemoveTier` undo's own
            // description before this arm is ever reached in practice; this text
            // is a defensive fallback, not something a cutter should see.
            Self::RestoreTierId { index, .. } => {
                format!(
                    "Restore tier identity for {}",
                    tier_label_at(design, *index)
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
            Self::AddConcaveTier { tier, .. } => {
                format!("Add concave tier {}", tier_display_name(&tier.name))
            }
            Self::RemoveConcaveTier { index } => {
                format!("Remove concave tier {}", concave_label_at(design, *index))
            }
            Self::ModifyConcaveTier { index, .. } => {
                format!("Modify concave tier {}", concave_label_at(design, *index))
            }
            Self::MoveConcaveTier { from, to } => {
                let label = concave_label_at(design, *from);
                match to.cmp(from) {
                    std::cmp::Ordering::Less => format!("Move concave tier {label} up"),
                    std::cmp::Ordering::Greater => format!("Move concave tier {label} down"),
                    std::cmp::Ordering::Equal => format!("Move concave tier {label}"),
                }
            }
            // Internal bookkeeping only, like `RestoreTierId`.
            Self::RestoreConcaveTierId { index, .. } => format!(
                "Restore concave tier identity for {}",
                concave_label_at(design, *index)
            ),
            Self::RestoreConcaveIndices { tiers } => {
                format!(
                    "Restore index positions for {} concave tier(s)",
                    tiers.len()
                )
            }
            Self::Batch(edits) => describe_batch(edits, design),
            Self::ReplaceSchedule(_) => "Edit instructions as text".to_string(),
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
///
/// What the tier table shows: the tier's own name, or its standard code (`P1`, `C2`, `T`)
/// when the name is empty or old-style (`1`, `A`). A rename is the one sentence that must
/// say the raw names ([`raw_tier_name_at`]), because the name is what changes.
fn tier_label_at(design: &Design, index: usize) -> String {
    let Some(tier) = design.tiers.get(index) else {
        return format!("tier {}", index + 1);
    };
    if (tier.name.trim().is_empty() || is_legacy_123_abc(&tier.name))
        && let Some(info) = compute_tier_labels(&design.tiers).get(index)
    {
        return info.code.clone();
    }
    tier_display_name(&tier.name)
}

/// The tier's name exactly as stored (a placeholder for an unnamed one): what a rename
/// sentence reads, since there the name itself is the thing that changes.
fn raw_tier_name_at(design: &Design, index: usize) -> String {
    design.tiers.get(index).map_or_else(
        || format!("tier {}", index + 1),
        |tier| tier_display_name(&tier.name),
    )
}

/// [`tier_label_at`] for [`Design::concave_tiers`].
fn concave_label_at(design: &Design, index: usize) -> String {
    design.concave_tiers.get(index).map_or_else(
        || format!("concave tier {}", index + 1),
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

/// [`Edit::describe`]'s [`Edit::SetTierTarget`] arm: a short, unit-labelled
/// description of one [`TierTarget`].
fn describe_tier_target(target: TierTarget) -> String {
    match target {
        TierTarget::DepthMm(mm) => format!("{mm:.2} mm depth"),
        TierTarget::GirdleThicknessMm(mm) => format!("{mm:.2} mm girdle thickness"),
        TierTarget::TableWidthMm(mm) => format!("{mm:.2} mm table width"),
    }
}

/// [`Edit::describe`]'s [`Edit::SetTierRelation`] arm: "Set P2 = P1 - 2" (tiers by
/// name) or "Clear relation for P2".
fn describe_tier_relation(
    index: usize,
    relation: Option<&TierRelation>,
    design: &Design,
) -> String {
    let label = tier_label_at(design, index);
    relation.map_or_else(
        || format!("Clear relation for {label}"),
        |relation| format!("Set {label} = {}", relation.to_display(design)),
    )
}

/// [`describe_tier_relation`]'s wording as the second half of a longer label ("Rename P2 to
/// P3 and set P3 = P1 - 2"): the same sentence, for a tier already called `label`, with its
/// first letter in lower case.
fn describe_relation_clause(
    label: &str,
    relation: Option<&TierRelation>,
    design: &Design,
) -> String {
    relation.map_or_else(
        || format!("clear relation for {label}"),
        |relation| format!("set {label} = {}", relation.to_display(design)),
    )
}

/// [`Edit::describe`]'s label for a [`MaterialSelection`] -- its name when set, else a
/// placeholder (an RI-override-only selection has no catalogue name to show), with a
/// body-color override appended in brackets (`Sapphire (Yellow)`, or
/// `Sapphire (custom color)` for a triple that matches no preset -- see
/// [`MaterialSelection::body_color_label`]).
fn material_display_name(material: &MaterialSelection) -> String {
    let mut label = material
        .name
        .clone()
        .unwrap_or_else(|| "(custom material)".to_string());
    if let Some(color) = material.body_color_label() {
        label = format!("{label} ({color})");
    }
    label
}

/// [`Edit::describe`]'s [`Edit::RetargetAngles`] arm: a single-tier retarget names
/// that tier and its new angle (the "Set P1 angle to 41.0 degrees" example); several
/// at once is always an Optimize/batch result in practice, so it reads as "Optimize N
/// tiers" instead of an unreadable per-tier list.
///
/// The angle is a magnitude: a pavilion tier stored at -41 reads `41.0`, because the side
/// of the girdle is the tier's, never a sign a person has to read.
fn describe_retarget_angles(changes: &[(usize, f64, f64)], design: &Design) -> String {
    match changes {
        [] => "Retarget angles".to_string(),
        [(index, _, new_deg)] => {
            format!(
                "Set {} angle to {:.1} degrees",
                tier_label_at(design, *index),
                new_deg.abs()
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
    if let Some(label) = describe_anchored_retarget(edits) {
        return label;
    }
    if let Some(label) = describe_relation_batch(edits, design) {
        return label;
    }
    // `RestoreTierId` is `Design::apply_edit`'s own invisible bookkeeping step
    // (see that variant's doc comment) -- a `RemoveTier` undo that otherwise
    // collapses to one `AddTier` should still read as "Add tier X", not "2
    // combined edits", just because it also carries this step along.
    let visible: Vec<&Edit> = edits
        .iter()
        .filter(|edit| {
            !matches!(
                edit,
                Edit::RestoreTierId { .. } | Edit::RestoreConcaveTierId { .. }
            )
        })
        .collect();
    match visible.as_slice() {
        [] => "No-op".to_string(),
        [only] => only.describe(design),
        many => format!("{} combined edits", many.len()),
    }
}

/// [`describe_batch`]'s label for a retarget that also re-anchors masts: angle changes
/// plus the [`Edit::SetConstraint`]s that keep each facet's girdle edge in place, and the
/// material change when there is one. `None` for a batch with no constraint change
/// (those keep their older labels) or with any other kind of edit in it.
fn describe_anchored_retarget(edits: &[Edit]) -> Option<String> {
    let mut has_angles = false;
    let mut has_constraint = false;
    let mut material: Option<&MaterialSelection> = None;
    for edit in edits {
        match edit {
            Edit::RetargetAngles { .. } => has_angles = true,
            Edit::SetConstraint { .. } => has_constraint = true,
            Edit::SetMaterial {
                material: selection,
            } => material = Some(selection),
            _ => return None,
        }
    }
    if !(has_angles && has_constraint) {
        return None;
    }
    Some(material.map_or_else(
        || "Retarget angles".to_string(),
        |selection| format!("Retarget for {}", material_display_name(selection)),
    ))
}

/// [`describe_batch`]'s label for the batches an editor session builds when tier
/// relations are in play: an edit plus the [`Edit::RetargetAngles`] that moves the
/// tiers following it, in either order (an undo entry holds the pair reversed).
///
/// - a [`Edit::SetTierRelation`] with its angle update reads as that relation edit;
/// - a [`Edit::ModifyTier`] with its angle update reads as that modification;
/// - angle changes only (a nudge or drag plus the tiers that follow) read as "Set X
///   angle to ..." for one tier and "Change angles of N tiers" for several;
/// - any other first edit followed only by angle updates and relation clears (a tier
///   removal that also frees the relations reading it) reads as that first edit;
/// - a tier form save that also set the tier's relation reads as both ("Rename P2 to P3 and
///   set P3 = P1 - 2"), see [`describe_tier_save_with_relation`].
///
/// `None` for any other batch, which keeps the generic label.
fn describe_relation_batch(edits: &[Edit], design: &Design) -> Option<String> {
    if let Some(label) = describe_tier_save_with_relation(edits, design) {
        return Some(label);
    }
    let follows = |edit: &Edit| {
        matches!(
            edit,
            Edit::RetargetAngles { .. } | Edit::SetTierRelation { relation: None, .. }
        )
    };
    if let [first, rest @ ..] = edits
        && !rest.is_empty()
        && !matches!(first, Edit::RetargetAngles { .. })
        && rest.iter().all(follows)
    {
        return Some(first.describe(design));
    }
    let [first, second] = edits else {
        return all_angle_changes_label(edits, design);
    };
    match (first, second) {
        (Edit::SetTierRelation { .. }, Edit::RetargetAngles { .. })
        | (Edit::RetargetAngles { .. }, Edit::SetTierRelation { .. })
        | (Edit::ModifyTier { .. }, Edit::RetargetAngles { .. })
        | (Edit::RetargetAngles { .. }, Edit::ModifyTier { .. }) => {
            let edit = if matches!(first, Edit::RetargetAngles { .. }) {
                second
            } else {
                first
            };
            Some(edit.describe(design))
        }
        _ => all_angle_changes_label(edits, design),
    }
}

/// Every edit of `edits` with the nested batches opened up, in order. A tier form save is a
/// batch inside the batch an editor session folds the following angles into.
fn flattened<'a>(edits: &'a [Edit], into: &mut Vec<&'a Edit>) {
    for edit in edits {
        match edit {
            Edit::Batch(inner) => flattened(inner, into),
            other => into.push(other),
        }
    }
}

/// [`describe_relation_batch`]'s label for a tier form save that also sets the tier's relation,
/// or for the undo of one: the tier part followed by the relation part, "Rename P2 to P3 and
/// set P3 = P1 - 2", "Modify tier P2 and set P2 = P1 - 2", "Add tier P5 and set P5 = P1 - 2".
///
/// The batch (nested batches opened up) holds exactly one [`Edit::AddTier`] or
/// [`Edit::ModifyTier`] and a [`Edit::SetTierRelation`] for that same tier, and nothing but the
/// edits that ride along with a tier save: the meet-name rewrites a rename brings
/// ([`Edit::SetConstraint`]), a depth target ([`Edit::SetTierTarget`]) and the angles that
/// follow ([`Edit::RetargetAngles`]). A change of name reads as a rename, and the relation
/// part names the tier by its name after the edit. The undo of such a save holds the same
/// edits the other way round and reads the same way ("Rename P3 to P2 and clear relation for
/// P2").
///
/// A [`Edit::RemoveTier`] with nothing but relation clears (the undo of adding a tier with a
/// relation, or a removal that frees the relations reading it) reads as the removal.
///
/// `None` for any other batch.
fn describe_tier_save_with_relation(edits: &[Edit], design: &Design) -> Option<String> {
    let mut flat = Vec::new();
    flattened(edits, &mut flat);
    let mut tier_part: Option<&Edit> = None;
    let mut relations: Vec<(usize, Option<&TierRelation>)> = Vec::new();
    for edit in flat {
        match edit {
            Edit::AddTier { .. } | Edit::ModifyTier { .. } | Edit::RemoveTier { .. } => {
                if tier_part.replace(edit).is_some() {
                    return None;
                }
            }
            Edit::SetTierRelation { index, relation } => {
                relations.push((*index, relation.as_ref()));
            }
            Edit::SetConstraint { .. }
            | Edit::SetTierTarget { .. }
            | Edit::RetargetAngles { .. }
            | Edit::RestoreTierId { .. } => {}
            _ => return None,
        }
    }
    match tier_part? {
        remove @ Edit::RemoveTier { .. } => (!relations.is_empty()
            && relations.iter().all(|(_, relation)| relation.is_none()))
        .then(|| remove.describe(design)),
        Edit::AddTier { index, tier } => {
            let (_, relation) = relations.iter().find(|(at, _)| at == index)?;
            let name = tier_display_name(&tier.name);
            Some(format!(
                "Add tier {name} and {}",
                describe_relation_clause(&name, *relation, design)
            ))
        }
        Edit::ModifyTier { index, tier } => {
            let (_, relation) = relations.iter().find(|(at, _)| at == index)?;
            let new = tier_display_name(&tier.name);
            let renamed = design
                .tiers
                .get(*index)
                .is_some_and(|current| current.name != tier.name);
            // A rename says the names as they are; any other change says what the table shows.
            let old = if renamed {
                raw_tier_name_at(design, *index)
            } else {
                tier_label_at(design, *index)
            };
            let first = if renamed {
                format!("Rename {old} to {new}")
            } else {
                format!("Modify tier {old}")
            };
            Some(format!(
                "{first} and {}",
                describe_relation_clause(&new, *relation, design)
            ))
        }
        _ => None,
    }
}

/// The label of a batch made only of [`Edit::RetargetAngles`] (at least two of them),
/// or `None` for anything else.
fn all_angle_changes_label(edits: &[Edit], design: &Design) -> Option<String> {
    if edits.len() < 2 {
        return None;
    }
    let mut tiers = Vec::new();
    for edit in edits {
        let Edit::RetargetAngles { changes } = edit else {
            return None;
        };
        tiers.extend(changes.iter().map(|&(index, _, new_deg)| (index, new_deg)));
    }
    let mut distinct: Vec<usize> = tiers.iter().map(|&(index, _)| index).collect();
    distinct.sort_unstable();
    distinct.dedup();
    match (distinct.as_slice(), tiers.last()) {
        ([index], Some(&(_, new_deg))) => Some(format!(
            "Set {} angle to {:.1} degrees",
            tier_label_at(design, *index),
            new_deg.abs()
        )),
        (many, _) => Some(format!("Change angles of {} tiers", many.len())),
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

/// Why an [`Edit`] could not be applied.
///
/// Almost always a position out of range for the design's current tier list.
/// The one exception: [`Design::apply_edit`]'s own [`Edit::SetSchedule`] arm
/// also returns this (with `index: 0`) for a zero gear-tooth count or symmetry
/// order, a schedule-wide validation failure that names no tier at all --
/// reusing this type's only shape rather than widening this crate's one
/// edit-error type for that single caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    /// Tier position this refers to.
    pub index: usize,
    /// Number of tiers the design had when the edit was rejected.
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
