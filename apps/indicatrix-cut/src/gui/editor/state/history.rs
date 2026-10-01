//! [`EditorState`]'s `History`-mutating entry points. Every one routes through the
//! shared [`indicatrix_editor::EditorSession`] (so the desktop and the web app edit
//! identically) and keeps the desktop's own signatures; the desktop supplies the
//! coalescing clock ([`coalesce_timestamp`]). `apply_optimize_outcome`/`is_dirty`
//! are the session's own methods, reached through `Deref`.

use super::core::EditorState;
use indicatrix_cut_core::{Edit, EditError};
use indicatrix_editor::{EditChange, NudgeOutcome, PinOutcome, scratch::ScratchDelta};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

/// The monotonic timestamp [`indicatrix_editor::EditorSession::apply_coalescing`]
/// takes: time elapsed since a process-wide origin, fixed on the first call. Only
/// differences between successive values matter to `History`, so the origin itself
/// never does.
pub(in crate::gui::editor) fn coalesce_timestamp() -> Duration {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed()
}

impl EditorState {
    /// Applies `edit` through [`indicatrix_cut_core::History::apply`] -- the only way
    /// this group mutates `design`. See this group's `mod.rs` doc comment.
    pub(in crate::gui::editor) fn apply(&mut self, edit: Edit) -> Result<(), EditError> {
        self.session.apply(edit).map(|_| ())
    }

    /// Like [`Self::apply`], but through
    /// [`indicatrix_cut_core::History::apply_coalescing`] instead -- what the
    /// angle-nudge keyboard/wheel handlers use so several nudges typed in quick
    /// succession (`key`, matched against the history's coalescing window) collapse
    /// into one undo step. See `angle_nudge_coalesce_key` for how the editor derives
    /// `key` from the nudge's target tier(s).
    ///
    /// Test-only since the production nudge path became [`Self::nudge_angles`]
    /// (the same coalescing apply, plus the zero-crossing clamp); the history tests
    /// and the identity pins still drive the raw coalescing apply directly.
    ///
    /// # Errors
    ///
    /// Propagates [`indicatrix_cut_core::History::apply_coalescing`]'s error
    /// verbatim.
    #[cfg(test)]
    pub(in crate::gui::editor) fn apply_coalescing(
        &mut self,
        edit: Edit,
        key: u64,
    ) -> Result<(), EditError> {
        self.session
            .apply_coalescing(edit, key, coalesce_timestamp())
            .map(|_| ())
    }

    /// Nudges every tier in `targets` by `delta_deg` as one coalescing edit, each
    /// clamped to its own side of zero -- see
    /// [`indicatrix_editor::EditorSession::nudge_angles`].
    ///
    /// # Errors
    ///
    /// The session's error, verbatim.
    pub(in crate::gui::editor) fn nudge_angles(
        &mut self,
        targets: &[usize],
        delta_deg: f64,
    ) -> Result<Option<NudgeOutcome>, EditError> {
        self.session
            .nudge_angles(targets, delta_deg, coalesce_timestamp())
    }

    /// Sets tier `tier`'s angle as one coalescing edit of a handle drag -- see
    /// [`indicatrix_editor::EditorSession::set_tier_angle`].
    ///
    /// `now` is the drag's GESTURE clock (one [`coalesce_timestamp`] read taken at
    /// pointer-down and passed to every call of the gesture): a pointer held still for
    /// longer than the coalescing window between two moves must not split the drag
    /// into two undo steps, and passing the same instant every time makes that
    /// impossible.
    ///
    /// # Errors
    ///
    /// The session's error, verbatim.
    pub(in crate::gui::editor) fn set_tier_angle(
        &mut self,
        tier: usize,
        angle_deg: f64,
        now: Duration,
    ) -> Result<Option<EditChange>, EditError> {
        self.session.set_tier_angle(tier, angle_deg, now)
    }

    /// Pins tier `tier`'s mast as one coalescing edit of a handle drag -- see
    /// [`indicatrix_editor::EditorSession::pin_tier_mast`] and, for `now`,
    /// [`Self::set_tier_angle`].
    ///
    /// # Errors
    ///
    /// The session's error, verbatim.
    pub(in crate::gui::editor) fn pin_tier_mast(
        &mut self,
        tier: usize,
        mast: f64,
        now: Duration,
    ) -> Result<Option<PinOutcome>, EditError> {
        self.session.pin_tier_mast(tier, mast, now)
    }

    /// Turns tier `tier`'s index-wheel positions by `k_teeth` (relative to now) as one
    /// coalescing edit of a handle drag -- see
    /// [`indicatrix_editor::EditorSession::rotate_tier_indices`] and, for `now`,
    /// [`Self::set_tier_angle`].
    ///
    /// # Errors
    ///
    /// The session's error, verbatim.
    pub(in crate::gui::editor) fn rotate_tier_indices(
        &mut self,
        tier: usize,
        k_teeth: i64,
        now: Duration,
    ) -> Result<Option<EditChange>, EditError> {
        self.session.rotate_tier_indices(tier, k_teeth, now)
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
        self.session.undo().map(|change| change.is_some())
    }

    /// Redoes through [`indicatrix_cut_core::History::redo`] -- see [`Self::undo`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::undo`], symmetrically for
    /// [`indicatrix_cut_core::History::redo`].
    pub(in crate::gui::editor) fn redo(&mut self) -> Result<bool, EditError> {
        self.session.redo().map(|change| change.is_some())
    }

    /// Compares `self.design`'s current settings/preform/yield-relevant fields
    /// against the scratch snapshot recorded the last time this ran, records a fresh
    /// snapshot, and returns which groups actually changed -- see
    /// [`indicatrix_editor::scratch::PushedScratch::record`]. Called exactly once per
    /// `view::refresh_editor_panel`/`view::push_stale_content` invocation.
    pub(in crate::gui::editor) fn record_scratch_push(&self) -> ScratchDelta {
        self.last_pushed_scratch
            .borrow_mut()
            .record(&self.session.design)
    }
}
