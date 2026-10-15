//! Tier-list edits, selection, undo and redo.
//!
//! Every function here goes through an `indicatrix_editor::EditorSession` method (the
//! ones the desktop's and the web app's tier tables call), so the three front ends edit
//! identically. After a call the front end compares the session's generation with the one
//! before it: when it moved, it runs `design::after_edit` (auto-solve and re-push) --
//! also after an `Err`, because a batch edit may have changed the design before it failed.
//!
//! Each edit reports which tiers it made untrustworthy ([`DirtyScope`]) to
//! [`crate::design_state::SolveState::note_edit`], which is what keeps the rows' stale
//! masts exact.
//!
//! This file holds the shared plumbing, selection, angles, structure and history;
//! `add` has the "add a tier" commands and `indices` a tier's name and index positions.

mod add;
mod indices;

use std::{fmt, time::Duration};

use indicatrix_cut_core::{Edit, EditError};
use indicatrix_editor::{
    EditChange, EditorSession,
    session::{InlineAngle, RemoveTierError, TierIndexMap, selection_after_remove},
};

use super::solve::DirtyScope;
use crate::app_state::AppState;

pub use add::{QuickAdd, add_tier, quick_add};
pub use indices::{add_index, remove_index, rotate_indices, set_indices_from_text, set_name};

/// What a successful call did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edited {
    /// Whether the design changed (the front end then runs `design::after_edit`).
    pub changed: bool,
    /// A sentence worth a toast, if any ("Removed P3 (8 facets)", "No change.").
    pub notice: Option<String>,
}

impl Edited {
    const fn applied() -> Self {
        Self {
            changed: true,
            notice: None,
        }
    }

    fn applied_with(notice: impl Into<String>) -> Self {
        Self {
            changed: true,
            notice: Some(notice.into()),
        }
    }

    fn unchanged(notice: impl Into<String>) -> Self {
        Self {
            changed: false,
            notice: Some(notice.into()),
        }
    }

    fn with_extra(mut self, extra: Option<String>) -> Self {
        if let Some(extra) = extra {
            self.notice = Some(match self.notice {
                Some(notice) => format!("{notice} {extra}"),
                None => extra,
            });
        }
        self
    }
}

/// Why an edit did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditRefusal {
    /// It failed; the text is ready for a toast.
    Failed(String),
    /// Nothing changed, but the edit would go through if the user confirms: other tiers
    /// still meet the one to remove by name. The text is the question; repeat the call
    /// with `cascade` set to go ahead.
    NeedsConfirmation(String),
}

impl fmt::Display for EditRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(text) | Self::NeedsConfirmation(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for EditRefusal {}

/// The result of an edit function.
pub type EditOutcome = Result<Edited, EditRefusal>;

fn failed(text: impl Into<String>) -> EditRefusal {
    EditRefusal::Failed(text.into())
}

/// The text of an edit error, preferring the typed reason a tier relation gave.
fn refusal_text(session: &mut EditorSession, error: &EditError) -> String {
    session
        .take_refusal()
        .map_or_else(|| error.to_string(), |refusal| refusal.to_string())
}

/// Applies `edit`, with a refusal worded for the cutter.
fn apply_edit(session: &mut EditorSession, edit: Edit) -> Result<EditChange, String> {
    session
        .apply(edit)
        .map_err(|error| refusal_text(session, &error))
}

/// Runs `run`, then tells the solve state which tiers the edit made untrustworthy when the
/// session's generation moved.
fn tracked<R>(state: &mut AppState, run: impl FnOnce(&mut AppState) -> (R, DirtyScope)) -> R {
    let before = state.session.current_generation();
    let (result, scope) = run(state);
    let after = state.session.current_generation();
    if after != before {
        state.solve.note_edit(before, after, scope);
    }
    result
}

const fn tier_count(state: &AppState) -> usize {
    state.session.design.tiers.len()
}

// ---- selection ------------------------------------------------------------------------

/// Selects tier `tier` (`None` clears the selection); an out-of-range tier clears it too.
/// Unless `select_mode` is on, the multi-selection empties (the desktop's rule).
pub fn select_tier(state: &mut AppState, tier: Option<usize>, select_mode: bool) {
    let count = tier_count(state);
    state.selected_tier = tier.filter(|&index| index < count);
    if !select_mode {
        state.session.multi_selected.clear();
    }
}

/// Select mode: toggles tier `tier` in the multi-selection. Returns whether it is in the
/// group afterwards.
pub fn toggle_multi(state: &mut AppState, tier: usize) -> bool {
    if tier >= tier_count(state) {
        return false;
    }
    if state.session.multi_selected.remove(&tier) {
        false
    } else {
        state.session.multi_selected.insert(tier);
        true
    }
}

/// Empties the multi-selection.
pub fn clear_multi(state: &mut AppState) {
    state.session.multi_selected.clear();
}

/// Puts every flat tier in the multi-selection.
pub fn select_all(state: &mut AppState) {
    state.session.multi_selected = (0..tier_count(state)).collect();
}

// ---- angle ----------------------------------------------------------------------------

/// Commits typed angle text for `tier` (arithmetic and tier names allowed, a leading `=`
/// makes a relation). A bit-identical value spends no undo step.
///
/// # Errors
/// The parse failure, a refusal because the tier's angle follows a relation, or the apply
/// error, as text.
pub fn set_angle_from_text(state: &mut AppState, tier: usize, text: &str) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.set_tier_angle_from_text(tier, text) {
            Ok(InlineAngle::Applied(_)) => Ok(Edited::applied()),
            Ok(InlineAngle::NoChange) => Ok(Edited::unchanged("No change.")),
            Ok(InlineAngle::Missing) => Ok(Edited::default()),
            Err(message) => Err(failed(message)),
        };
        (outcome, DirtyScope::one(tier))
    })
}

