//! Handling a `RemoteUpdate` from an in-flight chunk ([`handle_remote_update`]):
//! merging a finished chunk and requesting the next one, keeping a failed chunk's
//! prefix and handing its remainder back, finishing the epoch -- and, for
//! `LiveComputeTarget::RemoteOnly`, turning the epoch's merged remote sum into a
//! displayed, denoised image ([`redraw_from_epoch`]). See this group's own `mod.rs` doc
//! comment.

use super::{
    dispatch::dispatch_next_chunk,
    poll::{apply_actions, live_remote_allowed, sync_served_by_to_ui},
    state::{Orchestrator, lock},
};
use crate::{
    MainWindow, ViewportModel,
    bridge::{
        frame_cache::guide_pass::GuideCache,
        remote::{
            handoff::HandoffEvent,
            live_lane::{ChunkVerdict, LiveLane},
            remote_render::RemoteUpdate,
        },
        render_thread::{RenderContext, tonemap_running_average},
    },
    gui::{remote::worker_callbacks::backend_label, show_toast},
    settings::LiveComputeTarget,
};
use indicatrix::geometry::tool::StoneGeometry;
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
/// pre-existing remote-specific cadence to reuse. The epoch's final redraw is
/// deliberately never subject to this gap -- the fully-settled image must always be
/// shown.
const REMOTE_REDRAW_MIN_INTERVAL: Duration = Duration::from_millis(33);

/// Whether enough real time has passed since `last_redraw_at` (`None` meaning "no
/// redraw yet this session/request") for another Frame/Preview-triggered remote redraw
/// to be allowed, given `min_interval`. `now` is a parameter rather than read via
/// `Instant::now()` internally, purely so this stays pure and directly unit-testable.
#[must_use]
fn redraw_is_due(last_redraw_at: Option<Instant>, min_interval: Duration, now: Instant) -> bool {
    last_redraw_at.is_none_or(|t| now.duration_since(t) >= min_interval)
}

/// Whether the live viewport is currently combining (the EFFECTIVE target is `Both`:
/// a final-picture epoch never combines) -- callers skip pushing a remote-only redraw
/// while this holds, since the render thread's own display cycle is the combined
/// image's sole producer in that mode.
fn is_combining(render_ctx: &Arc<Mutex<RenderContext>>) -> bool {
    matches!(
        render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .effective_live_target(),
        LiveComputeTarget::Both
    )
}

/// The `on_update` callback of every chunk request: gates redraw-worthy events on the
/// connection thread, then carries the update out on the Slint event loop. `chunk` is
/// the epoch's dimensions plus the chunk's own accumulator, whose latest display frame
/// (final-picture live transfer) is what a `DisplayFrame` shows.
pub(super) fn handle_remote_update(
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    chunk: (u32, u32, &Arc<Mutex<Accumulator>>),
    update: RemoteUpdate,
) {
    let (width, height, chunk_accumulator) = chunk;
    let chunk_accumulator = Arc::clone(chunk_accumulator);
    let render_ctx = render_ctx.clone();
    let state = Arc::clone(state);

    // `Frame`/`Preview` events can arrive many times per second while a chunk is in
    // flight, each otherwise queuing its own UI-thread closure. Gate the two of them --
    // and only the two -- through `Orchestrator::redraw_gate` (at most one pending
    // closure) and `REMOTE_REDRAW_MIN_INTERVAL` (at most one redraw attempt per ~33ms)
    // before enqueueing anything: dropping such an update here is safe because a redraw
    // always re-reads the epoch's then-current sums when it runs. While combining, the
    // render thread owns redraws entirely, so they are dropped outright -- except that
    // the rate estimate still wants `Frame` progress, which `Progress` heartbeats
    // (never gated) also carry.
    //
    // `Connected`/`Progress`/`Done`/`Failed` are never gated this way: each is a
    // one-time state transition or a cheap bookkeeping update.
    // A `DisplayFrame` is gated the same way: the queued closure decodes whatever frame
    // is LATEST in the chunk's accumulator when it runs, so dropping one here is safe.
    let wants_gated_redraw = matches!(
        update,
        RemoteUpdate::Frame { .. }
            | RemoteUpdate::Preview { .. }
            | RemoteUpdate::DisplayFrame { .. }
    );
    if wants_gated_redraw {
        if is_combining(&render_ctx) {
            if let RemoteUpdate::Frame {
                request_id,
                samples_done,
            } = update
                && let Some(lane) = lock(&state).live_lane.as_mut()
            {
                lane.observe_progress(request_id, samples_done, Instant::now());
            }
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

    let ui_weak_for_next = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        // Release `redraw_gate`'s pending slot unconditionally, before anything else --
        // including the staleness check below, which can itself `return` early.
        if wants_gated_redraw {
            lock(&state).redraw_gate.take();
        }
        // By the time this queued closure actually runs, a later chunk or settle may
        // already be current. An update for anything else is stale and must never
        // drive this orchestrator's state.
        if lock(&state).current_request_id != Some(update.request_id()) {
            return;
        }
        apply_update(
            &UpdateCtx {
                ui: &ui,
                ui_weak: &ui_weak_for_next,
                render_ctx: &render_ctx,
                state: &state,
                chunk_accumulator: &chunk_accumulator,
                width,
                height,
            },
            update,
        );
    });
}

