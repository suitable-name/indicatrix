//! [`EditorSession`]: the design under edit, its undo/redo [`History`], the
//! tier-list multi-selection, the edit-generation counter and the saved-generation
//! mark that together make an editor.
//!
//! Both the desktop editor and the web app edit
//! a design through this type, so the two apply, coalesce, undo and redo
//! identically.
//!
//! # `History` is the only thing that mutates `Design`
//!
//! `History::undo`/`redo` replay recorded inverses and fail if something else
//! changed the design behind their back. So every mutation here goes through
//! `History` -- [`EditorSession::apply`], [`EditorSession::apply_coalescing`],
//! [`EditorSession::undo`], [`EditorSession::redo`] and
//! [`EditorSession::apply_optimize_outcome`], plus the convenience actions built on
//! [`EditorSession::apply`]. The one exception is
//! [`EditorSession::from_template`], which seeds a brand-new design's STARTING
//! tiers (and their `TierId`s) before any history exists (that is not an edit and
//! must not be undoable).
//!
//! # The multi-selection follows its tiers
//!
//! [`EditorSession::multi_selected`] holds row indices, so every edit, undo and redo
//! that adds, removes or moves a tier renumbers it to keep naming the same tiers (a
//! removed tier leaves the selection) -- see the `selection` module.
//! [`EditorSession::selected_concave`] does the same for the concave list.
//!
//! # No clock
//!
//! Nudge coalescing needs "how long since the last nudge". This crate reads no
//! clock (`std::time::Instant` panics on `wasm32-unknown-unknown`): every
//! coalescing entry point takes a caller-supplied `now`, a [`Duration`] since any
//! fixed origin. The desktop passes the time since a process-wide `Instant`; the
//! web app passes `performance.now()`.
//!
//! # Generation counter
//!
//! `generation` is an `Arc<AtomicU64>` so a background job (a solve in a thread or
//! a Worker round trip) can hold a clone and notice the design moved on. It is
//! bumped by every successful edit/undo/redo, never by a solve.
//! [`EditorSession::continue_generation_from`] carries the SAME counter across a
//! wholesale replacement (New, Load, Open), bumped once, so a job dispatched
//! against the replaced design still sees the change.

use crate::manipulate::{HandleKind, drag_coalesce_key};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, EditError, FreshDesignSpec, History, OptimizeOutcome, PreformSpec,
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

mod history_view;
mod relations;
mod selection;
mod tier_ops;

#[cfg(test)]
mod tests;

pub use selection::TierIndexMap;

pub use history_view::{HistorySnapshot, JumpFailure, JumpOutcome};
pub use relations::{ClearedRelation, RelationNotice, RelationRefusal, SessionEditError};
pub use tier_ops::{
    DetachOutcome, DuplicateOutcome, GeneratedSeries, InlineAngle, MirrorOutcome, MovedTier,
    RemoveTierError, RemovedTier, selection_after_remove,
};

/// The coalescing window every session's [`History`] is built with (via
/// [`History::with_coalesce_window`]) instead of [`History::new`]'s 500 ms default.
///
/// Long enough that a deliberate, unhurried scroll-wheel angle nudge (ticks slower than
/// 500 ms apart) still merges into one undo step.
///
/// A caller ends a run early with
/// [`History::end_coalesce_run`] when an interaction clearly finished (the desktop
/// does so on a bit-identical inline-cell commit).
pub const ANGLE_NUDGE_COALESCE_WINDOW: Duration = Duration::from_millis(1500);

/// What one successful edit, undo or redo changed -- enough for a UI to decide how
/// much to refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditChange {
    /// The session's generation after the change.
    pub generation: u64,
    /// `design.tiers.len()` before the change.
    pub tier_count_before: usize,
    /// `design.tiers.len()` after the change.
    pub tier_count_after: usize,
    /// `design.concave_tiers.len()` before the change.
    pub concave_count_before: usize,
    /// `design.concave_tiers.len()` after the change.
    pub concave_count_after: usize,
}

impl EditChange {
    /// Whether the change added or removed tiers (so a row list must be rebuilt
    /// rather than patched in place).
    #[must_use]
    pub const fn tier_count_changed(&self) -> bool {
        self.tier_count_before != self.tier_count_after
    }

