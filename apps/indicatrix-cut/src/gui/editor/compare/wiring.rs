//! The compare surfaces' Slint wiring: the entry callbacks on `MainWindow`'s own
//! `CompareModel`, opening a session on the pop-out window or the Retarget dialog's
//! embedded pane, the view callbacks both share, and Keep/Discard/Close.
//!
//! The surfaces, the one live session and frame delivery live in [`super::host`];
//! this module decides WHICH surface a request opens on:
//!
//! - "Compare…", the Optimize tab and the snapshot table open the pop-out window;
//! - the Retarget dialog asks for its embedded pane (`open_retarget_embedded`) after
//!   every solve -- unless the session was popped out into the window, in which case
//!   the window is refreshed instead and the pane keeps saying so;
//! - there is only ever ONE live session: opening replaces it, wherever it was.
//!
//! When the pop-out window of a Retarget session is closed (Close, Discard or the
//! titlebar X) while the dialog is still open, the pane takes the session back.

use super::{
    host::{self, NewLive, POPPED_OUT_STATUS, Surface, SurfaceHandle, TRACED_DEBOUNCE},
    origins::{self, KeepGuard, OpenRequest},
    render,
    session::{self, CompareOrigin, CompareSession, Renderer},
};
use crate::{
    CompareModel, CompareWindow, EditorModel, MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{callbacks, state::EditorState, view::SolidLastSolved},
        show_toast,
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
    },
};
use slint::{ComponentHandle, Image, Weak};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tracing::warn;

/// The editor handles Keep/Discard need, captured once.
#[derive(Clone)]
pub(super) struct CompareDeps {
    /// The main window.
    pub(super) ui: Weak<MainWindow>,
    /// The editor state (read only here -- Keep goes through the Apply handlers).
    pub(super) state: Rc<RefCell<EditorState>>,
    /// The render context (custom materials, the live camera pose).
    pub(super) render_ctx: Arc<Mutex<RenderContext>>,
    /// The shared solid viewport, for Retarget's Discard revert.
    pub(super) preview_state: Arc<SolidPreviewState>,
    /// The shared solve cache, for Retarget's Discard revert.
    pub(super) solid_last_solved: SolidLastSolved,
}

/// Closes the compare window and the embedded pane if they are open -- called
/// wherever the main window hides, so a leftover compare window never keeps the
/// event loop (and the process) alive after the main window is gone.
pub(in crate::gui) fn close_compare_window() {
    host::close_embedded_session();
    host::close_window(true);
}

/// The Retarget dialog is still open after its popped-out session went away: the
/// pane takes the (current) proposal back.
fn restore_embedded_if_open(ui: &MainWindow) {
    if ui.global::<RetargetModel>().get_is_open() {
        ui.global::<CompareModel>().invoke_open_retarget_embedded();
    }
}

/// Closes the window session (`hide` is `false` from the titlebar close, which
/// hides the window itself) and, for a Retarget session, hands it back to the
/// dialog's pane.
fn close_window_and_restore(deps: &CompareDeps, hide: bool) {
    let origin = host::close_window(hide);
    if origin == Some(CompareOrigin::Retarget)
        && let Some(ui) = deps.ui.upgrade()
    {
        restore_embedded_if_open(&ui);
    }
}

/// "Keep after": re-checks that the originating feature would still commit what
/// was compared, closes the window, then invokes that feature's OWN Apply handler
/// (`RetargetModel.apply()` / `EditorModel.optimize_apply()`), so undo, dirty and
/// generation bookkeeping are exactly the Apply button's.
fn on_keep(deps: &CompareDeps) {
    let Some(ui) = deps.ui.upgrade() else {
        return;
    };
    let Some(guard) = host::keep_guard() else {
        return;
    };
    let still_current = {
        let Ok(st) = deps.state.try_borrow() else {
            return;
        };
        origins::guard_still_matches(&guard, &ui, &st, &deps.render_ctx)
    };
    if !still_current {
        show_toast(
            &ui,
            "The change you compared is no longer the pending one -- open Compare again to \
             see the current one.",
            "warning",
        );
        return;
    }
    host::close_window(true);
    match guard {
        KeepGuard::Retarget(_) => ui.global::<RetargetModel>().invoke_apply(),
        KeepGuard::Optimize { .. } => ui.global::<EditorModel>().invoke_optimize_apply(),
        KeepGuard::Snapshot => {}
    }
}

/// "Discard": closes the window and turns the originating feature's viewport
/// ghost off -- Retarget's through the dialog's own revert, Optimize's "Preview"
/// checkbox through its own toggle handler (and a serial bump the inspector watches
/// to untick the box). The pending proposal/result itself stays, so the cutter can
/// adjust it and compare again.
fn on_discard(deps: &CompareDeps) {
    let origin = host::close_window(true);
    let Some(ui) = deps.ui.upgrade() else {
        return;
    };
    match origin {
        Some(CompareOrigin::Retarget) => {
            if let Ok(st) = deps.state.try_borrow() {
                callbacks::discard_retarget_preview(
                    &ui,
                    &deps.render_ctx,
                    &deps.preview_state,
                    &deps.solid_last_solved,
                    &st,
                );
            }
            restore_embedded_if_open(&ui);
        }
        Some(CompareOrigin::Optimize) => {
            let model = ui.global::<CompareModel>();
            model.set_optimize_preview_reset_serial(
                model.get_optimize_preview_reset_serial().wrapping_add(1),
            );
            ui.global::<EditorModel>()
                .invoke_optimize_preview_toggled(false);
        }
        Some(CompareOrigin::Snapshot) | None => {}
    }
}

