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
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError};

/// The diagram's own per-pixel facet-picking buffer, one frame behind.
///
/// Same contract as `gui::mod`'s `solid_pick` for the ordinary Solid view --
/// written by `SlintSolidSink::apply` from `PreviewFrame::diagram_pick`.
///
/// #121: the index wheel's own tooth-picking buffer (`PreviewFrame::
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
/// `preview_state` drives the facet-id-keyed highlight overlay (#18/#20): every id
/// below already comes from a pick buffer, so no `Design`/`FacetMap` access is
/// needed to call [`SolidPreviewState::request_facet_overlay`] -- see that
/// function's own doc comment. A hover clears back to [`FacetOverlay::default`]
/// (nothing highlighted) exactly when it clears `diagram_hover_text`, so the
/// highlight and the tooltip always agree.
///
/// `x`/`y` are multiplied by `ui.window().scale_factor()` before indexing `pick`
/// (#101): `render::camera_lighting::resubmit_at_current_pose` now sizes a
/// Diagram-mode request's pixel buffers to the window's PHYSICAL resolution, but
/// `x`/`y` arrive from Slint in logical pixels (`solid_viewport.slint`'s
/// `drag_area.mouse-x`/`mouse-y`) -- see that function's own doc comment for the
/// matching output-side scaling. `gui::editor::callbacks::tier_actions`'s Solid-view
/// hover/click callbacks need the identical treatment for the Solid rasterizer's
/// own `pick`/`x`/`y`; that half is out of this pass's file ownership.
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
                    // #121: nothing under the cursor in facet space -- check the
                    // SAME `(px, py)` against the index wheel's own tooth buffer
                    // (`tooth_pick`, a `PickBuffer` reused for tooth ids; see
                    // `DiagramPick`'s own doc comment) before giving up, so
                    // hovering a wheel tick reports which tooth instead of
                    // reading as "nothing here" the way it did before this
                    // buffer was threaded through.
                    let tooth = tooth_at(&tooth_pick, px, py);
                    ui.global::<SolidPreviewModel>()
                        .set_diagram_hovered_tooth(tooth.map_or(-1, |t| t as i32));
                    ui.global::<SolidPreviewModel>().set_diagram_hover_text(
                        tooth
                            .map_or_else(String::new, |t| format!("Index {t}"))
                            .into(),
                    );
                    preview_state.request_facet_overlay(FacetOverlay::default());
                    return;
                };
                ui.global::<SolidPreviewModel>()
                    .set_diagram_hovered_tooth(-1);
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
        let preview_state = Arc::clone(preview_state);
        let ui_weak = ui.as_weak();
        ui.global::<SolidPreviewModel>()
            .on_diagram_facet_click(move |x: f32, y: f32| {
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
                    // #121: a click that misses every facet may still have landed
                    // on the index wheel -- report which tooth (if any) through
                    // `diagram_clicked_tooth` so a future lane can act on it (e.g.
                    // highlighting every facet that shares it, per this crate's
                    // own `cad_todo.md` item 121's "click an index to find its
                    // facets"); this pass only threads the buffer through and
                    // reports the id, wiring the highlight is out of its file
                    // ownership (`gui::editor::callbacks::tier_actions`/
                    // `EditorState`).
                    let tooth = tooth_at(&tooth_pick, px, py);
                    if let Some(tooth) = tooth {
                        ui.global::<SolidPreviewModel>()
                            .set_diagram_clicked_tooth(tooth as i32);
                        return;
                    }
                    // #229: a click that misses the silhouette AND the wheel
                    // clears the tier selection instead of doing nothing -- the
                    // only other way back to "nothing selected" used to be the
                    // inspector's "New" button. Setting `selected_tier_index`
                    // alone is enough: `changed selected_tier_index` in
                    // `models/editor.slint` fires `selected_tier_changed`, which
                    // re-seeds/clears the inspector form on the Rust side (see
                    // `setup_solid_selected_tier_changed_callback`).
                    preview_state.request_facet_overlay(FacetOverlay::default());
                    ui.global::<EditorModel>().set_selected_tier_index(-1);
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
            });
    }
}
