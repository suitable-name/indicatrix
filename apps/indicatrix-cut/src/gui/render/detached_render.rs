//! Pops the live-rendered gemstone out of the "Live Render" sub-tab into its own
//! freely movable, always-on-top OS window, and back again.
//!
//! `DetachedRenderWindow` is a real top-level `Window` this module creates, shows,
//! hides, and re-shows via ordinary Slint component-handle calls -- see
//! `ui/detached_render_window.slint`'s doc comment for why that's a `Window`, not a
//! `PopupWindow` (an in-window overlay layer, not a second OS window).
//!
//! Always-on-top is not reachable through Slint's public `slint::Window` API, so it's
//! reached through the winit backend instead:
//! `slint::winit_030::WinitWindowAccessor::with_winit_window` hands back the
//! underlying `winit::window::Window`, whose `set_window_level` takes it from there.
//! That accessor only succeeds once the event loop is active, so
//! [`apply_always_on_top`] is only ever called from inside the pop-out toggle
//! callback (which by construction only fires once the event loop is already
//! pumping) and after `show()`; its `Option` return is checked and logged via `warn!`
//! rather than silently leaving a window that never actually went on top.
//!
//! One render thread, two possible display surfaces. Rather than have the render
//! thread know anything about detachment, `MainWindow`'s own `render_image`/
//! `has_render` stay the single always-updated destination, and this module hands the
//! frame-push closure an extra, optional mirror target: [`FrameTarget`], a shared
//! `Weak<DetachedRenderWindow>` also written into whenever it is `Some`. Both
//! destinations are kept fresh on every frame, not just the currently-visible one --
//! cheap (a `SharedPixelBuffer` clone is a refcounted pointer bump, not a pixel copy)
//! and what prevents a stale/frozen frame from being the first thing shown on either
//! side of a dock/undock.
//!
//! `RenderContext::tab_visible` used to be driven by one signal (`active_tab == 0`).
//! With the render now visible in a separate OS window regardless of which
//! main-window tab is selected, and with the Live Render/Edit sub-tabs each gaining
//! their own Solid/Path-traced view-mode toggle, this became a five-way decision --
//! [`render_is_visible`] is the pure function encoding it, and every writer (three
//! Slint signals wired inside [`setup_live_render_visibility_callbacks`], plus the two
//! view-mode toggles' own settings-persistence callbacks in `gui::mod`) recomputes it
//! through the shared [`recompute_tab_visible`] helper, so no call site's formula can
//! ever drift out of sync with another's.

