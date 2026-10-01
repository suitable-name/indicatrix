//! Per-facet and tier-wide index-wheel editing (#45): gear-tooth highlight,
//! per-facet Remove/Detach/Add, and tier-wide Rotate/Mirror.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::ComponentHandle;

use super::facet_overlay::{facet_map_from_aligned_solve, resubmit_facet_overlay};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// A clicked index-wheel tooth
/// (`solid_preview::diagram_wiring::setup_diagram_hover_and_click_callbacks`'s
/// own miss branch, which already reports the id through
/// `SolidPreviewModel.diagram_clicked_tooth` -- see that function's own doc
/// comment naming this callback as the intended consumer) now highlights every
/// facet sharing that tooth, using the reverse of the lookup
/// `setup_toggle_multi_select_callback` already does the forward direction of:
/// that one turns a set of TIER indices into their member facet ids via
/// `FacetMap::facets_of_tier`; this one turns one GEAR TOOTH into every facet id
/// whose own `FacetMap::index_on_gear` matches it, by scanning
/// `0..FacetMap::facet_count()`, since `facet_map.rs` exposes no dedicated
/// tooth-to-facets index. Reuses
/// [`FacetOverlay::multi_selected`] for the tint rather than adding a new overlay
/// field, which would need a `facet_map.rs`/`preview_state.rs` change.
///
/// `tooth < 0` (nothing hit, `SolidPreviewModel.diagram_clicked_tooth`'s own
/// default) clears the highlight instead of leaving a stale one from a previous
/// click.
///
/// Wired to `EditorModel.highlight_tooth(int)` (declared in
/// `ui/models/editor.slint`) -- `editor_tier_table.slint`'s own
/// `tracked_clicked_tooth` mirror (see that property's doc comment for why a
/// mirrored property, not a `changed` handler on the global itself, is what calls
/// it) invokes it whenever `SolidPreviewModel.diagram_clicked_tooth` changes.
///
/// `pub(super)` since [`super::tier_crud::setup_toggle_detach_callback`] is the
/// one call site that registers it.
pub(super) fn setup_highlight_tooth_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_highlight_tooth(move |tooth: i32| {
            let Some(preview_state) = auto_solve::preview_state() else {
                return;
            };
            if tooth < 0 {
                resubmit_facet_overlay(&preview_state, |overlay| overlay.multi_selected.clear());
                return;
            }
            let st = state.borrow();
            let facet_map = facet_map_from_aligned_solve(&st.design);
            let tooth = tooth as u32;
            let facet_ids: Vec<u32> = (0..facet_map.facet_count() as u32)
                .filter(|&facet_id| facet_map.index_on_gear(facet_id as usize) == tooth)
                .collect();
            resubmit_facet_overlay(&preview_state, |overlay| {
                overlay.multi_selected = facet_ids;
            });
        });
}

/// Per-facet index editing (#45): "Remove" -- removes one index-wheel occurrence
/// from a tier via [`Design::remove_orbit_member`]. Removing an occurrence that
/// belongs to a complete, non-detached orbit unit removes every member of that unit
/// with it (see that method's own doc comment) -- deleting "one facet" out of a
/// clean orbit never leaves `symmetry_order` describing a lie about what `indices`
/// actually holds. Wired up from `setup_toggle_detach_callback`'s own call site
/// (see that function's doc comment's "wiring point" section for why).
///
/// Wired to `EditorModel.facet_remove(int, float)` (declared in
/// `ui/models/editor.slint`; tier index, index-wheel position) -- the per-facet
/// index chips that call it live in `editor_inspector.slint`.
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_facet_remove_callback(
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
        .on_facet_remove(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .remove_orbit_member(tier_index, f64::from(position))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Per-facet index editing (#45): "Detach"/"Reattach" toggle for ONE index-wheel
/// occurrence -- the per-facet counterpart to `setup_toggle_detach_callback`'s
/// whole-tier button, via [`Design::detach_orbit_member`]/
/// [`Design::reattach_orbit_member`]. `position` already being in the tier's own
/// `detached` list picks the direction, matching `setup_toggle_detach_callback`'s
/// own "empty vs. non-empty" convention one level down (a single occurrence rather
/// than the whole tier).
///
/// Wired to `EditorModel.facet_toggle_detach(int, float)` (declared in
/// `ui/models/editor.slint`) -- called from the same per-facet chips in
/// `editor_inspector.slint` as [`setup_facet_remove_callback`].
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_facet_toggle_detach_callback(
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
        .on_facet_toggle_detach(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let position = f64::from(position);
            let mut st = state.borrow_mut();
            let Some(tier) = st.design.tiers.get(tier_index) else {
                return;
            };
            let edit_result = if tier.detached.contains(&position) {
                st.design.reattach_orbit_member(tier_index, position)
            } else {
                st.design.detach_orbit_member(tier_index, position)
            };
            match edit_result.and_then(|edit| st.apply(edit)) {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Per-facet index editing (#45): "Add" -- adds one index-wheel occurrence to a tier
/// via [`Design::add_orbit_member`], expanded to its complete symmetry orbit (see
/// that method's own doc comment): an addition can never leave a half-populated
/// orbit unit behind.
///
/// Wired to `EditorModel.facet_add(int, float)` (declared in
/// `ui/models/editor.slint`) -- called from the same per-facet chips in
/// `editor_inspector.slint` as [`setup_facet_remove_callback`].
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_facet_add_callback(
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
        .on_facet_add(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .add_orbit_member(tier_index, f64::from(position))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Tier-wide index editing (#45): "Rotate" -- rotates every index-wheel position in
/// the tier at `tier_index` (both `indices` and `detached`) by `k_teeth` around the
/// gear, via [`Design::rotate_indices`].
///
/// Wired to `EditorModel.tier_rotate_indices(int, float)` (declared in
/// `ui/models/editor.slint`; tier index, teeth to rotate by) -- the inspector's
/// "rotate this tier" stepper in `editor_inspector.slint` calls it.
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_tier_rotate_indices_callback(
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
        .on_tier_rotate_indices(move |tier_index: i32, k_teeth: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .rotate_indices(tier_index, f64::from(k_teeth))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Tier-wide index editing (#45): "Mirror" -- mirrors every index-wheel position in
/// the tier at `tier_index` (both `indices` and `detached`) to the other side of the
/// symmetry axis, via [`Design::mirror_indices`].
///
/// Wired to `EditorModel.tier_mirror_indices(int)` (declared in
/// `ui/models/editor.slint`) -- called from the same inspector control in
/// `editor_inspector.slint` as [`setup_tier_rotate_indices_callback`].
///
/// `pub(super)` for the same reason as [`setup_highlight_tooth_callback`] above.
pub(super) fn setup_tier_mirror_indices_callback(
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
        .on_tier_mirror_indices(move |tier_index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            // Through the shared session -- see
            // `indicatrix_editor::EditorSession::mirror_indices`.
            match st.mirror_indices(tier_index) {
                Ok(_) => {
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}
