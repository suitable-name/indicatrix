//! The Solid viewport's facet hover/click and the reverse tier-selection link.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::{ComponentHandle, Model};

use super::facet_overlay::{resubmit_facet_overlay, set_hovered_facet};
use crate::{
    EditorModel, EditorTierItem, MainWindow, SolidPreviewModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            manipulate::{drop_provisional_for_selection, provisional_active},
            state::{EditorState, apply_multi_selection, push_multi_selected_count, push_tiers},
            view::{SolidLastSolved, push_selected_tier_chips, submit_preview_replan},
        },
        solid_preview::preview_state::{SolidPickState, SolidPreviewState},
    },
};

thread_local! {
    /// The last clicked facet's own identifying text -- the SAME "tier name, index
    /// N of M" string [`SolidPreviewModel::hover_text`] shows transiently on hover.
    /// Kept around so a click's selection keeps reading on screen after the
    /// pointer leaves. Reused rather than a new `EditorModel`/`SolidPreviewModel`
    /// property: no `.slint` file declares one, and this gives a
    /// persistent per-facet readout with the existing tooltip mechanism alone.
    /// See [`setup_solid_facet_hover_callback`]'s miss branch and
    /// [`setup_solid_facet_click_callback`], which writes it. UI-thread-only,
    /// same reasoning as `super::facet_overlay`'s own `thread_local!`.
    static SELECTED_FACET_LABEL: RefCell<String> = const { RefCell::new(String::new()) };
    /// The last clicked facet's id, written and cleared exactly where
    /// [`SELECTED_FACET_LABEL`] is: the facet the manipulation handles
    /// (`gui::editor::manipulate`) sit on. A selection made from the tier table
    /// leaves it stale or unset, which the handles resolve by checking it belongs to
    /// the selected tier and falling back to the tier's first facet. UI-thread-only.
    static SELECTED_FACET_ID: RefCell<Option<u32>> = const { RefCell::new(None) };
}

/// The last clicked facet's id, if the last click landed on a facet/// [`SELECTED_FACET_ID`].
#[must_use]
pub(in crate::gui::editor) fn selected_facet_id() -> Option<u32> {
    SELECTED_FACET_ID.with(|cell| *cell.borrow())
}

/// Maps an incoming Solid-viewport pointer
/// position (`SolidPreviewModel.on_facet_hover`/`on_facet_click`'s own `x`/`y`,
/// LOGICAL pixels) onto the pick buffer's PHYSICAL-pixel coordinate space.
///
/// In Path-traced/Both mode (`SolidPreviewModel.view_mode` `1`/`2`) the solid/
/// edges rasters are now requested at the LETTERBOXED rectangle
/// `render::camera_lighting::contained_request_size` computes -- mirroring
/// `solid_viewport.slint`'s own `image-fit: contain` -- which can be smaller
/// than, and is centred within, the viewport's own raw rectangle (see that
/// function's own doc comment for when and why). Multiplying by
/// `scale_factor` alone (the pre-existing, still-necessary logical-to-physical
/// conversion) is not enough on its own in that case: it would still index the
/// pick buffer as though it covered the FULL, un-letterboxed viewport, landing a
/// click off the traced gem by exactly the letterbox bars' width/height.
///
/// Falls back to the plain scaled position (today's behavior) when
/// `auto_solve::render_ctx` has not been stashed yet, or `RenderContext.width`/
/// `height` is `0` (nothing requested) -- `contained_request_size` itself
/// already returns `viewport_size` unchanged for Solid/Diagram modes, so this
/// is a genuine no-op there, not just an approximation.
pub(in crate::gui::editor) fn map_to_pick_coordinates(
    ui: &MainWindow,
    x: f32,
    y: f32,
) -> (f32, f32) {
    let scale = ui.window().scale_factor();
    logical_to_pick_pixels((x, y), scale, pick_margin(ui))
}