    /// Whether the change added or removed concave tiers (they have their own list,
    /// so [`Self::tier_count_changed`] stays `false` for them).
    #[must_use]
    pub const fn concave_count_changed(&self) -> bool {
        self.concave_count_before != self.concave_count_after
    }
}

/// What [`EditorSession::nudge_angles`] applied: the change itself, plus the labels
/// of every tier whose nudge stopped at 0 degrees instead of crossing into the other
/// block (see [`clamp_nudge_to_side`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NudgeOutcome {
    /// See [`EditChange`].
    pub change: EditChange,
    /// `"tier 5 (Girdle)"`-style labels, in target order; empty when nothing was
    /// clamped.
    pub clamped_labels: Vec<String>,
}

/// What [`EditorSession::pin_tier_mast`] applied: the change itself, plus the meet
/// constraint the pin replaced, so the UI can say "was: meets P2".
#[derive(Debug, Clone, PartialEq)]
pub struct PinOutcome {
    /// See [`EditChange`].
    pub change: EditChange,
    /// The tier's constraint before the pin when it was a meet
    /// ([`MeetConstraint::MeetExisting`] or [`MeetConstraint::MeetNamed`]); `None`
    /// when the tier was already pinned by a scale reference. Only the call that
    /// actually replaced the meet reports it -- later calls of the same drag find the
    /// tier already pinned and report `None`, so the caller keeps the first answer.
    pub replaced_meet: Option<MeetConstraint>,
}

/// The editor's live state: a design plus the undo/redo history over it, the
/// multi-selection, and the generation/saved-generation pair behind "unsaved
/// changes". See this module's doc comment.
pub struct EditorSession {
    /// The design under edit. Mutate it only through this type's methods (see the
    /// module doc comment); reading it directly is fine.
    pub design: Design,
    /// The undo/redo history over [`Self::design`].
    pub history: History,
    /// The tier-list row indices currently Ctrl+click-toggled into a multi-select
    /// group. A transient UI selection, never part of `Design` or `History`: every
    /// successful edit/undo/redo renumbers it so it keeps naming the same tiers when
    /// tiers are added, removed or moved (a removed tier leaves the group), and
    /// clears it when the renumbering cannot be worked out.
    pub multi_selected: BTreeSet<usize>,
    /// The selected concave tier (a position in `design.concave_tiers`), if any. Kept
    /// apart from the flat selection because the two lists number independently; it
    /// is renumbered like [`Self::multi_selected`] and clears when its tier is removed.
    pub selected_concave: Option<usize>,
    /// Bumped by every successful edit/undo/redo (and once by
    /// [`Self::continue_generation_from`]). Shared so a background job can compare
    /// against it -- see the module doc comment.
    pub generation: Arc<AtomicU64>,
    /// [`Self::generation`]'s value as of the last successful save, or when this
    /// design was created/loaded -- see [`Self::is_dirty`]. Undoing back to exactly
    /// the saved content still reads as dirty: generation counts steps, not content.
    pub saved_generation: u64,
    /// The relations the last edit cleared, if nobody took them yet -- see
    /// [`Self::take_relation_notice`]. The next edit, undo, redo or jump drops it.
    relation_notice: Option<RelationNotice>,
    /// Whether a command made of several edits (removing every selected tier) is running:
    /// its edits add to [`Self::relation_notice`] instead of dropping it.
    notice_command_open: bool,
    /// Why the last edit of the old `apply` family was refused because of a tier
    /// relation -- see [`Self::take_refusal`].
    refusal: Option<SessionEditError>,
}

impl EditorSession {
    /// The startup design: a generously sized cylindrical preform (96 sides,
    /// half-width 1.5, L/W 1.0, depth 1.5) on a 96-tooth, 8-fold, RI 1.54 schedule
    /// with no tiers -- already a real, closed solid rather than an empty viewport.
    #[must_use]
    pub fn fresh() -> Self {
        let preform = PreformSpec::cylinder(96, 1.5, 1.0, 1.5);
        Self::with_history(
            Design::fresh(preform, 96, 8, 1.54),
            History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
        )
    }

    /// A brand-new design from the New Design dialog's full [`FreshDesignSpec`]
    /// (preform, gear, symmetry, mirror, starting material), with an empty history.
    #[must_use]
    pub fn from_spec(spec: FreshDesignSpec) -> Self {
        Self::with_history(
            Design::fresh_from_spec(spec),
            History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
        )
    }

