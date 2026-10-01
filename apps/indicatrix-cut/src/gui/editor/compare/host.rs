//! The compare surfaces and the one live session -- everything between the render
//! workers and Slint.
//!
//! # Two surfaces, one live session, UI thread only
//!
//! A comparison is shown on a [`Surface`]: the pop-out `CompareWindow`, or the
//! comparison pane embedded in the Retarget dialog (the main window's own
//! `CompareModel` instance -- Slint instantiates every global once per top-level
//! window). [`COMPARE`] holds the window (created on first use, then only hidden and
//! re-shown -- a real OS window is expensive to spin up, so it is kept), a weak handle to the main window, and the
//! ONE live session, which sits on exactly one surface at a time. A `thread_local!`
//! for the same reason `callbacks::retarget_actions::RETARGET_ASYNC` is one: Slint's
//! event loop is single-threaded. Opening while a session is live replaces it --
//! dropping the old session drops its workers (their threads end) and its timer,
//! and its late frames are rejected on their session id.
//!
//! Every borrow of [`COMPARE`] here is short and never spans a call that could
//! re-enter this module: model setters, `show`/`hide`, and the originating feature's
//! `invoke_*` handlers all run with the cell released.
//!
//! # The release rule
//!
//! The instant solid pair (and the difference overlay riding on it) follows an orbit
//! drag live; the traced pair never starts while the mouse button is held
//! ([`set_drag`], [`session::traced_may_start`]) -- it starts `TRACED_DEBOUNCE`
//! after the release, or after the last wheel tick.

use super::{
    origins::KeepGuard,
    render::{
        self, FrameEvent, FrameMsg, LatestWorker, Pixels, SideGeometry, TRACED_SPP, ViewRequest,
    },
    session::{
        self, CompareOrigin, CompareSession, FrameBook, FrameKind, Renderer, TracedProgress,
    },
};
use crate::{
    CompareModel, CompareWindow, MainWindow, gui::solid_preview::preview_state::CameraPose,
};
use slint::{ComponentHandle, Image, Timer, TimerMode, Weak};
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tracing::warn;

/// How long the pose must stay still before a traced pair is started -- orbiting
/// shows the instant solid pair meanwhile.
pub(super) const TRACED_DEBOUNCE: Duration = Duration::from_millis(250);

/// The embedded pane's status line while its session is shown in the pop-out window.
pub(super) const POPPED_OUT_STATUS: &str = "Shown in the Compare window";

/// `CompareModel.mode_index` of the difference-overlay layout.
const MODE_OVERLAY: i32 = 2;

thread_local! {
    /// The window, the main-window handle and the live session -- see this module's
    /// doc comment.
    static COMPARE: RefCell<Host> = const { RefCell::new(Host::new()) };
}

/// Where a comparison is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    /// The pop-out `CompareWindow`.
    Window,
    /// The pane embedded in the Retarget dialog (the main window's own model).
    Embedded,
}

/// A strong handle to a [`Surface`]'s top-level window, so its `CompareModel`
/// instance can be reached with [`SurfaceHandle::with_model`].
pub(super) enum SurfaceHandle {
    /// The pop-out window.
    Window(CompareWindow),
    /// The main window, whose own `CompareModel` instance backs the embedded pane.
    Embedded(MainWindow),
}

