//! The repeating timer tick ([`setup_remote_rendering`]/[`poll_tick`]) that watches
//! `RenderContext`'s camera/light pose, feeds `bridge::handoff::HandoffMachine`, and
//! carries out the actions it returns ([`apply_actions`]) -- deferring the one action
//! that actually dispatches to a remote worker to `super::dispatch::start_remote_render`,
//! called right after by `poll_tick` itself. See this group's own `mod.rs` doc comment.

use super::{
    dispatch::{connection_is_stale, start_remote_render},
    state::{Orchestrator, current_pose, lock},
};
use crate::{
    MainWindow, RemoteWorkerModel,
    bridge::{
        frame_cache::guide_pass::GuideCache,
        remote::{
            handoff::{HandoffAction, HandoffEvent, HandoffMachine, HandoffState},
            remote_render::RemoteConnectionHandle,
        },
        render_thread::{RedrawGate, RenderContext},
    },
    settings::{LiveComputeTarget, SettingsPersister},
};
use slint::ComponentHandle;
use std::{
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

/// How long orientation must stay unchanged before a remote handoff is attempted --
/// see `bridge::handoff`'s module docs on why this is what "settled" means.
const SETTLE_DEBOUNCE: Duration = Duration::from_millis(600);
/// How often the orchestrator timer below polls `RenderContext`'s camera/light pose
/// for a change. Independent of `SETTLE_DEBOUNCE` -- this is a poll granularity, not
/// the debounce itself.
const POLL_INTERVAL_MS: u64 = 100;

/// Wires up the preview-then-handoff orchestrator: a repeating `slint::Timer` polls
/// `render_ctx`'s camera/light pose, feeds `bridge::handoff::HandoffMachine`, and
/// dispatches to `bridge::remote_render` when the machine decides to hand off. Returns
/// the `slint::Timer` -- the caller (`gui::mod::run_gui`) MUST keep it alive for the
/// life of the window (a dropped `Timer` stops firing), the same requirement as any
/// other `slint::Timer` used this way.
#[must_use]
pub fn setup_remote_rendering(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) -> slint::Timer {
    let state = Arc::new(Mutex::new(Orchestrator {
        handoff: HandoffMachine::new(),
        remote_handle: None,
        accumulator: None,
        current_request_id: None,
        remote_connection: None,
        last_pose: current_pose(render_ctx),
        last_change_at: Instant::now(),
        guide_cache: GuideCache::new(),
        pending_guide_gen: None,
        pending_denoise_gen: None,
        last_denoised: None,
        redraw_gate: RedrawGate::new(),
        last_redraw_at: None,
    }));
    let next_request_id = Rc::new(AtomicU32::new(1));

    let timer = slint::Timer::default();
    let ui_weak = ui.as_weak();
    let render_ctx_poll = render_ctx.clone();
    let settings_store_poll = settings_store.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(POLL_INTERVAL_MS),
        move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            poll_tick(
                &ui,
                &render_ctx_poll,
                &settings_store_poll,
                &state,
                &next_request_id,
            );
        },
    );
    timer
}

fn poll_tick(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    state: &Arc<Mutex<Orchestrator>>,
    next_request_id: &Rc<AtomicU32>,
) {
    let pose = current_pose(render_ctx);
    let changed = pose != lock(state).last_pose;

    if changed {
        lock(state).last_pose = pose;
        lock(state).last_change_at = Instant::now();
        let actions = lock(state).handoff.handle(HandoffEvent::OrientationChanged);
        apply_actions(&actions, render_ctx, state);
        sync_served_by_to_ui(ui, state);
        sync_camera_moving_to_ctx(render_ctx, state);
        return;
    }

    let (should_check_settle, elapsed_enough) = {
        let s = lock(state);
        (
            matches!(s.handoff.state(), HandoffState::Previewing),
            s.last_change_at.elapsed() >= SETTLE_DEBOUNCE,
        )
    };
    if should_check_settle && elapsed_enough {
        // `LiveComputeTarget::LocalOnly` means "never hand off to remote at all", so
        // it's treated as no worker being configured right here, at the one place
        // `HandoffMachine` ever learns whether a worker is available; `bridge::handoff`
        // itself stays unaware this choice exists, seeing only `worker_available: bool`.
        let live_compute_target = render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .live_compute_target;
        let worker = if matches!(live_compute_target, LiveComputeTarget::LocalOnly) {
            None
        } else {
            settings_store
                .snapshot()
                .settings
                .remote_workers
                .first()
                .cloned()
        };
        // `worker` is freshly read from settings right before a possible dispatch, so
        // this is also the right place to notice the cached `remote_connection`'s
        // identity no longer matches (address/`cert_dir` edited, worker removed, or
        // remote compute switched off). Dropping it here, before `start_remote_render`
        // could reuse it, guarantees the next dispatch never sends a `RenderRequest`
        // over a connection authenticated under stale certificates. Never reached
        // while a remote render is genuinely in flight: `should_check_settle` requires
        // `HandoffState::Previewing`, which only re-enters after any previous
        // request's cleanup has run, so `remote_handle` is always already `None` here.
        if connection_is_stale(
            lock(state)
                .remote_connection
                .as_ref()
                .map(RemoteConnectionHandle::worker),
            worker.as_ref(),
        ) {
            lock(state).remote_connection = None;
        }
        let actions = lock(state).handoff.handle(HandoffEvent::SettleElapsed {
            worker_available: worker.is_some(),
        });
        apply_actions(&actions, render_ctx, state);
        if let Some(worker) = worker {
            start_remote_render(ui, render_ctx, worker, next_request_id, state);
        }
    }
    // `RenderContext::camera_moving` must be kept in sync on every tick, not just the
    // branches above that changed handoff state, since `Previewing` can also simply
    // persist unchanged tick to tick while a drag continues.
    sync_camera_moving_to_ctx(render_ctx, state);
    reconcile_served_by_after_release(ui, render_ctx, state);
}

