//! [`EditorState`]'s `History`-mutating methods (`apply`/`apply_coalescing`/
//! `undo`/`redo`/`apply_optimize_outcome`/`is_dirty`), the angle-nudge coalescing
//! key, and the scratch-field dirty-tracking [`EditorState::record_scratch_push`]
//! uses to avoid stomping an in-progress edit on an unrelated refresh.

use super::core::EditorState;
use indicatrix_cut_core::{Edit, EditError, MaterialSelection, OptimizeOutcome, PreformSpec};
use std::sync::atomic::Ordering as AtomicOrdering;

impl EditorState {
    /// Applies `edit` through [`indicatrix_cut_core::History::apply`] -- the only way
    /// this group mutates `design`. See this group's `mod.rs` doc comment.
    pub(in crate::gui::editor) fn apply(&mut self, edit: Edit) -> Result<(), EditError> {
        let Self {
            design, history, ..
        } = self;
        let outcome = history.apply(design, edit);
        if outcome.is_ok() {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        outcome
    }

    /// Like [`Self::apply`], but through
    /// [`indicatrix_cut_core::History::apply_coalescing`] instead -- what the
    /// angle-nudge keyboard/wheel handlers use so several nudges typed in quick
    /// succession (`key`, matched against `History`'s own 500ms window) collapse
    /// into one undo step. See `angle_nudge_coalesce_key` for how the editor
    /// derives `key` from the nudge's target tier(s).
    ///
    /// # Errors
    ///
    /// Propagates [`indicatrix_cut_core::History::apply_coalescing`]'s error
    /// verbatim.
    pub(in crate::gui::editor) fn apply_coalescing(
        &mut self,
        edit: Edit,
        key: u64,
    ) -> Result<(), EditError> {
        let Self {
            design, history, ..
        } = self;
        let outcome = history.apply_coalescing(design, edit, key, std::time::Instant::now());
        if outcome.is_ok() {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        outcome
    }

    /// Undoes through [`indicatrix_cut_core::History::undo`] -- see [`Self::apply`].
    ///
    /// # Errors
    ///
    /// Propagates [`indicatrix_cut_core::History::undo`]'s error verbatim (a failed
    /// replay of the recorded inverse edit) -- the caller (`gui::editor::callbacks::
    /// tier_actions::setup_undo_callback`) surfaces this via a toast rather than
    /// unwrapping/panicking, since `History::undo` returns `Err` here instead of
    /// panicking.
    pub(in crate::gui::editor) fn undo(&mut self) -> Result<bool, EditError> {
        let Self {
            design, history, ..
        } = self;
        let undone = history.undo(design)?;
        if undone {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        Ok(undone)
    }

    /// Redoes through [`indicatrix_cut_core::History::redo`] -- see [`Self::undo`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::undo`], symmetrically for
    /// [`indicatrix_cut_core::History::redo`].
    pub(in crate::gui::editor) fn redo(&mut self) -> Result<bool, EditError> {
        let Self {
            design, history, ..
        } = self;
        let redone = history.redo(design)?;
        if redone {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        Ok(redone)
    }

    /// Drops any [`Self::multi_selected`] index that no longer names a real tier --
    /// called after every successful [`Self::apply`]/[`Self::apply_coalescing`]/
    /// [`Self::undo`]/[`Self::redo`], since any of those can change `design.tiers`'
    /// length (an `AddTier`/`RemoveTier`, or undoing/redoing one).
    fn prune_multi_selected(&mut self) {
        let tier_count = self.design.tiers.len();
        self.multi_selected.retain(|&index| index < tier_count);
    }

    /// Applies `outcome`'s [`indicatrix_cut_core::AngleChange`]s through
    /// [`indicatrix_cut_core::apply_optimize_outcome`] -- the one path that turns an
    /// Optimize result into real, undoable edits (one [`Edit::ModifyTier`] per changed
    /// tier, via `History`). The fourth and last function allowed to touch `design`
    /// and `history` together.
    ///
    /// Bumps `generation` for `Ok(applied)` iff `applied > 0`, matching
    /// [`Self::apply`]/[`Self::undo`]/[`Self::redo`] -- but also on `Err`, unlike
    /// them: `apply_optimize_outcome` stops at the first tier index that no longer
    /// names a real tier, but every change before that point already went through
    /// `History::apply` for real, and its `Result` gives no way to learn "how many"
    /// on `Err`. Bumping unconditionally errs toward over-invalidating rather than
    /// silently treating a partially-edited design as unchanged.
    pub(in crate::gui::editor) fn apply_optimize_outcome(
        &mut self,
        outcome: &OptimizeOutcome,
    ) -> Result<usize, EditError> {
        let Self {
            design, history, ..
        } = self;
        let result = indicatrix_cut_core::apply_optimize_outcome(history, design, outcome);
        match &result {
            Ok(0) => {}
            Ok(_) | Err(_) => {
                self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            }
        }
        result
    }

    /// Whether `design` has changed since the last successful save/load/new -- see
    /// [`Self::saved_generation`]'s own doc comment for exactly what this compares
    /// and why. Read by the New/Load Selected/Open Native callbacks and window-close
    /// handling before they replace or discard this state, and by
    /// `EditorModel.recompute_dirty`'s handler (`gui::editor::native_io::
    /// setup_dirty_tracking`) to keep the status strip's "Unsaved" marker and the
    /// window title's leading marker live.
    #[must_use]
    pub(in crate::gui::editor) fn is_dirty(&self) -> bool {
        self.generation.load(AtomicOrdering::Relaxed) != self.saved_generation
    }

    /// Compares `self.design`'s current settings/preform/yield-relevant fields
    /// against the [`PushedScratch`] snapshot recorded the last time this ran,
    /// records a fresh snapshot, and returns which groups actually changed. See
    /// [`ScratchDelta`]'s own doc comment for why each group is independent.
    /// Called exactly once per `view::refresh_editor_panel`/`view::
    /// push_stale_content` invocation (never from inside a function those two
    /// call, or a group could be compared against a snapshot already
    /// overwritten by an earlier group in the same refresh).
    pub(in crate::gui::editor) fn record_scratch_push(&self) -> ScratchDelta {
        let design = &self.design;
        let mut cache = self.last_pushed_scratch.borrow_mut();
        let meta_now = (
            design.meta.headers.clone(),
            design.meta.footnotes.clone(),
            design.meta.gear_reference_angle,
        );
        let delta = ScratchDelta {
            material: cache.material.as_ref() != Some(&design.material),
            gear: cache.gear_teeth != Some(design.meta.gear_teeth),
            symmetry: cache.symmetry != Some((design.meta.symmetry_order, design.meta.mirror)),
            preform: cache.preform != Some(design.preform),
            girdle: cache.girdle_diameter_mm
                != GirdleDiameterPush::Observed(design.girdle_diameter_mm),
            meta: cache.meta.as_ref() != Some(&meta_now),
        };
        *cache = PushedScratch {
            material: Some(design.material.clone()),
            gear_teeth: Some(design.meta.gear_teeth),
            symmetry: Some((design.meta.symmetry_order, design.meta.mirror)),
            preform: Some(design.preform),
            girdle_diameter_mm: GirdleDiameterPush::Observed(design.girdle_diameter_mm),
            meta: Some(meta_now),
        };
        delta
    }
}

/// See [`EditorState::last_pushed_scratch`]. Every field is `None` (or, for
/// `girdle_diameter_mm`, [`GirdleDiameterPush::Unobserved`]) until the first push
/// observes it, so the very first refresh after New/Load always reports every
/// group changed (a correct, if slightly redundant, initial push).
#[derive(Default)]
pub(in crate::gui::editor) struct PushedScratch {
    pub(super) material: Option<MaterialSelection>,
    pub(super) gear_teeth: Option<i32>,
    pub(super) symmetry: Option<(u32, bool)>,
    pub(super) preform: Option<PreformSpec>,
    pub(super) girdle_diameter_mm: GirdleDiameterPush,
    /// `(headers, footnotes, gear_reference_angle)` -- see
    /// [`ScratchDelta::meta`]'s own doc comment.
    pub(super) meta: Option<(Vec<String>, Vec<String>, f64)>,
}

/// [`PushedScratch::girdle_diameter_mm`]'s own value -- NOT a plain
/// `Option<Option<f64>>`, since the design's own `girdle_diameter_mm` is already an
/// `Option<f64>` (unset vs a real dimension): nesting it in another `Option` would
/// conflate that "unset" state with "this push has never been observed yet", the
/// same distinction every sibling field in [`PushedScratch`] uses its own outer
/// `Option` for. This makes the two levels explicit instead.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum GirdleDiameterPush {
    /// No push has been recorded yet -- the very first refresh after New/Load
    /// always reports `girdle` changed (see [`PushedScratch`]'s own doc comment).
    #[default]
    Unobserved,
    /// The last-pushed value; `None` when the design itself has no girdle
    /// diameter set.
    Observed(Option<f64>),
}

/// Which of [`PushedScratch`]'s groups actually changed since the last push,
/// returned by [`EditorState::record_scratch_push`] -- five independent flags
/// rather than one "anything changed" bit, because a design-settings-only
/// change (say, Apply Gear) must
/// never also blank an in-progress, unrelated edit sitting in the Preform tab's
/// Half-Width field, and vice versa. `view::refresh_design_settings` gates the
/// material/gear/symmetry pushes on their own flags; the preform+yield push in
/// `view::refresh_editor_panel`/`view::push_stale_content` gates on `preform`/
/// `girdle`/`material` the same way.
pub(in crate::gui::editor) struct ScratchDelta {
    pub(in crate::gui::editor) material: bool,
    pub(in crate::gui::editor) gear: bool,
    pub(in crate::gui::editor) symmetry: bool,
    pub(in crate::gui::editor) preform: bool,
    pub(in crate::gui::editor) girdle: bool,
    /// Whether `design.meta`'s headers/footnotes/gear-reference-
    /// angle changed since the last push -- gates `EditorModel.design_title`/
    /// `design_extra_headers`/`design_footnotes`/`design_gear_reference_angle`'s
    /// own reseed in `view::refresh_design_settings`, same "don't blank an
    /// in-progress, unrelated edit" reasoning as every other field here.
    pub(in crate::gui::editor) meta: bool,
}

/// Whether an analysis result
/// stamped with `result_generation` (Deep Solve's [`EditorState::
/// deep_solve_result_generation`], Optimize's `pending_optimize`-stored
/// generation, or any future caller's own equivalent) is stale against
/// `current_generation` -- i.e. the design has moved on since that result was
/// computed. `None` (no result has ever completed) is never stale -- there is
/// nothing to badge yet, not a result "as stale as it gets."
///
/// Pure and unit tested directly: this is the one decision every "Stale: design
/// changed" badge in this app reduces to, whatever Slint property or `ui/
/// components/stale_badge.slint` instance ends up reading it.
#[must_use]
pub(in crate::gui::editor) const fn result_is_stale(
    result_generation: Option<u64>,
    current_generation: u64,
) -> bool {
    match result_generation {
        Some(g) => g != current_generation,
        None => false,
    }
}

/// The `key` [`EditorState::apply_coalescing`] passes through to
/// [`indicatrix_cut_core::History::apply_coalescing`] for an angle nudge targeting
/// exactly `targets` (a single row, or every row in a multi-select group) --
/// order-independent (sorts first) so nudging tiers `{3, 4}` always hashes the same
/// regardless of which one the wheel/keyboard event actually fired on, but distinct
/// from nudging tier `3` alone: a different target SET must never coalesce with a
/// nudge of a different one, even when they overlap.
pub(in crate::gui::editor) fn angle_nudge_coalesce_key(targets: &[usize]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut sorted = targets.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Hashes each element via an explicit loop, not `sorted.hash(&mut hasher)` on the
    // whole `Vec` -- clippy::nursery's `collection_is_never_read` does not recognize
    // a `Hash` call on the whole collection as "reading" it (it only ever mutates
    // `sorted` via `sort_unstable`/`dedup` from its point of view otherwise) and
    // flags it as dead, even though hashing every element genuinely does read the
    // sorted, deduplicated contents this function exists to key on.
    sorted.len().hash(&mut hasher);
    for value in &sorted {
        value.hash(&mut hasher);
    }
    hasher.finish()
}
