//! The tier table's editing actions -- delete, duplicate, move, detach, adopt, pin, the
//! multi-select batch actions, the step generator, mirror to the other block, Solve and
//! the auto-solve toggle -- and [`finish_edit`], the one path every edit ends in.
//!
//! Every action goes through an `indicatrix_editor::EditorSession` method (the same ones
//! the desktop's tier-list callbacks call), inside a short `borrow_mut()`; the borrow is
//! over before anything is pushed. Quick-add lives in [`super::edit_add`].

use super::{table, unsaved::confirm_action};
use crate::{
    TierTableModel,
    app::{
        Ctx,
        persist::schedule_save,
        push::{MessageKind, push_design, show_message},
        solve::{auto_solve, forget_failures, with_solved},
        state::WebApp,
    },
    views,
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::Edit;
use indicatrix_editor::session::{RemoveTierError, selection_after_remove};
use std::collections::BTreeSet;

/// Which tiers an edit made untrustworthy, for the rows' solved masts
/// (`indicatrix_editor::view_model::rows::tier_items_stale_with_last_solved`): the tiers
/// it touched, or every tier when its blast radius is not tracked (undo, redo, move,
/// remove -- the desktop's rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dirty {
    /// Every tier.
    All,
    /// Just these.
    Tiers(BTreeSet<usize>),
}

impl Dirty {
    /// Just tier `index`.
    #[must_use]
    pub fn one(index: usize) -> Self {
        Self::Tiers(BTreeSet::from([index]))
    }
}

/// What every edit ends with: remembers which tiers went stale, refreshes the design
/// summary (name, dirty marker, undo/redo), schedules the auto-solve and the
/// `sessionStorage` save, redraws the Solid/Diagram views and brings the tier table up to
/// date. Call with no `RefCell` borrow held.
pub fn finish_edit(ctx: &Ctx, dirty: Dirty) {
    table::mark_stale(dirty);
    if let Some(ui) = ctx.ui.upgrade() {
        push_design(&ui, &ctx.state.borrow());
    }
    auto_solve(ctx);
    schedule_save(ctx);
    views::request_refresh(ctx);
    table::sync(ctx);
    super::guide::check_progress(ctx);
}

/// Runs `f` on the app inside a short `borrow_mut()`.
pub fn with_app<R>(ctx: &Ctx, f: impl FnOnce(&mut WebApp) -> R) -> R {
    f(&mut ctx.state.borrow_mut())
}

/// Selects `index` as the plain selection (the multi-selection empties), without asking
/// the views to redraw -- [`finish_edit`] does that.
pub fn set_selection(app: &mut WebApp, index: Option<usize>) {
    app.selected_tier = index;
    if let Some(design) = app.design.as_mut() {
        design.session.multi_selected.clear();
    }
}

/// Delete / the row's x: removes tier `index`.
///
/// A tier other tiers still meet by name is refused until the user confirms "Remove
/// anyway", which clears those references as part of the same undoable edit.
pub fn remove_tier(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    remove_tier_at(ctx, index, false);
}

/// The message of the "Remove anyway?" confirmation for a refused removal.
fn remove_anyway_message(error: &RemoveTierError) -> String {
    match error {
        RemoveTierError::HasDependants {
            subject, labels, ..
        } => format!(
            "Removing {subject}: {} still meet it by name. Remove anyway and clear those \
             references?",
            labels.join(", ")
        ),
        RemoveTierError::Edit(e) => e.to_string(),
    }
}