/// [`map_to_pick_coordinates`]'s arithmetic on plain numbers: a LOGICAL pointer
/// position scaled to physical pixels, less the letterbox `margin`.
#[must_use]
pub(in crate::gui::editor) fn logical_to_pick_pixels(
    logical: (f32, f32),
    scale: f32,
    margin: (f32, f32),
) -> (f32, f32) {
    // Scaled first, then shifted, exactly as the hover/click callbacks always did
    // (a fused multiply-add would round differently by a hair).
    let physical = (logical.0 * scale, logical.1 * scale);
    (physical.0 - margin.0, physical.1 - margin.1)
}

/// The exact inverse of [`logical_to_pick_pixels`]: a pick-frame pixel back to the
/// LOGICAL viewport position it is drawn at, `(pick + margin) / scale`. What the
/// manipulation handles use to place themselves.
#[must_use]
pub(in crate::gui::editor) fn pick_pixels_to_logical(
    pick: (f32, f32),
    scale: f32,
    margin: (f32, f32),
) -> (f32, f32) {
    ((pick.0 + margin.0) / scale, (pick.1 + margin.1) / scale)
}

/// The PHYSICAL-pixel letterbox margin [`map_to_pick_coordinates`] subtracts: `(0, 0)`
/// in Solid/Diagram mode and before `auto_solve::render_ctx` is stashed, half the bars'
/// width/height in Path-traced/Both mode (see [`map_to_pick_coordinates`]'s doc).
pub(in crate::gui::editor) fn pick_margin(ui: &MainWindow) -> (f32, f32) {
    let scale = ui.window().scale_factor();
    let Some(render_ctx) = auto_solve::render_ctx() else {
        return (0.0, 0.0);
    };
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    let viewport_physical = (
        (ui.global::<SolidPreviewModel>().get_viewport_width() * scale) as u32,
        (ui.global::<SolidPreviewModel>().get_viewport_height() * scale) as u32,
    );
    let render_size = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (ctx.width, ctx.height)
    };
    let contained = crate::gui::render::camera_lighting::contained_request_size(
        view_mode,
        viewport_physical,
        render_size,
    );
    letterbox_margin(viewport_physical, contained)
}

/// The physical-pixel margin [`map_to_pick_coordinates`] subtracts before
/// indexing the pick buffer -- pure arithmetic, unit tested directly. `image-fit:
/// contain` centres the smaller `contained_physical` rectangle inside
/// `viewport_physical`, so the margin on each axis is exactly half of
/// whatever's left over; `saturating_sub` guards the (never expected, but never
/// unsafe either) case where `contained_physical` is somehow larger.
///
/// `pub(in crate::gui::editor)` since [`super::tests`] and the manipulation module's
/// tests exercise this directly.
#[must_use]
pub(in crate::gui::editor) fn letterbox_margin(
    viewport_physical: (u32, u32),
    contained_physical: (u32, u32),
) -> (f32, f32) {
    (
        viewport_physical.0.saturating_sub(contained_physical.0) as f32 / 2.0,
        viewport_physical.1.saturating_sub(contained_physical.1) as f32 / 2.0,
    )
}

