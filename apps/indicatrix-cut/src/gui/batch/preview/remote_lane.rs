//! The preview batch's REMOTE lane and its failure policy -- split from `engine.rs`
//! (which keeps the per-item local render and the shared bookkeeping) so each file
//! stays readable.
//!
//! Before this policy existed, a remote that failed an item cost the lane nothing: it
//! returned the item to the local pile and claimed the next one at once, with no log
//! line and no toast. A remote failing fast drained the entire shared pool into local
//! retries within seconds and the lane exited for good; a remote failing slowly cycled
//! through titles producing nothing. Now every failure is logged, consecutive failures
//! back off (1, 2, 4, 8 s), and the fifth in a row parks the lane for two minutes with
//! one toast -- see [`crate::gui::batch::remote_backoff`] for the schedule. The wait
//! happens BEFORE an item is claimed, so a failing remote never holds work hostage.

use super::engine::{
    BatchContext, LaneShared, PREVIEW_MAX_BOUNCES, PreviewItem, RecordRevision, ResolvedDesign,
    finish_item, finish_item_at_revision, panic_message, push_progress, resolve_design,
    set_remote_status,
};
use crate::{
    MainWindow,
    bridge::{
        preview_render::{self, PreviewJob, PreviewView},
        preview_wait::RemoteShortfall,
    },
    gui::batch::remote_backoff::{REMOTE_WAIT_SLICE, RemoteBackoff, wait_unless},
    settings::WorkerSettings,
};
use slint::Weak;
use std::{
    panic::{self, AssertUnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use tracing::{info, warn};

/// The toast shown once, when the remote lane starts sitting a failing remote out.
const SIT_OUT_TOAST: &str = "Remote worker is failing every preview request -- remote lane \
                             paused, rendering locally (see indicatrix-cut.log)";

/// What one remote attempt on one claimed item came to.
enum Attempt {
    /// The remote produced the PNG.
    Rendered(Vec<u8>),
    /// The remote was tried and fell short; counts against [`LaneHealth`].
    Failed(RemoteShortfall),
    /// No attempt was possible -- the design did not resolve, or `RemoteOnly` has no
    /// worker configured. Says nothing about the remote's health, so it never backs off.
    NotAttempted,
}

/// How the remote has been doing this batch.
#[derive(Default)]
struct LaneHealth {
    backoff: RemoteBackoff,
    /// Whether the sit-out toast has already been shown -- once per batch, not once per
    /// sit-out, so a permanently dead remote does not re-toast every two minutes.
    toasted: bool,
}

/// Runs `f` (one REMOTE view's render) under `catch_unwind`, so a panic tracing this one
/// view can never take the rest of the batch down with it -- see this group's `mod.rs`
/// doc comment's "Panic isolation" section. A panic becomes a [`RemoteShortfall::Failed`]
/// so it goes through the same failure policy as any other remote failure.
fn catch_render_checked(
    f: impl FnOnce() -> Result<Vec<u8>, RemoteShortfall>,
) -> Result<Vec<u8>, RemoteShortfall> {
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        Err(RemoteShortfall::Failed(format!(
            "render panicked: {}",
            panic_message(&*payload)
        )))
    })
}

