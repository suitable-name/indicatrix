//! Handling a `RemoteUpdate` from an in-flight dispatch ([`handle_remote_update`]),
//! and turning the accumulator's running sum into a displayed, denoised image
//! ([`redraw_from_accumulator`]). See this group's own `mod.rs` doc comment.

use super::{
    poll::{apply_actions, sync_served_by_to_ui},
    state::{Orchestrator, lock},
};
use crate::{
    MainWindow, RemoteWorkerModel, ViewportModel,
    bridge::{
        frame_cache::guide_pass::GuideCache,
        remote::{handoff::HandoffEvent, remote_render::RemoteUpdate},
        render_thread::{RenderContext, tonemap_running_average},
    },
    gui::{remote::worker_callbacks::backend_label, show_toast},
    settings::LiveComputeTarget,
};
use indicatrix_net::client::Accumulator;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use super::super::generation::{
    DenoiseGenerationJob, adopt_ready_denoise, adopt_ready_guides, spawn_denoise_generation,
};

/// The minimum real-time gap between two Frame/Preview-triggered remote redraws
/// `handle_remote_update` will perform -- mirrors `render_thread::
/// DENOISE_MIN_INTERVAL`'s role for the local path (a plain tonemap alone is "tens of
/// milliseconds even at 4K, not free"). 33ms (~30 FPS), since this crate has no
/// pre-existing remote-specific cadence to reuse. `RemoteUpdate::Done`'s redraw is
/// deliberately never subject to this gap (see its match arm below) -- the final,
/// fully-settled image must always be shown.
const REMOTE_REDRAW_MIN_INTERVAL: Duration = Duration::from_millis(33);

/// Whether enough real time has passed since `last_redraw_at` (`None` meaning "no
/// redraw yet this session/request") for another Frame/Preview-triggered remote redraw
/// to be allowed, given `min_interval`. `now` is a parameter rather than read via
/// `Instant::now()` internally, purely so this stays pure and directly unit-testable.
#[must_use]
fn redraw_is_due(last_redraw_at: Option<Instant>, min_interval: Duration, now: Instant) -> bool {
    last_redraw_at.is_none_or(|t| now.duration_since(t) >= min_interval)
}

/// Whether the live viewport is currently in `LiveComputeTarget::Both` -- callers skip
/// pushing a remote-only redraw while this holds, since the render thread's own
/// display cycle is the combined image's sole producer in that mode.
fn is_combining(render_ctx: &Arc<Mutex<RenderContext>>) -> bool {
    matches!(
        render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .live_compute_target,
        LiveComputeTarget::Both
    )
}