    /// [`Self::from_spec`], then seeded with template `template_index`'s tiers: `0`
    /// is "Empty"; `1..=N` index `indicatrix_cut_core::templates::TEMPLATES` at
    /// `template_index - 1`. Any index the table has no entry for (0, negative, past
    /// the end) is treated as empty rather than panicking.
    ///
    /// The tiers are the design's STARTING state, written directly rather than
    /// through `History`, so they are not undoable back to an empty schedule the
    /// cutter never saw. Each gets its `TierId` here ([`Design::ensure_tier_ids`]),
    /// so an edit that finds a tier by id (a depth, girdle-thickness or table-width
    /// target) works on a template tier from the first click.
    #[must_use]
    pub fn from_template(spec: FreshDesignSpec, template_index: i32) -> Self {
        let mut session = Self::from_spec(spec);
        if let Some(tiers) = template_tiers(template_index) {
            session.design.tiers = tiers;
            session.design.ensure_tier_ids();
        }
        session
    }

    /// A session over an already-built `design` (a load or an open) with `history`
    /// as its (normally empty) undo history, generation 0 and nothing selected.
    #[must_use]
    pub fn with_history(design: Design, history: History) -> Self {
        Self {
            design,
            history,
            multi_selected: BTreeSet::new(),
            selected_concave: None,
            generation: Arc::new(AtomicU64::new(0)),
            saved_generation: 0,
            relation_notice: None,
            notice_command_open: false,
            refusal: None,
        }
    }

    /// The current value of [`Self::generation`].
    #[must_use]
    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Makes this (replacement) session continue `previous`'s generation counter
    /// instead of its own fresh one: shares the SAME `Arc`, bumps it once, and marks
    /// the result saved, so the replacement reads as clean while every clone a
    /// background job took before the replacement observes the bump.
    pub fn continue_generation_from(&mut self, previous: &Self) {
        self.generation = Arc::clone(&previous.generation);
        let now = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.saved_generation = now;
    }

    /// Replaces this session wholesale with `replacement`, continuing this
    /// session's generation counter -- see [`Self::continue_generation_from`].
    pub fn replace_with(&mut self, mut replacement: Self) {
        replacement.continue_generation_from(self);
        *self = replacement;
    }