/// Renders `resolved`'s `view` against `worker` through
/// `bridge::preview_render::render_view_remote_checked`, which bounds the wait with
/// progress and wall-clock deadlines and names the reason for any shortfall.
fn render_item_remote(
    ctx: &BatchContext<'_>,
    worker: &WorkerSettings,
    resolved: &ResolvedDesign,
    view: PreviewView,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, RemoteShortfall> {
    let job = PreviewJob {
        planes: &resolved.planes,
        material: &resolved.material,
        size: ctx.preview_size,
        spp: ctx.preview_spp,
        max_bounces: PREVIEW_MAX_BOUNCES,
    };
    catch_render_checked(|| preview_render::render_view_remote_checked(&job, view, worker, cancel))
}

/// If the remote is in a backoff or sit-out window, shows that in the remote status line
/// and waits it out -- before the caller claims anything. Returns early on Cancel, and as
/// soon as the shared pool is empty (the local lanes took the rest), so a sit-out can
/// never hold a finished batch open.
fn wait_out_failures(shared: &LaneShared<'_>, ui_weak: &Weak<MainWindow>, backoff: &RemoteBackoff) {
    let Some(delay) = backoff.wait_needed(Instant::now()) else {
        return;
    };
    let title = format!(
        "Remote paused after {} failure(s) -- retrying in {delay:.0?}",
        backoff.consecutive_failures()
    );
    set_remote_status(shared.progress, true, &title);
    push_progress(
        ui_weak,
        shared.design_total,
        shared.local_lane_total,
        shared.progress,
    );
    wait_unless(delay, REMOTE_WAIT_SLICE, || {
        shared.cancel.load(Ordering::Relaxed) || shared.queue.shared_is_empty()
    });
}

/// Tries `resolved` on the remote, if there is both a design and a worker to try it with.
fn attempt_remote(
    shared: &LaneShared<'_>,
    worker: Option<&WorkerSettings>,
    resolved: Option<&ResolvedDesign>,
    view: PreviewView,
) -> Attempt {
    match (resolved, worker) {
        (Some(r), Some(w)) => match render_item_remote(shared.ctx, w, r, view, shared.cancel) {
            Ok(bytes) => Attempt::Rendered(bytes),
            Err(shortfall) => Attempt::Failed(shortfall),
        },
        _ => Attempt::NotAttempted,
    }
}

/// Logs a remote success, noting a recovery if failures preceded it, and resets the
/// backoff.
fn note_success(health: &mut LaneHealth) {
    let failures = health.backoff.consecutive_failures();
    if failures > 0 {
        info!(recovered_after = failures, "preview remote lane recovered");
    }
    health.backoff.record_success();
}

/// Logs a remote failure and advances the backoff; shows the sit-out toast the first
/// time the lane starts sitting the remote out. A user Cancel is not a failure of the
/// remote and is ignored. `RemoteOnly` (`fallback_to_local == false`) has no local lane
/// to hand work to, so it keeps its per-item failures but neither pauses nor toasts.
fn note_failure(
    health: &mut LaneHealth,
    ui_weak: &Weak<MainWindow>,
    item: PreviewItem,
    shortfall: &RemoteShortfall,
    fallback_to_local: bool,
) {
    if matches!(shortfall, RemoteShortfall::Cancelled) {
        return;
    }
    let retry_in = health.backoff.record_failure(Instant::now());
    warn!(
        entry_id = item.entry_id,
        view = ?item.view,
        %shortfall,
        consecutive = health.backoff.consecutive_failures(),
        ?retry_in,
        "preview remote lane: item failed; requeued locally"
    );
    if fallback_to_local && health.backoff.sitting_out() && !health.toasted {
        health.toasted = true;
        let _ = ui_weak.upgrade_in_event_loop(|ui| {
            crate::gui::show_toast(&ui, SIT_OUT_TOAST, "warning");
        });
    }
}

/// Hands back an item the remote did not render: to the local lane's retry pile when
/// there is a fallback, otherwise counted `failed` for good.
fn dispose_unrendered(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    item: PreviewItem,
    fallback_to_local: bool,
) {
    if fallback_to_local {
        // Not accounted for yet -- the local lane will attempt this exact item next,
        // and IT is the one that finally tallies/completes its design. See
        // `gui::batch::batch_queue`'s module doc comment for why this never goes back
        // to the shared pool.
        shared.queue.return_to_local(item);
    } else {
        // `RemoteOnly`: no local fallback exists, so this failure is final.
        finish_item(shared, ui_weak, item.entry_id, item.view, None);
    }
}

/// Runs the REMOTE lane: claims fresh items via `WorkQueue::claim_shared` only (never
/// a local-retried one -- that pile is reserved for the local lane, see
/// `gui::batch::batch_queue`'s doc comment) until the shared pool is empty, then
/// signals `remote_lane_done` so the local lane knows no further requeues are coming.
///
/// `worker` is `None` only for `RemoteOnly` with no worker configured -- treated as an
/// immediate shortfall (no attempt possible) rather than a connection failure, so this
/// never touches the network in that case. `fallback_to_local` is `true` only for
/// `LiveComputeTarget::Both` -- for `RemoteOnly` every failure is final and tallied
/// `failed` directly: a remote failure is reported as failed and never silently
/// re-rendered locally.
///
/// Consecutive remote failures back off before the NEXT claim (`fallback_to_local`
/// only: with no local lane to take the work, pausing would just idle the batch).
pub(super) fn run_remote_lane(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    worker: Option<&WorkerSettings>,
    fallback_to_local: bool,
    remote_lane_done: &AtomicBool,
) {
    let mut health = LaneHealth::default();
    loop {
        if fallback_to_local {
            wait_out_failures(shared, ui_weak, &health.backoff);
        }
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(item) = shared.queue.claim_shared() else {
            break;
        };

        let resolved = resolve_design(shared.ctx, item.entry_id);
        let title = resolved
            .as_ref()
            .map_or_else(|| format!("Design #{}", item.entry_id), |r| r.title.clone());
        set_remote_status(shared.progress, true, &title);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );

        match attempt_remote(shared, worker, resolved.as_ref(), item.view) {
            Attempt::Rendered(bytes) => {
                note_success(&mut health);
                let revision = resolved
                    .as_ref()
                    .map_or(RecordRevision::Unknown, |r| r.revision);
                finish_item_at_revision(
                    shared,
                    ui_weak,
                    item.entry_id,
                    item.view,
                    Some(bytes),
                    revision,
                );
            }
            Attempt::Failed(shortfall) => {
                note_failure(&mut health, ui_weak, item, &shortfall, fallback_to_local);
                dispose_unrendered(shared, ui_weak, item, fallback_to_local);
            }
            Attempt::NotAttempted => dispose_unrendered(shared, ui_weak, item, fallback_to_local),
        }
    }
    set_remote_status(shared.progress, false, "");
    push_progress(
        ui_weak,
        shared.design_total,
        shared.local_lane_total,
        shared.progress,
    );
    // Ordered AFTER every possible `return_to_local` call above (all inside the loop
    // this follows) -- see `gui::batch::batch_queue`'s doc comment for why the local
    // lane's own stop condition depends on that ordering.
    remote_lane_done.store(true, Ordering::Release);
}
