//! [`History`]: the undo/redo stack itself, built entirely on
//! [`crate::design::Design::apply_edit`]'s command/inverse pairs.

use super::edit_type::{Edit, EditError};
use crate::design::Design;
use std::time::Duration;

/// One recorded step of a [`History`]: the [`Edit`] that moves the design across it, the
/// words for it, and a revision number.
///
/// Keeping the three together makes it impossible for the label or the revision to drift
/// away from the edit when a step moves between the undo and the redo stack.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    /// The edit [`History::undo`] (on the undo stack) or [`History::redo`] (on the redo
    /// stack) replays next.
    edit: Edit,
    /// What the step did, worded by [`Edit::describe`] when it was made (or by the caller's
    /// own words, see `named`).
    label: String,
    /// See [`HistoryEntry::revision`].
    revision: u64,
    /// Whether `label` came from the caller ([`History::apply_labeled`]) rather than from
    /// [`Edit::describe`]. [`Edit::describe`] words what an edit does to the design, which
    /// for an inverse edit reads as the opposite action; a step with the caller's own words
    /// keeps them for the Undo and Redo hints too ([`History::undo_label`]).
    named: bool,
}

/// One step of a [`History`] as a list shows it -- see [`History::entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// What the step did, worded the way [`Edit::describe`] words it when the step was made
    /// ("Set P1 angle to 41.0 degrees"). A run of merged nudges or one drag is one entry,
    /// worded by its latest change.
    pub label: String,
    /// The step number: 1 for the first step, counting up to [`History::len_total`].
    /// Position 0 is the design before any step. The design "after this step" sits at this
    /// position.
    pub position: usize,
    /// Changes whenever the design at [`Self::position`] changes: a new step, or more nudges
    /// merged into the newest step. It does not change when the step is undone or redone.
    /// Unique within one [`History`], so `(position, revision)` names one design state for
    /// as long as that history lives -- what a thumbnail cache keys on.
    pub revision: u64,
    /// Whether the step is currently undone (it sits on the redo side).
    pub undone: bool,
}

/// Why [`History::jump_to`] or [`History::design_at`] could not reach a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpError {
    /// The history has no step at that position.
    OutOfRange {
        /// The position asked for.
        position: usize,
        /// How many steps the history has ([`History::len_total`]).
        steps: usize,
    },
    /// A step could not be replayed (see [`History::undo`]): the design was changed behind
    /// the history's back.
    Replay {
        /// The position the design stands at: the last one reached before the failure.
        at: usize,
        /// Why the replay failed.
        error: EditError,
    },
}

impl std::fmt::Display for JumpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfRange { position, steps } => {
                write!(f, "There is no step {position}. The history has {steps}.")
            }
            Self::Replay { at, error } => {
                write!(
                    f,
                    "The history could not be replayed past step {at}: {error}."
                )
            }
        }
    }
}

impl std::error::Error for JumpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::OutOfRange { .. } => None,
            Self::Replay { error, .. } => Some(error),
        }
    }
}

/// An undo/redo stack of [`Edit`]s over one [`Design`].
///
/// Holds no copy of the design itself -- only the sequence of edits needed to move it
/// backward or forward -- so its memory cost is proportional to how much has actually
/// changed, not to the design's size or the schedule's tier count.
#[derive(Debug, Clone, PartialEq)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The revision the next new step (or the next merge into the newest step) gets -- see
    /// [`HistoryEntry::revision`].
    next_revision: u64,
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
    last_coalesce: Option<(u64, Duration)>,
    /// This instance's own coalescing window -- [`Self::COALESCE_WINDOW`] unless
    /// built via [`Self::with_coalesce_window`]. A fixed,
    /// crate-wide 500ms suits one caller (a keyboard/wheel angle nudge) but not
    /// every possible coalescing caller equally -- e.g. a slower, more deliberate
    /// interaction might want a longer window -- so this is a per-`History`
    /// value rather than a single shared `const`.
    coalesce_window: Duration,
    /// A bounded, append-only journal of [`Edit::describe`] strings for every edit
    /// actually applied via [`Self::apply`]/[`Self::apply_coalescing`], oldest
    /// first -- unlike `undo`/`redo`, [`Self::undo`]/
    /// [`Self::redo`] never remove or reorder entries here: this is a trail of
    /// what was done, not a stack of what could still be replayed, so undoing an
    /// edit does not erase it from the record. Bounded to [`Self::MAX_LOG_ENTRIES`]
    /// so an old design's native sidecar `[history]` table cannot grow forever.
    /// A coalesced run ([`Self::apply_coalescing`]) updates its own single entry in
    /// place on every merge rather than appending one per nudge, matching how it
    /// already collapses to a single undo step.
    log: Vec<String>,
}