    /// Whether the design changed since the last save/load/new -- see
    /// [`Self::saved_generation`].
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.generation.load(Ordering::Relaxed) != self.saved_generation
    }

    /// Records the current generation as saved.
    pub fn mark_saved(&mut self) {
        self.saved_generation = self.current_generation();
    }

    /// Applies `edit` through [`History::apply`], keeping tier relations true (see the
    /// `relations` module: [`Self::try_apply`] is the same call with a typed error).
    ///
    /// # Errors
    ///
    /// [`History::apply`]'s error, verbatim; nothing changes on `Err`. An edit refused
    /// because of a tier relation also fails with an `EditError` (naming the driven
    /// tier); [`Self::take_refusal`] then has the real reason.
    pub fn apply(&mut self, edit: Edit) -> Result<EditChange, EditError> {
        self.try_apply(edit)
            .map_err(|error| self.legacy_edit_error(error))
    }

    /// Like [`Self::apply`], but through [`History::apply_coalescing`]: an edit with
    /// the same `key` as the previous one, within the history's coalescing window of
    /// it (measured with the caller's `now`, see the module doc comment), merges
    /// into the previous undo step. See [`angle_nudge_coalesce_key`] for the key an
    /// angle nudge uses.
    ///
    /// # Errors
    ///
    /// [`History::apply_coalescing`]'s error, verbatim; a refusal because of a tier
    /// relation is reported as for [`Self::apply`].
    pub fn apply_coalescing(
        &mut self,
        edit: Edit,
        key: u64,
        now: Duration,
    ) -> Result<EditChange, EditError> {
        self.try_apply_coalescing(edit, key, now)
            .map_err(|error| self.legacy_edit_error(error))
    }

    /// Undoes through [`History::undo`]. `Ok(None)` when there was nothing to undo.
    ///
    /// # Errors
    ///
    /// [`History::undo`]'s error (a failed replay of the recorded inverse).
    pub fn undo(&mut self) -> Result<Option<EditChange>, EditError> {
        Ok(self.undo_mapped()?.map(|(change, _)| change))
    }

    /// [`Self::undo`], also returning how the undone step renumbered the tier rows, for a
    /// caller that keeps a row selection of its own (the desktop's single selected row)
    /// and must make it follow its tier the way [`Self::multi_selected`] does. See
    /// [`TierIndexMap::map_table_row`].
    ///
    /// # Errors
    ///
    /// [`History::undo`]'s error (a failed replay of the recorded inverse).
    pub fn undo_mapped(&mut self) -> Result<Option<(EditChange, TierIndexMap)>, EditError> {
        self.expire_relation_notice();
        let before = self.tier_counts();
        let renumbering = self.history.peek_undo().map(TierIndexMap::of);
        let undone = self.history.undo(&mut self.design)?;
        Ok(undone.then(|| {
            let change = self.record_change(before, renumbering.as_ref());
            (change, renumbering.unwrap_or_default())
        }))
    }

    /// Redoes through [`History::redo`]. `Ok(None)` when there was nothing to redo.
    ///
    /// # Errors
    ///
    /// [`History::redo`]'s error, symmetrically to [`Self::undo`].
    pub fn redo(&mut self) -> Result<Option<EditChange>, EditError> {
        Ok(self.redo_mapped()?.map(|(change, _)| change))
    }

    /// [`Self::redo`], also returning the step's renumbering of the tier rows, as
    /// [`Self::undo_mapped`] does for an undo.
    ///
    /// # Errors
    ///
    /// [`History::redo`]'s error, symmetrically to [`Self::undo`].
    pub fn redo_mapped(&mut self) -> Result<Option<(EditChange, TierIndexMap)>, EditError> {
        self.expire_relation_notice();
        let before = self.tier_counts();
        let renumbering = self.history.peek_redo().map(TierIndexMap::of);
        let redone = self.history.redo(&mut self.design)?;
        Ok(redone.then(|| {
            let change = self.record_change(before, renumbering.as_ref());
            (change, renumbering.unwrap_or_default())
        }))
    }

    /// Applies an Optimize result's angle changes as real, undoable edits through
    /// [`indicatrix_cut_core::apply_optimize_outcome`] (one `Edit::Batch` of
    /// `Edit::ModifyTier`s, one per changed tier, so a single undo reverts the whole
    /// Optimize). The batch is all-or-nothing: a change whose tier index is out of
    /// range or whose recorded angle no longer matches the tier fails before the
    /// design is touched. Bumps the generation iff `Ok(applied)` with `applied > 0`,
    /// or on `Err` -- a failed apply leaves the design as it was, but the outcome was
    /// computed against a design that has since moved, so this errs toward
    /// over-invalidating.
    ///
    /// # Errors
    ///
    /// [`indicatrix_cut_core::apply_optimize_outcome`]'s error, verbatim.
    pub fn apply_optimize_outcome(
        &mut self,
        outcome: &OptimizeOutcome,
    ) -> Result<usize, EditError> {
        if !self.design.tier_relations.is_empty() {
            return self.apply_optimize_outcome_with_relations(outcome);
        }
        let result = indicatrix_cut_core::apply_optimize_outcome(
            &mut self.history,
            &mut self.design,
            outcome,
        );
        match &result {
            Ok(0) => {}
            Ok(_) | Err(_) => {
                self.generation.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }

    /// Nudges every tier in `targets` by `delta_deg` as ONE coalescing
    /// `Edit::RetargetAngles` (keyed by [`angle_nudge_coalesce_key`]), each new angle
    /// clamped to its tier's original side of zero (see [`clamp_nudge_to_side`]).
    ///
    /// `Ok(None)` (nothing applied) when `targets` is empty or names a tier that
    /// does not exist.
    ///
    /// # Errors
    ///
    /// [`Self::apply_coalescing`]'s error.
    pub fn nudge_angles(
        &mut self,
        targets: &[usize],
        delta_deg: f64,
        now: Duration,
    ) -> Result<Option<NudgeOutcome>, EditError> {
        self.nudge_with(targets, now, |_| delta_deg)
    }

    /// Like [`Self::nudge_angles`], but `delta_deg` moves the number each tier's row
    /// SHOWS: the tier table prints a pavilion tier's angle without its minus sign (the
    /// side is the label), so a positive delta makes the shown number bigger for a crown
    /// and a pavilion tier alike, and the stored sign stays. See [`displayed_nudge_delta`].
    /// What the inline angle cell's Up/Down/wheel and the multi-select Offset box use.
    ///
    /// `Ok(None)` (nothing applied) when `targets` is empty or names a tier that does not
    /// exist.
    ///
    /// # Errors
    ///
    /// [`Self::apply_coalescing`]'s error.
    pub fn nudge_displayed_angles(
        &mut self,
        targets: &[usize],
        delta_deg: f64,
        now: Duration,
    ) -> Result<Option<NudgeOutcome>, EditError> {
        self.nudge_with(targets, now, |tier| {
            displayed_nudge_delta(tier.angle_deg, delta_deg)
        })
    }

    /// The one nudge both public entry points share: `delta_for` gives each tier's SIGNED
    /// change.
    fn nudge_with(
        &mut self,
        targets: &[usize],
        now: Duration,
        delta_for: impl Fn(&ConstraintTier) -> f64,
    ) -> Result<Option<NudgeOutcome>, EditError> {
        let mut clamped_labels: Vec<String> = Vec::new();
        let changes: Option<Vec<(usize, f64, f64)>> = targets
            .iter()
            .map(|&index| {
                self.design.tiers.get(index).map(|tier| {
                    let wanted = tier.angle_deg + delta_for(tier);
                    let nudged = clamp_nudge_to_side(tier.angle_deg, wanted);
                    if nudged != wanted {
                        clamped_labels.push(tier_nudge_label(tier, index));
                    }
                    (index, tier.angle_deg, nudged)
                })
            })
            .collect();
        let Some(changes) = changes else {
            return Ok(None);
        };
        if changes.is_empty() {
            return Ok(None);
        }
        let key = angle_nudge_coalesce_key(targets);
        let change = self.apply_coalescing(Edit::RetargetAngles { changes }, key, now)?;
        Ok(Some(NudgeOutcome {
            change,
            clamped_labels,
        }))
    }

    /// Sets tier `tier`'s angle to `angle_deg` as a coalescing `Edit::RetargetAngles`
    /// (one change) keyed by `drag_coalesce_key(tier, HandleKind::Angle)`, so every
    /// call of one angle-handle drag within the coalescing window is ONE undo step whose
    /// undo restores the pre-drag angle bit for bit. The angle is held to the tier's
    /// original side of zero like [`Self::nudge_angles`] (see [`clamp_nudge_to_side`]).
    ///
    /// `Ok(None)` (nothing applied) when the tier does not exist, `angle_deg` is not
    /// finite, or the (clamped) angle equals the current one bit for bit.
    ///
    /// # Errors
    ///
    /// [`Self::apply_coalescing`]'s error.
    pub fn set_tier_angle(
        &mut self,
        tier: usize,
        angle_deg: f64,
        now: Duration,
    ) -> Result<Option<EditChange>, EditError> {
        let Some(current) = self.design.tiers.get(tier) else {
            return Ok(None);
        };
        let old = current.angle_deg;
        let new = clamp_nudge_to_side(old, angle_deg);
        if !new.is_finite() || new.to_bits() == old.to_bits() {
            return Ok(None);
        }
        let key = drag_coalesce_key(tier, HandleKind::Angle);
        let edit = Edit::RetargetAngles {
            changes: vec![(tier, old, new)],
        };
        self.apply_coalescing(edit, key, now).map(Some)
    }

    /// Pins tier `tier`'s mast at `mast` as a coalescing `Edit::SetConstraint`
    /// (`ScaleReference(mast)`) keyed by
    /// `drag_coalesce_key(tier, HandleKind::Depth)`: one depth-handle drag is one undo step, and undo restores
    /// the tier's previous constraint exactly (a `MeetNamed` tier meets its named
    /// facets again).
    ///
    /// `Ok(None)` when the tier does not exist, `mast` is not finite, or the tier is
    /// already pinned at exactly `mast`. See [`PinOutcome::replaced_meet`] for what the
    /// outcome reports about the constraint the pin replaced.
    ///
    /// # Errors
    ///
    /// [`Self::apply_coalescing`]'s error.
    pub fn pin_tier_mast(
        &mut self,
        tier: usize,
        mast: f64,
        now: Duration,
    ) -> Result<Option<PinOutcome>, EditError> {
        let Some(current) = self.design.tiers.get(tier) else {
            return Ok(None);
        };
        if !mast.is_finite() {
            return Ok(None);
        }
        let previous = current.constraint.clone();
        if matches!(&previous, MeetConstraint::ScaleReference(m) if m.to_bits() == mast.to_bits()) {
            return Ok(None);
        }
        let replaced_meet =
            (!matches!(previous, MeetConstraint::ScaleReference(_))).then_some(previous);
        let key = drag_coalesce_key(tier, HandleKind::Depth);
        let edit = Edit::SetConstraint {
            index: tier,
            constraint: MeetConstraint::ScaleReference(mast),
        };
        let change = self.apply_coalescing(edit, key, now)?;
        Ok(Some(PinOutcome {
            change,
            replaced_meet,
        }))
    }

    /// Turns tier `tier`'s index-wheel positions (`indices` and `detached`) by
    /// `k_teeth` whole teeth as a coalescing `Design::rotate_indices` edit keyed by
    /// `drag_coalesce_key(tier, HandleKind::Index)`: one index-handle drag is one
    /// undo step.
    ///
    /// `k_teeth` is RELATIVE TO THE TIER AS IT IS NOW: each call rotates the current
    /// positions, so a drag that reports its total turn (`DragValue::IndexTeeth`) must
    /// pass only the teeth added since the previous call.
    ///
    /// `Ok(None)` when `k_teeth` is 0.
    ///
    /// # Errors
    ///
    /// `Design::rotate_indices`'s error (no such tier), or [`Self::apply_coalescing`]'s.
    pub fn rotate_tier_indices(
        &mut self,
        tier: usize,
        k_teeth: i64,
        now: Duration,
    ) -> Result<Option<EditChange>, EditError> {
        if k_teeth == 0 {
            return Ok(None);
        }
        let edit = self.design.rotate_indices(tier, k_teeth as f64)?;
        let key = drag_coalesce_key(tier, HandleKind::Index);
        self.apply_coalescing(edit, key, now).map(Some)
    }

    /// The tier table's "Complete orbit": expands EVERY incomplete orbit unit tier
    /// `index` decomposes into to its full symmetric membership, one
    /// `Design::add_orbit_member` edit (one undo step) per unit, anchored on the
    /// unit's own first member.
    ///
    /// `Ok(false)` when there is nothing to do (no such tier, or every unit is
    /// already complete); `Ok(true)` when every unit was completed. A failing unit
    /// does not stop the others; the LAST failure is returned.
    ///
    /// # Errors
    ///
    /// The last [`EditError`] one of the per-unit edits returned.
    pub fn complete_orbit(&mut self, index: usize) -> Result<bool, EditError> {
        let Ok(units) = self.design.orbit_units(index) else {
            return Ok(false);
        };
        let anchors: Vec<f64> = units
            .iter()
            .filter(|unit| !unit.is_complete())
            .filter_map(|unit| unit.members.first().copied())
            .collect();
        if anchors.is_empty() {
            return Ok(false);
        }
        let mut last_err = None;
        for position in anchors {
            let outcome = self
                .design
                .add_orbit_member(index, position)
                .and_then(|edit| self.apply(edit));
            if let Err(e) = outcome {
                last_err = Some(e);
            }
        }
        last_err.map_or(Ok(true), Err)
    }

    /// Mirrors every index-wheel position of tier `index` (both `indices` and
    /// `detached`) to the other side of the symmetry axis, via
    /// `Design::mirror_indices`, as one undoable edit.
    ///
    /// # Errors
    ///
    /// `Design::mirror_indices`'s error (no such tier), or [`Self::apply`]'s.
    pub fn mirror_indices(&mut self, index: usize) -> Result<EditChange, EditError> {
        self.design
            .mirror_indices(index)
            .and_then(|edit| self.apply(edit))
    }

    /// Bumps the generation, renumbers the selection through `renumbering` (the edit's
    /// row shifts; `None` when they are unknown, which clears the selection rather
    /// than leave it naming arbitrary tiers), and describes the change.
    fn record_change(
        &mut self,
        (tier_count_before, concave_count_before): (usize, usize),
        renumbering: Option<&TierIndexMap>,
    ) -> EditChange {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.renumber_selection(renumbering);
        let (tier_count_after, concave_count_after) = self.tier_counts();
        EditChange {
            generation,
            tier_count_before,
            tier_count_after,
            concave_count_before,
            concave_count_after,
        }
    }

    /// Renumbers the selections through `renumbering` (see [`Self::record_change`]) without
    /// touching the generation.
    fn renumber_selection(&mut self, renumbering: Option<&TierIndexMap>) {
        let (tier_count_after, concave_count_after) = self.tier_counts();
        self.selected_concave = renumbering
            .and_then(|map| map.map_concave(self.selected_concave?))
            .filter(|&index| index < concave_count_after);
        self.multi_selected = renumbering.map_or_else(BTreeSet::new, |map| {
            map.remap(&self.multi_selected)
                .into_iter()
                .filter(|&index| index < tier_count_after)
                .collect()
        });
    }

    /// `(flat, concave)` tier counts, for [`EditChange`].
    const fn tier_counts(&self) -> (usize, usize) {
        (self.design.tiers.len(), self.design.concave_tiers.len())
    }
}

/// Template `template_index`'s starting tiers -- see [`EditorSession::from_template`]
/// for the index convention. `None` for "Empty" and any index the table lacks.
#[must_use]
pub fn template_tiers(template_index: i32) -> Option<Vec<ConstraintTier>> {
    usize::try_from(template_index - 1)
        .ok()
        .and_then(|table_index| indicatrix_cut_core::templates::TEMPLATES.get(table_index))
        .map(indicatrix_cut_core::templates::TemplateSpec::tiers)
}

/// Clamps a nudged angle to the ORIGINAL tier's crown/pavilion side instead of letting it
/// cross zero.
///
/// The meet solver's side rule (negative is pavilion, non-negative crown, `-0.0` forces
/// pavilion) means a nudge that crosses zero would silently reclassify the tier into the
/// other block.
///
/// `-0.0`/`0.0` are the two
/// boundary values, so the clamped result still carries the correct side.
#[must_use]
pub const fn clamp_nudge_to_side(current: f64, nudged: f64) -> f64 {
    if current.is_sign_negative() == nudged.is_sign_negative() {
        return nudged;
    }
    if current.is_sign_negative() {
        -0.0
    } else {
        0.0
    }
}

/// The signed change that moves the number a tier's row SHOWS by `displayed_delta`
/// degrees.
///
/// The tier table prints the angle's magnitude (a pavilion tier's `-40` reads `40`, the
/// side being the label), so "up" has to make that number bigger. A crown tier's stored
/// angle is positive and takes the delta as it is; a pavilion tier's is negative (a `-0.0`
/// culet included), so its stored value moves the other way. The result still goes
/// through [`clamp_nudge_to_side`], which stops a tier at zero.
#[must_use]
pub const fn displayed_nudge_delta(current: f64, displayed_delta: f64) -> f64 {
    if current.is_sign_negative() {
        -displayed_delta
    } else {
        displayed_delta
    }
}

/// A tier's short label for a clamped-nudge explanation -- `"tier 5 (Girdle)"` when
/// named, else `"tier 5"` (1-based, matching the tier table's own `#` column).
#[must_use]
pub fn tier_nudge_label(tier: &ConstraintTier, index: usize) -> String {
    if tier.name.is_empty() {
        format!("tier {}", index + 1)
    } else {
        format!("tier {} ({})", index + 1, tier.name)
    }
}

/// Whether an analysis result stamped with `result_generation` is stale against
/// `current_generation`, i.e. the design moved on since it was computed.
///
/// `None` (no
/// result yet) is never stale -- there is nothing to badge.
#[must_use]
pub const fn result_is_stale(result_generation: Option<u64>, current_generation: u64) -> bool {
    match result_generation {
        Some(g) => g != current_generation,
        None => false,
    }
}

/// The coalescing key for an angle nudge targeting exactly `targets` (a single row,
/// or every row in a multi-select group).
///
/// Order-independent (sorted and deduplicated
/// first), so nudging `{3, 4}` hashes the same whichever row the event fired on, but
/// distinct from nudging `3` alone: a different target SET never coalesces.
#[must_use]
pub fn angle_nudge_coalesce_key(targets: &[usize]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut sorted = targets.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Hashes each element via an explicit loop, not `sorted.hash(&mut hasher)` on the
    // whole `Vec` -- clippy::nursery's `collection_is_never_read` does not recognize
    // a `Hash` call on the whole collection as "reading" it and flags it as dead.
    sorted.len().hash(&mut hasher);
    for value in &sorted {
        value.hash(&mut hasher);
    }
    hasher.finish()
}