/// The Solid viewport's hover callback -- looks up the facet under the cursor against
/// the pick buffer of the LAST rendered frame and sets `editor_solid_hover_text` from
/// `solid_hover_text`, the same frame's own `PreviewFrame::hover_text` table
/// (`SlintSolidSink::apply`, `gui::mod`). A silent no-op off the silhouette or before
/// anything has ever rendered.
///
/// # Indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// Rebuilding `FacetMap::from_design(&st.design, &solved)` -- a full
/// `Design::planes_from_solved`-equivalent rebuild plus a fresh `dedup_planes` pass --
/// on every single mouse-move event would be wasteful. `solid_hover_text` already
/// holds exactly the string that map would produce for each facet id, computed ONCE
/// per rendered frame by the worker thread
/// (`preview_state::update_diagram_memory_from_design`'s Solid-mode counterpart), so
/// this degrades to one `Vec::get`. Neither `Design` nor the last-solved mast cache
/// is needed here at all (an editor edit that hasn't re-rendered yet still shows the
/// PREVIOUS frame's hover text, exactly as it shows the previous frame's picked
/// geometry -- no new staleness).
///
/// `solid_hover_text` reaches this callback through `solid_pick_state` (see
/// [`SolidPickState`]'s own doc comment): `gui::mod::build_main_window` owns the
/// `Arc<Mutex<Vec<String>>>` `SlintSolidSink` writes every frame, and bundles it into
/// the `SolidPickState` this function's caller passes through.
pub(in crate::gui::editor) fn setup_solid_facet_hover_callback(
    ui: &MainWindow,
    solid_pick_state: &SolidPickState,
) {
    let solid_pick = Arc::clone(&solid_pick_state.pick);
    let solid_hover_text = Arc::clone(&solid_pick_state.hover_text);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_hover(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // The pick buffer is rasterized at the viewport's PHYSICAL size
            // (`view::scaled_viewport_size`) but `x`/`y` are the pointer's LOGICAL
            // position, so they are MULTIPLIED by the same `scale_factor` to reach
            // the physical pixel they name: at 2x, logical x=400 in an 800-wide
            // viewport is physical x=800 in a 1600-wide pick buffer. Dividing would
            // collapse every pick into the top-left quarter of the image.
            // `solid_preview::diagram_wiring` does the same for the diagram's own
            // pick buffer. In Path-traced/Both mode the pick
            // buffer is smaller than the raw viewport (letterboxed), so
            // `map_to_pick_coordinates` also subtracts the centred margin --
            // see that function's own doc comment. `.max(0.0) as u32` saturates a
            // negative position (past the image's edge, or inside a letterbox bar)
            // to `0`; `PickBuffer::facet_at` already bounds-checks against the
            // frame's width/height, so this simply misses (`None`) rather than
            // reading garbage.
            let (px, py) = map_to_pick_coordinates(&ui, x, y);
            let facet_id = solid_pick
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|pick| pick.facet_at(px.max(0.0) as u32, py.max(0.0) as u32));
            let Some(facet_id) = facet_id else {
                // Falls back to the last CLICKED facet's own
                // label (if any) instead of blanking the tooltip outright, so the
                // selection stays readable once the pointer leaves it                // `SELECTED_FACET_LABEL`'s own doc comment.
                let selected_label = SELECTED_FACET_LABEL.with(|cell| cell.borrow().clone());
                ui.global::<SolidPreviewModel>()
                    .set_hover_text(selected_label.into());
                if let Some(preview_state) = auto_solve::preview_state() {
                    set_hovered_facet(&preview_state, None);
                }
                return;
            };
            let text = solid_hover_text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(facet_id as usize)
                .cloned()
                .unwrap_or_default();
            ui.global::<SolidPreviewModel>().set_hover_text(text.into());
            if let Some(preview_state) = auto_solve::preview_state() {
                set_hovered_facet(&preview_state, Some(facet_id));
            }
        });
}

