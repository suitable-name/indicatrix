//! Rust-side wiring for the Diagram view mode's own hover/click callbacks.
//!
//! `app.slint`'s `editor_solid_diagram_facet_hover`/`_click`, forwarded from
//! `solid_viewport.slint`'s `diagram_facet_hover`/`diagram_facet_click` -- see that
//! component's own doc comment for why Diagram mode needs separate callbacks from
//! the Solid rasterizer's `facet_hover`/`facet_click`.
//!
//! Unlike `gui::editor::callbacks::tier_actions`'s `setup_solid_facet_hover_callback`/
//! `setup_solid_facet_click_callback` (which build a fresh `facet_map::FacetMap`
//! from `EditorState`'s `Design` on every hover), these callbacks do NOT need
//! `Design`/`FacetMap` access at all: `preview_state::render_request` already
//! builds the facet-id-indexed hover-text and tier tables on the WORKER thread
//! (see [`super::preview_state::PreviewFrame::diagram_hover_text`]'s doc comment
//! for why) and hands them here as plain `Arc<Mutex<..>>` state, exactly like
//! `solid_pick`/`solid_last_solved` already work for the Solid view. That is what
//! lets this module live under `solid_preview/` and be wired from `gui::mod`
//! directly, with no dependency on `gui::editor`'s `EditorState`.

use super::preview_state::{FacetOverlay, PickBuffer, SolidPreviewState};
use crate::{EditorModel, MainWindow, SolidPreviewModel};
use indicatrix_solid::preview::{panel_kind_from_index, view::toggle_enlarged_panel};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};

thread_local! {
    /// The Diagram (2D) view's own last-clicked facet label, mirroring
    /// `gui::editor::callbacks::tier_actions::SELECTED_FACET_LABEL`'s
    /// Solid-viewport mechanism independently rather than sharing it -- this
    /// module deliberately has no dependency on `gui::editor` at all (see this
    /// file's own module doc comment), and the Solid/Diagram views are two
    /// separate rendered images with two separate pick buffers, so "the last
    /// clicked facet" is naturally a per-view concept: clicking a facet in one
    /// view has no reason to change what the other view's tooltip keeps reading
    /// after the pointer leaves it. UI-thread-only, same reasoning as
    /// `tier_actions::FACET_OVERLAY`'s own `thread_local!`.
    static DIAGRAM_SELECTED_FACET_LABEL: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The diagram's own per-pixel facet-picking buffer, one frame behind.
///
/// Same contract as `gui::mod`'s `solid_pick` for the ordinary Solid view --
/// written by `SlintSolidSink::apply` from `PreviewFrame::diagram_pick`.
///
/// The index wheel's own tooth-picking buffer (`PreviewFrame::
/// diagram_tooth_pick`) is threaded through as a SECOND value of this exact
/// same type -- see [`setup_diagram_hover_and_click_callbacks`]'s `tooth_pick`
/// parameter.
pub type DiagramPick = Arc<Mutex<Option<PickBuffer>>>;
/// Facet id -> hover tooltip text, from `PreviewFrame::diagram_hover_text`.
pub type DiagramHoverText = Arc<Mutex<Option<Vec<String>>>>;
/// Facet id -> owning tier index, from `PreviewFrame::diagram_facet_tier`.
pub type DiagramFacetTier = Arc<Mutex<Option<Vec<Option<usize>>>>>;

/// Looks up the index-wheel tooth (if any) at physical pixel `(px, py)` in
/// `tooth_pick` -- shared by both the hover and click callbacks in
/// [`setup_diagram_hover_and_click_callbacks`] below, and split out purely to
/// keep that function under clippy's function-length lint.
fn tooth_at(tooth_pick: &DiagramPick, px: u32, py: u32) -> Option<u32> {
    tooth_pick
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|tooth_pick| tooth_pick.facet_at(px, py))
}

