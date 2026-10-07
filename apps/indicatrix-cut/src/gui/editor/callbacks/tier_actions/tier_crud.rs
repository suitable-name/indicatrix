//! Tier-list structural edits: remove, duplicate, detach/reattach (and the
//! `setup_toggle_detach_callback` wiring hub it doubles as), move, complete-orbit,
//! and multi-select clear/remove.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_editor::session::RemoveTierError;
use slint::{ComponentHandle, Model};

use super::{
    adopt::{setup_adopt_all_callback, setup_adopt_selected_callback, setup_pin_to_mast_callback},
    concave_tier::{
        Services, concave_at, duplicate_concave_now, move_concave_now, remove_concave_now,
    },
    facet_editing::{
        setup_facet_add_callback, setup_facet_remove_callback, setup_facet_toggle_detach_callback,
        setup_highlight_tooth_callback, setup_tier_mirror_indices_callback,
        setup_tier_rotate_indices_callback,
    },
    facet_overlay::resubmit_facet_overlay,
    misc::{adjust_selection_after_remove, bump_form_reset_pulse},
    tier_generation::{
        setup_generate_step_series_callback, setup_mirror_tier_to_other_block_callback,
    },
};
use crate::{
    EditorModel, EditorTierItem, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            native_io::ask_write_confirm,
            relation_ui::{setup_relation_callbacks, take_cleared_relations_sentence},
            state::{EditorState, apply_multi_selection, push_multi_selected_count, push_tiers},
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// The tier-list row's own "x" button (and the tier list's Delete/Backspace, both of
/// which call straight through `EditorModel.remove_tier`): applies
/// `Edit::RemoveTier` through `EditorState::apply`, then
/// [`adjust_selection_after_remove`] to keep `EditorModel.selected_tier_index`
/// pointing at the right row (or nothing) once the removal has shifted everything
/// after it down by one.
pub(in crate::gui::editor) fn setup_remove_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_remove_tier(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(position) = usize::try_from(index) else {
                return;
            };
            // A position past the flat tiers names a concave tier: no dependants to
            // ask about, so it removes straight away.
            let concave = concave_at(&state.borrow(), index);
            if let Some(concave_index) = concave {
                remove_concave_now(
                    &ui,
                    &Services {
                        state: &state,
                        render_ctx: &render_ctx,
                        preview_state: &preview_state,
                        solid_last_solved: &solid_last_solved,
                    },
                    concave_index,
                    index,
                );
                return;
            }
            let index = position;
            remove_tier_now(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                index,
                false,
            );
        });
}

