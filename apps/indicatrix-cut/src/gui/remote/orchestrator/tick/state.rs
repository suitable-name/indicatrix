//! [`Orchestrator`]'s own state: the pose snapshot compared every poll tick
//! ([`Pose`]/[`current_pose`]), and the [`lock`] helper every other file in this group
//! uses to reach it. See this group's own `mod.rs` doc comment.

use super::super::generation::{PendingDenoiseGeneration, PendingGuideGeneration};
use crate::{
    bridge::{
        frame_cache::guide_pass::{GuideCache, GuideKey},
        remote::{
            handoff::HandoffMachine,
            live_lane::LiveLane,
            remote_render::{RemoteConnectionHandle, RemoteRenderHandle},
        },
        render_thread::{RedrawGate, RenderContext},
    },
    settings::LiveComputeTarget,
};
use indicatrix_net::SceneState;
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

/// A snapshot of everything the orchestrator watches for a change, cheap to compare
/// every poll tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pose {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) light_yaw: f32,
    pub(super) light_pitch: f32,
}

pub(super) fn current_pose(ctx: &Mutex<RenderContext>) -> Pose {
    let guard = ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Pose {
        yaw: guard.yaw,
        pitch: guard.pitch,
        distance: guard.distance,
        light_yaw: guard.light_yaw,
        light_pitch: guard.light_pitch,
    }
}

