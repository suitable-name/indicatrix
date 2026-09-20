//! [`History`]: the undo/redo stack itself, built entirely on
//! [`crate::design::Design::apply_edit`]'s command/inverse pairs.

use super::edit_type::{Edit, EditError};
use crate::design::Design;
use std::time::{Duration, Instant};

/// An undo/redo stack of [`Edit`]s over one [`Design`].
///
/// Holds no copy of the design itself -- only the sequence of edits needed to move it
/// backward or forward -- so its memory cost is proportional to how much has actually
/// changed, not to the design's size or the schedule's tier count.
#[derive(Debug, Clone, PartialEq)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// The `(key, timestamp)` [`Self::apply_coalescing`] last succeeded with, or
    /// `None` right after construction or any ordinary [`Self::apply`]/[`Self::undo`]/
    /// [`Self::redo`] -- see that method's own doc comment for how this decides
    /// whether the NEXT `apply_coalescing` call merges into the same undo entry or
    /// starts a new one. Never compared for equality/`Debug` purposes beyond the
    /// derived impls above (a coalescing run in progress is not itself part of two
    /// `History`s being "the same edit sequence" in any test that matters -- every
    /// existing `assert_eq!(design, ...)` in this crate compares `Design`, never
    /// `History`, and the handful of direct `History` comparisons are `can_undo`/
    /// `can_redo` checks that don't care about this field either).
    last_coalesce: Option<(u64, Instant)>,
    /// This instance's own coalescing window -- [`Self::COALESCE_WINDOW`] unless
    /// built via [`Self::with_coalesce_window`]. CAD audit item 165: a fixed,
    /// crate-wide 500ms suited one caller (a keyboard/wheel angle nudge) but not
    /// every possible coalescing caller equally -- e.g. a slower, more deliberate
    /// interaction might want a longer window -- so this is now a per-`History`
    /// value rather than a single `const` every caller was stuck with.
    coalesce_window: Duration,
}

impl History {
    /// The default coalescing window: how long after [`Self::apply_coalescing`]
    /// last succeeded with a given key a following call with the SAME key still
    /// merges into that same undo entry, for a `History` built via [`Self::new`]
    /// -- see that method's own doc comment. [`Self::with_coalesce_window`] builds
    /// a `History` with a different window instead.
    const COALESCE_WINDOW: Duration = Duration::from_millis(500);