/// Wires `editor_solid_diagram_facet_hover`/`_click`. Call once from
/// `gui::build_main_window`, alongside the ordinary Solid hover/click wiring.
///
/// `preview_state` drives the facet-id-keyed highlight overlay: every id
/// below already comes from a pick buffer, so no `Design`/`FacetMap` access is
/// needed to call [`SolidPreviewState::request_facet_overlay`] -- see that
/// function's own doc comment. A hover clears back to [`FacetOverlay::default`]
/// (nothing highlighted) exactly when it clears `diagram_hover_text`, so the
/// highlight and the tooltip always agree.
///
/// `x`/`y` are multiplied by `ui.window().scale_factor()` before indexing `pick`:
/// `render::camera_lighting::resubmit_at_current_pose` sizes a Diagram-mode
/// request's pixel buffers to the window's PHYSICAL resolution, but `x`/`y`
/// arrive from Slint in logical pixels (`solid_viewport.slint`'s
/// `drag_area.mouse-x`/`mouse-y`) -- see that function's own doc comment for the
/// matching output-side scaling. `gui::editor::callbacks::tier_actions`'s Solid-view
/// hover/click callbacks need the identical treatment for the Solid rasterizer's
/// own `pick`/`x`/`y`.
pub fn setup_diagram_hover_and_click_callbacks(
    ui: &MainWindow,
    pick: &DiagramPick,
    tooth_pick: &DiagramPick,
    hover_text: &DiagramHoverText,
    facet_tier: &DiagramFacetTier,
    preview_state: &Arc<SolidPreviewState>,
) {
    {
        let pick = Arc::clone(pick);
        let tooth_pick = Arc::clone(tooth_pick);
        let hover_text = Arc::clone(hover_text);
        let preview_state = Arc::clone(preview_state);
        let ui_weak = ui.as_weak();
        ui.global::<SolidPreviewModel>()
            .on_diagram_facet_hover(move |x: f32, y: f32| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let scale = ui.window().scale_factor();
                let px = (x * scale).max(0.0) as u32;
                let py = (y * scale).max(0.0) as u32;
                let Some(facet_id) = pick
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .and_then(|pick| pick.facet_at(px, py))
                else {
                    // Nothing under the cursor in facet space -- check the
                    // SAME `(px, py)` against the index wheel's own tooth buffer
                    // (`tooth_pick`, a `PickBuffer` reused for tooth ids; see
                    // `DiagramPick`'s own doc comment) before giving up, so
                    // hovering a wheel tick reports which tooth instead of
                    // reading as "nothing here".
                    let tooth = tooth_at(&tooth_pick, px, py);
                    // Neither a facet nor a tooth under the cursor falls back
                    // to the last CLICKED facet's own label (if any) instead of
                    // blanking the tooltip outright, so the selection stays
                    // readable once the pointer leaves it -- mirrors
                    // `tier_actions::setup_solid_facet_hover_callback`'s own
                    // miss branch for the Solid view.
                    let text = tooth.map_or_else(
                        || DIAGRAM_SELECTED_FACET_LABEL.with(|cell| cell.borrow().clone()),
                        |t| format!("Index {t}"),
                    );
                    ui.global::<SolidPreviewModel>()
                        .set_diagram_hover_text(text.into());
                    preview_state.request_facet_overlay(FacetOverlay::default());
                    return;
                };
                let text = hover_text
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .and_then(|table| table.get(facet_id as usize))
                    .cloned()
                    .unwrap_or_default();
                ui.global::<SolidPreviewModel>()
                    .set_diagram_hover_text(text.into());
                preview_state.request_facet_overlay(FacetOverlay {
                    hovered: Some(facet_id),
                    ..FacetOverlay::default()
                });
            });
    }

    {
        let pick = Arc::clone(pick);
        let tooth_pick = Arc::clone(tooth_pick);
        let facet_tier = Arc::clone(facet_tier);
        let hover_text = Arc::clone(hover_text);
        let preview_state = Arc::clone(preview_state);
        let ui_weak = ui.as_weak();
        ui.global::<SolidPreviewModel>()
            .on_diagram_facet_click(move |x: f32, y: f32| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                handle_diagram_facet_click(
                    &ui,
                    x,
                    y,
                    DiagramPickBuffers {
                        pick: &pick,
                        tooth_pick: &tooth_pick,
                        facet_tier: &facet_tier,
                        hover_text: &hover_text,
                    },
                    &preview_state,
                );
            });
    }
}

/// Wires `SolidPreviewModel.diagram_double_click`: enlarge the panel under the pointer.
///
/// A double-click on a panel enlarges it to fill the view, and a double-click while
/// a panel is enlarged (or on no panel) goes back to all three -- the same rule as
/// the web app, through `indicatrix_solid::preview::view::toggle_enlarged_panel`.
///
/// `panel_pick` is the last diagram frame's per-pixel panel buffer
/// (`PreviewFrame::diagram_panel_pick`), read with the same logical-to-physical
/// scaling as the hover and click callbacks above. Only the property is written
/// here: `SolidPreviewModel`'s `changed diagram_enlarged_panel` handler raises
/// `diagram_enlarged_panel_changed`, which `gui::editor` turns into the replan that
/// redraws the diagram (see `gui::editor::setup::setup_tier_cutoff_callback`), so a pill click, the
/// Escape key and this double-click all redraw through one path.
pub fn setup_diagram_double_click_callback(ui: &MainWindow, panel_pick: &DiagramPick) {
    let panel_pick = Arc::clone(panel_pick);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_diagram_double_click(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let scale = ui.window().scale_factor();
            let px = (x * scale).max(0.0) as u32;
            let py = (y * scale).max(0.0) as u32;
            let clicked = panel_pick
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .and_then(|buffer| buffer.facet_at(px, py))
                .and_then(|index| panel_kind_from_index(i32::try_from(index).ok()?));
            let model = ui.global::<SolidPreviewModel>();
            let next = toggle_enlarged_panel(model.get_diagram_enlarged_panel(), clicked);
            model.set_diagram_enlarged_panel(next);
        });
}

