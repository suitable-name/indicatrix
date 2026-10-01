//! Decides `RenderContext::tab_visible`: whether the live-rendered image is on screen
//! right now, and therefore whether the tracer (local or remote) should be running.
//!
//! The answer combines several inputs: the outer tab, the Live Render/Edit sub-tab,
//! each sub-tab's own Solid/Path-traced view-mode toggle, and one override -- while the
//! Retarget dialog is open (`RetargetModel.is_open`), its embedded comparison pane is
//! the only live view, so the viewport behind it is paused. [`render_is_visible`] is
//! the pure function encoding it, and every writer (the two Slint signals wired inside
//! [`setup_live_render_visibility_callbacks`], plus the two view-mode toggles' own
//! settings-persistence callbacks in `gui::main_window` and the Retarget open/close
//! handlers) recomputes it through the shared [`recompute_tab_visible`] helper, so no
//! call site's formula can ever drift out of sync with another's.

use crate::{
    MainWindow, RetargetModel, SolidPreviewModel, ViewportModel,
    bridge::render_thread::RenderContext, gui::solid_preview::preview_state::SolidPreviewState,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Whether the live-rendered image is visible anywhere right now -- the render
/// thread's `RenderContext::tab_visible` should be `true` exactly when this is. Pure
/// and unit-tested directly rather than only exercised indirectly through the Slint
/// callbacks that each recompute it.
///
/// The tracer only runs where it can actually be seen, which depends on the Live
/// Render/Edit sub-tab's OWN view-mode toggle, not just which sub-tab is selected:
/// - Live Render tab (`render_view_tab == 0`): visible only in Path-traced mode
///   (`live_view_mode == 1`, `ViewportModel.live_view_mode`). In Solid mode the
///   viewport shows the flat-shaded CPU rasterizer instead, and the spectral tracer
///   must not burn GPU/remote compute time on a frame nobody is looking at.
/// - Edit tab (`render_view_tab == 1`): visible in Path-traced (`1`) or Both (`2`)
///   mode (`SolidPreviewModel.view_mode`), not Solid (`0`) or Diagram (`3`) --
///   `render_view_tab == 1` alone must not suspend tracing unconditionally, or the
///   Edit tab's own "Path-traced"/"Both" pills would show a stale, frozen frame
///   instead of a live one.
///
/// `modal_compare_open` (the Retarget dialog, whose embedded comparison pane is the
/// only view of the change) overrides everything above: the viewport behind the
/// modal is hidden, so neither local tracing nor remote dispatch may run for it.
#[must_use]
const fn render_is_visible(
    active_tab: i32,
    render_view_tab: i32,
    live_view_mode: i32,
    solid_view_mode: i32,
    modal_compare_open: bool,
) -> bool {
    !modal_compare_open
        && active_tab == 0
        && ((render_view_tab == 0 && live_view_mode == 1)
            || (render_view_tab == 1 && (solid_view_mode == 1 || solid_view_mode == 2)))
}

/// Reads the current signals off `ui` and writes the result into
/// `render_ctx.tab_visible`. Called after every one of them changes, rather than each
/// callback keeping its own partial condition, which is exactly the kind of
/// duplication that could let one call site's formula silently disagree with another's.
///
/// `pub(in crate::gui)`: also called directly from `gui::main_window`'s
/// `on_live_view_mode_changed` handler and from its `SolidPreviewModel.
/// on_view_mode_changed` handler -- both view-mode toggles [`render_is_visible`]
/// depends on, alongside the two tab signals wired inside this module.
///
/// Also clears `RenderContext::camera_drag_held`: a tab, sub-tab or view-mode switch
/// mid-drag must never leave the preview gate closed.
pub(in crate::gui) fn recompute_tab_visible(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let visible = render_is_visible(
        ui.get_active_tab(),
        ui.get_render_view_tab(),
        ui.global::<ViewportModel>().get_live_view_mode(),
        ui.global::<SolidPreviewModel>().get_view_mode(),
        ui.global::<RetargetModel>().get_is_open(),
    );
    let mut ctx = RenderContext::lock(render_ctx);
    ctx.tab_visible = visible;
    // A view switch mid-drag must never leave the preview gate closed (the release
    // may go to a different, now-hidden viewport); the next press re-arms it.
    ctx.camera_drag_held = false;
}

/// Wires the outer `active_tab_changed` and the "Live Render"/"Edit" sub-tab change --
/// two of the four signals [`render_is_visible`] combines (the other two,
/// `ViewportModel.live_view_mode` and `SolidPreviewModel.view_mode`, are wired in
/// `gui::main_window` itself, since both already have their own settings-persistence
/// callback to hang [`recompute_tab_visible`] off).
///
/// `preview_state` is the solid-inspection preview controller -- the sub-tab change
/// handler re-issues the last solved plane set at the new tab's own pose/view-mode
/// (via `camera_lighting::resubmit_at_current_pose`) so switching tabs shows the right
/// variant immediately rather than whatever the previous tab last rendered.
pub(in crate::gui) fn setup_live_render_visibility_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx_at = render_ctx.clone();
    let ui_weak_at = ui.as_weak();
    ui.on_active_tab_changed(move |_tab| {
        if let Some(ui) = ui_weak_at.upgrade() {
            recompute_tab_visible(&ui, &render_ctx_at);
        }
    });

    let render_ctx_rv = render_ctx.clone();
    let preview_state_rv = Arc::clone(preview_state);
    let ui_weak_rv = ui.as_weak();
    ui.on_render_view_tab_changed(move |_idx| {
        if let Some(ui) = ui_weak_rv.upgrade() {
            recompute_tab_visible(&ui, &render_ctx_rv);
            // Re-request the right image for the newly selected sub-tab -- e.g.
            // switching Edit -> Live Render while both are in Solid mode must show the
            // Live tab's own (possibly differently sized) solid raster immediately,
            // not whatever the Edit tab last rendered. `resubmit_at_current_pose`
            // already encodes exactly this "which variant, and is it even visible"
            // logic (see its own doc comment), so this reuses it rather than
            // duplicating it.
            //
            // Deferred by one event-loop turn (`slint::Timer::single_shot`) rather
            // than called synchronously right here -- resubmitting immediately would
            // run before the newly instantiated `SolidViewportView`/`GemViewportView`
            // has pushed its own `changed width`/`changed height` size, so the very
            // first frame after a tab switch would be sized to whichever viewport's
            // size properties happen to be left over from before.
            let render_ctx_deferred = render_ctx_rv.clone();
            let preview_state_deferred = Arc::clone(&preview_state_rv);
            let ui_weak_deferred = ui.as_weak();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                let Some(ui) = ui_weak_deferred.upgrade() else {
                    return;
                };
                let ctx = render_ctx_deferred
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                crate::gui::render::camera_lighting::resubmit_at_current_pose(
                    &ui,
                    &ctx,
                    &preview_state_deferred,
                );
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_render_sub_tab_traces_only_in_path_traced_mode() {
        // Solid mode (0): the flat-shaded CPU raster is shown instead -- the spectral
        // tracer must not burn GPU/remote time on a frame nobody is looking at.
        assert!(!render_is_visible(0, 0, 0, 0, false));
        // Path-traced mode (1): visible.
        assert!(render_is_visible(0, 0, 1, 0, false));
    }

    #[test]
    fn edit_sub_tab_traces_only_in_path_traced_or_both_mode() {
        // Solid (0) and Diagram (3): no path-traced image is shown there, so tracing
        // stays suspended.
        assert!(!render_is_visible(0, 1, 1, 0, false));
        assert!(!render_is_visible(0, 1, 1, 3, false));
        // Path-traced (1) and Both (2): these pills trace LIVE, showing a fresh frame
        // rather than a stale, frozen one.
        assert!(render_is_visible(0, 1, 1, 1, false));
        assert!(render_is_visible(0, 1, 1, 2, false));
    }

    #[test]
    fn never_visible_outside_the_3d_tab() {
        for render_view_tab in [0, 1] {
            for live_view_mode in [0, 1] {
                for solid_view_mode in [0, 1, 2, 3] {
                    assert!(!render_is_visible(
                        1,
                        render_view_tab,
                        live_view_mode,
                        solid_view_mode,
                        false
                    ));
                    assert!(!render_is_visible(
                        2,
                        render_view_tab,
                        live_view_mode,
                        solid_view_mode,
                        false
                    ));
                }
            }
        }
    }

    #[test]
    fn an_open_modal_comparison_pauses_the_viewport_in_every_configuration() {
        // The Retarget dialog's embedded pane is the only live view: the viewport
        // behind the modal is paused whichever tab, sub-tab or view mode would
        // otherwise have kept it tracing.
        for active_tab in [0, 1, 2] {
            for render_view_tab in [0, 1] {
                for live_view_mode in [0, 1] {
                    for solid_view_mode in [0, 1, 2, 3] {
                        assert!(!render_is_visible(
                            active_tab,
                            render_view_tab,
                            live_view_mode,
                            solid_view_mode,
                            true
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn closing_the_modal_comparison_restores_the_prior_visibility() {
        // Path-traced Live Render tab: visible, paused while the modal is open,
        // visible again on close.
        assert!(render_is_visible(0, 0, 1, 0, false));
        assert!(!render_is_visible(0, 0, 1, 0, true));
        assert!(render_is_visible(0, 0, 1, 0, false));
    }
}
