//! Per-facet and tier-wide index editing from the Tier tab, and the tier's cheater
//! offset and note (the desktop's `tier_actions::facet_editing` and
//! `tier_actions::design_forms`): every action is one undoable edit through
//! `EditorSession`, on the tier the form has loaded.

use super::finish;
use crate::{
    AppWindow, InspectorModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    editor::edit::Dirty,
};
use indicatrix_cut_core::{Edit, EditError};
use indicatrix_editor::EditorSession;
use slint::{ComponentHandle, SharedString};
use std::collections::BTreeSet;

/// What one facet-level action does to the tier.
enum Action {
    /// Detach or reattach one occurrence.
    ToggleDetach(f32),
    /// Remove one occurrence (a complete orbit unit goes with it).
    Remove(f32),
    /// Add one occurrence, expanded to its orbit.
    Add(f64),
    /// Rotate every position by this many teeth.
    Rotate(f64),
    /// Mirror every position across the symmetry axis.
    Mirror,
    /// Set (or clear) the cheater offset in degrees.
    Cheater(Option<f64>),
    /// Set (or clear) the tier's note.
    Note(Option<String>),
}

/// The exact `f64` position of the chip whose `f32` is `position` (the chips carry an
/// `f32`, which is not the tier's own `f64` for a fractional position like 13.71).
fn exact_position(session: &EditorSession, tier: usize, position: f32) -> f64 {
    session
        .design
        .tiers
        .get(tier)
        .and_then(|t| {
            t.indices
                .iter()
                .chain(&t.detached)
                .copied()
                .find(|&p| (p as f32).to_bits() == position.to_bits())
        })
        .unwrap_or_else(|| f64::from(position))
}

/// Applies `action` to tier `tier`.
fn apply(session: &mut EditorSession, tier: usize, action: Action) -> Result<(), EditError> {
    match action {
        Action::ToggleDetach(position) => {
            let position = exact_position(session, tier, position);
            let detached = session
                .design
                .tiers
                .get(tier)
                .is_some_and(|t| t.detached.contains(&position));
            let edit = if detached {
                session.design.reattach_orbit_member(tier, position)?
            } else {
                session.design.detach_orbit_member(tier, position)?
            };
            session.apply(edit).map(|_| ())
        }
        Action::Remove(position) => {
            let position = exact_position(session, tier, position);
            let edit = session.design.remove_orbit_member(tier, position)?;
            session.apply(edit).map(|_| ())
        }
        Action::Add(position) => {
            let edit = session.design.add_orbit_member(tier, position)?;
            session.apply(edit).map(|_| ())
        }
        Action::Rotate(teeth) => {
            let edit = session.design.rotate_indices(tier, teeth)?;
            session.apply(edit).map(|_| ())
        }
        Action::Mirror => session.mirror_indices(tier).map(|_| ()),
        Action::Cheater(offset_deg) => session
            .apply(Edit::SetCheaterOffset {
                index: tier,
                offset_deg,
            })
            .map(|_| ()),
        Action::Note(note) => session
            .apply(Edit::SetTierNote { index: tier, note })
            .map(|_| ()),
    }
}

/// Runs `action` on the loaded tier and ends the edit, or says why it failed.
fn run(ctx: &Ctx, action: Action) -> bool {
    let Some(ui) = ctx.ui.upgrade() else {
        return false;
    };
    let Ok(tier) = usize::try_from(ui.global::<InspectorModel>().get_loaded_tier_index()) else {
        return false;
    };
    // A cheater offset or a note is a cutting-sheet annotation: no mast moves.
    let dirty = match action {
        Action::Cheater(_) | Action::Note(_) => Dirty::Tiers(BTreeSet::new()),
        _ => Dirty::one(tier),
    };
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return false;
        };
        apply(&mut design_state.session, tier, action)
    };
    match result {
        Ok(()) => {
            finish(ctx, dirty);
            true
        }
        Err(e) => {
            show_message(ctx, MessageKind::Error, &e.to_string());
            false
        }
    }
}

fn not_a_number(ctx: &Ctx, what: &str, text: &str) {
    show_message(
        ctx,
        MessageKind::Error,
        &format!("{what} '{}' is not a number.", text.trim()),
    );
}

fn facet_add(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let text = model.get_new_facet_position();
    if text.trim().is_empty() {
        return;
    }
    match text.trim().parse::<f64>() {
        Ok(position) if position.is_finite() => {
            if run(ctx, Action::Add(position)) {
                model.set_new_facet_position(SharedString::new());
            }
        }
        _ => not_a_number(ctx, "Position", &text),
    }
}

fn rotate(ctx: &Ctx, direction: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let text = ui.global::<InspectorModel>().get_rotate_by_text();
    match text.trim().parse::<f64>() {
        Ok(teeth) if teeth.is_finite() => {
            run(ctx, Action::Rotate(teeth * f64::from(direction)));
        }
        _ => not_a_number(ctx, "Rotation", &text),
    }
}

fn apply_cheater(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let text = ui.global::<InspectorModel>().get_cheater_text();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        run(ctx, Action::Cheater(None));
        return;
    }
    match trimmed.parse::<f64>() {
        Ok(deg) if deg.is_finite() => {
            run(ctx, Action::Cheater(Some(deg)));
        }
        _ => not_a_number(ctx, "Cheater offset", &text),
    }
}

fn apply_note(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let text = ui.global::<InspectorModel>().get_note_text();
    let trimmed = text.trim();
    run(
        ctx,
        Action::Note((!trimmed.is_empty()).then(|| trimmed.to_string())),
    );
}

/// Registers the facet-level callbacks.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<InspectorModel>();
    let c = ctx.clone();
    model.on_facet_toggle_detach(move |position| {
        run(&c, Action::ToggleDetach(position));
    });
    let c = ctx.clone();
    model.on_facet_remove(move |position| {
        run(&c, Action::Remove(position));
    });
    let c = ctx.clone();
    model.on_facet_add(move || facet_add(&c));
    let c = ctx.clone();
    model.on_rotate(move |direction| rotate(&c, direction));
    let c = ctx.clone();
    model.on_mirror(move || {
        run(&c, Action::Mirror);
    });
    let c = ctx.clone();
    model.on_apply_cheater(move || apply_cheater(&c));
    let c = ctx.clone();
    model.on_apply_note(move || apply_note(&c));
}