/// The Diagram view's shared pick-buffer/hover-text/facet-tier handles --
/// bundled (rather than four more parameters) purely to keep
/// [`handle_diagram_facet_click`] under clippy's argument-count lint.
#[derive(Clone, Copy)]
struct DiagramPickBuffers<'a> {
    pick: &'a DiagramPick,
    tooth_pick: &'a DiagramPick,
    facet_tier: &'a DiagramFacetTier,
    hover_text: &'a DiagramHoverText,
}

/// [`setup_diagram_hover_and_click_callbacks`]'s click handler body -- split
/// out purely to keep that function under clippy's function-length lint.
fn handle_diagram_facet_click(
    ui: &MainWindow,
    x: f32,
    y: f32,
    buffers: DiagramPickBuffers<'_>,
    preview_state: &SolidPreviewState,
) {
    let DiagramPickBuffers {
        pick,
        tooth_pick,
        facet_tier,
        hover_text,
    } = buffers;
    let scale = ui.window().scale_factor();
    let px = (x * scale).max(0.0) as u32;
    let py = (y * scale).max(0.0) as u32;
    let Some(facet_id) = pick
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|pick| pick.facet_at(px, py))
    else {
        // A click that misses every facet may still have landed on the
        // index wheel -- report which tooth (if any) through
        // `diagram_clicked_tooth` so a caller can act on it (e.g.
        // highlighting every facet that shares it). This function only
        // threads the buffer through and reports the id; wiring the
        // highlight itself lives in `gui::editor::callbacks::
        // tier_actions`/`EditorState`.
        let tooth = tooth_at(tooth_pick, px, py);
        if let Some(tooth) = tooth {
            let tooth = tooth as i32;
            ui.global::<SolidPreviewModel>()
                .set_diagram_clicked_tooth(tooth);
            // Also invoked directly, not left to `editor_tier_table.slint`'s
            // `tracked_clicked_tooth` mirror alone: that mirror's `changed`
            // handler is QUEUED and only fires when the property's value
            // actually differs from what it last saw, so clicking the SAME tooth twice
            // in a row -- a real, common gesture, e.g. re-confirming a
            // highlight after looking away -- would otherwise silently do
            // nothing the second time. `EditorModel.invoke_highlight_tooth`
            // is an ordinary synchronous call, so calling it here as well
            // costs nothing on the normal (tooth actually changed) path --
            // that mirror's own call runs again shortly after, redundantly
            // but harmlessly recomputing the identical highlight.
            ui.global::<EditorModel>().invoke_highlight_tooth(tooth);
            return;
        }
        // A click that misses the silhouette AND the wheel clears the
        // tier selection instead of doing nothing, giving a way back
        // to "nothing selected" beyond the inspector's "New" button.
        // Setting `selected_tier_index` alone is enough: `changed
        // selected_tier_index` in `models/editor.slint` fires
        // `selected_tier_changed`, which re-seeds/clears the inspector
        // form on the Rust side (see
        // `setup_solid_selected_tier_changed_callback`).
        preview_state.request_facet_overlay(FacetOverlay::default());
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        // Same direct-invocation reasoning as the tooth-hit branch above,
        // for the opposite edge: this also resets `diagram_clicked_tooth`
        // to its own "nothing clicked" default so a LATER re-click on the
        // very tooth that was highlighted before this miss still raises a
        // real value change (and so the mirror's own `changed` handler)
        // instead of silently no-op-ing against a stale, never-reset value.
        ui.global::<SolidPreviewModel>()
            .set_diagram_clicked_tooth(-1);
        ui.global::<EditorModel>().invoke_highlight_tooth(-1);
        // A miss also clears whatever facet was previously identified
        // -- nothing is selected any more, so nothing should keep
        // reading in the tooltip. Mirrors
        // `tier_actions::setup_solid_facet_click_callback`'s own miss
        // branch for the Solid view.
        DIAGRAM_SELECTED_FACET_LABEL.with(|cell| cell.borrow_mut().clear());
        ui.global::<SolidPreviewModel>()
            .set_diagram_hover_text("".into());
        return;
    };
    let tier_index = facet_tier
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|table| table.get(facet_id as usize).copied())
        .flatten();
    preview_state.request_facet_overlay(FacetOverlay {
        selected_facet: Some(facet_id),
        ..FacetOverlay::default()
    });
    if let Some(tier_index) = tier_index {
        ui.global::<EditorModel>()
            .set_selected_tier_index(tier_index as i32);
    }
    // Identifies the clicked facet itself, not just its owning tier --
    // reusing the SAME per-facet label the hover callback shows
    // transiently (`hover_text`), but kept in
    // `DIAGRAM_SELECTED_FACET_LABEL` so it survives the pointer
    // leaving the facet. Mirrors `tier_actions::
    // setup_solid_facet_click_callback`'s own Solid-view mechanism.
    let label = hover_text
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|table| table.get(facet_id as usize))
        .cloned()
        .unwrap_or_default();
    DIAGRAM_SELECTED_FACET_LABEL.with(|cell| cell.borrow_mut().clone_from(&label));
    ui.global::<SolidPreviewModel>()
        .set_diagram_hover_text(label.into());
}