/// Everything [`apply_update`] needs on the Slint event loop, bundled.
struct UpdateCtx<'a> {
    ui: &'a MainWindow,
    ui_weak: &'a Weak<MainWindow>,
    render_ctx: &'a Arc<Mutex<RenderContext>>,
    state: &'a Arc<Mutex<Orchestrator>>,
    chunk_accumulator: &'a Mutex<Accumulator>,
    width: u32,
    height: u32,
}

/// Carries one current (not stale) update out on the Slint event loop.
fn apply_update(c: &UpdateCtx<'_>, update: RemoteUpdate) {
    let (ui, render_ctx, state) = (c.ui, c.render_ctx, c.state);
    match update {
        RemoteUpdate::Connected { info, .. } => {
            // A no-op for every chunk after the epoch's first (the machine is
            // already `RemoteRendering`).
            let actions = lock(state)
                .handoff
                .handle(HandoffEvent::RemoteStreamStarted);
            apply_actions(&actions, render_ctx, state);
            {
                let mut s = lock(state);
                s.worker_label = backend_label(info.render.as_ref());
                s.remote_hdr = Some(info.render.as_ref().is_some_and(|r| r.hdr));
            }
            sync_served_by_to_ui(ui, render_ctx, state);
        }
        RemoteUpdate::Frame {
            request_id,
            samples_done,
        } => {
            tracing::trace!("remote chunk {request_id}: {samples_done} samples done");
            if let Some(lane) = lock(state).live_lane.as_mut() {
                lane.observe_progress(request_id, samples_done, Instant::now());
            }
            redraw_from_epoch(ui, render_ctx, state, c.width, c.height);
        }
        RemoteUpdate::Preview { .. } => {
            redraw_from_epoch(ui, render_ctx, state, c.width, c.height);
        }
        RemoteUpdate::Progress {
            request_id,
            samples_done,
        } => {
            if let Some(lane) = lock(state).live_lane.as_mut() {
                lane.observe_progress(request_id, samples_done, Instant::now());
            }
        }
        RemoteUpdate::Done {
            request_id,
            cancelled,
        } => {
            if cancelled {
                // A drag/scene-change cancel already abandoned the lane before this
                // arrived (nothing left to do, caught by `chunk_paused` finding no
                // matching in-flight chunk below); a suspend cancel
                // (`poll::suspend_live_remote`) left the lane in place, waiting for
                // exactly this confirmation to merge the chunk's valid prefix.
                on_chunk_paused(state, request_id);
            } else {
                // The final display frame may have been coalesced away by the gate.
                if lane_is_display_only(state) {
                    show_display_frame(ui, state, c.chunk_accumulator, c.width, c.height);
                }
                on_chunk_done(ui, c.ui_weak, render_ctx, state, request_id);
            }
        }
        RemoteUpdate::Failed {
            request_id,
            message,
        } => on_chunk_failed(ui, c.ui_weak, render_ctx, state, request_id, &message),
        RemoteUpdate::Unsupported {
            request_id,
            message,
        } => {
            if lane_is_display_only(state) {
                on_display_only_refused(ui, render_ctx, state, &message);
            } else {
                on_chunk_failed(ui, c.ui_weak, render_ctx, state, request_id, &message);
            }
        }
        RemoteUpdate::DisplayFrame {
            request_id,
            samples_done,
        } => {
            tracing::trace!("remote display frame {request_id}: {samples_done} samples");
            show_display_frame(ui, state, c.chunk_accumulator, c.width, c.height);
        }
        RemoteUpdate::CapabilityChanged { render, .. } => {
            // A coordinator gained or lost joined workers: keep "served by" true, and
            // its HDR-map support current for the next settle.
            {
                let mut s = lock(state);
                s.worker_label = backend_label(render.as_ref());
                s.remote_hdr = Some(render.as_ref().is_some_and(|r| r.hdr));
            }
            sync_served_by_to_ui(ui, render_ctx, state);
        }
        // Only a `FinalImageRequest` produces one, and the live view never sends it.
        RemoteUpdate::FinalImage { .. } => {}
    }
}

