//! Session history navigation, preserving the shared engine's redo branch and selection.

use crate::{app_state::AppState, design_state::DirtyScope};

/// Committed navigation is replay, not a replacement edit. Even partial replay
/// failure invalidates dependent results and reports the tiers actually changed.
///
/// # Errors
///
/// The replay's message when an edit on the way to `position` is refused; the edits
/// replayed before it stay applied.
pub fn jump_to(state: &mut AppState, position: usize) -> Result<bool, String> {
    let before = state.session.current_generation();
    let (outcome, map) = state.session.jump_to_mapped(position);
    let change = match &outcome {
        Ok(value) => value.change,
        Err(error) => error.change,
    };
    // The catalogue follows the history even when the replay stopped part-way.
    let recipes = crate::recipe_steps::sync(state);
    if let Some(change) = change {
        state.selected_tier = state
            .selected_tier
            .and_then(|row| {
                map.map_table_row(
                    row,
                    change.tier_count_before,
                    change.tier_count_after,
                    change.concave_count_after,
                )
            })
            .filter(|&row| row < change.tier_count_after);
        state
            .solve
            .note_edit(before, state.session.current_generation(), DirtyScope::All);
        state.operations.invalidate();
    } else if recipes {
        state.operations.invalidate();
    }
    outcome
        .map(|value| value.change.is_some())
        .map_err(|error| error.to_string())
}
