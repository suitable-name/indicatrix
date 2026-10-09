//! Starting a settled epoch's remote work ([`start_remote_render`]), dispatching its
//! chunks one after another on the persistent connection ([`dispatch_next_chunk`]),
//! the persistent-connection staleness check that gates reusing a cached connection
//! ([`connection_is_stale`]), and building the `indicatrix_net::SceneState` every
//! chunk sends ([`scene_state_from_snapshot`]). See this group's own `mod.rs` doc
//! comment.

use super::{
    decisions::wants_display_only,
    state::{Orchestrator, lock},
    update::handle_remote_update,
};
use crate::{
    MainWindow,
    bridge::{
        export_thread::SceneSnapshot,
        frame_cache::guide_pass::GuideCache,
        remote::{
            live_lane::LiveLane,
            remote_render::{self, RemoteRenderRequest, RemoteUpdate},
        },
        render_thread::RenderContext,
        sample_cursor::LiveEpoch,
    },
    settings::{LiveComputeTarget, RemoteEndpoint, WorkerSettings},
};
use indicatrix::{
    geometry::tool::StoneGeometry,
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
};
use indicatrix_net::SceneState;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex, PoisonError, atomic::Ordering},
    time::Instant,
};

/// Starts a settled pose's image epoch: installs a fresh [`LiveEpoch`] (the shared
/// sample cursor over `[0, target_samples)` plus the remote sums) in `RenderContext`
/// together with `remote_active`/`dirty` in ONE locked mutation, creates the epoch's
/// [`LiveLane`], and dispatches its first chunk. `target_samples` is the live view's
/// single global target: local and remote together trace exactly that many samples.
///
/// Callers must already have checked `bridge::remote::live_remote_dispatch` (the HDR
/// guard) -- `poll::poll_tick` does, right before feeding `SettleElapsed`.
///
/// Final-picture live transfer: when the endpoint's live transfer is "Final picture" (and this
/// connection has not refused it, see `decisions::wants_display_only`), the epoch is a
/// display-only one: ONE `DisplayOnly` request over the budget, no local combining
/// (`RenderContext::live_display_only` makes a `Both` epoch act as `RemoteOnly`), no
/// guide prepass (the remote denoises), and the decoded display frames are the image.
pub(super) fn start_remote_render(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    endpoint: RemoteEndpoint,
    state: &Arc<Mutex<Orchestrator>>,
) {
    // Read live off `RenderContext` at dispatch time -- an epoch always uses whatever
    // size/budget/mode is CURRENTLY configured. The scene generation is read in the
    // same lock and BEFORE the scene is captured below: if anything changes after this
    // point (during the capture, or before the render thread's next frame), the render
    // thread sees a different generation than the one stamped into the epoch and
    // releases it rather than merging two scenes (`live_split::epoch_scene_mismatch`).
    let (width, height, target_samples, live_compute_target, scene_generation) = {
        let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        (
            ctx.width,
            ctx.height,
            ctx.target_samples,
            ctx.live_compute_target,
            ctx.scene_generation(),
        )
    };
    if width == 0 || height == 0 || target_samples == 0 {
        return;
    }
    let display_only = wants_display_only(
        live_compute_target,
        endpoint.live_transfer,
        lock(state).display_only_refused,
    );
    let combining = matches!(live_compute_target, LiveComputeTarget::Both) && !display_only;
    // Refuses (see `SceneSnapshot::capture`'s own doc comment) rather than dispatching
    // the wrong stone to a remote worker when the design's material does not resolve --
    // the render loop's own suspended branch (`render_thread::mod`) is what surfaces the
    // refusal to the user; here it is enough to skip this dispatch and let the next
    // settle retry once the material resolves again.
    let Ok(snapshot) = SceneSnapshot::capture(render_ctx) else {
        return;
    };
    let scene = scene_state_from_snapshot(&snapshot, width, height);

    // Kick off the guide-buffer prepass NOW, at dispatch time, rather than waiting for
    // the first remote redraw to need it -- camera pose and gem geometry are both
    // already known here, so this overlaps the network round trip and the remote
    // render itself (see `bridge::frame_cache::guide_pass`'s module doc comment). Cancel whatever
    // generation was still running for a previous pose first.
    //
    // Skipped for `LiveComputeTarget::Both`: local tracing keeps running in that mode,
    // producing its OWN first-hit guide buffers for this exact pose as a side effect of
    // its ordinary trace loop, and the render thread's display cycle (not this
    // orchestrator) denoises the merged image with them. Skipped for a display-only
    // epoch too: the remote's frames arrive already denoised.
    if !combining && !display_only {
        // The path signature refracts at the material's index, so it keys the guides.
        let n_d = snapshot.material.dispersion.n_d();
        let guide_key = GuideCache::key_for_geom(
            width,
            height,
            snapshot.yaw,
            snapshot.pitch,
            snapshot.distance,
            StoneGeometry {
                planes: &snapshot.active_planes,
                tools: &snapshot.tools,
            },
            n_d,
        );
        let guide_camera = Camera::new(
            snapshot.yaw,
            snapshot.pitch,
            snapshot.distance,
            DEFAULT_FOV_DEG,
        );
        let mut s = lock(state);
        if let Some(previous) = s.pending_guide_gen.take() {
            previous.cancel.store(true, Ordering::Relaxed);
        }
        s.pending_guide_gen = Some(super::super::generation::spawn_guide_generation(
            guide_key,
            guide_camera,
            snapshot.active_planes, // moved: `snapshot` isn't used again after this
            snapshot.tools,
            width,
            height,
            n_d,
        ));
    }

    // Discards the local preview and starts this settle's epoch in ONE locked
    // mutation, so the render thread can never observe `remote_active`/`dirty` freshly
    // true while still holding a STALE (previous epoch's, or absent) epoch -- and the
    // `dirty` restarts local accumulation from zero, so the drag-time preview is never
    // summed into the settled image. This is where `HandoffAction::DiscardLocalPreview`'s
    // actual work happens -- see `apply_actions`'s deferred arm for it.
    let epoch = Arc::new(LiveEpoch::new(width, height, target_samples).for_scene(scene_generation));
    {
        let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        ctx.remote_active = true;
        ctx.dirty = true;
        ctx.live_epoch = Some(Arc::clone(&epoch));
        ctx.live_display_only = display_only;
    }

    // Reuse the cached connection when its identity already matches the endpoint (the
    // common case once a session is under way), or create one lazily when it doesn't
    // (`poll_tick`'s `connection_is_stale` check already cleared a mismatched one; or
    // this is the very first dispatch this session has ever made). This is the only
    // place a `RemoteConnectionHandle` is ever created.
    {
        let mut s = lock(state);
        if s.remote_connection
            .as_ref()
            .is_none_or(|h| h.worker() != &endpoint.connection)
        {
            s.remote_connection = Some(remote_render::spawn_remote_connection(endpoint.connection));
            s.display_only_refused = false;
            s.remote_hdr = None;
            #[cfg(feature = "zoning")]
            {
                s.remote_zoning = None;
            }
        }
        s.live_lane = Some(if display_only {
            LiveLane::display_only(epoch)
        } else {
            LiveLane::new(epoch, combining)
        });
        s.lane_scene = Some(scene);
    }
    dispatch_next_chunk(&ui.as_weak(), render_ctx, state);
}

