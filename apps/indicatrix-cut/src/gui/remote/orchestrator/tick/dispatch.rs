//! Starting a remote render for a just-settled pose ([`start_remote_render`]), the
//! persistent-connection staleness check that gates reusing a cached one
//! ([`connection_is_stale`]), and building the `indicatrix_net::SceneState` it sends
//! ([`scene_state_from_snapshot`]). See this group's own `mod.rs` doc comment.

use super::{
    state::{Orchestrator, lock},
    update::handle_remote_update,
};
use crate::{
    MainWindow,
    bridge::{
        export_thread::SceneSnapshot,
        frame_cache::guide_pass::GuideCache,
        remote::remote_render::{self, RemoteUpdate},
        render_thread::RenderContext,
    },
    settings::{LiveComputeTarget, WorkerSettings},
};
use indicatrix::optics::raytracer::Camera;
use indicatrix_net::{SceneState, client::Accumulator};
use slint::ComponentHandle;
use std::{
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
};

pub(super) fn start_remote_render(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    worker: WorkerSettings,
    next_request_id: &Rc<AtomicU32>,
    state: &Arc<Mutex<Orchestrator>>,
) {
    // `samples`/`live_compute_target` are read live off `RenderContext` here, the exact
    // same treatment `width`/`height` already get (see this function's own doc comment)
    // -- a remote render always uses whatever sample budget/mode is CURRENTLY
    // configured, not a value captured once at some earlier time.
    let (width, height, samples, live_compute_target) = {
        let ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        (
            ctx.width,
            ctx.height,
            ctx.remote_render_samples,
            ctx.live_compute_target,
        )
    };
    if width == 0 || height == 0 {
        return;
    }
    let combining = matches!(live_compute_target, LiveComputeTarget::Both);
    let snapshot = SceneSnapshot::capture(render_ctx);
    let scene = scene_state_from_snapshot(&snapshot, width, height);

    // Kick off the guide-buffer prepass NOW, at dispatch time, rather than waiting for
    // the first `FRAME`/`PREVIEW` redraw to need it -- camera pose and gem geometry are
    // both already known here, so this overlaps the network round trip and the remote
    // render itself instead of stalling the UI thread on the first post-settle redraw
    // (see `bridge::guide_pass`'s module doc comment). Cancel whatever generation was
    // still running for a previous pose first -- a fresh dispatch always means a fresh
    // pose (`start_remote_render` only ever runs after `HandoffEvent::SettleElapsed`).
    //
    // Skipped entirely for `LiveComputeTarget::Both`: local tracing keeps running for
    // that mode (see `render_thread::mod`'s doc comment), producing its OWN first-hit
    // guide buffers for this exact pose as a side effect of its ordinary trace loop --
    // fresher and cheaper than a separate async prepass, so this would-be-redundant
    // dispatch is skipped rather than racing a second computation of the same thing.
    if !combining {
        let guide_key = GuideCache::key_for(
            width,
            height,
            snapshot.yaw,
            snapshot.pitch,
            snapshot.distance,
            &snapshot.active_planes,
        );
        let guide_camera = Camera::new(snapshot.yaw, snapshot.pitch, snapshot.distance, 42.0);
        let mut s = lock(state);
        if let Some(previous) = s.pending_guide_gen.take() {
            previous.cancel.store(true, Ordering::Relaxed);
        }
        s.pending_guide_gen = Some(super::super::generation::spawn_guide_generation(
            guide_key,
            guide_camera,
            snapshot.active_planes, // moved: `snapshot` isn't used again after this
            width,
            height,
        ));
    }

    let accumulator = Arc::new(Mutex::new(Accumulator::new(width, height)));
    lock(state).accumulator = Some(Arc::clone(&accumulator));

    // Discards the local preview and starts this settle's dispatch in ONE locked
    // mutation, together with the shared-accumulator hand-off the render thread reads
    // (`RenderContext::remote_accumulator`/`remote_reserved_samples`), so that thread
    // can never observe `remote_active`/`dirty` freshly true while still holding a
    // STALE (previous epoch's, or absent) accumulator/reservation -- see
    // `RenderContext::remote_accumulator`'s own doc comment. This is also where
    // `HandoffAction::DiscardLocalPreview`'s actual work happens -- see
    // `apply_actions`'s now-deferred arm for it.
    {
        let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        ctx.remote_active = true;
        ctx.dirty = true;
        ctx.remote_accumulator = combining.then(|| Arc::clone(&accumulator));
        ctx.remote_reserved_samples = if combining { samples } else { 0 };
    }

    let request_id = next_request_id.fetch_add(1, Ordering::Relaxed);
    // Recorded BEFORE the request is ever dispatched below, on this same UI-thread call
    // -- so `handle_remote_update`'s stale-id check always sees the new id in place
    // before any update for it (or a leftover one for whatever this just superseded)
    // could possibly reach the event loop. See `Orchestrator::current_request_id`'s own
    // doc comment.
    lock(state).current_request_id = Some(request_id);
    let ui_weak = ui.as_weak();
    let state_for_updates = Arc::clone(state);
    let render_ctx_for_updates = Arc::clone(render_ctx);
    let accumulator_for_redraw = Arc::clone(&accumulator);

    // Reuse the cached connection when its identity already matches `worker` (the
    // common case once a session is under way), or create one lazily when it doesn't
    // (this settle's own `connection_is_stale`
    // check, above, already cleared a mismatched one; or this is the very first
    // dispatch this session has ever made). This is the only place a
    // `RemoteConnectionHandle` is ever created.
    let mut s = lock(state);
    if s.remote_connection
        .as_ref()
        .is_none_or(|h| h.worker() != &worker)
    {
        s.remote_connection = Some(remote_render::spawn_remote_connection(worker.clone()));
    }
    let handle = s
        .remote_connection
        .as_ref()
        .expect("just ensured Some immediately above")
        .render(
            remote_render::RemoteRenderRequest {
                worker,
                request_id,
                scene,
                first_sample: 0,
                samples,
                width,
                height,
            },
            accumulator,
            move |update: RemoteUpdate| {
                handle_remote_update(
                    &ui_weak,
                    &render_ctx_for_updates,
                    &state_for_updates,
                    &accumulator_for_redraw,
                    width,
                    height,
                    update,
                );
            },
        );
    s.remote_handle = Some(handle);
}