/// Sets tier `tier`'s angle to the magnitude `magnitude_deg`.
///
/// The table shows no sign; the tier keeps its side of zero. Consecutive calls of one slider drag within the history's
/// coalescing window are ONE undo step.
///
/// # Errors
/// The apply error, as text (for example the tier's angle follows a relation).
pub fn set_angle_value(
    state: &mut AppState,
    tier: usize,
    magnitude_deg: f64,
    now: Duration,
) -> EditOutcome {
    tracked(state, |state| {
        let Some(current) = state.session.design.tiers.get(tier) else {
            return (Ok(Edited::default()), DirtyScope::one(tier));
        };
        let magnitude = magnitude_deg.abs();
        let signed = if current.angle_deg.is_sign_negative() {
            -magnitude
        } else {
            magnitude
        };
        let outcome = match state.session.set_tier_angle(tier, signed, now) {
            Ok(Some(_)) => Ok(Edited::applied()),
            Ok(None) => Ok(Edited::default()),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::one(tier))
    })
}

/// Nudges tier `tier`'s shown angle by `delta_deg`.
///
/// Positive makes the number bigger on a crown and a pavilion tier alike. When `tier`
/// belongs to a multi-selection of two or more, the whole group moves. A nudge that would cross 0 degrees stops there and says
/// so. Nudges within the history's coalescing window are ONE undo step.
///
/// # Errors
/// The apply error, as text.
pub fn nudge_angle(
    state: &mut AppState,
    tier: usize,
    delta_deg: f64,
    now: Duration,
) -> EditOutcome {
    let multi = &state.session.multi_selected;
    let targets: Vec<usize> = if multi.len() > 1 && multi.contains(&tier) {
        multi.iter().copied().collect()
    } else {
        vec![tier]
    };
    nudge_targets(state, &targets, delta_deg, now)
}

/// The select-mode batch bar's Offset: nudges every multi-selected tier by `delta_deg`.
///
/// # Errors
/// The apply error, as text.
pub fn offset_multi(state: &mut AppState, delta_deg: f64, now: Duration) -> EditOutcome {
    let targets: Vec<usize> = state.session.multi_selected.iter().copied().collect();
    nudge_targets(state, &targets, delta_deg, now)
}

fn nudge_targets(
    state: &mut AppState,
    targets: &[usize],
    delta_deg: f64,
    now: Duration,
) -> EditOutcome {
    tracked(state, |state| {
        let scope = DirtyScope::Tiers(targets.iter().copied().collect());
        let outcome = match state
            .session
            .nudge_displayed_angles(targets, delta_deg, now)
        {
            Ok(None) => Ok(Edited::default()),
            Ok(Some(nudged)) if nudged.clamped_labels.is_empty() => Ok(Edited::applied()),
            Ok(Some(nudged)) => Ok(Edited::applied_with(format!(
                "{} stopped at 0\u{b0}. Type the angle with a minus sign to cross to the other block.",
                nudged.clamped_labels.join(", ")
            ))),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, scope)
    })
}

// ---- structural edits -----------------------------------------------------------------

/// Duplicates tier `tier` right after itself and selects the copy.
///
/// # Errors
/// The apply error, as text.
pub fn duplicate(state: &mut AppState, tier: usize) -> EditOutcome {
    tracked(state, |state| match state.session.duplicate_tier(tier) {
        Ok(Some(copy)) => {
            state.selected_tier = Some(copy.new_index);
            state.session.multi_selected.clear();
            (
                Ok(Edited::applied_with(format!(
                    "Duplicated {} as {}.",
                    copy.source_label, copy.duplicate_label
                ))),
                DirtyScope::one(copy.new_index),
            )
        }
        Ok(None) => (Ok(Edited::default()), DirtyScope::All),
        Err(error) => (
            Err(failed(refusal_text(&mut state.session, &error))),
            DirtyScope::All,
        ),
    })
}