fn lane_is_display_only(state: &Arc<Mutex<Orchestrator>>) -> bool {
    lock(state)
        .live_lane
        .as_ref()
        .is_some_and(LiveLane::is_display_only)
}

/// The decoded RGBA8 of `accumulator`'s latest display frame (final-picture live
/// transfer), `None` before the first one arrived. Pure: no Slint, no lock.
///
/// # Errors
///
/// A message when the frame's size is not the epoch's `width x height` or it does not
/// decode (`indicatrix_net::display::decode_rgba8`'s bounded checks).
fn display_frame_rgba(
    accumulator: &Accumulator,
    width: u32,
    height: u32,
) -> Result<Option<Vec<u8>>, String> {
    let Some(frame) = accumulator.last_display_frame() else {
        return Ok(None);
    };
    if (frame.width, frame.height) != (width, height) {
        return Err(format!(
            "a {}x{} display frame for a {width}x{height} view",
            frame.width, frame.height
        ));
    }
    indicatrix_net::display::decode_rgba8(frame.encoding, width, height, &frame.bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Shows the chunk's latest display frame as the settled image -- already tone-mapped
/// and denoised by the remote, so no local denoise/tonemap touches it.
fn show_display_frame(
    ui: &MainWindow,
    state: &Arc<Mutex<Orchestrator>>,
    chunk_accumulator: &Mutex<Accumulator>,
    width: u32,
    height: u32,
) {
    let decoded = display_frame_rgba(
        &chunk_accumulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
        width,
        height,
    );
    match decoded {
        Ok(Some(rgba)) => {
            lock(state).last_redraw_at = Some(Instant::now());
            push_image(ui, width, height, &rgba);
        }
        Ok(None) => {}
        Err(message) => tracing::warn!("dropping a remote display frame: {message}"),
    }
}

/// The remote refused a display-only request (`UNSUPPORTED_REQUEST`: a plain worker).
/// Remember it for this connection, drop the epoch, and let the next poll tick
/// re-dispatch the settled view with full data -- with ONE note.
fn on_display_only_refused(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    message: &str,
) {
    {
        let mut s = lock(state);
        s.display_only_refused = true;
        s.remote_handle = None;
        if let Some(mut lane) = s.live_lane.take() {
            lane.abandon();
        }
        s.lane_scene = None;
    }
    render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .release_remote();
    let actions = lock(state).handoff.handle(HandoffEvent::RemoteFailed);
    apply_actions(&actions, render_ctx, state);
    show_toast(
        ui,
        &format!(
            "The remote cannot send finished live frames ({message}); using full data \
             instead."
        ),
        "info",
    );
}

/// Requests the epoch's next chunk if [`live_remote_allowed`] (a hidden viewport, a
/// Pause, or a running export just pauses the lane between chunks; `poll::
/// resume_idle_lane` picks it up again once allowed), and finishes the epoch once the
/// lane has nothing left to claim.
pub(super) fn continue_lane(
    ui: &MainWindow,
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    if live_remote_allowed(render_ctx) && dispatch_next_chunk(ui_weak, render_ctx, state) {
        return;
    }
    let finished = lock(state)
        .live_lane
        .as_ref()
        .is_some_and(LiveLane::is_finished);
    if finished {
        finish_epoch(ui, render_ctx, state);
    }
}

/// A chunk's `DONE` (not cancelled): merge it into the epoch, then continue the lane.
fn on_chunk_done(
    ui: &MainWindow,
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    request_id: u32,
) {
    let merged = lock(state)
        .live_lane
        .as_mut()
        .and_then(|lane| lane.chunk_done(request_id, Instant::now()));
    if merged.is_none() {
        return;
    }
    lock(state).remote_handle = None;
    continue_lane(ui, ui_weak, render_ctx, state);
}

/// The in-flight chunk `request_id`'s confirmed cancellation
/// (`RemoteUpdate::Done{cancelled: true}`) after `poll::suspend_live_remote` asked for
/// it: merges its valid prefix via `LiveLane::chunk_paused` (unlike [`on_chunk_done`],
/// counting neither a failure nor a rate sample -- the worker was cut off, not slow or
/// broken) and clears `remote_handle`. Deliberately does NOT call [`continue_lane`]:
/// the lane is left idle on purpose, for `poll::resume_idle_lane` to continue once
/// `live_remote_allowed` again -- calling it here would just re-check the same
/// still-suspended condition a poll tick later. A no-op if `request_id` doesn't match
/// the lane's in-flight chunk (already superseded by a drag/scene-change abandon, which
/// takes the lane away entirely).
fn on_chunk_paused(state: &Arc<Mutex<Orchestrator>>, request_id: u32) {
    let merged = lock(state)
        .live_lane
        .as_mut()
        .and_then(|lane| lane.chunk_paused(request_id));
    if merged.is_none() {
        return;
    }
    lock(state).remote_handle = None;
}

/// A chunk failed (worker error, transport error or liveness timeout): the lane keeps
/// its valid prefix and hands the remainder back. After too many failures in a row the
/// lane gives up for this epoch with ONE status note: under `Both` the image finishes
/// locally (the merged remote prefix stays in it); under `RemoteOnly` the epoch is
/// released and local rendering restarts, exactly as a failed remote render always did.
fn on_chunk_failed(
    ui: &MainWindow,
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    request_id: u32,
    message: &str,
) {
    let verdict = lock(state)
        .live_lane
        .as_mut()
        .and_then(|lane| lane.chunk_failed(request_id));
    lock(state).remote_handle = None;
    match verdict {
        None => {}
        Some(ChunkVerdict::Continue) => {
            tracing::warn!("remote chunk {request_id} failed ({message}); retrying");
            continue_lane(ui, ui_weak, render_ctx, state);
        }
        Some(ChunkVerdict::GaveUp) => {
            // Never re-dispatched for this exact scene and mode (see
            // `decisions::should_redispatch`); the next scene change or mode switch is a
            // new epoch and may try the worker again.
            let mode = render_ctx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .live_compute_target;
            {
                let mut s = lock(state);
                s.gave_up_for = s
                    .live_lane
                    .as_ref()
                    .map(|lane| (lane.epoch().scene_generation(), mode));
            }
            let actions = lock(state).handoff.handle(HandoffEvent::RemoteFailed);
            apply_actions(&actions, render_ctx, state);
            if is_combining(render_ctx) {
                show_toast(
                    ui,
                    &format!(
                        "Remote worker failed twice in a row ({message}); finishing this \
                         image locally."
                    ),
                    "info",
                );
            } else {
                {
                    let mut s = lock(state);
                    s.live_lane = None;
                    s.lane_scene = None;
                }
                render_ctx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .release_remote();
                show_toast(ui, &format!("Remote render failed: {message}"), "error");
            }
        }
    }
}

/// The lane claimed and finished everything it could: show the final remote image
/// (`RemoteOnly`) and tell the handoff machine the remote render is done.
/// `ctx.remote_active` is deliberately NOT cleared -- the settled image keeps
/// combining (and, in `RemoteOnly`, local stays paused) until a real invalidation.
fn finish_epoch(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    let Some((width, height)) = lock(state)
        .live_lane
        .as_ref()
        .map(|lane| lane.epoch().dimensions())
    else {
        return;
    };
    redraw_from_epoch(ui, render_ctx, state, width, height);
    let actions = lock(state).handoff.handle(HandoffEvent::RemoteDone);
    apply_actions(&actions, render_ctx, state);
    sync_served_by_to_ui(ui, render_ctx, state);
    lock(state).remote_handle = None;
}

/// Reads the epoch's merged remote sum (finished chunks plus the in-flight one) and
/// its matching sample count, plus the pose/geometry/denoise-toggle state, decides what
/// to display, and pushes the result to the viewport image. Only for
/// `LiveComputeTarget::RemoteOnly` -- a no-op while combining (the render thread's
/// display cycle owns the image then) or when no epoch is live.
///
/// The color is `remote_sum / remote_count` with the count read under the SAME locks as
/// the sum (`LiveEpoch::remote_snapshot`), and that same count is what the background
/// denoise generation receives, so the tone mapper and the denoiser always divide by
/// the count that actually matches the radiance.
///
/// This runs on the Slint UI/event-loop thread, for every chunk `Frame`/`Preview` and
/// the epoch's end -- which, mid-render, can arrive many times per second. That is
/// exactly why the actual (multi-second at 4K) denoise pass must never run inline here
/// -- this function only does cheap work synchronously (a plain tonemap, tens of
/// milliseconds even at 4K) and defers the expensive pass to a background thread via
/// `spawn_denoise_generation`, swapping in its result on a later redraw once
/// `adopt_ready_denoise` confirms it is ready and still valid for the pose on screen.
///
/// `state`'s lock is taken briefly, several times, rather than once for the whole
/// function -- never held across the plain tonemap below (see
/// `Orchestrator::last_redraw_at`'s "tens of milliseconds even at 4K, not free" doc
/// comment): `handle_remote_update`'s own rate-limit check takes this same lock
/// SYNCHRONOUSLY on the connection thread and must never wait out a tonemap.
pub(super) fn redraw_from_epoch(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
    width: u32,
    height: u32,
) {
    if is_combining(render_ctx) {
        return;
    }
    // A final-picture epoch sums no radiance: its image is the remote's display frames
    // (`show_display_frame`), never a tonemap of the (empty) epoch sums.
    let Some(epoch) = lock(state)
        .live_lane
        .as_ref()
        .filter(|lane| !lane.is_display_only())
        .map(|lane| Arc::clone(lane.epoch()))
    else {
        return;
    };
    if epoch.dimensions() != (width, height) {
        return;
    }
    let (buffer, remote_count) = epoch.remote_snapshot();
    let samples_done = remote_count.max(1);
    let (yaw, pitch, distance, planes, tools, denoise_enabled) = {
        let ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        (
            ctx.yaw,
            ctx.pitch,
            ctx.distance,
            ctx.active_planes.clone(),
            ctx.active_tools.clone(),
            ctx.denoise_enabled,
        )
    };
    let stone = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    let desired_key = GuideCache::key_for_geom(width, height, yaw, pitch, distance, stone);

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
                        .ensure_geom(width, height, yaw, pitch, distance, stone)
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
                            tools: tools.as_ref().clone(),
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

    push_image(ui, width, height, &bytes);
}

/// Shows `rgba` (`width * height * 4` bytes) as the viewport's rendered image.
fn push_image(ui: &MainWindow, width: u32, height: u32, rgba: &[u8]) {
    let mut fb = crate::bridge::pixel_buffer::FramebufferTransfer::new(width, height);
    let image = fb.copy_from_gpu_slice(rgba);
    ui.global::<ViewportModel>()
        .set_render_image(slint::Image::from_rgba8(image));
    ui.global::<ViewportModel>().set_has_render(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_net::messages::{DisplayEncoding, DisplayFrameHeader, StreamEvent};

    // ---- display_frame_rgba: the final-picture live transfer's frame -> image path --

    /// An accumulator holding `rgba` as request 1's latest display frame, encoded the
    /// way a coordinator sends it.
    fn with_display_frame(width: u32, height: u32, rgba: &[u8]) -> Accumulator {
        let png = indicatrix_net::display::encode_rgba8(DisplayEncoding::Png, width, height, rgba)
            .expect("encodes");
        let mut acc = Accumulator::new(width, height);
        acc.begin_request(1);
        let header = DisplayFrameHeader {
            request_id: 1,
            samples_done: 32,
            width,
            height,
            encoding: DisplayEncoding::Png,
            payload_len: png.len() as u32,
        };
        acc.apply(&StreamEvent::DisplayFrame(header), Some(&png))
            .expect("applies");
        acc
    }

    #[test]
    fn a_display_frame_decodes_to_exactly_the_pixels_the_remote_sent() {
        let rgba: Vec<u8> = (0..3 * 2 * 4).map(|i| (i * 7 % 256) as u8).collect();
        let acc = with_display_frame(3, 2, &rgba);
        assert_eq!(display_frame_rgba(&acc, 3, 2), Ok(Some(rgba)));
    }

    #[test]
    fn no_display_frame_yet_shows_nothing_and_a_mis_sized_one_is_refused() {
        assert_eq!(display_frame_rgba(&Accumulator::new(2, 2), 2, 2), Ok(None));
        let acc = with_display_frame(2, 2, &[5_u8; 16]);
        assert!(
            display_frame_rgba(&acc, 4, 4).is_err(),
            "a frame for another view size never reaches the viewport"
        );
    }

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