use crate::{
    DetachedRenderWindow, MainWindow, SolidPreviewModel, ViewportModel,
    bridge::render_thread::RenderContext, gui::solid_preview::preview_state::SolidPreviewState,
};
use slint::{
    ComponentHandle, Weak,
    winit_030::{WinitWindowAccessor, winit::window::WindowLevel},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::warn;

/// Shared with the render thread's frame-push closure -- see this module's doc
/// comment. `Some(weak)` exactly while the detached window is popped out, `None`
/// while docked. A plain type alias rather than a newtype: this is purely a shared
/// mutable cell, not a type with any behaviour of its own.
pub(super) type FrameTarget = Arc<Mutex<Option<Weak<DetachedRenderWindow>>>>;

/// Whether the live-rendered image is visible anywhere right now -- the render
/// thread's `RenderContext::tab_visible` should be `true` exactly when this is. Pure
/// and unit-tested directly rather than only exercised indirectly through the five
/// Slint callbacks that each recompute it.
///
/// The detached window, once popped out, is visible regardless of which outer tab or
/// inner sub-tab is selected on the main window, so `detached` alone already answers
/// the question when `true`; the other four arguments only matter for the docked case.
///
/// Docked, the tracer only runs where it can actually be seen -- and that now depends
/// on the Live Render/Edit sub-tab's OWN view-mode toggle, not just which sub-tab is
/// selected:
/// - Live Render tab (`render_view_tab == 0`): visible only in Path-traced mode
///   (`live_view_mode == 1`, `ViewportModel.live_view_mode`). In Solid mode the
///   viewport shows the flat-shaded CPU rasterizer instead, and the spectral tracer
///   must not burn GPU/remote compute time on a frame nobody is looking at.
/// - Edit tab (`render_view_tab == 1`): visible in Path-traced (`1`) or Both (`2`)
///   mode (`SolidPreviewModel.view_mode`), not Solid (`0`) or Diagram (`3`). Before
///   this gate existed, `render_view_tab == 1` alone suspended tracing unconditionally
///   -- so the Edit tab's own "Path-traced"/"Both" pills showed a stale, frozen frame
///   instead of a live one. This makes them trace live, matching what they claim to
///   show.
#[must_use]
const fn render_is_visible(
    active_tab: i32,
    render_view_tab: i32,
    detached: bool,
    live_view_mode: i32,
    solid_view_mode: i32,
) -> bool {
    detached
        || (active_tab == 0
            && ((render_view_tab == 0 && live_view_mode == 1)
                || (render_view_tab == 1 && (solid_view_mode == 1 || solid_view_mode == 2))))
}

/// Reads the current signals off `ui` and writes the result into
/// `render_ctx.tab_visible`. Called after every one of them changes, rather than each
/// callback keeping its own partial condition, which is exactly the kind of
/// duplication that could let one call site's formula silently disagree with another's.
///
/// `pub(in crate::gui)`: also called directly from `gui::mod`'s
/// `on_live_view_mode_changed` handler and from its `SolidPreviewModel.
/// on_view_mode_changed` handler -- both view-mode toggles [`render_is_visible`] now
/// depends on, alongside the three tab/detach signals wired inside this module.
pub(in crate::gui) fn recompute_tab_visible(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let visible = render_is_visible(
        ui.get_active_tab(),
        ui.get_render_view_tab(),
        ui.get_live_render_detached(),
        ui.global::<ViewportModel>().get_live_view_mode(),
        ui.global::<SolidPreviewModel>().get_view_mode(),
    );
    render_ctx.lock().unwrap().tab_visible = visible;
}

/// Applies always-on-top to `win`'s underlying OS window -- see this module's doc
/// comment for the ordering constraint this relies on its caller already satisfying
/// (called after `show()`, from a Slint callback). Logs a `warn!` rather than failing
/// silently whenever the winit window isn't reachable.
fn apply_always_on_top(win: &DetachedRenderWindow) {
    let applied = win
        .window()
        .with_winit_window(|winit_window| {
            winit_window.set_window_level(WindowLevel::AlwaysOnTop);
        })
        .is_some();
    if !applied {
        warn!(
            "Could not set the detached Live Render window always-on-top -- no winit \
             window was available (wrong backend, or the event loop isn't running yet); \
             the window will show but may not stay above the main window."
        );
    }
}

/// Docks the live render back into the main window's "Live Render" sub-tab: hides the
/// detached window (never destroys it -- see [`open_detached_window`]'s doc comment
/// for why it's kept alive across toggles), clears the frame-routing target so the
/// render thread stops mirroring into a hidden window, reflects the new state into
/// `MainWindow.live_render_detached`, and recomputes overall visibility. Shared by both
/// ways docking back can happen: the window's own "Dock Back" button, and closing it
/// via its OS titlebar X.
fn dock_back(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    window_slot: &Rc<RefCell<Option<DetachedRenderWindow>>>,
    frame_target: &FrameTarget,
) {
    if let Some(win) = window_slot.borrow().as_ref()
        && let Err(e) = win.hide()
    {
        warn!("Failed to hide the detached Live Render window: {e}");
    }
    *frame_target.lock().unwrap() = None;
    ui.set_live_render_detached(false);
    recompute_tab_visible(ui, render_ctx);
}

/// Pops the live render out: creates the detached window on first use (kept alive and
/// merely hidden/shown on every subsequent toggle, rather than destroyed and
/// recreated -- a real OS/winit window is comparatively expensive to spin up, and
/// re-creating it would mean re-registering `on_close_requested`/`on_redock` every
/// single toggle for no benefit), seeds it with the main window's own current frame so
/// there is no blank flash before the render thread's next cycle lands, shows it,
/// applies always-on-top, and points the frame-routing target at it.
fn open_detached_window(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    window_slot: &Rc<RefCell<Option<DetachedRenderWindow>>>,
    frame_target: &FrameTarget,
) {
    if window_slot.borrow().is_none() {
        let win = match DetachedRenderWindow::new() {
            Ok(win) => win,
            Err(e) => {
                warn!("Failed to create the detached Live Render window: {e}");
                ui.set_live_render_detached(false);
                return;
            }
        };

        // Closing via the OS titlebar X must dock the view back, not leave the render
        // nowhere -- see this module's doc comment.
        let ui_weak = ui.as_weak();
        let render_ctx_cr = render_ctx.clone();
        let window_slot_cr = window_slot.clone();
        let frame_target_cr = frame_target.clone();
        win.window().on_close_requested(move || {
            if let Some(ui) = ui_weak.upgrade() {
                dock_back(&ui, &render_ctx_cr, &window_slot_cr, &frame_target_cr);
            }
            slint::CloseRequestResponse::HideWindow
        });

        // The window's own "Dock Back" button -- same effect as the titlebar X above.
        let ui_weak_rd = ui.as_weak();
        let render_ctx_rd = render_ctx.clone();
        let window_slot_rd = window_slot.clone();
        let frame_target_rd = frame_target.clone();
        win.on_redock(move || {
            if let Some(ui) = ui_weak_rd.upgrade() {
                dock_back(&ui, &render_ctx_rd, &window_slot_rd, &frame_target_rd);
            }
        });

        *window_slot.borrow_mut() = Some(win);
    }

    let slot = window_slot.borrow();
    let win = slot
        .as_ref()
        .expect("just created or already present above");

    // Seed the detached window with whatever the docked view is already showing, so
    // there's no blank flash before the render thread's next frame lands.
    win.set_render_image(ui.global::<ViewportModel>().get_render_image());
    win.set_has_render(ui.global::<ViewportModel>().get_has_render());

    if let Err(e) = win.show() {
        warn!("Failed to show the detached Live Render window: {e}");
        ui.set_live_render_detached(false);
        return;
    }
    apply_always_on_top(win);

    *frame_target.lock().unwrap() = Some(win.as_weak());
    ui.set_live_render_detached(true);
    recompute_tab_visible(ui, render_ctx);
}

/// Wires the "Live Render"/"Edit" sub-tab change, the pop-out toggle, and (moved here
/// from `gui::material_quality`, see that module's own comment at the old call site)
/// the outer `active_tab_changed` -- three of the five signals [`render_is_visible`]
/// combines (the other two, `ViewportModel.live_view_mode` and `SolidPreviewModel.
/// view_mode`, are wired in `gui::mod` itself, since both already have their own
/// settings-persistence callback to hang `recompute_tab_visible` off). Returns the
/// [`FrameTarget`] the render thread's own frame-push closure
/// (`gui::mod::build_main_window`) must also write into every frame; see this module's
/// doc comment's "Frame routing" section.
///
/// `preview_state` is B2's solid-inspection preview controller -- the sub-tab change
/// handler re-issues the last solved plane set at the new tab's own pose/view-mode
/// (via `camera_lighting::resubmit_at_current_pose`) so switching tabs shows the right
/// variant immediately rather than whatever the previous tab last rendered.
pub(in crate::gui) fn setup_live_render_visibility_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
) -> FrameTarget {
    let frame_target: FrameTarget = Arc::new(Mutex::new(None));
    let window_slot: Rc<RefCell<Option<DetachedRenderWindow>>> = Rc::new(RefCell::new(None));

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
            // #26: deferred by one event-loop turn (`slint::Timer::single_shot`)
            // rather than called synchronously right here -- this handler used to
            // resubmit before the newly instantiated `SolidViewportView`/
            // `GemViewportView` had pushed its own `changed width`/`changed height`
            // size, so the very first frame after a tab switch was sized to
            // whichever viewport's size properties happened to be left over from
            // before.
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

    let render_ctx_dt = render_ctx.clone();
    let ui_weak_dt = ui.as_weak();
    let window_slot_dt = window_slot;
    let frame_target_dt = frame_target.clone();
    ui.on_live_render_detach_toggled(move |want_detached: bool| {
        let Some(ui) = ui_weak_dt.upgrade() else {
            return;
        };
        if want_detached {
            open_detached_window(&ui, &render_ctx_dt, &window_slot_dt, &frame_target_dt);
        } else {
            dock_back(&ui, &render_ctx_dt, &window_slot_dt, &frame_target_dt);
        }
    });

    frame_target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_is_visible_regardless_of_either_tab_or_either_view_mode() {
        for active_tab in [0, 1, 2] {
            for render_view_tab in [0, 1] {
                for live_view_mode in [0, 1] {
                    for solid_view_mode in [0, 1, 2, 3] {
                        assert!(render_is_visible(
                            active_tab,
                            render_view_tab,
                            true,
                            live_view_mode,
                            solid_view_mode
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn docked_live_render_sub_tab_traces_only_in_path_traced_mode() {
        // Solid mode (0): the flat-shaded CPU raster is shown instead -- the spectral
        // tracer must not burn GPU/remote time on a frame nobody is looking at.
        assert!(!render_is_visible(0, 0, false, 0, 0));
        // Path-traced mode (1): visible, matching this tab's sole behaviour before
        // `live_view_mode` existed.
        assert!(render_is_visible(0, 0, false, 1, 0));
    }

    #[test]
    fn docked_edit_sub_tab_traces_only_in_path_traced_or_both_mode() {
        // Solid (0) and Diagram (3): no path-traced image is shown there, so tracing
        // stays suspended -- unchanged from before this gate existed.
        assert!(!render_is_visible(0, 1, false, 1, 0));
        assert!(!render_is_visible(0, 1, false, 1, 3));
        // Path-traced (1) and Both (2): these pills must now trace LIVE rather than
        // showing whatever frame was last rendered before the Edit tab suspended it.
        assert!(render_is_visible(0, 1, false, 1, 1));
        assert!(render_is_visible(0, 1, false, 1, 2));
    }

    #[test]
    fn docked_is_never_visible_outside_the_3d_tab() {
        for render_view_tab in [0, 1] {
            for live_view_mode in [0, 1] {
                for solid_view_mode in [0, 1, 2, 3] {
                    assert!(!render_is_visible(
                        1,
                        render_view_tab,
                        false,
                        live_view_mode,
                        solid_view_mode
                    ));
                    assert!(!render_is_visible(
                        2,
                        render_view_tab,
                        false,
                        live_view_mode,
                        solid_view_mode
                    ));
                }
            }
        }
    }

    #[test]
    fn docking_and_undocking_round_trips_visibility_for_every_outer_tab() {
        // A dock/undock toggle must be able to flip visibility on and off again for
        // every combination of the other signals -- pinned here at the "always
        // visible when docked" corner of each toggle (Path-traced / Both).
        for active_tab in [0, 1, 2] {
            for render_view_tab in [0, 1] {
                assert!(
                    render_is_visible(active_tab, render_view_tab, true, 1, 1),
                    "popped out must always be visible (active_tab={active_tab}, \
                     render_view_tab={render_view_tab})"
                );
                let docked = render_is_visible(active_tab, render_view_tab, false, 1, 1);
                assert_eq!(docked, active_tab == 0);
            }
        }
    }
}