/// Claims the current epoch's next remote chunk and sends it as one
/// `RenderRequest{first_sample, samples}` on the persistent connection, recording its
/// request id as current. Returns `false` (sending nothing) when there is no lane, no
/// connection, or nothing left to claim -- in the last case the lane is now finished.
///
/// Called once per settle by [`start_remote_render`] and then once per finished (or
/// retried) chunk by `update::handle_remote_update`, so the next request goes out the
/// moment the previous chunk's `DONE` arrives.
pub(super) fn dispatch_next_chunk(
    ui_weak: &Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &Arc<Mutex<Orchestrator>>,
) -> bool {
    let mut s = lock(state);
    let request_id = s.alloc_request_id();
    let Orchestrator {
        live_lane,
        lane_scene,
        remote_connection,
        ..
    } = &mut *s;
    let (Some(lane), Some(scene), Some(connection)) = (
        live_lane.as_mut(),
        lane_scene.as_ref(),
        remote_connection.as_ref(),
    ) else {
        return false;
    };
    let Some(chunk) = lane.next_chunk(request_id, Instant::now()) else {
        return false;
    };
    let (width, height) = lane.epoch().dimensions();
    let request = RemoteRenderRequest {
        worker: connection.worker().clone(),
        request_id: chunk.request_id,
        scene: scene.clone(),
        first_sample: chunk.first_sample,
        samples: chunk.samples,
        width,
        height,
        // The live viewport: latency first (a coordinator serves it from its own lane).
        intent: indicatrix_net::messages::RequestIntent::Interactive,
        display_only: lane.is_display_only(),
    };
    let ui_weak = ui_weak.clone();
    let render_ctx = Arc::clone(render_ctx);
    let state_for_updates = Arc::clone(state);
    // The chunk's accumulator also holds its latest display frame (final-picture live
    // transfer), which the update handler decodes.
    let chunk_accumulator = Arc::clone(&chunk.accumulator);
    let handle = connection.render(request, chunk.accumulator, move |update: RemoteUpdate| {
        handle_remote_update(
            &ui_weak,
            &render_ctx,
            &state_for_updates,
            (width, height, &chunk_accumulator),
            update,
        );
    });
    // Recorded on this same UI-thread call, before any update for this request could
    // reach the event loop -- see `Orchestrator::current_request_id`'s doc comment.
    s.current_request_id = Some(request_id);
    s.remote_handle = Some(handle);
    true
}

/// Whether the persistently-cached connection identity `cached` should be torn down
/// given the remote `wanted` for the settle currently being checked -- `wanted` being
/// `None` means "no remote configured for this settle" (remote compute switched off via
/// `LiveComputeTarget::LocalOnly`, or `AppSettings::remote` was removed).
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
        backdrop: snapshot.backdrop,
        surface_glare: snapshot.surface_glare,
        // The concave tools, so the worker traces the same stone as the local tracer; empty
        // for a planar design.
        tools: snapshot.tools.clone(),
        // The loaded HDR map by content hash, else the studio rig.
        environment: crate::bridge::remote::hdr_asset::scene_environment(snapshot.env_map.as_ref()),
        // The material's emitters, so the worker traces the same glow as the local tracer;
        // empty for a non-fluorescent material.
        fluorescence: snapshot.fluorescence.as_ref().clone(),
        head_shadow_deg: snapshot.head_shadow_deg,
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
    fn the_live_dispatch_scene_carries_the_viewports_surface_glare() {
        let ctx = Mutex::new(RenderContext {
            surface_glare: 0.4,
            ..Default::default()
        });
        let snapshot = SceneSnapshot::capture(&ctx).expect("default resolves");
        let state = scene_state_from_snapshot(&snapshot, 64, 64);
        assert_eq!(state.surface_glare.to_bits(), 0.4f32.to_bits());
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