/// The question for removing tiers that others still meet by name.
fn remove_anyway_question(error: &RemoveTierError) -> String {
    match error {
        RemoveTierError::HasDependants {
            subject, labels, ..
        } => format!(
            "Removing {subject}: {} still meet it by name. Remove anyway and clear those references?",
            labels.join(", ")
        ),
        RemoveTierError::Edit(error) => error.to_string(),
    }
}

/// Removes tier `tier`. Tiers that other tiers still meet by name are refused with
/// [`EditRefusal::NeedsConfirmation`] unless `cascade` is set, which clears those
/// references in the same undoable edit.
///
/// # Errors
/// The confirmation question, or the apply error as text.
pub fn remove(state: &mut AppState, tier: usize, cascade: bool) -> EditOutcome {
    tracked(state, |state| {
        match state.session.remove_tier_with(tier, cascade) {
            Ok(removed) => {
                state.selected_tier = selection_after_remove(state.selected_tier, tier);
                let plural = if removed.facet_count == 1 { "" } else { "s" };
                let relations = state.session.take_relation_notice().map(|n| n.to_string());
                (
                    Ok(Edited::applied_with(format!(
                        "Removed {} ({} facet{plural}).",
                        removed.name, removed.facet_count
                    ))
                    .with_extra(relations)),
                    DirtyScope::All,
                )
            }
            Err(error @ RemoveTierError::HasDependants { .. }) => (
                Err(EditRefusal::NeedsConfirmation(remove_anyway_question(
                    &error,
                ))),
                DirtyScope::All,
            ),
            Err(error) => (Err(failed(error.to_string())), DirtyScope::All),
        }
    })
}

/// Removes every multi-selected tier (select mode's batch Delete); see [`remove`] for
/// `cascade`.
///
/// # Errors
/// The confirmation question, or the last apply error as text.
pub fn remove_multi(state: &mut AppState, cascade: bool) -> EditOutcome {
    tracked(state, |state| {
        let result = state.session.remove_multi_selected_with(cascade);
        if !matches!(result, Err(RemoveTierError::HasDependants { .. })) {
            state.selected_tier = None;
        }
        let outcome = match result {
            Ok(0) => Ok(Edited::default()),
            Ok(count) => {
                let relations = state.session.take_relation_notice().map(|n| n.to_string());
                let plural = if count == 1 { "" } else { "s" };
                Ok(
                    Edited::applied_with(format!("Removed {count} tier{plural}."))
                        .with_extra(relations),
                )
            }
            Err(error @ RemoveTierError::HasDependants { .. }) => Err(
                EditRefusal::NeedsConfirmation(remove_anyway_question(&error)),
            ),
            Err(error) => Err(failed(error.to_string())),
        };
        (outcome, DirtyScope::All)
    })
}

/// Moves tier `tier` one place earlier (`direction < 0`) or later in the cutting order;
/// the selection follows it.
///
/// # Errors
/// The apply error, as text.
pub fn move_tier(state: &mut AppState, tier: usize, direction: i32) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.move_tier(tier, direction) {
            Ok(Some(moved)) => {
                state.selected_tier = Some(moved.target);
                Ok(Edited::applied())
            }
            Ok(None) => Ok(Edited::default()),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::All)
    })
}

/// Detaches all occurrences of tier `tier` from their orbit, or reattaches them when some
/// are detached.
///
/// # Errors
/// The apply error, as text.
pub fn toggle_detach(state: &mut AppState, tier: usize) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.toggle_detach(tier) {
            Ok(Some(toggled)) => {
                let verb = if toggled.detached {
                    "Detached"
                } else {
                    "Reattached"
                };
                Ok(Edited::applied_with(format!("{verb} {}.", toggled.label)))
            }
            Ok(None) => Ok(Edited::default()),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::one(tier))
    })
}

/// Expands every incomplete orbit of tier `tier` to its full symmetric membership.
///
/// # Errors
/// The last apply error, as text.
pub fn complete_orbit(state: &mut AppState, tier: usize) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.complete_orbit(tier) {
            Ok(true) => Ok(Edited::applied()),
            Ok(false) => Ok(Edited::unchanged(
                "Nothing to complete: this tier's orbit is already whole.",
            )),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::one(tier))
    })
}