pub(super) fn handle_remote_update(
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    accumulator: &Arc<Mutex<Accumulator>>,
    width: u32,
    height: u32,
    update: RemoteUpdate,
) {
    let render_ctx = render_ctx.clone();
    let state = Arc::clone(state);
    let accumulator = Arc::clone(accumulator);

    // `Frame`/`Preview` events can arrive many times per second while a remote render
    // is in flight, each otherwise queuing its own UI-thread closure that clones the
    // whole accumulator buffer and tonemaps it (`upgrade_in_event_loop` queues rather
    // than runs immediately, so nothing bounds how many such closures could pile up).
    // Gate the two of them -- and only the two -- through `Orchestrator::redraw_gate`
    // (at most one pending closure) and `REMOTE_REDRAW_MIN_INTERVAL` (at most one
    // redraw attempt per ~33ms) before enqueueing anything: dropping such an update
    // here is safe because a redraw always re-reads the accumulator's then-current
    // running sum when it runs, never a value snapshotted now.
    //
    // `Connected`/`Progress`/`Done`/`Failed` are never gated this way: each is a
    // one-time state transition (or, for `Done`, the terminal redraw that must always
    // be shown) rather than a redundant intermediate frame.
    let wants_gated_redraw = matches!(
        update,
        RemoteUpdate::Frame { .. } | RemoteUpdate::Preview { .. }
    );
    if wants_gated_redraw {
        if is_combining(&render_ctx) {
            // The render thread's own display cycle owns redraws entirely while
            // combining -- checked here too, not just inside the closure, to avoid
            // enqueuing a closure that would do nothing anyway.
            return;
        }
        let orch = lock(&state);
        let due = redraw_is_due(
            orch.last_redraw_at,
            REMOTE_REDRAW_MIN_INTERVAL,
            Instant::now(),
        );
        if !due || orch.redraw_gate.submit(()).is_none() {
            return;
        }
        drop(orch);
    }

    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        // Release `redraw_gate`'s pending slot unconditionally, before anything else --
        // including the staleness check below, which can itself `return` early.
        // `redraw_gate.submit` above set `pending` on the promise that this closure
        // would eventually call `take()`; skipping that on a stale-update early return
        // would leave `pending` stuck `true` forever, blocking every future redraw.
        if wants_gated_redraw {
            lock(&state).redraw_gate.take();
        }
        // By the time this queued closure actually runs, a later settle may already
        // have dispatched a fresh request, setting `current_request_id` to the new id.
        // An update for anything else is stale and must never drive this
        // orchestrator's state.
        if lock(&state).current_request_id != Some(update.request_id()) {
            return;
        }
        match update {
            RemoteUpdate::Connected { info, .. } => {
                let actions = lock(&state)
                    .handoff
                    .handle(HandoffEvent::RemoteStreamStarted);
                apply_actions(&actions, &render_ctx, &state);
                ui.global::<RemoteWorkerModel>()
                    .set_served_by_worker_name(backend_label(info.render.as_ref()).into());
            }
            RemoteUpdate::Frame { samples_done, .. } => {
                tracing::trace!("remote render: {samples_done} samples done");
                // While combining, this orchestrator's own denoise-and-push pipeline
                // is skipped entirely: the render thread's periodic display cycle
                // already folds this same accumulator's current total into the
                // combined image it pushes, at a cadence at least as fast as remote
                // `FRAME` events typically arrive. A redraw here would show a
                // remote-only partial sum and fight the render thread's combined push
                // for the same `render_image` property.
                if !is_combining(&render_ctx) {
                    redraw_from_accumulator(&ui, &accumulator, &render_ctx, &state, width, height);
                }
            }
            RemoteUpdate::Preview { .. } => {
                if !is_combining(&render_ctx) {
                    redraw_from_accumulator(&ui, &accumulator, &render_ctx, &state, width, height);
                }
            }
            RemoteUpdate::Progress { samples_done, .. } => {
                tracing::trace!("remote render progress: {samples_done} samples done");
            }
            RemoteUpdate::Done { cancelled, .. } => {
                if cancelled {
                    // A cancellation the worker confirmed after this orchestrator had
                    // already moved on -- nothing further to do; local preview is
                    // already back in charge.
                    return;
                }
                if !is_combining(&render_ctx) {
                    redraw_from_accumulator(&ui, &accumulator, &render_ctx, &state, width, height);
                }
                let actions = lock(&state).handoff.handle(HandoffEvent::RemoteDone);
                apply_actions(&actions, &render_ctx, &state);
                // `ctx.remote_active` is deliberately not cleared here -- a finished
                // remote render is the settled, full-quality image; clearing it would
                // let local tracing race back in and progressively overwrite it with a
                // rough low-spp restart. It stays set until
                // `resolve_remote_ownership` releases it once the scene is genuinely
                // invalidated.
                sync_served_by_to_ui(&ui, &state);
                let mut s = lock(&state);
                s.remote_handle = None;
                s.accumulator = None;
            }
            RemoteUpdate::Failed { message, .. } => {
                let actions = lock(&state).handoff.handle(HandoffEvent::RemoteFailed);
                apply_actions(&actions, &render_ctx, &state);
                {
                    let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
                    ctx.remote_active = false;
                    ctx.dirty = true;
                    // Same reasoning as `DiscardRemotePartial` in `apply_actions`: a
                    // failed request's shared accumulator must never keep being folded
                    // into the combined display local is about to restart fresh.
                    ctx.remote_accumulator = None;
                    ctx.remote_reserved_samples = 0;
                }
                let mut s = lock(&state);
                s.remote_handle = None;
                s.accumulator = None;
                drop(s);
                show_toast(&ui, &format!("Remote render failed: {message}"), "error");
            }
        }
    });
}

