//! A tier's name and index positions: rename, replace, add, remove and turn.

use std::time::Duration;

use indicatrix_cut_core::Edit;
use indicatrix_editor::{
    EditorSession,
    loading::{name_collides, non_integral_index_warning, parse_index_list},
    tier_save::{other_tier_names_excluding, tier_save_edit, unnaming_blocked_message},
};

use super::{DirtyScope, EditOutcome, Edited, apply_edit, failed, refusal_text, tracked};
use crate::app_state::AppState;

/// Renames tier `tier`.
///
/// Names must be unique (facet names resolve meet constraints), and a tier that others meet by name cannot be left unnamed; tiers that meet it by name
/// follow the rename in the same undo step.
///
/// # Errors
/// The collision or unnaming message, or the apply error.
pub fn set_name(state: &mut AppState, tier: usize, name: &str) -> EditOutcome {
    tracked(state, |state| {
        let outcome = rename(&mut state.session, tier, name.trim());
        (outcome, DirtyScope::one(tier))
    })
}

fn rename(session: &mut EditorSession, tier: usize, name: &str) -> EditOutcome {
    let Some(current) = session.design.tiers.get(tier) else {
        return Ok(Edited::default());
    };
    if current.name == name {
        return Ok(Edited::unchanged("No change."));
    }
    let index = i32::try_from(tier).unwrap_or(-1);
    let others = other_tier_names_excluding(&session.design, index);
    if name_collides(name, &others) {
        return Err(failed(
            "Another tier already has that name. Facet names must be unique so meet constraints resolve to the right tier.",
        ));
    }
    if let Some(message) = unnaming_blocked_message(&session.design, index, name) {
        return Err(failed(message));
    }
    let mut renamed = current.clone();
    name.clone_into(&mut renamed.name);
    let (_, edit) = tier_save_edit(&session.design, index, renamed, None);
    apply_edit(session, edit).map_err(failed)?;
    Ok(Edited::applied())
}

/// Replaces tier `tier`'s index positions with those `text` names (a list, `start:step:stop`
/// or `base xN`). A tier keeps at least one index.
///
/// # Errors
/// The parse error (naming the offending token) or the apply error.
pub fn set_indices_from_text(state: &mut AppState, tier: usize, text: &str) -> EditOutcome {
    tracked(state, |state| {
        let outcome = replace_indices(&mut state.session, tier, text);
        (outcome, DirtyScope::one(tier))
    })
}

fn replace_indices(session: &mut EditorSession, tier: usize, text: &str) -> EditOutcome {
    let Some(current) = session.design.tiers.get(tier) else {
        return Ok(Edited::default());
    };
    let indices = parse_index_list(text, session.design.meta.gear_teeth_abs()).map_err(failed)?;
    if indices.is_empty() {
        return Err(failed(
            "A tier needs at least one index. Remove the tier instead of its last index.",
        ));
    }
    if indices == current.indices {
        return Ok(Edited::unchanged("No change."));
    }
    let mut updated = current.clone();
    updated
        .detached
        .retain(|detached| indices.iter().any(|index| (index - detached).abs() < 1e-9));
    let warning = non_integral_index_warning(&indices);
    updated.indices = indices;
    apply_edit(
        session,
        Edit::ModifyTier {
            index: tier,
            tier: updated,
        },
    )
    .map_err(failed)?;
    Ok(Edited::applied().with_extra(warning))
}

/// The index list of tier `tier` as parseable text (full precision, space separated).
fn index_text(state: &AppState, tier: usize) -> Option<String> {
    let tier = state.session.design.tiers.get(tier)?;
    Some(
        tier.indices
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Adds the index position `value` to tier `tier`.
///
/// # Errors
/// The position is off the gear, repeats one, or the apply failed.
pub fn add_index(state: &mut AppState, tier: usize, value: f64) -> EditOutcome {
    let Some(existing) = index_text(state, tier) else {
        return Ok(Edited::default());
    };
    set_indices_from_text(state, tier, &format!("{existing} {value}"))
}

/// Removes the `position`-th index (in list order) from tier `tier`.
///
/// # Errors
/// The tier would be left without an index, or the apply failed.
pub fn remove_index(state: &mut AppState, tier: usize, position: usize) -> EditOutcome {
    let Some(current) = state.session.design.tiers.get(tier) else {
        return Ok(Edited::default());
    };
    if position >= current.indices.len() {
        return Ok(Edited::default());
    }
    let remaining = current
        .indices
        .iter()
        .enumerate()
        .filter(|&(at, _)| at != position)
        .map(|(_, index)| index.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    set_indices_from_text(state, tier, &remaining)
}

/// Turns tier `tier`'s index positions by `teeth` whole teeth (negative turns the other
/// way). Consecutive turns within the history's coalescing window are ONE undo step.
///
/// # Errors
/// The apply error, as text.
pub fn rotate_indices(state: &mut AppState, tier: usize, teeth: i64, now: Duration) -> EditOutcome {
    tracked(state, |state| {
        let outcome = match state.session.rotate_tier_indices(tier, teeth, now) {
            Ok(Some(_)) => Ok(Edited::applied()),
            Ok(None) => Ok(Edited::default()),
            Err(error) => Err(failed(refusal_text(&mut state.session, &error))),
        };
        (outcome, DirtyScope::one(tier))
    })
}