/// Mirrors tier `tier`'s index positions to the other side of the symmetry axis.
///
/// # Errors
/// The apply error, as text.
pub fn mirror_indices(state: &mut AppState, tier: usize) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.mirror_indices(tier) {
            Ok(_) => Ok(Edited::applied()),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::one(tier))
    })
}

/// Switches tier `tier` to the meet its source file stated.
///
/// # Errors
/// The apply error, as text.
pub fn adopt(state: &mut AppState, tier: usize) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.adopt_imported_meet(tier) {
            Ok(true) => Ok(Edited::applied()),
            Ok(false) => Ok(Edited::unchanged(
                "This tier has no imported meet to adopt.",
            )),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::one(tier))
    })
}

fn adopted(count: usize, from_selection: bool) -> Edited {
    if count == 0 {
        return Edited::unchanged(
            "Nothing to adopt: this design has no imported meets (its tiers already state their own meets).",
        );
    }
    let plural = if count == 1 { "" } else { "s" };
    let from = if from_selection {
        " from the selection"
    } else {
        ""
    };
    Edited::applied_with(format!("Adopted {count} imported meet{plural}{from}."))
}

/// Adopts every imported meet in the design (one undo step).
///
/// # Errors
/// The apply error, as text.
pub fn adopt_all(state: &mut AppState) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.adopt_all_imported_meets() {
            Ok(count) => Ok(adopted(count, false)),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::All)
    })
}

/// Adopts the imported meets of the multi-selected tiers (one undo step).
///
/// # Errors
/// The apply error, as text.
pub fn adopt_selected(state: &mut AppState) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.adopt_selected_imported_meets() {
            Ok(count) => Ok(adopted(count, true)),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::All)
    })
}

// ---- undo and redo --------------------------------------------------------------------

/// Whether there is something to undo.
#[must_use]
pub const fn can_undo(state: &AppState) -> bool {
    state.session.history.can_undo()
}

/// Whether there is something to redo.
#[must_use]
pub const fn can_redo(state: &AppState) -> bool {
    state.session.history.can_redo()
}

/// Words for the step an undo would take back ("Add tier C3"), empty when none.
#[must_use]
pub fn undo_hint(state: &AppState) -> String {
    let history = &state.session.history;
    history.undo_label().map_or_else(
        || {
            history
                .peek_undo()
                .map_or_else(String::new, |edit| edit.describe(&state.session.design))
        },
        str::to_owned,
    )
}

/// Words for the step a redo would bring back, empty when none.
#[must_use]
pub fn redo_hint(state: &AppState) -> String {
    let history = &state.session.history;
    history.redo_label().map_or_else(
        || {
            history
                .peek_redo()
                .map_or_else(String::new, |edit| edit.describe(&state.session.design))
        },
        str::to_owned,
    )
}

/// Makes the single selection follow its tier through an undo or redo.
fn follow_selection(state: &mut AppState, change: &EditChange, renumbering: &TierIndexMap) {
    let flat_after = tier_count(state);
    state.selected_tier = state
        .selected_tier
        .and_then(|row| {
            renumbering.map_table_row(
                row,
                change.tier_count_before,
                change.tier_count_after,
                change.concave_count_after,
            )
        })
        .filter(|&row| row < flat_after);
}

/// Undoes the last step; the selection follows its tier.
///
/// # Errors
/// The failed replay of the recorded inverse, as text.
pub fn undo(state: &mut AppState) -> EditOutcome {
    let hint = undo_hint(state);
    tracked(state, |state| {
        let outcome = match state.session.undo_mapped() {
            Ok(Some((change, renumbering))) => {
                follow_selection(state, &change, &renumbering);
                Ok(Edited::applied_with(history_notice("Undid", &hint)))
            }
            Ok(None) => Ok(Edited::unchanged("Nothing to undo.")),
            Err(error) => Err(failed(error.to_string())),
        };
        (outcome, DirtyScope::All)
    })
}

/// Redoes the last undone step; the selection follows its tier.
///
/// # Errors
/// The failed replay of the recorded edit, as text.
pub fn redo(state: &mut AppState) -> EditOutcome {
    let hint = redo_hint(state);
    tracked(state, |state| {
        let outcome = match state.session.redo_mapped() {
            Ok(Some((change, renumbering))) => {
                follow_selection(state, &change, &renumbering);
                Ok(Edited::applied_with(history_notice("Redid", &hint)))
            }
            Ok(None) => Ok(Edited::unchanged("Nothing to redo.")),
            Err(error) => Err(failed(error.to_string())),
        };
        (outcome, DirtyScope::All)
    })
}

fn history_notice(verb: &str, hint: &str) -> String {
    if hint.is_empty() {
        format!("{verb}.")
    } else {
        format!("{verb}: {hint}")
    }
}