/// Removes tier `index` (clearing other tiers' meet references to it when `cascade`)
/// and refreshes the panel, the preview and the selection. A removal refused because
/// other tiers still meet the tier by name asks "Remove anyway?" and, once accepted,
/// repeats itself with `cascade` set.
fn remove_tier_now(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    index: usize,
    cascade: bool,
) {
    let mut st = state.borrow_mut();
    // The removed tier's name and facet count for the confirmation toast come back
    // with the edit; an already out-of-range index errors and toasts that error.
    match st.remove_tier_with(index, cascade) {
        Ok(removed) => {
            // Tier count changed -- the alignment check falls back to a full solve.
            refresh_editor_panel_stale(ui, render_ctx, &st, &BTreeSet::new());
            adjust_selection_after_remove(ui, index as i32);
            submit_preview_replan(
                ui,
                render_ctx,
                preview_state,
                solid_last_solved,
                &st,
                BTreeSet::new(),
                false,
            );
            // Tiers that followed the removed one keep their angles but follow nothing now:
            // the session says which, and the toast repeats it.
            let freed = take_cleared_relations_sentence(&mut st);
            drop(st);
            let plural = if removed.facet_count == 1 { "" } else { "s" };
            let mut message = format!(
                "Removed {} ({} facet{plural}), Undo",
                removed.name, removed.facet_count
            );
            if let Some(freed) = freed {
                message.push_str(". ");
                message.push_str(&freed);
            }
            show_toast(ui, &message, "info");
        }
        Err(error @ RemoveTierError::HasDependants { .. }) if !cascade => {
            drop(st);
            let state = Rc::clone(state);
            let render_ctx = Arc::clone(render_ctx);
            let preview_state = Arc::clone(preview_state);
            let solid_last_solved = Arc::clone(solid_last_solved);
            ask_write_confirm(
                ui,
                "Remove anyway?",
                error.to_string(),
                "Remove anyway",
                None,
                move |ui| {
                    remove_tier_now(
                        ui,
                        &state,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        index,
                        true,
                    );
                },
            );
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}

/// A row's "Duplicate" button and the tier list's Ctrl+D: inserts a copy of the
/// named tier (name suffixed `'`, same indices/angle/constraint/detached set)
/// immediately AFTER the source row as a new `Edit::AddTier` through
/// `EditorState::apply` -- not appended at the end, since cut order is meaningful
/// (`Edit::AddTier` already supports an arbitrary insertion index, so this passes
/// the source row's own position plus one rather than appending at the end) --
/// then moves the tier-list selection to the copy.
/// The copy's `imported_meet` is always cleared -- it is a new, user-authored row,
/// not itself something a real `.asc` file's `G` field ever made a claim about,
/// even though the tier it was copied FROM might carry one.
pub(in crate::gui::editor) fn setup_duplicate_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_duplicate_tier(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(position) = usize::try_from(index) else {
                return;
            };
            let concave = concave_at(&state.borrow(), index);
            if let Some(concave_index) = concave {
                duplicate_concave_now(
                    &ui,
                    &Services {
                        state: &state,
                        render_ctx: &render_ctx,
                        preview_state: &preview_state,
                        solid_last_solved: &solid_last_solved,
                    },
                    concave_index,
                );
                return;
            }
            let index = position;
            let mut st = state.borrow_mut();
            // The copy's counted `" (N)"` name, cleared `imported_meet` and insertion
            // right after the source all live in `EditorSession::duplicate_tier`
            // (shared with the web app's tier table).
            match st.duplicate_tier(index) {
                Ok(None) => {}
                Ok(Some(duplicated)) => {
                    let new_index = duplicated.new_index;
                    // `AddTier` changes the tier count -- same full-solve fallback
                    // `setup_save_tier_callback`'s own `AddTier` path uses.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::from([new_index]));
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([new_index]),
                        false,
                    );
                    drop(st);
                    ui.global::<EditorModel>()
                        .set_selected_tier_index(new_index as i32);
                    // Names the change instead of leaving a
                    // mis-clicked Duplicate indistinguishable from a no-op.
                    show_toast(
                        &ui,
                        &format!(
                            "Duplicated {} as {}",
                            duplicated.source_label, duplicated.duplicate_label
                        ),
                        "info",
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The tier-list row's "Detach"/"Reattach" toggle: applies
/// `Design::detach_all_in_tier`/`Design::reattach_all_in_tier` through
/// `EditorState::apply`, flipping [`EditorTierItem::is_detached`](crate::EditorTierItem)
/// so one button serves both directions. An explicit, visible escape hatch: a
/// symmetric tier's occurrences stay linked (an edit moves the whole orbit) until the
/// user clicks this, and detaching never happens as a side effect of any other action.
///
/// Also the wiring point for [`setup_move_tier_callback`]/[`setup_complete_orbit_
/// callback`]/[`setup_clear_multi_select_callback`]/[`setup_remove_multi_selected_
/// callback`]/[`setup_facet_remove_callback`]/[`setup_facet_toggle_detach_callback`]/
/// [`setup_facet_add_callback`]/[`setup_tier_rotate_indices_callback`]/
/// [`setup_tier_mirror_indices_callback`]/[`setup_generate_step_series_callback`]/
/// [`setup_mirror_tier_to_other_block_callback`] (the last two share this one
/// entry point) -- `gui::editor::mod::setup_editor_callbacks` has one fixed call
/// site per `setup_*` function name, so a genuinely new callback can only be wired
/// up by piggybacking its own `setup_*` call onto an EXISTING call site that
/// already receives every argument it needs; this is the one existing call already
/// carrying `render_ctx`/`preview_state`/`solid_last_solved` alongside `ui`/`state`.
pub(in crate::gui::editor) fn setup_toggle_detach_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // Stashes the shared `RenderContext` handle for
    // `auto_solve::render_ctx()` -- this is one of several `setup_*_callback`s
    // already given the `Arc` directly by `gui::editor::mod::setup_editor_
    // callbacks`, and it runs once here, synchronously, before
    // `setup_editor_callbacks` returns and the
    // event loop starts -- so by the time a user can hover or click anything,
    // `setup_solid_facet_hover_callback`/`setup_solid_facet_click_callback`
    // (whose own fixed call site never receives `render_ctx` at all) can already
    // read it back to map a click through the letterboxed pick rectangle.
    auto_solve::stash_render_ctx(render_ctx);

    let state_toggle = Rc::clone(state);
    let render_ctx_toggle = Arc::clone(render_ctx);
    let preview_state_toggle = Arc::clone(preview_state);
    let solid_last_solved_toggle = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_toggle_detach(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if index < 0 {
                return;
            }
            let mut st = state_toggle.borrow_mut();
            let index = index as usize;
            match st.toggle_detach(index) {
                Ok(None) => {}
                Ok(Some(toggled)) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx_toggle,
                        &st,
                        &BTreeSet::from([index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx_toggle,
                        &preview_state_toggle,
                        &solid_last_solved_toggle,
                        &st,
                        BTreeSet::from([index]),
                        false,
                    );
                    // Names the change and which way it went -- `toggled.detached` is
                    // whatever the apply just left the tier as, so `true` means "just
                    // detached," `false` means "just reattached."
                    drop(st);
                    if !toggled.label.is_empty() {
                        let verb = if toggled.detached {
                            "Detached"
                        } else {
                            "Reattached"
                        };
                        show_toast(&ui, &format!("{verb} {}", toggled.label), "info");
                    }
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });

    setup_move_tier_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_complete_orbit_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_clear_multi_select_callback(ui, state);
    setup_remove_multi_selected_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    // #45: per-facet index editing -- piggybacked here for the same reason the four
    // calls above are (see this function's own doc comment's "wiring point" section):
    // this is the one existing `editor::setup_editor_callbacks` call site that already
    // receives `render_ctx`/`preview_state`/`solid_last_solved` alongside `ui`/`state`.
    setup_facet_remove_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_facet_toggle_detach_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_facet_add_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_tier_rotate_indices_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_tier_mirror_indices_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    //    // "wiring point" reason given above.
    setup_adopt_all_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_adopt_selected_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_pin_to_mast_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_highlight_tooth_callback(ui, state);
    //    // reason given above.
    setup_generate_step_series_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    // Tier relations: "Remove relation" and the hint for an angle that cannot be edited
    // (the linked step series registers with the plain one just above).
    setup_relation_callbacks(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_mirror_tier_to_other_block_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
}

/// Row reorder (`Alt+Up`/`Alt+Down` and the tier list's own move buttons,
/// `editor_tier_table.slint`): moves the tier at `index` to `index + direction` as
/// one `Edit::MoveTier` -- a single `History` step (one undo press restores the
/// original order) with its own honest "Move tier P1 up/down" label, rather than
/// the two independently-undoable `Edit::ModifyTier` content swaps this used
/// before `Edit::MoveTier` existed. `MoveTier` renumbers every tier strictly
/// between `from` and `to` (see its own doc comment), so this always forces a full
/// re-solve rather than passing a `dirty` set of just the two endpoints.
fn setup_move_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_move_tier(move |index: i32, direction: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(position) = usize::try_from(index) else {
                return;
            };
            let concave = concave_at(&state.borrow(), index);
            if let Some(concave_index) = concave {
                move_concave_now(
                    &ui,
                    &Services {
                        state: &state,
                        render_ctx: &render_ctx,
                        preview_state: &preview_state,
                        solid_last_solved: &solid_last_solved,
                    },
                    concave_index,
                    direction,
                );
                return;
            }
            let index = position;
            let mut st = state.borrow_mut();
            // The end-of-list guard and the single `Edit::MoveTier` live in
            // `EditorSession::move_tier` (shared with the web app's tier table).
            match st.move_tier(index, direction) {
                Ok(None) => {}
                Ok(Some(moved)) => {
                    let target = moved.target;
                    // A move can shift index-wheel alignment for every tier between
                    // the old and new position, not tracked precisely here -- same
                    // "blast radius unknown" treatment as Undo/Redo.
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &(0..st.design.tiers.len()).collect(),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        true,
                    );
                    drop(st);
                    ui.global::<EditorModel>()
                        .set_selected_tier_index(target as i32);
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The tier table's "Complete orbit" action (shown on a row whose
/// [`EditorTierItem::orbit_incomplete`](crate::EditorTierItem) is set): expands
/// EVERY incomplete orbit unit the tier currently decomposes into
/// (`Design::orbit_units`) to its full symmetric membership via one
/// `Design::add_orbit_member` call each, anchored on that unit's own first member
/// -- a tier with several independent incomplete units (rare, but possible) is
/// fully completed in one click, as several separately-undoable `History` steps
/// rather than a new batch primitive.
fn setup_complete_orbit_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_complete_orbit(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            // One `add_orbit_member` edit per incomplete unit, through the shared
            // session -- see `indicatrix_editor::EditorSession::complete_orbit`.
            match st.complete_orbit(index) {
                Ok(false) => {}
                Ok(true) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::from([index]));
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The tier table header's "Clear" action on its "N selected" indicator: empties
/// [`EditorState::multi_selected`] without touching `Design`, matching
/// [`setup_solid_selected_tier_changed_callback`]'s own clearing path.
fn setup_clear_multi_select_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_clear_multi_select(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        if st.multi_selected.is_empty() {
            return;
        }
        st.multi_selected.clear();
        let mut rows: Vec<EditorTierItem> = ui.global::<EditorModel>().get_tiers().iter().collect();
        apply_multi_selection(&mut rows, &st.multi_selected);
        push_tiers(&ui, rows);
        push_multi_selected_count(&ui, 0);
        if let Some(preview_state) = auto_solve::preview_state() {
            resubmit_facet_overlay(&preview_state, |overlay| overlay.multi_selected.clear());
        }
    });
}

/// The tier table header's "Delete" action on its "N selected" indicator: removes
/// every multi-selected tier, highest index first (so removing one never shifts
/// an index still waiting to be removed out from under this loop) -- as several
/// separately-undoable `Edit::RemoveTier`s, matching [`setup_complete_orbit_
/// callback`]'s "several `History` steps, no new batch primitive" trade-off.
fn setup_remove_multi_selected_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_remove_multi_selected(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            remove_multi_selected_now(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                false,
            );
        });
}

/// Removes every multi-selected tier (clearing the references of tiers outside the
/// selection when `cascade`) and refreshes the panel and the preview. A removal refused
/// because unselected tiers still meet a selected one by name asks "Remove anyway?" and,
/// once accepted, repeats itself with `cascade` set.
fn remove_multi_selected_now(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    cascade: bool,
) {
    let mut st = state.borrow_mut();
    // Highest index first, one `Edit::RemoveTier` each, the last failure reported --
    // see `EditorSession::remove_multi_selected_with`.
    match st.remove_multi_selected_with(cascade) {
        Ok(0) => {}
        Ok(removed_count) => {
            // Tier count changed -- the length check falls back to a full solve
            // regardless of `dirty`.
            refresh_editor_panel_stale(ui, render_ctx, &st, &BTreeSet::new());
            submit_preview_replan(
                ui,
                render_ctx,
                preview_state,
                solid_last_solved,
                &st,
                BTreeSet::new(),
                false,
            );
            let freed = take_cleared_relations_sentence(&mut st);
            drop(st);
            ui.global::<EditorModel>().set_selected_tier_index(-1);
            bump_form_reset_pulse(ui);
            let mut message = format!("Removed {removed_count} tier(s).");
            if let Some(freed) = freed {
                message.push(' ');
                message.push_str(&freed);
            }
            show_toast(ui, &message, "info");
        }
        Err(error @ RemoveTierError::HasDependants { .. }) if !cascade => {
            drop(st);
            let state = Rc::clone(state);
            let render_ctx = Arc::clone(render_ctx);
            let preview_state = Arc::clone(preview_state);
            let solid_last_solved = Arc::clone(solid_last_solved);
            ask_write_confirm(
                ui,
                "Remove anyway?",
                error.to_string(),
                "Remove anyway",
                None,
                move |ui| {
                    remove_multi_selected_now(
                        ui,
                        &state,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        true,
                    );
                },
            );
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}