/// Registers the view callbacks both surfaces share on `model`: orbit, zoom, reset,
/// the drag release rule, and the layout, renderer and size switches. Each acts only
/// while the live session sits on `surface`.
fn register_view_callbacks(model: &CompareModel<'_>, surface: Surface) {
    model.on_orbit(move |dx, dy| {
        host::change_pose(surface, |pose, _| session::orbit(pose, dx, dy));
    });
    model.on_zoom(move |delta| {
        host::change_pose(surface, |pose, radius| session::zoom(pose, delta, radius));
    });
    model.on_reset_pose(move || {
        host::change_pose(surface, |_, radius| session::default_pose(radius));
    });
    model.on_drag_begin(move || host::set_drag(surface, true));
    model.on_drag_end(move || host::set_drag(surface, false));
    // The slot size changes with the layout; `size_changed` follows on its own.
    model.on_mode_changed(move |_| {
        if host::is_live_on(surface) {
            host::request_frames(TRACED_DEBOUNCE);
        }
    });
    model.on_renderer_changed(move |index| {
        if host::set_renderer(surface, Renderer::from_index(index)) {
            host::request_frames(Duration::ZERO);
        }
    });
    model.on_size_changed(move |width, height| {
        let Some(handle) = host::handle(surface) else {
            return;
        };
        let size = render::pixel_size(width, height, handle.scale_factor());
        if host::set_size(surface, size) {
            host::request_frames(TRACED_DEBOUNCE);
        }
    });
}

/// Registers the window's own callbacks -- once, when the window is created.
fn register_window_callbacks(window: &CompareWindow, deps: &CompareDeps) {
    let model = window.global::<CompareModel>();
    register_view_callbacks(&model, Surface::Window);
    let keep_deps = deps.clone();
    model.on_keep(move || on_keep(&keep_deps));
    let discard_deps = deps.clone();
    model.on_discard(move || on_discard(&discard_deps));
    let close_deps = deps.clone();
    model.on_close(move || close_window_and_restore(&close_deps, true));
    // The titlebar X is "Close": the proposal stays pending, nothing is reverted.
    let x_deps = deps.clone();
    window.window().on_close_requested(move || {
        close_window_and_restore(&x_deps, false);
        slint::CloseRequestResponse::HideWindow
    });
}

/// The window, created (and its callbacks registered) on first use.
fn ensure_window(deps: &CompareDeps) -> Option<CompareWindow> {
    if let Some(existing) = host::window() {
        return Some(existing);
    }
    let window = match CompareWindow::new() {
        Ok(window) => window,
        Err(error) => {
            warn!("Could not create the compare window: {error}");
            return None;
        }
    };
    register_window_callbacks(&window, deps);
    host::store_window(&window);
    Some(window)
}

/// The live viewport's camera pose -- the first view of a session that has no
/// previous session to continue from.
fn viewport_pose(deps: &CompareDeps) -> CameraPose {
    let ctx = deps
        .render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    CameraPose {
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
    }
}

/// A session moves from the pane to the window: the window starts with the layout
/// and renderer the pane was showing.
fn carry_view_settings(window: &SurfaceHandle) {
    let Some(pane) = host::handle(Surface::Embedded) else {
        return;
    };
    let (mode, renderer) =
        pane.with_model(|model| (model.get_mode_index(), model.get_renderer_index()));
    window.with_model(|model| {
        model.set_mode_index(mode);
        model.set_renderer_index(renderer);
    });
}

/// The surface `old` was on is no longer the live one: empties it, the window
/// hidden, the pane saying where its session went.
fn retire_previous_surface(old: Option<Surface>, new: Surface) {
    match (old, new) {
        (Some(Surface::Window), Surface::Embedded) => {
            host::close_window(true);
        }
        (Some(Surface::Embedded), Surface::Window) => {
            if let Some(pane) = host::handle(Surface::Embedded) {
                host::reset_surface(&pane, POPPED_OUT_STATUS);
            }
        }
        _ => {}
    }
}

/// The handle to open a session on: the window is created on first use.
fn target_handle(deps: &CompareDeps, surface: Surface) -> Option<SurfaceHandle> {
    match surface {
        Surface::Window => ensure_window(deps).map(SurfaceHandle::Window),
        Surface::Embedded => host::handle(surface),
    }
}