/// Whether the persistently-cached connection identity `cached` should be torn down
/// given the worker `wanted` for the settle currently being checked -- `wanted` being
/// `None` means "no remote worker configured for this settle" (remote compute switched
/// off via `LiveComputeTarget::LocalOnly`, or the configured worker's own entry was
/// removed from `AppSettings::remote_workers`).
///
/// Comparing the WHOLE `WorkerSettings` (not just `address`/`cert_dir`) is deliberately
/// coarse: a `transfer_mode`/`cadence_ms`/`preview_scale` tweak forces a reconnect it
/// doesn't strictly need, but that costs one extra handshake on a settings save
/// (rare, off the camera-drag hot path) in exchange for never having to reason about
/// which subset of fields is "connection identity" versus "per-request preference" --
/// and getting that subset wrong in the other direction (missing a field that DOES
/// affect the certificates used, say) is exactly the kind of mistake this exists to
/// rule out. `address`/`cert_dir` changing MUST force a reconnect either way: a
/// connection already authenticated under the OLD certificates must never be reused as
/// if the new ones had been verified -- see `RemoteConnectionHandle`'s own doc comment.
///
/// Pure and side-effect-free, so this decision is directly unit-testable with plain
/// `WorkerSettings` values -- no socket, no `Orchestrator`, no Slint type involved.
#[must_use]
pub(super) fn connection_is_stale(
    cached: Option<&WorkerSettings>,
    wanted: Option<&WorkerSettings>,
) -> bool {
    match (cached, wanted) {
        (Some(c), Some(w)) => c != w,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Builds the fully-resolved `indicatrix_net::SceneState` a remote worker needs from a
/// local `SceneSnapshot` plus the session's render resolution -- see
/// `indicatrix_net::scene::SceneState`'s own doc comment on why every field must be a
/// resolved value, never a name/id the worker can't look up.
///
/// Frosted girdle: `snapshot.facet_finishes` is already either empty (toggle off at
/// capture time) or `girdle_facet_finishes(&snapshot.active_planes)` (toggle on) -- see
/// `export_thread::scene_snapshot::SceneSnapshot::capture`. `SceneState::girdle_frosted`
/// carries that same on/off bit rather than the resolved list itself (see that field's
/// own doc comment on why), so a non-empty `facet_finishes` here becomes `true`: the
/// remote worker re-derives the identical `Vec<FacetFinish>` from the identical
/// `planes` it already receives below, via the same deterministic
/// `girdle_facet_finishes` function.
fn scene_state_from_snapshot(snapshot: &SceneSnapshot, width: u32, height: u32) -> SceneState {
    SceneState {
        width,
        height,
        yaw: snapshot.yaw,
        pitch: snapshot.pitch,
        distance: snapshot.distance,
        light_yaw: snapshot.light_yaw,
        light_pitch: snapshot.light_pitch,
        exposure: snapshot.exposure,
        max_bounces: snapshot.max_bounces,
        lighting_preset: snapshot.lighting_preset,
        material: snapshot.material.clone(),
        planes: snapshot.active_planes.clone(),
        girdle_frosted: !snapshot.facet_finishes.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- `connection_is_stale`: when the cached persistent connection must be torn
    // down and replaced -- pure logic, no socket, no `Orchestrator`. ------------------

    fn worker_named(name: &str) -> WorkerSettings {
        WorkerSettings {
            name: name.to_string(),
            address: format!("{name}.local:9443"),
            cert_dir: format!("/certs/{name}"),
            ..WorkerSettings::default()
        }
    }

    #[test]
    fn connection_is_stale_is_false_when_nothing_is_cached_yet() {
        // Before the first ever dispatch: nothing to tear down, regardless of whether
        // a worker is configured.
        assert!(!connection_is_stale(None, None));
        assert!(!connection_is_stale(None, Some(&worker_named("a"))));
    }

    #[test]
    fn connection_is_stale_is_false_when_the_cached_worker_is_unchanged() {
        let w = worker_named("a");
        assert!(!connection_is_stale(Some(&w), Some(&w)));
    }

    #[test]
    fn connection_is_stale_is_true_when_the_address_or_cert_dir_differs() {
        let cached = worker_named("a");
        let mut edited_address = cached.clone();
        edited_address.address = "somewhere-else.local:9443".to_string();
        assert!(
            connection_is_stale(Some(&cached), Some(&edited_address)),
            "an edited address must force a reconnect"
        );

        let mut edited_cert_dir = cached.clone();
        edited_cert_dir.cert_dir = "/certs/somewhere-else".to_string();
        assert!(
            connection_is_stale(Some(&cached), Some(&edited_cert_dir)),
            "an edited cert_dir must force a reconnect -- the whole point of mutual \
             TLS here is that a connection authenticated under the OLD certificates \
             must never be reused as if the new ones had been verified"
        );
    }

    #[test]
    fn connection_is_stale_is_true_when_the_configured_worker_was_removed() {
        assert!(
            connection_is_stale(Some(&worker_named("a")), None),
            "no worker configured any more (removed from the list, or remote compute \
             switched off) must tear down a cached connection rather than leave it \
             dangling"
        );
    }

    #[test]
    fn connection_is_stale_is_true_when_a_different_worker_is_now_first() {
        assert!(connection_is_stale(
            Some(&worker_named("a")),
            Some(&worker_named("b"))
        ));
    }
}