impl History {
    /// The default coalescing window: how long after [`Self::apply_coalescing`]
    /// last succeeded with a given key a following call with the SAME key still
    /// merges into that same undo entry, for a `History` built via [`Self::new`]
    /// -- see that method's own doc comment. [`Self::with_coalesce_window`] builds
    /// a `History` with a different window instead.
    const COALESCE_WINDOW: Duration = Duration::from_millis(500);

    /// The bound on [`Self::log`] -- see that field's own doc comment.
    const MAX_LOG_ENTRIES: usize = 200;

    /// A fresh, empty history (nothing to undo or redo), coalescing with the
    /// default [`Self::COALESCE_WINDOW`] (500ms).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            next_revision: 0,
            last_coalesce: None,
            coalesce_window: Self::COALESCE_WINDOW,
            log: Vec::new(),
        }
    }

    /// Like [`Self::new`], but [`Self::apply_coalescing`] uses `window` instead of
    /// the default [`Self::COALESCE_WINDOW`] -- lets a caller
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
            next_revision: 0,
            last_coalesce: None,
            coalesce_window: window,
            log: Vec::new(),
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
        self.apply_worded(design, edit, None)
    }

    /// Like [`Self::apply`], but the step is worded `label` instead of by
    /// [`Edit::describe`].
    ///
    /// For an edit whose own words do not say what the cutter did: opening a saved variant
    /// is one `ReplaceSchedule` (or a batch holding one), which would read "Edit
    /// instructions as text" or "2 combined edits" in the history list. The words are kept
    /// for the step's whole life (undo, redo, the history list, the description log), and
    /// for the Undo and Redo hints ([`Self::undo_label`], [`Self::redo_label`]).
    ///
    /// A blank `label` counts as none: the step is worded by [`Edit::describe`] as for
    /// [`Self::apply`].
    ///
    /// # Errors
    ///
    /// As [`Self::apply`]: [`Design::apply_edit`]'s error verbatim, with `design` and this
    /// history untouched.
    pub fn apply_labeled(
        &mut self,
        design: &mut Design,
        edit: Edit,
        label: &str,
    ) -> Result<(), EditError> {
        self.apply_worded(design, edit, Some(label))
    }

    /// [`Self::apply`] and [`Self::apply_labeled`]: the step is worded `label` when that
    /// holds any text, else by [`Edit::describe`].
    fn apply_worded(
        &mut self,
        design: &mut Design,
        edit: Edit,
        label: Option<&str>,
    ) -> Result<(), EditError> {
        let own_words = label.map(str::trim).filter(|words| !words.is_empty());
        let description = own_words.map_or_else(|| edit.describe(design), str::to_owned);
        let inverse = design.apply_edit(edit)?;
        let revision = self.take_revision();
        self.undo.push(Step {
            edit: inverse,
            label: description.clone(),
            revision,
            named: own_words.is_some(),
        });
        self.redo.clear();
        self.last_coalesce = None;
        self.push_log_entry(description);
        Ok(())
    }

    /// The next revision number -- see [`HistoryEntry::revision`].
    const fn take_revision(&mut self) -> u64 {
        let revision = self.next_revision;
        self.next_revision += 1;
        revision
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
    /// # The `now` timestamp
    ///
    /// `now` is a caller-supplied monotonic timestamp: the time elapsed since an
    /// arbitrary origin the caller keeps fixed for this `History`'s lifetime (the
    /// desktop measures it from a process-wide `std::time::Instant`; a browser can
    /// pass `performance.now()` converted with `Duration::from_secs_f64(ms / 1000.0)`).
    /// Only differences between successive `now` values matter, so the origin itself
    /// never does. It is a plain [`Duration`] rather than a `std::time::Instant`
    /// because `Instant::now()` panics on `wasm32-unknown-unknown`. A `now` earlier
    /// than the previous call's (a clock that stepped back) counts as zero elapsed
    /// time, exactly like `Instant::saturating_duration_since`.
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
        now: Duration,
    ) -> Result<(), EditError> {
        let description = edit.describe(design);
        let inverse = design.apply_edit(edit)?;
        let merges = self.last_coalesce.is_some_and(|(last_key, last_time)| {
            last_key == key && now.saturating_sub(last_time) <= self.coalesce_window
        });
        let revision = self.take_revision();
        if merges {
            // The undo entry recorded by the FIRST nudge in this run already reverts
            // all the way back to before it started -- discard this call's own
            // inverse rather than pushing it, so one undo still reverts the whole run.
            let _ = inverse;
            // The step now ends at a different design, so it takes a new revision and
            // the words of the latest change (a merge only happens right after a push or
            // a merge, so the undo stack is not empty).
            if let Some(step) = self.undo.last_mut() {
                step.label.clone_from(&description);
                step.revision = revision;
            }
            // Likewise, update the run's own single log entry in place rather than
            // appending a new one per nudge -- see `Self::log`'s own doc comment.
            if let Some(last) = self.log.last_mut() {
                *last = description;
            } else {
                self.push_log_entry(description);
            }
        } else {
            self.undo.push(Step {
                edit: inverse,
                label: description.clone(),
                revision,
                named: false,
            });
            self.push_log_entry(description);
        }
        self.redo.clear();
        self.last_coalesce = Some((key, now));
        Ok(())
    }

    /// Appends `description` to [`Self::log`], dropping the oldest entry once
    /// [`Self::MAX_LOG_ENTRIES`] would otherwise be exceeded.
    fn push_log_entry(&mut self, description: String) {
        self.log.push(description);
        if self.log.len() > Self::MAX_LOG_ENTRIES {
            self.log.remove(0);
        }
    }

    /// The bounded trail of human-readable edit descriptions recorded by
    /// [`Self::apply`]/[`Self::apply_coalescing`], oldest first -- see [`Self::log`]'s
    /// own doc comment. Feeds a native sidecar's `[history]` table (via
    /// `indicatrix_cut_core::native::SaveExtras::history_entries`).
    #[must_use]
    pub fn description_log(&self) -> &[String] {
        &self.log
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
    /// relying on this never failing should report the error
    /// (e.g. a toast) rather than unwrap/expect it.
    pub fn undo(&mut self, design: &mut Design) -> Result<bool, EditError> {
        let Some(step) = self.undo.pop() else {
            return Ok(false);
        };
        // An undo must never merge into a LATER apply_coalescing call as if nothing
        // happened in between.
        self.last_coalesce = None;
        match design.apply_edit(step.edit.clone()) {
            Ok(inverse) => {
                // The step keeps its words and revision: it is the same step, now undone.
                self.redo.push(Step {
                    edit: inverse,
                    label: step.label,
                    revision: step.revision,
                    named: step.named,
                });
                Ok(true)
            }
            Err(err) => {
                // Put the edit back rather than dropping it -- a failed replay must not
                // silently lose the undo entry.
                self.undo.push(step);
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
        let Some(step) = self.redo.pop() else {
            return Ok(false);
        };
        // Same reasoning as `Self::undo`'s matching line.
        self.last_coalesce = None;
        match design.apply_edit(step.edit.clone()) {
            Ok(inverse) => {
                self.undo.push(Step {
                    edit: inverse,
                    label: step.label,
                    revision: step.revision,
                    named: step.named,
                });
                Ok(true)
            }
            Err(err) => {
                self.redo.push(step);
                Err(err)
            }
        }
    }

    /// Moves `design` to `position` -- as many [`Self::undo`]s or [`Self::redo`]s as it takes.
    /// Position 0 is the design before any step; [`Self::len_undo`] is where it is now;
    /// [`Self::len_total`] is the furthest. Returns how many steps it moved (0 when `design`
    /// is already there). Jumping is not an edit: it records nothing, so every step stays on
    /// its side of the stack and a jump back is just another jump.
    ///
    /// `design` must be the design this history belongs to, as undo and redo need.
    ///
    /// # Errors
    ///
    /// [`JumpError::OutOfRange`] before anything moves when `position` is past the last
    /// step. [`JumpError::Replay`] when a step cannot be replayed (see [`Self::undo`]); the
    /// design then stays at the last step that did work, and the history agrees with it.
    pub fn jump_to(&mut self, design: &mut Design, position: usize) -> Result<usize, JumpError> {
        let steps = self.len_total();
        if position > steps {
            return Err(JumpError::OutOfRange { position, steps });
        }
        let mut moved = 0;
        while self.undo.len() > position {
            match self.undo(design) {
                Ok(true) => moved += 1,
                Ok(false) => break,
                Err(error) => {
                    return Err(JumpError::Replay {
                        at: self.undo.len(),
                        error,
                    });
                }
            }
        }
        while self.undo.len() < position {
            match self.redo(design) {
                Ok(true) => moved += 1,
                Ok(false) => break,
                Err(error) => {
                    return Err(JumpError::Replay {
                        at: self.undo.len(),
                        error,
                    });
                }
            }
        }
        Ok(moved)
    }

    /// A copy of `design` as it was (or will be, for an undone step) at `position`, worked
    /// out by replaying the steps between here and there on the copy. Neither `design` nor
    /// this history changes.
    ///
    /// `design` must be the design this history belongs to, standing at
    /// [`Self::len_undo`].
    ///
    /// # Errors
    ///
    /// The same two as [`Self::jump_to`].
    pub fn design_at(&self, design: &Design, position: usize) -> Result<Design, JumpError> {
        let steps = self.len_total();
        if position > steps {
            return Err(JumpError::OutOfRange { position, steps });
        }
        let current = self.undo.len();
        let mut copy = design.clone();
        // Walking back replays the undo stack from its top; walking forward replays the redo
        // stack from its top. The inverses `apply_edit` returns are not needed here.
        if position < current {
            for (walked, step) in self.undo.iter().rev().take(current - position).enumerate() {
                copy.apply_edit(step.edit.clone())
                    .map_err(|error| JumpError::Replay {
                        at: current - walked,
                        error,
                    })?;
            }
        } else {
            for (walked, step) in self.redo.iter().rev().take(position - current).enumerate() {
                copy.apply_edit(step.edit.clone())
                    .map_err(|error| JumpError::Replay {
                        at: current + walked,
                        error,
                    })?;
            }
        }
        Ok(copy)
    }

    /// How many steps [`Self::undo`] can still take back. This is also the current
    /// position: the design sits after the first `len_undo()` steps (position 0 is the
    /// design before any step).
    #[must_use]
    pub const fn len_undo(&self) -> usize {
        self.undo.len()
    }

    /// How many undone steps [`Self::redo`] can still bring back.
    #[must_use]
    pub const fn len_redo(&self) -> usize {
        self.redo.len()
    }

    /// The number of steps in all: `len_undo() + len_redo()`. A position is valid for a jump
    /// when it is at most this.
    #[must_use]
    pub const fn len_total(&self) -> usize {
        self.undo.len() + self.redo.len()
    }

    /// Every step, oldest first, then the undone ones: the steps [`Self::undo`] can take
    /// back in the order they were made, followed by the steps [`Self::redo`] can bring
    /// back in the order they would come back. Entry `i` has position `i + 1`; the entries
    /// with `position <= len_undo()` are done, the rest are undone.
    ///
    /// A run of merged nudges or one drag ([`Self::apply_coalescing`]) is one entry.
    #[must_use]
    pub fn entries(&self) -> Vec<HistoryEntry> {
        let done = self.undo.iter().map(|step| (step, false));
        let undone = self.redo.iter().rev().map(|step| (step, true));
        done.chain(undone)
            .enumerate()
            .map(|(index, (step, undone))| HistoryEntry {
                label: step.label.clone(),
                position: index + 1,
                revision: step.revision,
                undone,
            })
            .collect()
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
        self.undo.last().map(|step| &step.edit)
    }

    /// The [`Edit`] [`History::redo`] would replay next -- see
    /// [`Self::peek_undo`]'s doc comment; the same reasoning applies
    /// symmetrically. `None` iff [`Self::can_redo`] is `false`.
    #[must_use]
    pub fn peek_redo(&self) -> Option<&Edit> {
        self.redo.last().map(|step| &step.edit)
    }

    /// The words [`Self::apply_labeled`] gave the step [`Self::undo`] would take back next,
    /// or `None` when there is no such step or it was worded by [`Edit::describe`] (then the
    /// caller words the hint from [`Self::peek_undo`], as before).
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.undo
            .last()
            .filter(|step| step.named)
            .map(|step| step.label.as_str())
    }

    /// [`Self::undo_label`] for the step [`Self::redo`] would bring back next.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo
            .last()
            .filter(|step| step.named)
            .map(|step| step.label.as_str())
    }

    /// Explicitly ends any [`Self::apply_coalescing`] run in progress, without
    /// making a new edit -- lets a caller with a real
    /// interaction boundary of its own (pointer release or focus loss on the
    /// control driving the coalesced edits) name that boundary directly instead
    /// of only ever discovering a run ended once [`Self::COALESCE_WINDOW`]
    /// (or a custom [`Self::with_coalesce_window`] value) had silently elapsed.
    /// A no-op, not an error, when no run is in progress.
    ///
    /// # Call sites
    ///
    /// `gui::editor::callbacks::tier_actions` (via `gui::editor::state::
    /// EditorState`'s own wrapper) calls this from two real, reachable
    /// interaction boundaries: `setup_inline_set_angle_callback`'s
    /// committed-but-unchanged branch (the cutter opened the angle cell, looked,
    /// and closed it without changing anything), and
    /// `apply_selected_tier_change` (the tier list's selection actually changed
    /// -- a different row or a viewport click elsewhere -- so a nudge run on the
    /// tier just navigated away from must not sit open for a later, unrelated
    /// nudge on that same tier to merge into).
    ///
    /// The one boundary this does NOT cover -- ending a run on the nudge
    /// control's OWN pointer-release/focus-loss, the way a slider drag would --
    /// needs a real pointer/focus event from `TierAngleCell`
    /// (`editor_tier_table.slint`), which has no such event to forward today;
    /// adding one is a `.slint` change outside every wave this doc comment has
    /// been written under. A genuine slow, evenly-paced scroll-wheel session
    /// (each tick further apart than the coalescing window) will keep producing
    /// one undo step per tick until that lands -- the STATUS this fix responds
    /// to calls this the part "not achievable exactly as specified" and asks for
    /// the boundary above instead.
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