/// `render_thread::mod`'s `resolve_remote_ownership` can release `ctx.remote_active`
/// entirely on its own, off this orchestrator's timer, whenever a non-drag scene
/// change (material/lighting/quality/etc.) arrives while a completed remote render is
/// still displayed. A real camera drag already keeps `HandoffMachine::served_by`
/// truthful (the pose-changed branch above), but a non-drag release has no handoff
/// event to ride along with, so this polls for it: whenever `served_by() == Remote`
/// but the render loop has since cleared `ctx.remote_active`, local tracing has
/// already silently taken back over -- feed `HandoffEvent::SceneInvalidated` so the
/// "served by remote" indicator stops claiming otherwise. A no-op on every other tick.
fn reconcile_served_by_after_release(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let still_remote_active = render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remote_active;
    if still_remote_active {
        return;
    }
    let served_by_remote = matches!(
        lock(state).handoff.served_by(),
        crate::bridge::remote::handoff::ImageSource::Remote
    );
    if served_by_remote {
        lock(state).handoff.handle(HandoffEvent::SceneInvalidated);
        sync_served_by_to_ui(ui, state);
    }
}

/// Mirrors `HandoffMachine::state() == Previewing` onto `RenderContext::camera_moving`,
/// the single source of truth for "is the camera currently moving" shared by both the
/// remote handoff and the local preview-then-settle feature.
fn sync_camera_moving_to_ctx(
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let moving = matches!(lock(state).handoff.state(), HandoffState::Previewing);
    render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .camera_moving = moving;
}

/// Mirrors `HandoffMachine::served_by` onto `MainWindow.served_by_remote`, the single
/// source of truth for "which backend served the current image" the worker-list panel
/// displays. Called after every `HandoffMachine::handle` call that could change it (a
/// fresh drag flips it back to `Local` immediately, not just on remote completion).
pub(super) fn sync_served_by_to_ui(ui: &MainWindow, state: &Arc<Mutex<Orchestrator>>) {
    let served_by_remote = matches!(
        lock(state).handoff.served_by(),
        crate::bridge::remote::handoff::ImageSource::Remote
    );
    ui.global::<RemoteWorkerModel>()
        .set_served_by_remote(served_by_remote);
}

/// Carries out the [`HandoffAction`]s a `HandoffMachine::handle` call returned, against
/// the real `RenderContext` and the orchestrator's own remote-render/accumulator state.
/// Does not dispatch [`HandoffAction::SendRenderRequestToWorker`] itself (that needs the
/// worker config and UI handle, supplied by `start_remote_render` right after this
/// returns) -- only the discard/cancel side effects.
pub(super) fn apply_actions(
    actions: &[HandoffAction],
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    for action in actions {
        match action {
            // `DiscardLocalPreview` and `SendRenderRequestToWorker` always fire together
            // and are both deferred to the same call site: `start_remote_render`,
            // called by `poll_tick` right after this. For `LiveComputeTarget::Both`,
            // that's also where the `remote_active`/`dirty`/shared-accumulator hand-off
            // has to land, in one locked mutation with the discard/dispatch.
            HandoffAction::DiscardLocalPreview | HandoffAction::SendRenderRequestToWorker => {}
            HandoffAction::SendCancelToWorker => {
                if let Some(handle) = &lock(state).remote_handle {
                    handle.cancel();
                }
            }
            HandoffAction::DiscardRemotePartial => {
                let mut s = lock(state);
                s.remote_handle = None;
                s.accumulator = None;
                // The pose that made this render's guides valid is gone too -- abandon
                // whatever prepass was still running for it (see
                // `PendingGuideGeneration`'s doc comment).
                if let Some(pending) = s.pending_guide_gen.take() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
                // Same idea for the background denoise pass -- no cooperative cancel
                // for it, so this just drops the orchestrator's reference; the thread
                // still runs to completion, but nothing here will adopt its result
                // once a fresh `RenderRequest` overwrites this pose's state.
                s.pending_denoise_gen = None;
                s.last_denoised = None;
                drop(s);
                let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
                ctx.remote_active = false;
                ctx.dirty = true; // resume local previewing fresh, not from a discarded buffer
                // The just-cancelled request's shared accumulator, if any, must never
                // keep being folded into a combined display: a fresh local preview is
                // about to start from an empty buffer, and combining it with a
                // now-abandoned request's partial contribution would violate the
                // handoff module's "never carry a buffer from one source to the other"
                // invariant.
                ctx.remote_accumulator = None;
                ctx.remote_reserved_samples = 0;
            }
        }
    }
}