/// State shared by the orchestrator's timer tick (always on the Slint event loop) and
/// its `on_update` closure (constructed there, but must be `Send` to cross into
/// `bridge::remote::remote_render`'s worker thread as the closure argument). `Arc<Mutex<_>>`
/// rather than the `Rc<RefCell<_>>` most other UI-thread-only state in this crate uses,
/// for exactly that reason.
///
/// One access does NOT happen on the Slint event loop: `tick::update::
/// handle_remote_update`'s rate-limit/gate check for a `Frame`/`Preview` event takes
/// this lock synchronously on the connection thread, before it ever queues a UI-thread
/// closure (see `bridge::remote::remote_render::connection::mod`'s module doc comment,
/// "Why a busy consumer can never make either deadline fire early"). That access is an
/// O(1) check-and-mutate, so it is only ever held briefly -- which is exactly why
/// `redraw_from_epoch` (the Slint-event-loop side) must never hold this SAME lock
/// across its own tonemap/denoise work: doing so would stall that connection-thread
/// access for however long the redraw took.
pub(super) struct Orchestrator {
    pub(super) handoff: HandoffMachine,
    /// The handle of the chunk request currently on the wire (for `CANCEL`), `None`
    /// between chunks and once the epoch's remote work has ended.
    pub(super) remote_handle: Option<RemoteRenderHandle>,
    /// The current settle's remote chunk lane -- see `bridge::remote::live_lane`.
    /// `None` before the first settle and after any release (drag, failure in
    /// `RemoteOnly`, a release the render loop or a settings change made). Its epoch is
    /// always the same `Arc` as `RenderContext::live_epoch` while both are set;
    /// `poll::reconcile_lane_with_ctx` drops the lane the moment they diverge.
    pub(super) live_lane: Option<LiveLane>,
    /// The fully-resolved scene every chunk of the current epoch is requested with --
    /// captured once at the settle, so every chunk traces the identical scene.
    pub(super) lane_scene: Option<SceneState>,
    /// The `request_id` of the most recently dispatched chunk request, `None` before
    /// the first. `handle_remote_update` drops any incoming update whose
    /// `request_id()` doesn't match: since an update queues via
    /// `upgrade_in_event_loop` rather than running immediately, one for a chunk this
    /// orchestrator has since superseded could otherwise drive state belonging to the
    /// wrong request.
    pub(super) current_request_id: Option<u32>,
    /// The next chunk request id to hand out (see [`Self::alloc_request_id`]). Unique
    /// per persistent connection's lifetime, which is what the worker-side epoch
    /// gating needs.
    pub(super) next_request_id: u32,
    /// Whether the one-time "remote can't render this scene" status note has been
    /// shown for the refusal currently in force (reset once remote is allowed again,
    /// e.g. after the HDR map is cleared).
    pub(super) refusal_noted: bool,
    /// The backend description of the worker serving this session's live chunks (from
    /// its `WELCOME`), for the "served by" indicator -- see `decisions::served_by_label`.
    pub(super) worker_label: String,
    /// The scene generation the poll tick last saw and since when it has been
    /// unchanged -- the re-dispatch debounce (`decisions::RedispatchInputs::scene_stable`).
    pub(super) scene_seen: (u64, Instant),
    /// `(scene generation, compute target)` of the last epoch whose remote lane gave up
    /// after repeated failures: never re-dispatched for exactly that scene and mode.
    pub(super) gave_up_for: Option<(u64, LiveComputeTarget)>,
    /// The persistent mutual-TLS connection to whatever worker `poll_tick` last
    /// dispatched against, reused across settles instead of a fresh handshake each
    /// time. `None` before the first dispatch, or once `connection_is_stale` decides
    /// the cached identity no longer matches what's configured (dropping the `Some`
    /// tears the old connection down). Window close tears it down too, for free, when
    /// the owning `Orchestrator` is dropped along with its `slint::Timer`.
    pub(super) remote_connection: Option<RemoteConnectionHandle>,
    /// Final-picture live transfer: the remote answered a `DisplayOnly` request with
    /// `UNSUPPORTED_REQUEST` (a plain worker), so "Final picture" live transfer falls
    /// back to full data -- with ONE note -- until the connection is replaced (reset
    /// wherever `remote_connection` is dropped for staleness).
    pub(super) display_only_refused: bool,
    /// The live remote's `RenderCapability::hdr` (protocol v14) from its last `WELCOME`
    /// or `CAPABILITY_CHANGED`; `None` before the connection first reported one. The
    /// settle decision (`bridge::remote::live_remote_dispatch`) keeps HDR scenes local
    /// once this says `Some(false)`. Reset wherever `remote_connection` is replaced.
    pub(super) remote_hdr: Option<bool>,
    pub(super) last_pose: Pose,
    pub(super) last_change_at: Instant,
    /// Guide buffers (depth/normal/facet-id) for denoising a remote-sourced merged
    /// image -- see `bridge::frame_cache::guide_pass`'s module docs.
    pub(super) guide_cache: GuideCache,
    /// The guide-buffer prepass currently running for the pose this orchestrator last
    /// dispatched a `RenderRequest` for -- `None` before the first dispatch, or once
    /// its result has been adopted/superseded/cancelled. See
    /// [`PendingGuideGeneration`]'s doc comment.
    pub(super) pending_guide_gen: Option<PendingGuideGeneration>,
    /// The full denoise-and-tonemap pass currently running for `last_denoised`'s (or
    /// the about-to-be-`redraw`n) pose -- `None` whenever nothing is in flight. See
    /// [`PendingDenoiseGeneration`]'s doc comment for why this, unlike
    /// `pending_guide_gen`, gets redispatched once per completed generation rather
    /// than once per `RenderRequest`.
    pub(super) pending_denoise_gen: Option<PendingDenoiseGeneration>,
    /// The most recently completed background denoise, tagged with the pose key
    /// ([`GuideKey`]) it is valid for. `redraw_from_epoch` shows this instead of
    /// re-deriving a plain, noisy tonemap every redraw, so the displayed image doesn't
    /// flicker between denoised and noisy while a fresher generation is still cooking.
    /// Cleared once the pose changes, via the same structural [`GuideKey`] equality
    /// `adopt_ready_guides` uses, not a timer or generation counter.
    pub(super) last_denoised: Option<(GuideKey, Vec<u8>)>,
    /// Coalesces a burst of `RemoteUpdate::Frame`/`Preview` events into at most one
    /// pending `redraw_from_epoch` closure -- see [`RedrawGate`]'s doc comment.
    /// Carries no payload (`()`): whenever the one pending closure runs, it reads
    /// the live epoch/`render_ctx`/this `Orchestrator` fresh, so "latest wins" falls
    /// out for free.
    pub(super) redraw_gate: RedrawGate<()>,
    /// The `Instant` of the last redraw `redraw_from_epoch` actually performed --
    /// `None` before the first one this session. Used to rate-limit Frame/Preview-
    /// triggered redraws to at most once per `REMOTE_REDRAW_MIN_INTERVAL`, mirroring
    /// `render_thread::DENOISE_MIN_INTERVAL`'s role for the local path: a plain
    /// tonemap is "tens of milliseconds" at 1080p+, not free, and a fast remote stream
    /// can otherwise arrive faster than any human can perceive a redraw changing.
    pub(super) last_redraw_at: Option<Instant>,
}

impl Orchestrator {
    /// Hands out the next chunk request id (wrapping, never `0`-special).
    pub(super) const fn alloc_request_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        id
    }
}

/// Locks `state`, recovering from a poisoned mutex the same way every other shared
/// state in this crate does (`std::sync::PoisonError::into_inner`) -- a panic on one
/// event's handling must not permanently wedge every future one.
pub(super) fn lock(state: &Arc<Mutex<Orchestrator>>) -> std::sync::MutexGuard<'_, Orchestrator> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}