/// The Solid viewport's click callback -- the forward half of the "click selects the
/// tier in the list" link (see [`setup_solid_selected_tier_changed_callback`] for the
/// reverse half).
///
/// # Indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// See [`setup_solid_facet_hover_callback`]'s matching doc section: `solid_facet_tier`
/// is the last rendered frame's own `PreviewFrame::facet_tier` table, so resolving a
/// clicked facet to its owning tier is one `Vec::get` instead of a fresh
/// `FacetMap::from_design` rebuild. Neither `Design` nor the last-solved mast cache is
/// needed here at all. `solid_facet_tier` reaches this callback the same way
/// `solid_hover_text` reaches the hover callback: through `solid_pick_state`, bundled
/// there by `gui::mod::build_main_window`.
pub(in crate::gui::editor) fn setup_solid_facet_click_callback(
    ui: &MainWindow,
    solid_pick_state: &SolidPickState,
) {
    let solid_pick = Arc::clone(&solid_pick_state.pick);
    let solid_facet_tier = Arc::clone(&solid_pick_state.facet_tier);
    let solid_hover_text = Arc::clone(&solid_pick_state.hover_text);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_click(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // While the Slice tool holds a provisional tier, the frame's `facet_tier`
            // table may name that tier -- an index `EditorState` does not have -- so a
            // click selects nothing until Keep or Discard.
            if provisional_active() {
                return;
            }
            // See `setup_solid_facet_hover_callback`'s matching comment for why the
            // incoming (logical) coordinates go through `map_to_pick_coordinates`
            // (scaled AND, in Path-traced/Both mode, letterbox-corrected) here.
            let (px, py) = map_to_pick_coordinates(&ui, x, y);
            let Some(facet_id) = solid_pick
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|pick| pick.facet_at(px.max(0.0) as u32, py.max(0.0) as u32))
            else {
                // A click that misses the silhouette clears
                // the tier selection instead of leaving it alone -- matching what
                // `solid_preview::diagram_wiring`'s own Diagram-mode click miss
                // branch already does.
                // Setting `selected_tier_index` alone is enough: `changed
                // selected_tier_index` in `models/editor.slint` fires
                // `selected_tier_changed`, which re-seeds/clears the inspector
                // form on the Rust side (`setup_solid_selected_tier_changed_
                // callback`).
                ui.global::<EditorModel>().set_selected_tier_index(-1);
                // A miss also clears whatever facet was
                // previously identified -- nothing is selected any more, so
                // nothing should keep reading in the tooltip.
                SELECTED_FACET_LABEL.with(|cell| cell.borrow_mut().clear());
                SELECTED_FACET_ID.with(|cell| *cell.borrow_mut() = None);
                ui.global::<SolidPreviewModel>().set_hover_text("".into());
                return;
            };
            let tier_index = solid_facet_tier
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(facet_id as usize)
                .copied()
                .flatten();
            if let Some(tier_index) = tier_index {
                ui.global::<EditorModel>()
                    .set_selected_tier_index(tier_index as i32);
            }
            // Identifies the clicked facet itself, not just its
            // owning tier -- GemCad/GCS-style "which index is this facet, and
            // which member of the orbit did I click" -- reusing the SAME per-facet
            // label `setup_solid_facet_hover_callback` shows transiently on hover
            // (`solid_hover_text`), but kept in `SELECTED_FACET_LABEL` so it
            // survives the pointer leaving the facet.
            let label = solid_hover_text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(facet_id as usize)
                .cloned()
                .unwrap_or_default();
            SELECTED_FACET_LABEL.with(|cell| cell.borrow_mut().clone_from(&label));
            SELECTED_FACET_ID.with(|cell| *cell.borrow_mut() = Some(facet_id));
            ui.global::<SolidPreviewModel>()
                .set_hover_text(label.into());
            // The clicked facet stays lit regardless of whether it resolved to a
            // tier above -- a facet under the cursor is always a real pick.
            if let Some(preview_state) = auto_solve::preview_state() {
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.selected_facet = Some(facet_id);
                });
            }
        });
}

