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
//! main-window tab is selected, that became a three-way decision --
//! [`render_is_visible`] is the pure function encoding it, and
//! [`setup_live_render_visibility_callbacks`] wires all three Slint signals to
//! recompute it through that one function, so the three call sites can never drift
//! into disagreeing formulas.

use crate::{
    DetachedRenderWindow, MainWindow, ViewportModel, bridge::render_thread::RenderContext,
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
/// and unit-tested directly rather than only exercised indirectly through the three
/// Slint callbacks that each recompute it.
///
/// The detached window, once popped out, is visible regardless of which outer tab or
/// inner sub-tab is selected on the main window, so `detached` alone already answers
/// the question when `true`; the other two arguments only matter for the docked case.
#[must_use]
const fn render_is_visible(active_tab: i32, render_view_tab: i32, detached: bool) -> bool {
    detached || (active_tab == 0 && render_view_tab == 0)
}

/// Reads the three current signals off `ui` and writes the result into
/// `render_ctx.tab_visible`. Called after every one of them changes, rather than each
/// callback keeping its own partial condition, which is exactly the kind of
/// duplication that could let one call site's formula silently disagree with another's.
fn recompute_tab_visible(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let visible = render_is_visible(
        ui.get_active_tab(),
        ui.get_render_view_tab(),
        ui.get_live_render_detached(),
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
/// the outer `active_tab_changed` -- the three signals [`render_is_visible`] combines.
/// Returns the [`FrameTarget`] the render thread's own frame-push closure
/// (`gui::mod::build_main_window`) must also write into every frame; see this module's
/// doc comment's "Frame routing" section.
pub(in crate::gui) fn setup_live_render_visibility_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
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
    let ui_weak_rv = ui.as_weak();
    ui.on_render_view_tab_changed(move |_idx| {
        if let Some(ui) = ui_weak_rv.upgrade() {
            recompute_tab_visible(&ui, &render_ctx_rv);
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
    fn detached_is_visible_regardless_of_either_tab() {
        for active_tab in [0, 1, 2] {
            for render_view_tab in [0, 1] {
                assert!(render_is_visible(active_tab, render_view_tab, true));
            }
        }
    }

    #[test]
    fn docked_is_visible_only_on_the_3d_tabs_own_live_render_sub_tab() {
        assert!(render_is_visible(0, 0, false));
        assert!(!render_is_visible(0, 1, false), "Edit sub-tab, docked");
        assert!(!render_is_visible(1, 0, false), "Cutting Schedule tab");
        assert!(!render_is_visible(2, 0, false), "Files & Downloads tab");
    }

    #[test]
    fn docking_and_undocking_round_trips_visibility_for_every_outer_tab() {
        // A dock/undock toggle must be able to flip visibility on and off again for
        // every combination of the other two signals.
        for active_tab in [0, 1, 2] {
            for render_view_tab in [0, 1] {
                assert!(
                    render_is_visible(active_tab, render_view_tab, true),
                    "popped out must always be visible (active_tab={active_tab}, \
                     render_view_tab={render_view_tab})"
                );
                let docked = render_is_visible(active_tab, render_view_tab, false);
                assert_eq!(docked, active_tab == 0 && render_view_tab == 0);
            }
        }
    }
}