impl SurfaceHandle {
    /// Runs `f` on this surface's own `CompareModel` instance.
    pub(super) fn with_model<R>(&self, f: impl FnOnce(CompareModel<'_>) -> R) -> R {
        match self {
            Self::Window(window) => f(window.global::<CompareModel>()),
            Self::Embedded(ui) => f(ui.global::<CompareModel>()),
        }
    }

    /// The window's device scale factor.
    pub(super) fn scale_factor(&self) -> f32 {
        match self {
            Self::Window(window) => window.window().scale_factor(),
            Self::Embedded(ui) => ui.window().scale_factor(),
        }
    }
}

/// [`COMPARE`]'s payload.
struct Host {
    window: Option<CompareWindow>,
    ui: Option<Weak<MainWindow>>,
    next_id: u64,
    live: Option<LiveSession>,
}

impl Host {
    const fn new() -> Self {
        Self {
            window: None,
            ui: None,
            next_id: 0,
            live: None,
        }
    }
}

/// What [`replace_live`] needs to start a session.
pub(super) struct NewLive {
    /// The surface that shows it.
    pub(super) surface: Surface,
    /// Which feature asked.
    pub(super) origin: CompareOrigin,
    /// What Keep would commit.
    pub(super) guard: KeepGuard,
    /// The renderer currently selected on that surface.
    pub(super) renderer: Renderer,
    /// The image slot in physical pixels, `(0, 0)` until laid out.
    pub(super) size_px: (u32, u32),
}

/// One open comparison. `ready` is `None` while both sides are still solving.
pub(super) struct LiveSession {
    id: u64,
    surface: Surface,
    origin: CompareOrigin,
    guard: KeepGuard,
    renderer: Renderer,
    /// The image slot in physical pixels, `(0, 0)` until the surface has laid out.
    size_px: (u32, u32),
    /// Whether the mouse button is held on the image (an orbit drag in progress).
    drag_held: bool,
    ready: Option<ReadySession>,
}

impl LiveSession {
    /// The surface this session is shown on.
    pub(super) const fn surface(&self) -> Surface {
        self.surface
    }