    /// A fresh, empty history (nothing to undo or redo), coalescing with the
    /// default [`Self::COALESCE_WINDOW`] (500ms).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            last_coalesce: None,
            coalesce_window: Self::COALESCE_WINDOW,
        }
    }

    /// Like [`Self::new`], but [`Self::apply_coalescing`] uses `window` instead of
    /// the default [`Self::COALESCE_WINDOW`] -- CAD audit item 165: lets a caller
    /// with a different natural pace for its own coalesced interaction (or one
    /// that always ends a run explicitly via [`Self::end_coalesce_run`] and so
    /// wants a short or even zero window as a pure safety net) pick its own value
    /// rather than being stuck with the one every other caller in this crate
    /// shares.
    #[must_use]
    pub const fn with_coalesce_window(window: Duration) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            last_coalesce: None,
            coalesce_window: window,
        }
    }

    /// Applies `edit` to `design`, pushing its inverse onto the undo stack
    /// and clearing the redo stack (the standard rule: making a new edit
    /// after undoing abandons the undone branch, exactly like a text
    /// editor). Leaves `design` untouched if `edit` is invalid.
    ///
    /// Ends any [`Self::apply_coalescing`] run in progress -- an ordinary `apply`
    /// between two coalescing calls with the same key must NOT let the second one
    /// merge across it.
    ///
    /// # Errors
    ///
    /// Propagates [`Design::apply_edit`]'s error verbatim.
    pub fn apply(&mut self, design: &mut Design, edit: Edit) -> Result<(), EditError> {
        let inverse = design.apply_edit(edit)?;
        self.undo.push(inverse);
        self.redo.clear();
        self.last_coalesce = None;
        Ok(())
    }

    /// Like [`Self::apply`], but merges into the MOST RECENT undo entry instead of
    /// pushing a new one when this same `key` last succeeded here less than this
    /// instance's own coalescing window (500ms by default -- see
    /// [`Self::COALESCE_WINDOW`]/[`Self::with_coalesce_window`]) before `now` --
    /// what the editor's angle-nudge
    /// keyboard/wheel handlers use so N nudges typed in quick succession collapse
    /// into ONE undo step rather than one per keystroke, while a nudge that starts a
    /// fresh burst (a different tier/selection, or the same one after a pause) still
    /// gets its own undo entry.
    ///
    /// `key` is caller-defined and opaque to this crate -- the editor hashes the
    /// sorted set of tier indices a nudge targets (see
    /// `gui::editor::state::angle_nudge_coalesce_key`) so nudging tier 3 alone never
    /// merges with nudging tiers {3, 4} together, even though both involve tier 3.
    ///
    /// Merging keeps the OLDER inverse already on the undo stack (the one that
    /// reverts all the way back to before this whole coalesced run) and discards the
    /// new inverse [`Design::apply_edit`] just computed (which would only revert the
    /// latest nudge) -- exactly the "amend the last entry" semantics `History` has no
    /// separate storage for, since the old inverse already IS that combined undo
    /// step.
    ///
    /// # Errors
    ///
    /// Propagates [`Design::apply_edit`]'s error verbatim -- `design` (and this
    /// `History`) are left untouched on `Err`, same as [`Self::apply`].
    pub fn apply_coalescing(
        &mut self,
        design: &mut Design,
        edit: Edit,
        key: u64,
        now: Instant,
    ) -> Result<(), EditError> {
        let inverse = design.apply_edit(edit)?;
        let merges = self.last_coalesce.is_some_and(|(last_key, last_time)| {
            last_key == key && now.saturating_duration_since(last_time) <= self.coalesce_window
        });
        if merges {
            // The undo entry recorded by the FIRST nudge in this run already reverts
            // all the way back to before it started -- discard this call's own
            // inverse rather than pushing it, so one undo still reverts the whole run.
            let _ = inverse;
        } else {
            self.undo.push(inverse);
        }
        self.redo.clear();
        self.last_coalesce = Some((key, now));
        Ok(())
    }

    /// Undoes the most recent [`History::apply`], moving its inverse onto
    /// the redo stack. Returns `Ok(false)` (leaving `design` untouched) when
    /// there is nothing to undo, `Ok(true)` on a successful undo.
    ///
    /// # Errors
    ///
    /// Returns the [`EditError`] if replaying the recorded inverse against
    /// `design` fails -- this would mean either `design` was mutated by
    /// something other than this `History` since the edit was recorded, or
    /// the recorded index math itself has a bug. The failed edit is pushed
    /// back onto the undo stack (not dropped) so no history is lost and the
    /// caller can surface the error without corrupting the stack -- a caller
    /// that used to rely on this never failing should report the error
    /// (e.g. a toast) rather than unwrap/expect it.
    pub fn undo(&mut self, design: &mut Design) -> Result<bool, EditError> {
        let Some(edit) = self.undo.pop() else {
            return Ok(false);
        };
        // An undo must never merge into a LATER apply_coalescing call as if nothing
        // happened in between.
        self.last_coalesce = None;
        match design.apply_edit(edit.clone()) {
            Ok(inverse) => {
                self.redo.push(inverse);
                Ok(true)
            }
            Err(err) => {
                // Put the edit back rather than dropping it -- a failed replay must not
                // silently lose the undo entry.
                self.undo.push(edit);
                Err(err)
            }
        }
    }

    /// Re-applies the most recently undone edit, moving its inverse back
    /// onto the undo stack. Returns `Ok(false)` (leaving `design` untouched)
    /// when there is nothing to redo, `Ok(true)` on a successful redo.
    ///
    /// # Errors
    ///
    /// Same invariant/recovery behaviour as [`History::undo`] (symmetrically
    /// against the redo stack).
    pub fn redo(&mut self, design: &mut Design) -> Result<bool, EditError> {
        let Some(edit) = self.redo.pop() else {
            return Ok(false);
        };
        // Same reasoning as `Self::undo`'s matching line.
        self.last_coalesce = None;
        match design.apply_edit(edit.clone()) {
            Ok(inverse) => {
                self.undo.push(inverse);
                Ok(true)
            }
            Err(err) => {
                self.redo.push(edit);
                Err(err)
            }
        }
    }

    /// `true` iff [`History::undo`] would do something.
    #[must_use]
    pub const fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// `true` iff [`History::redo`] would do something.
    #[must_use]
    pub const fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The [`Edit`] [`History::undo`] would replay next, without applying it --
    /// lets a caller (`crate::resolve::resolve_after_edit`) classify which tiers an
    /// upcoming undo will touch *before* committing to it, so it can re-solve only
    /// those once the undo actually runs. `None` iff [`Self::can_undo`] is `false`.
    #[must_use]
    pub fn peek_undo(&self) -> Option<&Edit> {
        self.undo.last()
    }

    /// The [`Edit`] [`History::redo`] would replay next -- see
    /// [`Self::peek_undo`]'s doc comment; the same reasoning applies
    /// symmetrically. `None` iff [`Self::can_redo`] is `false`.
    #[must_use]
    pub fn peek_redo(&self) -> Option<&Edit> {
        self.redo.last()
    }

    /// Explicitly ends any [`Self::apply_coalescing`] run in progress, without
    /// making a new edit -- CAD audit item 165: lets a caller with a real
    /// interaction boundary of its own (pointer release or focus loss on the
    /// control driving the coalesced edits) name that boundary directly instead
    /// of only ever discovering a run ended once [`Self::COALESCE_WINDOW`]
    /// (or a custom [`Self::with_coalesce_window`] value) had silently elapsed.
    /// A no-op, not an error, when no run is in progress.
    ///
    /// # Handoff
    ///
    /// Nothing calls this yet: the one caller today
    /// (`gui::editor::callbacks::tier_actions::setup_nudge_angle_callback`, via
    /// `gui::editor::state::EditorState`'s own wrapper around
    /// [`Self::apply_coalescing`]) is outside this crate and outside this group's
    /// file ownership. Wiring it up means calling this once a `TierAngleCell`'s
    /// pointer-release or focus-loss fires -- both files that would touch
    /// (`callbacks/tier_actions.rs`, the `TierAngleCell` component) belong to a
    /// different lane.
    pub const fn end_coalesce_run(&mut self) {
        self.last_coalesce = None;
    }
}

impl Default for History {
    /// Same as [`Self::new`] -- written out by hand (rather than
    /// `#[derive(Default)]`) because the `coalesce_window` field needs
    /// [`Self::COALESCE_WINDOW`] as its default, not `Duration`'s own zero
    /// default a derived impl would give it.
    fn default() -> Self {
        Self::new()
    }
}