/// [`remove_tier`] for a valid index; `cascade` clears other tiers' references to it.
fn remove_tier_at(ctx: &Ctx, index: usize, cascade: bool) {
    let result = with_app(ctx, |app| {
        let outcome = app
            .design
            .as_mut()?
            .session
            .remove_tier_with(index, cascade);
        if outcome.is_ok() {
            app.selected_tier = selection_after_remove(app.selected_tier, index);
        }
        Some(outcome)
    });
    match result {
        None => {}
        Some(Ok(removed)) => {
            finish_edit(ctx, Dirty::All);
            let plural = if removed.facet_count == 1 { "" } else { "s" };
            show_message(
                ctx,
                MessageKind::Info,
                &format!(
                    "Removed {} ({} facet{plural}), Undo",
                    removed.name, removed.facet_count
                ),
            );
        }
        Some(Err(e @ RemoveTierError::HasDependants { .. })) => {
            let c = ctx.clone();
            confirm_action(
                ctx,
                "Remove tier?",
                &remove_anyway_message(&e),
                "Remove anyway",
                move || remove_tier_at(&c, index, true),
            );
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// Duplicate / Ctrl+D: a copy right after tier `index`, selected.
pub fn duplicate_tier(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let result = with_app(ctx, |app| {
        let outcome = app.design.as_mut()?.session.duplicate_tier(index);
        if let Ok(Some(duplicated)) = &outcome {
            set_selection(app, Some(duplicated.new_index));
        }
        Some(outcome)
    });
    match result {
        None | Some(Ok(None)) => {}
        Some(Ok(Some(duplicated))) => {
            finish_edit(ctx, Dirty::one(duplicated.new_index));
            show_message(
                ctx,
                MessageKind::Info,
                &format!(
                    "Duplicated {} as {}",
                    duplicated.source_label, duplicated.duplicate_label
                ),
            );
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// Alt+Up / Alt+Down and the row's arrows: moves tier `index` up (`direction < 0`) or
/// down in the cutting order; the selection follows it.
pub fn move_tier(ctx: &Ctx, index: i32, direction: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let result = with_app(ctx, |app| {
        let outcome = app.design.as_mut()?.session.move_tier(index, direction);
        if let Ok(Some(moved)) = &outcome {
            set_selection(app, Some(moved.target));
        }
        Some(outcome)
    });
    match result {
        None | Some(Ok(None)) => {}
        // A move renumbers every tier between the two positions.
        Some(Ok(Some(_))) => finish_edit(ctx, Dirty::All),
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// The row's Detach / Reattach.
fn toggle_detach(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let result = with_app(ctx, |app| {
        Some(app.design.as_mut()?.session.toggle_detach(index))
    });
    match result {
        None | Some(Ok(None)) => {}
        Some(Ok(Some(toggled))) => {
            finish_edit(ctx, Dirty::one(index));
            let verb = if toggled.detached {
                "Detached"
            } else {
                "Reattached"
            };
            show_message(ctx, MessageKind::Info, &format!("{verb} {}", toggled.label));
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// The row's Adopt: switches tier `index` to the meet its source file stated.
fn adopt(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let result = with_app(ctx, |app| {
        Some(app.design.as_mut()?.session.adopt_imported_meet(index))
    });
    match result {
        None | Some(Ok(false)) => {}
        Some(Ok(true)) => finish_edit(ctx, Dirty::one(index)),
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// "Adopt all" (`selected == false`) or "Adopt sel.".
fn adopt_many(ctx: &Ctx, selected: bool) {
    let result = with_app(ctx, |app| {
        let session = &mut app.design.as_mut()?.session;
        Some(if selected {
            session.adopt_selected_imported_meets()
        } else {
            session.adopt_all_imported_meets()
        })
    });
    match result {
        None => {}
        // A template-built or hand-authored design has no imported meets; the Optimize tab
        // sends its reader here ("adopt it"), so say why nothing happened.
        Some(Ok(0)) => show_message(
            ctx,
            MessageKind::Info,
            "Nothing to adopt: this design has no imported meets (its tiers already state \
             their own meets).",
        ),
        Some(Ok(count)) => {
            // The values adopted are what the last solve produced, but which rows they
            // move is not tracked: every mast is re-solved.
            finish_edit(ctx, Dirty::All);
            let plural = if count == 1 { "" } else { "s" };
            let from = if selected { " from the selection" } else { "" };
            show_message(
                ctx,
                MessageKind::Info,
                &format!("Adopted {count} imported meet{plural}{from}"),
            );
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// The row's Pin: freezes tier `index`'s solved mast as an exact scale reference. Only
/// possible against a solve of the current design (a stale mast would pin the wrong
/// number).
fn pin(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let mast = ctx
        .state
        .borrow()
        .current_solved()
        .and_then(|tiers| tiers.get(index))
        .map(|tier| tier.mast);
    let Some(mast) = mast else {
        show_message(
            ctx,
            MessageKind::Warning,
            "Solve the design first: a stale mast cannot be pinned.",
        );
        return;
    };
    let result = with_app(ctx, |app| {
        Some(app.design.as_mut()?.session.apply(Edit::SetConstraint {
            index,
            constraint: MeetConstraint::ScaleReference(mast),
        }))
    });
    match result {
        None => {}
        Some(Ok(_)) => {
            finish_edit(ctx, Dirty::one(index));
            show_message(
                ctx,
                MessageKind::Info,
                &format!("Pinned tier {} at {mast:.4}", index + 1),
            );
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// The block letter's click: flips a facet at exactly 0 degrees between Crown ("0") and
/// Pavilion ("-0") by committing that text to its angle.
fn flip_block(ctx: &Ctx, index: i32, to_crown: bool) {
    super::nudge::commit_angle(ctx, index, if to_crown { "0" } else { "-0" });
}

/// The missing-anchor row's "Add Anchor": selects the tier and opens the inspector's Tier
/// tab on it with "Exact scale value" chosen and the cursor in the value field (the
/// desktop's `suggested_constraint_kind` route).
fn add_anchor(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    super::inspector::open_anchor(ctx, index);
    table::sync(ctx);
}

/// The multi-select "Delete": removes every multi-selected tier.
///
/// Tiers that other (unselected) tiers still meet by name are refused until the user
/// confirms "Remove anyway".
fn remove_multi(ctx: &Ctx) {
    remove_multi_with(ctx, false);
}

/// [`remove_multi`]; `cascade` clears other tiers' references to the removed ones.
fn remove_multi_with(ctx: &Ctx, cascade: bool) {
    let result = with_app(ctx, |app| {
        let outcome = app
            .design
            .as_mut()?
            .session
            .remove_multi_selected_with(cascade);
        // A refusal changed nothing, so the selection stays.
        if !matches!(outcome, Err(RemoveTierError::HasDependants { .. })) {
            app.selected_tier = None;
        }
        Some(outcome)
    });
    match result {
        None | Some(Ok(0)) => {}
        Some(Ok(count)) => {
            finish_edit(ctx, Dirty::All);
            show_message(ctx, MessageKind::Info, &format!("Removed {count} tier(s)."));
        }
        Some(Err(e @ RemoveTierError::HasDependants { .. })) => {
            let c = ctx.clone();
            confirm_action(
                ctx,
                "Remove tiers?",
                &remove_anyway_message(&e),
                "Remove anyway",
                move || remove_multi_with(&c, true),
            );
        }
        // Some removals may have gone through before the failing one.
        Some(Err(e)) => {
            finish_edit(ctx, Dirty::All);
            show_message(ctx, MessageKind::Error, &e.to_string());
        }
    }
}

/// The command bar's Delete: the multi-selection when there is one, else the selected tier.
fn delete_selected(ctx: &Ctx) {
    let (multi, selected) = {
        let app = ctx.state.borrow();
        (
            app.design
                .as_ref()
                .is_some_and(|d| !d.session.multi_selected.is_empty()),
            app.selected_tier,
        )
    };
    if multi {
        remove_multi(ctx);
    } else if let Some(index) = selected.and_then(|i| i32::try_from(i).ok()) {
        remove_tier(ctx, index);
    }
}

/// "Generate steps": a ladder of tiers appended after the last one.
fn generate_steps(
    ctx: &Ctx,
    name_prefix: &str,
    start: &str,
    step: &str,
    count: i32,
    indices: &str,
    anchor: &str,
) {
    let result = with_app(ctx, |app| {
        Some(app.design.as_mut()?.session.generate_step_series(
            name_prefix,
            start,
            step,
            count,
            indices,
            anchor,
        ))
    });
    match result {
        None => {}
        Some(Ok(series)) => {
            finish_edit(
                ctx,
                Dirty::Tiers((series.start_index..series.start_index + series.added).collect()),
            );
            show_message(
                ctx,
                MessageKind::Info,
                &format!("Generated {} tier(s).", series.added),
            );
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e),
    }
}

/// "Mirror tier to other block": a mirrored copy appended after the last tier, selected.
fn mirror_to_other_block(ctx: &Ctx, index: i32, suffix: &str) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let result = with_app(ctx, |app| {
        let outcome = app
            .design
            .as_mut()?
            .session
            .mirror_tier_to_other_block(index, suffix);
        if let Ok(Some(mirrored)) = &outcome {
            set_selection(app, Some(mirrored.new_index));
        }
        Some(outcome)
    });
    match result {
        None | Some(Ok(None)) => {}
        Some(Ok(Some(mirrored))) => {
            finish_edit(ctx, Dirty::one(mirrored.new_index));
            show_message(
                ctx,
                MessageKind::Info,
                &format!("Mirrored to {}", mirrored.label),
            );
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// The command bar's Solve (and the stale badge's Recompute).
fn solve(ctx: &Ctx) {
    if with_app(ctx, |app| app.current_solved().is_some()) {
        show_message(ctx, MessageKind::Info, "The design is already solved.");
        return;
    }
    // Pressing Solve asks again even after the automatic attempts gave up.
    forget_failures();
    with_solved(ctx, |ctx, result| {
        if let Err(message) = result {
            show_message(ctx, MessageKind::Error, &format!("Cannot solve: {message}"));
        }
    });
    table::sync(ctx);
}

/// The auto-solve budget combo (Off, 150 ms, 300 ms, 1 s, 3 s): switching auto-solve on,
/// or raising the budget, solves a stale design at once when the new budget allows it.
fn set_auto_solve_budget(ctx: &Ctx, budget_ms: i32) {
    let budget_ms = u32::try_from(budget_ms).unwrap_or(0);
    let changed = with_app(ctx, |app| {
        let changed = app.auto_solve_budget_ms != budget_ms;
        app.auto_solve_budget_ms = budget_ms;
        changed
    });
    if !changed {
        return;
    }
    if budget_ms > 0 {
        auto_solve(ctx);
    }
    schedule_save(ctx);
    table::sync(ctx);
}

/// Wires the editing callbacks of `TierTableModel`.
pub fn wire(model: &TierTableModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_remove_tier(move |index| remove_tier(&c, index));
    let c = ctx.clone();
    model.on_duplicate_tier(move |index| duplicate_tier(&c, index));
    let c = ctx.clone();
    model.on_move_tier(move |index, direction| move_tier(&c, index, direction));
    let c = ctx.clone();
    model.on_toggle_detach(move |index| toggle_detach(&c, index));
    let c = ctx.clone();
    model.on_adopt(move |index| adopt(&c, index));
    let c = ctx.clone();
    model.on_pin(move |index| pin(&c, index));
    let c = ctx.clone();
    model.on_flip_block(move |index, to_crown| flip_block(&c, index, to_crown));
    let c = ctx.clone();
    model.on_add_anchor(move |index| add_anchor(&c, index));
    let c = ctx.clone();
    model.on_delete_selected(move || delete_selected(&c));
    let c = ctx.clone();
    model.on_remove_multi(move || remove_multi(&c));
    let c = ctx.clone();
    model.on_adopt_all(move || adopt_many(&c, false));
    let c = ctx.clone();
    model.on_adopt_selected(move || adopt_many(&c, true));
    let c = ctx.clone();
    model.on_generate_steps(move |name, start, step, count, indices, anchor| {
        generate_steps(&c, &name, &start, &step, count, &indices, &anchor);
    });
    let c = ctx.clone();
    model.on_mirror_to_other_block(move |index, suffix| mirror_to_other_block(&c, index, &suffix));
    let c = ctx.clone();
    model.on_solve(move || solve(&c));
    let c = ctx.clone();
    model.on_set_auto_solve_budget_ms(move |budget_ms| set_auto_solve_budget(&c, budget_ms));
    super::edit_add::wire(model, ctx);
}