    /// The session's current shared pose, once it has solved.
    pub(super) fn pose(&self) -> Option<CameraPose> {
        self.ready.as_ref().map(|ready| ready.session.pose)
    }
}

/// A solved session with its render workers.
struct ReadySession {
    session: CompareSession,
    frames: FrameBook,
    latest: Arc<AtomicU64>,
    solid: LatestWorker<ViewRequest>,
    traced: LatestWorker<ViewRequest>,
    traced_timer: Timer,
    traced_progress: TracedProgress,
}

/// The status line for `live`'s current state.
fn current_status(live: &LiveSession) -> String {
    let ready = live.ready.as_ref();
    session::status_text(
        ready.map(|r| &r.session),
        live.renderer,
        ready.map_or(TracedProgress::Done, |r| r.traced_progress),
        TRACED_SPP,
    )
}

/// The status line while both sides are still solving.
pub(super) fn solving_status(renderer: Renderer) -> String {
    session::status_text(None, renderer, TracedProgress::Done, TRACED_SPP)
}

/// A strong handle to `surface`'s window, `None` before it exists (or once the
/// main window is gone).
fn handle_for(host: &Host, surface: Surface) -> Option<SurfaceHandle> {
    match surface {
        Surface::Window => host
            .window
            .as_ref()
            .map(|window| SurfaceHandle::Window(window.clone_strong())),
        Surface::Embedded => host
            .ui
            .as_ref()
            .and_then(Weak::upgrade)
            .map(SurfaceHandle::Embedded),
    }
}

/// A strong handle to `surface`'s window.
pub(super) fn handle(surface: Surface) -> Option<SurfaceHandle> {
    COMPARE.with(|cell| handle_for(&cell.borrow(), surface))
}

/// Remembers the main window (the embedded surface's owner) -- once, at setup.
pub(super) fn set_main_window(ui: &MainWindow) {
    COMPARE.with(|cell| cell.borrow_mut().ui = Some(ui.as_weak()));
}

/// The pop-out window, once created.
pub(super) fn window() -> Option<CompareWindow> {
    COMPARE.with(|cell| {
        cell.borrow()
            .window
            .as_ref()
            .map(ComponentHandle::clone_strong)
    })
}

/// Stores the freshly created pop-out window.
pub(super) fn store_window(window: &CompareWindow) {
    COMPARE.with(|cell| cell.borrow_mut().window = Some(window.clone_strong()));
}

/// Runs `f` on the live session (if any), returning its surface's handle alongside
/// the result so the caller can touch Slint with the cell released.
pub(super) fn with_live<T>(f: impl FnOnce(&mut LiveSession) -> T) -> Option<(SurfaceHandle, T)> {
    COMPARE.with(|cell| {
        let mut host = cell.borrow_mut();
        let surface = host.live.as_ref()?.surface;
        let handle = handle_for(&host, surface)?;
        let live = host.live.as_mut()?;
        Some((handle, f(live)))
    })
}

/// [`with_live`], but only when the live session sits on `surface`.
pub(super) fn with_live_on<T>(
    surface: Surface,
    f: impl FnOnce(&mut LiveSession) -> T,
) -> Option<(SurfaceHandle, T)> {
    let (handle, result) = with_live(|live| (live.surface == surface).then(|| f(live)))?;
    result.map(|result| (handle, result))
}

/// The live session's surface and origin, if any.
pub(super) fn live_kind() -> Option<(Surface, CompareOrigin)> {
    with_live(|live| (live.surface, live.origin)).map(|(_, kind)| kind)
}

/// The live window session's Keep guard, iff both sides solved.
pub(super) fn keep_guard() -> Option<KeepGuard> {
    with_live_on(Surface::Window, |live| {
        live.ready
            .as_ref()
            .is_some_and(|ready| ready.session.can_keep())
            .then(|| live.guard.clone())
    })
    .and_then(|(_, guard)| guard)
}

/// Makes `new` the live session, returning its id and the session it replaced (to
/// be dropped by the caller with the cell released).
pub(super) fn replace_live(new: NewLive) -> (u64, Option<LiveSession>) {
    COMPARE.with(|cell| {
        let mut host = cell.borrow_mut();
        host.next_id = host.next_id.wrapping_add(1);
        let live = LiveSession {
            id: host.next_id,
            surface: new.surface,
            origin: new.origin,
            guard: new.guard,
            renderer: new.renderer,
            size_px: new.size_px,
            drag_held: false,
            ready: None,
        };
        (host.next_id, host.live.replace(live))
    })
}

/// Empties `handle`'s surface: no session, no images, `status` on the status line.
pub(super) fn reset_surface(handle: &SurfaceHandle, status: &str) {
    handle.with_model(|model| {
        model.set_is_open(false);
        model.set_embedded_open(false);
        model.set_before_image(Image::default());
        model.set_after_image(Image::default());
        model.set_overlay_image(Image::default());
        model.set_status(status.into());
    });
}

/// Takes the live session iff it sits on `surface`.
fn take_live_on(surface: Surface) -> Option<LiveSession> {
    COMPARE.with(|cell| {
        let mut host = cell.borrow_mut();
        if host
            .live
            .as_ref()
            .is_some_and(|live| live.surface == surface)
        {
            host.live.take()
        } else {
            None
        }
    })
}

/// Drops the window's live session (its workers and timer with it) and empties the
/// window; hides it when `hide` (`false` from the titlebar close, which hides the
/// window itself). Returns the origin of the dropped session, if there was one.
pub(super) fn close_window(hide: bool) -> Option<CompareOrigin> {
    let window = handle(Surface::Window)?;
    let old = take_live_on(Surface::Window);
    let origin = old.as_ref().map(|live| live.origin);
    // Dropped with the cell released: the timer and worker senders go here.
    drop(old);
    reset_surface(&window, "");
    if hide
        && let SurfaceHandle::Window(window) = &window
        && let Err(error) = window.hide()
    {
        warn!("Could not hide the compare window: {error}");
    }
    origin
}

/// Drops the embedded pane's session (if it has one) and shows `status` in the
/// emptied pane.
pub(super) fn drop_embedded(status: &str) {
    let dropped = take_live_on(Surface::Embedded);
    drop(dropped);
    if let Some(pane) = handle(Surface::Embedded) {
        reset_surface(&pane, status);
    }
}

/// The Retarget dialog is closing: drops the embedded pane's session, and a
/// Retarget session that was popped out into the window goes with it.
pub(super) fn close_embedded_session() {
    if live_kind() == Some((Surface::Window, CompareOrigin::Retarget)) {
        close_window(true);
    }
    drop_embedded("");
}

/// Whether the live session sits on `surface`.
pub(super) fn is_live_on(surface: Surface) -> bool {
    with_live_on(surface, |_| ()).is_some()
}

/// Records the image slot's new size on `surface`'s live session; `false` when the
/// session is not on that surface.
pub(super) fn set_size(surface: Surface, size_px: (u32, u32)) -> bool {
    with_live_on(surface, |live| live.size_px = size_px).is_some()
}

/// Switches `surface`'s live session to `renderer`; `false` when the session is not
/// on that surface.
pub(super) fn set_renderer(surface: Surface, renderer: Renderer) -> bool {
    with_live_on(surface, |live| live.renderer = renderer).is_some()
}

/// Hands a worker report to the UI thread -- the `post` every worker is spawned
/// with. Fails silently only once the event loop has already quit.
fn post_frame(msg: FrameMsg) {
    let _ = slint::invoke_from_event_loop(move || deliver_frame(msg));
}

/// What one accepted worker report changes on screen.
struct FrameUpdate {
    images: Option<(Pixels, Pixels)>,
    overlay: Option<Pixels>,
    status: String,
}

/// `overlay` (RGBA8, `size.0 * size.1 * 4` bytes) as pixels; `None` on a length
/// mismatch (never expected).
fn overlay_pixels(overlay: &[u8], size: (u32, u32)) -> Option<Pixels> {
    let expected = (size.0 as usize) * (size.1 as usize) * 4;
    if size.0 == 0 || size.1 == 0 || overlay.len() != expected {
        return None;
    }
    let mut pixels = Pixels::new(size.0, size.1);
    pixels.make_mut_bytes().copy_from_slice(overlay);
    Some(pixels)
}

/// The status line for a preview worker that panicked: which preview stopped and how
/// to get it back.
fn worker_failed_text(worker: &str) -> String {
    format!(
        "The {worker} preview stopped after an internal error. Change the view to try again, or close and reopen the compare window to restart it."
    )
}

/// Applies `msg` to the live session `live` (if it still belongs to it) and says
/// what to show. The difference overlay travels with the solid pair and follows the
/// CURRENT view, so it is kept even when a traced frame already owns the images.
fn accept_frame(live: &mut LiveSession, msg: FrameMsg) -> Option<FrameUpdate> {
    if live.id != msg.session_id {
        return None;
    }
    let renderer = live.renderer;
    let ready = live.ready.as_mut()?;
    let current = msg.generation == ready.frames.generation();
    let mut update = FrameUpdate {
        images: None,
        overlay: None,
        status: String::new(),
    };
    match msg.event {
        FrameEvent::Solid {
            before,
            after,
            overlay,
        } => {
            if current {
                update.overlay = overlay_pixels(&overlay, (before.width(), before.height()));
            }
            if ready
                .frames
                .accept(FrameKind::Solid, msg.generation, renderer)
            {
                update.images = Some((before, after));
            }
        }
        FrameEvent::TracedProgress { side } => {
            if current && renderer == Renderer::Traced {
                ready.traced_progress = TracedProgress::Rendering { side };
            }
        }
        FrameEvent::Traced { before, after } => {
            if ready
                .frames
                .accept(FrameKind::Traced, msg.generation, renderer)
            {
                ready.traced_progress = TracedProgress::Done;
                update.images = Some((before, after));
            }
        }
        FrameEvent::WorkerFailed { worker } => {
            update.status = worker_failed_text(worker);
            return Some(update);
        }
    }
    update.status = current_status(live);
    Some(update)
}

/// Shows a worker report iff it belongs to the live session and still describes
/// the current view ([`FrameBook::accept`]). The overlay image is pushed only while
/// the overlay layout is showing.
fn deliver_frame(msg: FrameMsg) {
    let Some((handle, Some(update))) = with_live(|live| accept_frame(live, msg)) else {
        return;
    };
    handle.with_model(|model| {
        if let Some((before, after)) = update.images {
            model.set_before_image(Image::from_rgba8(before));
            model.set_after_image(Image::from_rgba8(after));
        }
        if let Some(overlay) = update.overlay
            && model.get_mode_index() == MODE_OVERLAY
        {
            model.set_overlay_image(Image::from_rgba8(overlay));
        }
        model.set_status(update.status.into());
    });
}

/// Starts a new view generation: the solid pair right away, and -- in Traced mode
/// -- the traced pair after `traced_delay` of no further view change (and never
/// while a drag is held).
pub(super) fn request_frames(traced_delay: Duration) {
    let status = with_live(|live| {
        let size = live.size_px;
        let renderer = live.renderer;
        if let Some(ready) = live.ready.as_mut()
            && size.0 > 0
            && size.1 > 0
        {
            let generation = ready.frames.bump();
            ready.latest.store(generation, Ordering::Release);
            ready.solid.submit(ViewRequest {
                generation,
                pose: ready.session.pose,
                size,
            });
            if renderer == Renderer::Traced {
                ready.traced_progress = TracedProgress::Waiting;
                ready
                    .traced_timer
                    .start(TimerMode::SingleShot, traced_delay, submit_traced);
            } else {
                ready.traced_timer.stop();
            }
        }
        current_status(live)
    });
    if let Some((handle, status)) = status {
        handle.with_model(|model| model.set_status(status.into()));
    }
}

/// The traced debounce timer's action: traces the CURRENT view (whatever the
/// latest generation is by now) -- unless a drag is held, in which case the
/// release ([`set_drag`]) starts the wait again.
fn submit_traced() {
    let status = with_live(|live| {
        let size = live.size_px;
        if live.renderer == Renderer::Traced
            && session::traced_may_start(live.drag_held)
            && let Some(ready) = live.ready.as_mut()
        {
            ready.traced.submit(ViewRequest {
                generation: ready.frames.generation(),
                pose: ready.session.pose,
                size,
            });
            ready.traced_progress = TracedProgress::Rendering { side: 1 };
        }
        current_status(live)
    });
    if let Some((handle, status)) = status {
        handle.with_model(|model| model.set_status(status.into()));
    }
}

/// The mouse button went down (`held`) or up on `surface`'s image. On release, a
/// traced pair still waiting for the pose to settle restarts its debounce.
pub(super) fn set_drag(surface: Surface, held: bool) {
    with_live_on(surface, |live| {
        live.drag_held = held;
        let renderer = live.renderer;
        if !held
            && renderer == Renderer::Traced
            && let Some(ready) = live.ready.as_mut()
            && ready.traced_progress == TracedProgress::Waiting
        {
            ready
                .traced_timer
                .start(TimerMode::SingleShot, TRACED_DEBOUNCE, submit_traced);
        }
    });
}

/// Applies `change` to `surface`'s shared pose (`None` while still solving), then
/// re-renders with the traced debounce.
pub(super) fn change_pose(surface: Surface, change: impl FnOnce(CameraPose, f64) -> CameraPose) {
    let changed = with_live_on(surface, |live| {
        live.ready.as_mut().map(|ready| {
            ready.session.pose = change(ready.session.pose, ready.session.radius);
        })
    });
    if matches!(changed, Some((_, Some(())))) {
        request_frames(TRACED_DEBOUNCE);
    }
}

/// A freshly solved session arrives: keeps it iff it is still the live one, spawns
/// its two workers, and renders the first view.
pub(super) fn on_session_built(id: u64, session: CompareSession) {
    let sides = || {
        (
            SideGeometry::from_side(&session.before),
            SideGeometry::from_side(&session.after),
        )
    };
    let latest = Arc::new(AtomicU64::new(0));
    let solid = render::spawn_solid_worker(id, sides(), Arc::clone(&latest), post_frame);
    let traced = render::spawn_traced_worker(id, sides(), Arc::clone(&latest), post_frame);
    let can_keep = session.can_keep();
    let labels = (session.before.label.clone(), session.after.label.clone());
    let stored = with_live(|live| {
        if live.id != id {
            return false;
        }
        live.ready = Some(ReadySession {
            session,
            frames: FrameBook::default(),
            latest,
            solid,
            traced,
            traced_timer: Timer::default(),
            traced_progress: TracedProgress::Done,
        });
        true
    });
    let Some((handle, true)) = stored else {
        return;
    };
    handle.with_model(|model| {
        model.set_can_keep(can_keep);
        model.set_before_label(labels.0.into());
        model.set_after_label(labels.1.into());
    });
    request_frames(Duration::ZERO);
}

#[cfg(test)]
mod tests {
    use super::worker_failed_text;

    /// The text names the failed preview and both ways back; the expectation is the
    /// literal wording, so it is derived by reading `worker_failed_text`.
    #[test]
    fn the_failure_text_names_the_preview_and_how_to_restart_it() {
        for worker in ["solid", "traced"] {
            let text = worker_failed_text(worker);
            assert!(text.starts_with(&format!("The {worker} preview stopped")));
            assert!(text.contains("close and reopen the compare window"));
        }
    }
}
