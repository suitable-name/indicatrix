//! The tier list's multi-select: Ctrl+click toggle and Shift+click range-select.

use std::{cell::RefCell, rc::Rc};

use slint::{ComponentHandle, Model};

use super::facet_overlay::{facet_map_from_aligned_solve, resubmit_facet_overlay};
use crate::{
    EditorModel, EditorTierItem, MainWindow,
    gui::editor::{
        auto_solve,
        state::{EditorState, apply_multi_selection, push_multi_selected_count, push_tiers},
    },
};

/// The tier list's Ctrl+click: toggles one row into/out of
/// [`EditorState::multi_selected`], then patches `EditorTierItem::multi_selected`
/// onto the ALREADY-PUSHED `editor_tiers` model in place, rather than calling
/// [`refresh_editor_panel_stale`] -- toggling a multi-select highlight changes
/// nothing about `Design`, so it must never re-label the validation banner "Not
/// solved" the way every real edit's stale-refresh does. See
/// `state::tier_items_stale`'s own doc comment for why this function
/// exists instead of threading the selection through that builder.
pub(in crate::gui::editor) fn setup_toggle_multi_select_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    // Piggybacked here (before `state` below is shadowed by its own clone) for
    // the reason `setup_toggle_detach_callback`'s own doc comment gives:
    // `gui::editor::mod::setup_editor_callbacks` has one fixed call site per
    // `setup_*` function name, so a new callback is wired up from an EXISTING
    // call site instead.
    setup_select_tier_range_callback(ui, state);

    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_toggle_multi_select(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            if !st.multi_selected.remove(&index) {
                st.multi_selected.insert(index);
            }
            let multi_selected = st.multi_selected.clone();
            let mut rows: Vec<EditorTierItem> =
                ui.global::<EditorModel>().get_tiers().iter().collect();
            apply_multi_selection(&mut rows, &multi_selected);
            push_tiers(&ui, rows);
            push_multi_selected_count(&ui, multi_selected.len());
            // Resolves every multi-selected TIER to its member FACET ids so the
            // group is visible in 3D too, not only as a one-pixel row border --
            // `FacetOverlay::multi_selected` is a flat facet id list, built against
            // the last rendered frame's masts the same way the hover/click
            // callbacks already build one.
            if let Some(preview_state) = auto_solve::preview_state() {
                let facet_map = facet_map_from_aligned_solve(&st.design);
                let facet_ids: Vec<u32> = multi_selected
                    .iter()
                    .flat_map(|&tier_index| facet_map.facets_of_tier(tier_index).iter().copied())
                    .collect();
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.multi_selected = facet_ids;
                });
            }
        });
}

/// The tier list's Shift+click: replaces
/// [`EditorState::multi_selected`] wholesale with every tier index between the
/// current selection anchor (`EditorModel.selected_tier_index`) and `index`,
/// inclusive of both ends. Unlike [`setup_toggle_multi_select_callback`]'s
/// Ctrl+click, which only ever flips ONE row in or out, a plain Shift+click
/// always REPLACES the whole set -- the spreadsheet/GCS convention
/// `editor_tier_table.slint`'s row click handler now follows for Shift, checked
/// before that same handler's existing Ctrl+click branch.
///
/// No anchor yet (`selected_tier_index < 0` -- a brand-new design, before any row
/// has ever been selected) falls back to a single-row selection of `index`: there
/// is nothing sensible to range from.
///
/// Reuses the exact row-patch/count-push/facet-overlay-resubmit tail
/// [`setup_toggle_multi_select_callback`] already has, and is piggybacked onto
/// that callback's own registration for the reason `setup_toggle_detach_callback`'s
/// own doc comment gives: `gui::editor::mod::setup_editor_callbacks` (not this
/// lane's file to edit) has one fixed call site per `setup_*` function name.
fn setup_select_tier_range_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_select_tier_range(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let anchor = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index())
                .unwrap_or(index);
            let (lo, hi) = if anchor <= index {
                (anchor, index)
            } else {
                (index, anchor)
            };
            let mut st = state.borrow_mut();
            st.multi_selected = (lo..=hi).collect();
            let multi_selected = st.multi_selected.clone();
            let mut rows: Vec<EditorTierItem> =
                ui.global::<EditorModel>().get_tiers().iter().collect();
            apply_multi_selection(&mut rows, &multi_selected);
            push_tiers(&ui, rows);
            push_multi_selected_count(&ui, multi_selected.len());
            // Same facet-overlay resubmit `setup_toggle_multi_select_callback` uses
            // -- resolves every multi-selected TIER to its member FACET ids so the
            // range is visible in 3D too, not only as a row border.
            if let Some(preview_state) = auto_solve::preview_state() {
                let facet_map = facet_map_from_aligned_solve(&st.design);
                let facet_ids: Vec<u32> = multi_selected
                    .iter()
                    .flat_map(|&tier_index| facet_map.facets_of_tier(tier_index).iter().copied())
                    .collect();
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.multi_selected = facet_ids;
                });
            }
        });
}
