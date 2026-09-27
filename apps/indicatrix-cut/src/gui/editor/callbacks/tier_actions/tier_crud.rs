//! Tier-list structural edits: remove, duplicate, detach/reattach (and the
//! `setup_toggle_detach_callback` wiring hub it doubles as), move, complete-orbit,
//! and multi-select clear/remove.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_cut_core::Edit;
use slint::{ComponentHandle, Model};

use super::{
    adopt::{setup_adopt_all_callback, setup_adopt_selected_callback, setup_pin_to_mast_callback},
    facet_editing::{
        setup_facet_add_callback, setup_facet_remove_callback, setup_facet_toggle_detach_callback,
        setup_highlight_tooth_callback, setup_tier_mirror_indices_callback,
        setup_tier_rotate_indices_callback,
    },
    facet_overlay::resubmit_facet_overlay,
    misc::{adjust_selection_after_remove, bump_form_reset_pulse},
    nudge::tier_nudge_label,
    tier_generation::{
        setup_generate_step_series_callback, setup_mirror_tier_to_other_block_callback,
        unique_duplicate_name,
    },
};
use crate::{
    EditorModel, EditorTierItem, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            state::{EditorState, apply_multi_selection, push_multi_selected_count, push_tiers},
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// The tier-list row's own "x" button (and the tier list's Delete/Backspace, both of
/// which call straight through `EditorModel.remove_tier`): applies
/// [`Edit::RemoveTier`] through `EditorState::apply`, then
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
            if index < 0 {
                return;
            }
            let mut st = state.borrow_mut();
            // Captured before the removal for the confirmation toast below --
            // `None` (an already out-of-range index) just skips that toast, the
            // same as today's silent behavior.
            let removed_summary = st.design.tiers.get(index as usize).map(|tier| {
                let name = if tier.name.is_empty() {
                    "(unnamed)".to_string()
                } else {
                    tier.name.clone()
                };
                (name, tier.indices.len())
            });
            match st.apply(Edit::RemoveTier {
                index: index as usize,
            }) {
                Ok(()) => {
                    // Tier count changed -- the alignment check falls back to a full solve.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    adjust_selection_after_remove(&ui, index);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                    drop(st);
                    if let Some((name, facet_count)) = removed_summary {
                        let plural = if facet_count == 1 { "" } else { "s" };
                        show_toast(
                            &ui,
                            &format!("Removed {name} ({facet_count} facet{plural}), Undo"),
                            "info",
                        );
                    }
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// A row's "Duplicate" button and the tier list's Ctrl+D: inserts a copy of the
/// named tier (name suffixed `'`, same indices/angle/constraint/detached set)
/// immediately AFTER the source row as a new [`Edit::AddTier`] through
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
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(source) = st.design.tiers.get(index) else {
                return;
            };
            let mut duplicate = source.clone();
            let source_label = if source.name.is_empty() {
                "(unnamed)".to_string()
            } else {
                source.name.clone()
            };
            // Uses a counted `" (N)"` suffix instead of appending an apostrophe --
            // see `unique_duplicate_name`'s own doc comment for why an apostrophe
            // scheme piles up unreadable "P1''''" names AND silently creates a
            // duplicate name that a meet resolver secretly binds to the FIRST tier
            // holding it.
            let existing_names: Vec<String> =
                st.design.tiers.iter().map(|t| t.name.clone()).collect();
            duplicate.name = unique_duplicate_name(&source.name, &existing_names);
            let duplicate_label = duplicate.name.clone();
            duplicate.imported_meet = None;
            let new_index = index + 1;
            match st.apply(Edit::AddTier {
                index: new_index,
                tier: duplicate,
            }) {
                Ok(()) => {
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
                        &format!("Duplicated {source_label} as {duplicate_label}"),
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
            let Some(tier) = st.design.tiers.get(index) else {
                return;
            };
            let edit_result = if tier.detached.is_empty() {
                st.design.detach_all_in_tier(index)
            } else {
                st.design.reattach_all_in_tier(index)
            };
            match edit_result.and_then(|edit| st.apply(edit)) {
                Ok(()) => {
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
                    // Names the change and which way it went
                    // -- `tier.detached` is now whatever this apply just left it
                    // as, so a non-empty set here means "just detached," empty
                    // means "just reattached."
                    let now_detached = st
                        .design
                        .tiers
                        .get(index)
                        .is_some_and(|tier| !tier.detached.is_empty());
                    let label = st
                        .design
                        .tiers
                        .get(index)
                        .map(|tier| tier_nudge_label(tier, index));
                    drop(st);
                    if let Some(label) = label {
                        let verb = if now_detached {
                            "Detached"
                        } else {
                            "Reattached"
                        };
                        show_toast(&ui, &format!("{verb} {label}"), "info");
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
/// one [`Edit::MoveTier`] -- a single `History` step (one undo press restores the
/// original order) with its own honest "Move tier P1 up/down" label, rather than
/// the two independently-undoable [`Edit::ModifyTier`] content swaps this used
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
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let tier_count = st.design.tiers.len();
            let target = if direction < 0 {
                index.checked_sub(1)
            } else {
                index.checked_add(1).filter(|&t| t < tier_count)
            };
            let Some(target) = target else {
                return;
            };
            match st.apply(Edit::MoveTier {
                from: index,
                to: target,
            }) {
                Ok(()) => {
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
            let Ok(units) = st.design.orbit_units(index) else {
                return;
            };
            let anchors: Vec<f64> = units
                .iter()
                .filter(|unit| !unit.is_complete())
                .filter_map(|unit| unit.members.first().copied())
                .collect();
            if anchors.is_empty() {
                return;
            }
            let mut last_err = None;
            for position in anchors {
                let outcome = st
                    .design
                    .add_orbit_member(index, position)
                    .and_then(|edit| st.apply(edit));
                if let Err(e) = outcome {
                    last_err = Some(e);
                }
            }
            match last_err {
                None => {
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
                Some(e) => show_toast(&ui, &e.to_string(), "error"),
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
/// separately-undoable [`Edit::RemoveTier`]s, matching [`setup_complete_orbit_
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
            let mut st = state.borrow_mut();
            let targets: Vec<usize> = st.multi_selected.iter().rev().copied().collect();
            if targets.is_empty() {
                return;
            }
            let removed_count = targets.len();
            let mut last_err = None;
            for index in targets {
                if let Err(e) = st.apply(Edit::RemoveTier { index }) {
                    last_err = Some(e);
                }
            }
            match last_err {
                None => {
                    // Tier count changed -- the length check falls back to a full
                    // solve regardless of `dirty`.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                    drop(st);
                    ui.global::<EditorModel>().set_selected_tier_index(-1);
                    bump_form_reset_pulse(&ui);
                    show_toast(&ui, &format!("Removed {removed_count} tier(s)."), "info");
                }
                Some(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}
