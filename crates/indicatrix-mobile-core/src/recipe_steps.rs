//! Custom-material recipes that travel with undo.
//!
//! The session's custom-material catalogue (`PropsState::custom_materials` and the
//! snapshots beside it) is not part of a `Design`, so a history step cannot carry it. Most
//! edits never change a recipe, but opening a saved variant can: the variant was saved
//! with its own definition of a custom material that has the same name as one in the
//! catalogue but other optics. Replacing the catalogue entry and leaving Undo to bring back
//! only the design would make Undo restore a design whose material now means something
//! else.
//!
//! So the recipe change is recorded next to the history step that caused it, as a
//! [`RecipeStep`] keyed by that step's `(position, revision)` (a pair that names one
//! design state for as long as the history lives, see `HistoryEntry::revision`):
//!
//! - while the design stands at or after the step, the catalogue holds the variant's recipe;
//! - while it stands before the step (Undo, or a jump back), the catalogue holds the recipe
//!   it had before;
//! - a new edit made after going back drops the step together with the redo branch it
//!   belonged to (the entry at its position no longer has its revision).
//!
//! [`sync`] applies this and is called after every history move; [`record`] is called by
//! [`crate::variants::open`] inside the same operation as the edit.

use crate::app_state::AppState;
use indicatrix::optics::materials::GemMaterial;
use indicatrix_formats::native::CustomMaterialSnapshot;

/// One catalogue entry: the material and what a design file stores for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    /// The traced material.
    pub material: GemMaterial,
    /// The snapshot a native save of a design using it carries.
    pub snapshot: Option<CustomMaterialSnapshot>,
}

/// A recipe change owned by one history step.
#[derive(Debug, Clone, PartialEq)]
pub struct RecipeStep {
    document: u64,
    position: usize,
    revision: u64,
    name: String,
    /// The catalogue entry before the step; `None` when the name was not in the catalogue.
    before: Option<Recipe>,
    after: Recipe,
}

/// The catalogue entry called `name` now.
#[must_use]
pub fn current(state: &AppState, name: &str) -> Option<Recipe> {
    state.props.find_custom(name).map(|material| Recipe {
        material: material.clone(),
        snapshot: state.props.snapshot_for(name).cloned(),
    })
}

fn install(state: &mut AppState, name: &str, recipe: Option<&Recipe>) -> Result<(), String> {
    if let Some(recipe) = recipe {
        state
            .props
            .register_custom(recipe.material.clone(), recipe.snapshot.clone())?;
        // The opened document's own copy follows, as `document::install` registers
        // a restored material in both places.
        state
            .document
            .register_custom_material(recipe.material.clone())?;
    } else {
        state
            .props
            .custom_materials
            .retain(|gem| !gem.name.eq_ignore_ascii_case(name));
        state
            .props
            .custom_snapshots
            .retain(|(known, _)| !known.eq_ignore_ascii_case(name));
        state
            .document
            .custom_materials
            .retain(|gem| !gem.name.eq_ignore_ascii_case(name));
    }
    state.props.forget_checks();
    Ok(())
}

/// Puts `after` in the catalogue for `name` and records the change against the history
/// step the session is at (call it right after the step was applied).
///
/// # Errors
///
/// The refusal sentence when the catalogue does not accept the material (a built-in's
/// name); nothing is recorded then.
pub fn record(state: &mut AppState, name: &str, after: Recipe) -> Result<(), String> {
    let before = current(state, name);
    install(state, name, Some(&after))?;
    let position = state.session.history_position();
    let Some(entry) = position
        .checked_sub(1)
        .and_then(|index| state.session.history_entries().into_iter().nth(index))
    else {
        // No step to hang the change on: undo cannot reach it, so the catalogue keeps it.
        return Ok(());
    };
    let document = state.operations.document_id();
    state.props.recipe_steps.retain(|step| {
        !(step.document == document && step.position == position && step.name == name)
    });
    state.props.recipe_steps.push(RecipeStep {
        document,
        position,
        revision: entry.revision,
        name: name.to_owned(),
        before,
        after,
    });
    Ok(())
}

/// Brings the catalogue in line with where the history stands. Call after every undo,
/// redo and jump. Returns whether a recipe changed (the caller then invalidates what
/// depends on the material).
pub fn sync(state: &mut AppState) -> bool {
    if state.props.recipe_steps.is_empty() {
        return false;
    }
    let document = state.operations.document_id();
    let position = state.session.history_position();
    let entries = state.session.history_entries();
    // Steps of another document, or whose entry is gone (the branch was dropped by a new
    // edit), are forgotten: the catalogue already holds what stands at the present.
    let mut steps = std::mem::take(&mut state.props.recipe_steps);
    steps.retain(|step| {
        step.document == document
            && step
                .position
                .checked_sub(1)
                .and_then(|index| entries.get(index))
                .is_some_and(|entry| entry.revision == step.revision)
    });
    let mut changed = false;
    // One verdict per name. Steps of a name stand in the order of their positions: the
    // newest one at or before the design's position decides ("after"); with none of them
    // reached, the oldest step's "before" is what the catalogue had.
    let mut names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let mut of_name: Vec<&RecipeStep> = steps.iter().filter(|step| step.name == name).collect();
        of_name.sort_by_key(|step| step.position);
        let reached = of_name.iter().rev().find(|step| step.position <= position);
        let wanted = reached.map_or_else(
            || of_name.first().and_then(|step| step.before.as_ref()),
            |step| Some(&step.after),
        );
        if current(state, name).as_ref() != wanted {
            if let Err(message) = install(state, name, wanted) {
                tracing::warn!(%message, "a custom material recipe could not follow Undo");
            } else {
                changed = true;
            }
        }
    }
    state.props.recipe_steps = steps;
    changed
}