/// The reverse link: whenever the tier list's selection changes (a row click, or
/// [`setup_solid_facet_click_callback`] setting `editor_selected_tier_index` from a
/// viewport click), re-submits a redraw with the new `selected_tier` so the overlay
/// tint follows it. Cheap: the mesh is unchanged (`dirty` empty), so the worker's
/// `MeshCache` hits and only the style/render redo.
///
/// Also narrows [`EditorState::multi_selected`] back down to nothing here: EVERY
/// path that fires `selected_tier_changed` is, by construction, a plain (non-Ctrl)
/// selection -- `setup_toggle_multi_select_callback`'s own Ctrl+click path never
/// touches `editor_selected_tier_index` at all, so it never reaches this callback --
/// so this is the one place that needs to clear a forgotten multi-select group
/// before it silently widens the next angle nudge (see `setup_nudge_angle_callback`'s
/// own doc comment for that batch behaviour). Patches the already-pushed
/// `editor_tiers` model in place, the same as `setup_toggle_multi_select_callback`
/// does, rather than a full [`refresh_editor_panel_stale`] -- narrowing the
/// multi-select highlight is not itself a `Design` edit and must not re-label the
/// validation banner "Not solved".
///
/// Runs [`apply_selected_tier_change`] directly and unconditionally -- unlike an
/// earlier version of this callback, which assumed Slint invoked it SYNCHRONOUSLY
/// from inside `set_selected_tier_index` and deferred to a zero-duration `Timer`
/// whenever `state.try_borrow_mut()` found the `RefCell` already held (by
/// `apply_loaded_design`/`adjust_selection_after_remove`, both of which write the
/// property while still holding their own guard). That assumption was wrong:
/// `EditorModel.selected_tier_changed` is invoked from `ui/models/editor.slint`'s
/// own `changed selected_tier_index => { ... }`, and `changed` handlers are QUEUED
/// -- they run on the next
/// event-loop turn or tree flush, strictly after the Rust call that set the
/// property has already returned and dropped every borrow it held. The busy
/// check could therefore never actually observe a held borrow from EITHER of
/// those two callers; it was dead weight kept alive by a false assumption, not a
/// real race it was guarding against.
pub(in crate::gui::editor) fn setup_solid_selected_tier_changed_callback(
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
        .on_selected_tier_changed(move |_index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            apply_selected_tier_change(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
            );
        });
}

/// The body of [`setup_solid_selected_tier_changed_callback`]'s handler, factored out
/// purely to keep that function short: [`setup_solid_selected_tier_changed_callback`]
/// calls this directly and unconditionally.
fn apply_selected_tier_change(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // A provisional slice cannot outlive a selection change: this replan renders the
    // committed design, so the slice ends first (before `state` is borrowed).
    drop_provisional_for_selection(ui);
    let mut st = state.borrow_mut();
    // Ending a coalesce run needs a real
    // interaction boundary, and the ideal one (pointer release/focus loss on the
    // `TierAngleCell` doing the nudging) lives in `editor_tier_table.slint`, not
    // here (see `History::end_coalesce_run`'s own doc comment). This runs on a
    // genuine `changed selected_tier_index` (a different row/facet clicked, or
    // the selection cleared) AND on a plain re-click of the row already selected
    // (`editor_view.slint`'s `tier_table.tier_selected` handler invokes
    // `EditorModel.selected_tier_changed` directly for that one case, since a
    // same-value property write raises no `changed` at all) -- never on the
    // nudge control's own repeated ticks, so a wheel-nudge run in progress on the
    // tier the cutter just navigated away from must not sit open for an
    // unrelated later nudge on that same tier (after selecting elsewhere and
    // back within the coalescing window) to merge into.
    st.history.end_coalesce_run();
    if !st.multi_selected.is_empty() {
        st.multi_selected.clear();
        let mut rows: Vec<EditorTierItem> = ui.global::<EditorModel>().get_tiers().iter().collect();
        apply_multi_selection(&mut rows, &st.multi_selected);
        push_tiers(ui, rows);
        push_multi_selected_count(ui, 0);
        resubmit_facet_overlay(preview_state, |overlay| overlay.multi_selected.clear());
    }
    // The inspector's index chips describe whichever tier is now loaded, and
    // a plain row click (no edit) reaches Rust nowhere else.
    push_selected_tier_chips(ui, &st);
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        &st,
        BTreeSet::new(),
        false,
    );
}