/// Opens (or re-targets) `surface` on `request`: shows it at once with a
/// "Solving…" status, and solves both sides on a background thread -- a real
/// design can take seconds to solve, which must never freeze the UI. Replaces the
/// live session wherever it was; a session continuing on the same or another
/// surface keeps its camera.
fn open_session(deps: &CompareDeps, surface: Surface, request: OpenRequest) {
    let Some(target) = target_handle(deps, surface) else {
        return;
    };
    if surface == Surface::Window
        && host::live_kind().is_some_and(|(live, _)| live == Surface::Embedded)
    {
        carry_view_settings(&target);
    }
    let OpenRequest {
        origin,
        guard,
        before,
        after,
    } = request;
    let scale = target.scale_factor();
    // A refresh on the same surface keeps the previous frames on screen until the
    // new ones land, rather than flashing the pane empty on every re-solve.
    let refresh = host::live_kind().is_some_and(|(live, _)| live == surface);
    let (renderer, size_px) = target.with_model(|model| {
        model.set_before_label(before.label.clone().into());
        model.set_after_label(after.label.clone().into());
        if !refresh {
            model.set_before_image(Image::default());
            model.set_after_image(Image::default());
            model.set_overlay_image(Image::default());
        }
        model.set_can_keep(false);
        model.set_show_keep_discard(origin.offers_keep());
        model.set_split_fraction(session::clamp_split_fraction(model.get_split_fraction()));
        (
            Renderer::from_index(model.get_renderer_index()),
            render::pixel_size(model.get_image_width(), model.get_image_height(), scale),
        )
    });
    let (id, old) = host::replace_live(NewLive {
        surface,
        origin,
        guard,
        renderer,
        size_px,
    });
    let pose = old
        .as_ref()
        .and_then(host::LiveSession::pose)
        .unwrap_or_else(|| viewport_pose(deps));
    retire_previous_surface(old.as_ref().map(host::LiveSession::surface), surface);
    // The replaced session (workers, timer) is dropped with the cell released.
    drop(old);
    target.with_model(|model| {
        model.set_status(host::solving_status(renderer).into());
        model.set_is_open(true);
        model.set_embedded_open(surface == Surface::Embedded);
    });
    if let SurfaceHandle::Window(window) = &target
        && let Err(error) = window.show()
    {
        warn!("Could not show the compare window: {error}");
        host::close_window(false);
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("compare-solve".to_string())
        .spawn(move || {
            let session = CompareSession::build(before, after, origin, pose);
            let _ = slint::invoke_from_event_loop(move || host::on_session_built(id, session));
        });
    if let Err(error) = spawned {
        warn!("Could not start the compare solve thread: {error}");
    }
}

/// Builds `origin`'s request from the live editor state and opens it on `surface`,
/// or reports why there is nothing to compare: a toast for the window, the pane's
/// own status line for the embedded pane (which refreshes unasked).
fn open_from(
    deps: &CompareDeps,
    surface: Surface,
    build: impl FnOnce(
        &MainWindow,
        &EditorState,
        &Arc<Mutex<RenderContext>>,
    ) -> Result<OpenRequest, String>,
) {
    let Some(ui) = deps.ui.upgrade() else {
        return;
    };
    let request = {
        // A writer mid-edit holds the state: skip this click rather than panic.
        let Ok(st) = deps.state.try_borrow() else {
            return;
        };
        build(&ui, &st, &deps.render_ctx)
    };
    match request {
        Ok(request) => open_session(deps, surface, request),
        Err(reason) if surface == Surface::Embedded => host::drop_embedded(&reason),
        Err(reason) => show_toast(&ui, &reason, "info"),
    }
}

/// The Retarget dialog's pane (re)opens on the current proposal -- on the window
/// instead when this Retarget session has been popped out into it.
fn open_retarget_embedded(deps: &CompareDeps) {
    let surface = if host::live_kind() == Some((Surface::Window, CompareOrigin::Retarget)) {
        Surface::Window
    } else {
        Surface::Embedded
    };
    open_from(deps, surface, origins::retarget_request);
}

/// Wires `MainWindow`'s own `CompareModel` instance: the three pop-out entry points
/// (the Retarget dialog's and the Optimize tab's "Compare…" buttons and the snapshot
/// table's "Compare visually…"), and the embedded pane's view callbacks and
/// open/close handlers.
pub(super) fn setup_entry_callbacks(ui: &MainWindow, deps: &CompareDeps) {
    host::set_main_window(ui);
    let model = ui.global::<CompareModel>();
    let retarget_deps = deps.clone();
    model.on_open_retarget(move || {
        open_from(&retarget_deps, Surface::Window, origins::retarget_request);
    });
    let optimize_deps = deps.clone();
    model.on_open_optimize(move || {
        open_from(&optimize_deps, Surface::Window, |_, st, render_ctx| {
            origins::optimize_request(st, render_ctx)
        });
    });
    let snapshot_deps = deps.clone();
    model.on_open_snapshot(move || {
        open_from(&snapshot_deps, Surface::Window, |_, st, render_ctx| {
            origins::snapshot_request(st, render_ctx)
        });
    });
    register_view_callbacks(&model, Surface::Embedded);
    let embedded_deps = deps.clone();
    model.on_open_retarget_embedded(move || open_retarget_embedded(&embedded_deps));
    model.on_close_embedded(host::close_embedded_session);
}