/// Reads the accumulator's current running sum plus the pose/geometry/denoise-toggle
/// state needed out of the real `Accumulator`/`RenderContext`/`Orchestrator`, decides
/// what to display, and pushes the result to the viewport image.
///
/// This runs on the Slint UI/event-loop thread, for every `RemoteUpdate::Frame`/
/// `Preview`/`Done` -- which, mid-render, can arrive many times per second. That is
/// exactly why the actual (multi-second at 4K) denoise pass must never run inline
/// here -- this function only does cheap work synchronously (a plain tonemap, tens of
/// milliseconds even at 4K) and defers the expensive pass to a background thread via
/// `spawn_denoise_generation`, swapping in its result on a later redraw once
/// `adopt_ready_denoise` confirms it is ready and still valid for the pose on screen.
///
/// `state`'s lock is taken TWICE here, briefly, rather than once for the whole function
/// -- never held across the plain tonemap below (see `Orchestrator::last_redraw_at`'s
/// "tens of milliseconds even at 4K, not free" doc comment). Holding one lock end to end
/// across that tonemap call would stall `handle_remote_update`'s own rate-limit check
/// (this same lock, taken SYNCHRONOUSLY on the connection thread, before it ever queues
/// a UI-thread closure -- see its own call site) for exactly as long as the tonemap
/// took, contrary to `connection::run`/`run_connection`'s documented "O(1) between
/// reads" liveness argument (`bridge/remote/remote_render/connection/mod.rs`'s module
/// doc comment).
fn redraw_from_accumulator(
    ui: &MainWindow,
    accumulator: &Arc<Mutex<Accumulator>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    width: u32,
    height: u32,
) {
    let (buffer, samples_done) = {
        let acc = accumulator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (acc.buffer().to_vec(), acc.samples_done().max(1))
    };
    let (yaw, pitch, distance, planes, denoise_enabled) = {
        let ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        (
            ctx.yaw,
            ctx.pitch,
            ctx.distance,
            ctx.active_planes.clone(),
            ctx.denoise_enabled,
        )
    };
    let desired_key = GuideCache::key_for(width, height, yaw, pitch, distance, &planes);

    // First (brief) lock: adopt any freshly finished background denoise for this pose,
    // and read out a cached denoised frame if one is already valid -- both O(1) next to
    // the tonemap below, so held only long enough for that.
    let cached_denoised = {
        let mut orch = lock(state);
        if denoise_enabled {
            // A fresher generation just finished -- adopt it as the new "last known
            // good" for this pose. Only clear the slot on adoption; otherwise it's
            // still legitimately in flight.
            if let Some(fresh) =
                adopt_ready_denoise(&desired_key, orch.pending_denoise_gen.as_ref())
            {
                orch.pending_denoise_gen = None;
                orch.last_denoised = Some((desired_key.clone(), fresh));
            }
            // Show the freshest denoised frame for this pose if we have one (even if a
            // newer generation is still cooking -- a few-samples-stale denoised image
            // beats flickering back to noise every redraw); otherwise fall through to
            // the plain tonemap below, UNLOCKED.
            match orch.last_denoised.as_ref() {
                Some((key, bytes)) if *key == desired_key => Some(bytes.clone()),
                _ => {
                    orch.last_denoised = None;
                    None
                }
            }
        } else {
            orch.pending_denoise_gen = None;
            orch.last_denoised = None;
            None
        }
    };

    // The plain tonemap -- "tens of milliseconds even at 4K, not free" -- runs with NO
    // lock held at all: `handle_remote_update`'s rate-limit check takes this same
    // `state` lock synchronously on the connection thread, and must never wait out
    // a redraw's tonemap to get it.
    let bytes = cached_denoised
        .unwrap_or_else(|| tonemap_running_average(width, height, samples_done, &buffer));

    // Second (brief) lock: dispatch a fresh background denoise generation if warranted,
    // and stamp the redraw timestamp the rate limit reads.
    {
        let mut orch = lock(state);

        // Keep exactly one background denoise generation in flight per pose: dispatch a
        // fresh one whenever denoising is on and nothing is already running for the
        // current pose (`pending_denoise_gen`'s key not matching `desired_key` covers
        // "nothing dispatched yet", "the one just adopted above", and "pose changed
        // since the last dispatch" identically). Needs guides for this pose already
        // ready -- otherwise there is nothing to hand the background thread yet; a
        // later redraw, once the guide prepass lands, dispatches then.
        if denoise_enabled {
            let pending_matches = orch
                .pending_denoise_gen
                .as_ref()
                .is_some_and(|p| p.key == desired_key);
            if !pending_matches {
                let Orchestrator {
                    guide_cache,
                    pending_guide_gen,
                    ..
                } = &mut *orch;
                if adopt_ready_guides(&desired_key, guide_cache, pending_guide_gen.as_ref()) {
                    let guides = guide_cache
                        .ensure(width, height, yaw, pitch, distance, &planes)
                        .clone();
                    orch.pending_denoise_gen = Some(spawn_denoise_generation(
                        desired_key,
                        DenoiseGenerationJob {
                            width,
                            height,
                            samples_done,
                            buffer,
                            guides,
                            yaw,
                            pitch,
                            distance,
                            // `DenoiseGenerationJob::planes` is a plain `Vec` (a
                            // one-shot background job's own owned copy, not
                            // `RenderContext`'s hot-path per-frame snapshot) --
                            // `Arc::clone` above kept the read cheap; this is the one
                            // place that copy actually has to happen.
                            planes: planes.as_ref().clone(),
                        },
                    ));
                }
            }
        }

        // Stamps the moment this redraw happened so the pre-enqueue check can
        // rate-limit the next Frame/Preview-triggered one. Set unconditionally, since
        // every path above produced a fresh `bytes` to show.
        orch.last_redraw_at = Some(Instant::now());
    }

    let mut fb = crate::bridge::pixel_buffer::FramebufferTransfer::new(width, height);
    let image = fb.copy_from_gpu_slice(&bytes);
    ui.global::<ViewportModel>()
        .set_render_image(slint::Image::from_rgba8(image));
    ui.global::<ViewportModel>().set_has_render(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- redraw_is_due: remote redraw rate limit ----

    #[test]
    fn no_prior_redraw_is_always_due() {
        assert!(redraw_is_due(
            None,
            REMOTE_REDRAW_MIN_INTERVAL,
            Instant::now()
        ));
    }

    #[test]
    fn a_redraw_within_the_interval_is_not_due() {
        let last = Instant::now();
        let now = last + Duration::from_millis(10);
        assert!(
            !redraw_is_due(Some(last), REMOTE_REDRAW_MIN_INTERVAL, now),
            "10ms after the last redraw is well inside the ~33ms interval"
        );
    }

    #[test]
    fn a_redraw_exactly_at_the_interval_boundary_is_due() {
        let last = Instant::now();
        let now = last + REMOTE_REDRAW_MIN_INTERVAL;
        assert!(
            redraw_is_due(Some(last), REMOTE_REDRAW_MIN_INTERVAL, now),
            "the boundary itself (elapsed >= min_interval) must count as due"
        );
    }

    #[test]
    fn a_redraw_well_past_the_interval_is_due() {
        let last = Instant::now();
        let now = last + REMOTE_REDRAW_MIN_INTERVAL + Duration::from_secs(1);
        assert!(redraw_is_due(Some(last), REMOTE_REDRAW_MIN_INTERVAL, now));
    }

    /// A whole burst of updates arriving within one interval must only ever find one of
    /// them due -- the time-based half of rate-limiting redraws (`RedrawGate`
    /// separately bounds the queue to one pending closure; this bounds how often a
    /// redraw is even attempted).
    #[test]
    fn a_burst_of_updates_within_one_interval_finds_at_most_one_due() {
        let start = Instant::now();
        let mut last_redraw_at: Option<Instant> = None;
        let mut due_count = 0;
        for ms in [0u64, 5, 10, 15, 20, 25, 30] {
            let now = start + Duration::from_millis(ms);
            if redraw_is_due(last_redraw_at, REMOTE_REDRAW_MIN_INTERVAL, now) {
                due_count += 1;
                last_redraw_at = Some(now);
            }
        }
        assert_eq!(
            due_count, 1,
            "only the FIRST update of a burst inside one ~33ms interval may redraw"
        );

        // Once the interval genuinely elapses, the next update must be due again.
        let now = start + REMOTE_REDRAW_MIN_INTERVAL + Duration::from_millis(1);
        assert!(redraw_is_due(
            last_redraw_at,
            REMOTE_REDRAW_MIN_INTERVAL,
            now
        ));
    }
}
