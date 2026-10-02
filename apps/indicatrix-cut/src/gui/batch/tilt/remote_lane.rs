//! The tilt batch's REMOTE dispatchers -- split from `engine.rs` (which keeps the
//! resolve/compute/dispatch of one design and the shared bookkeeping) so each file stays
//! readable.
//!
//! `AppSettings::remote_batch_lanes` dispatchers run [`run_remote_lane`] at once over the
//! batch's one shared queue (`gui::batch::remote_dispatch`), each keeping one whole
//! design's four-axis sweep in flight. A design stays all-or-nothing: one request, one
//! reply, never split between dispatchers, so a design is either saved with all four
//! axes or handed to the local lane (or counted failed) as a whole.

use super::engine::{
    LaneShared, increment_completed, panic_message, process_remote_entry, push_progress,
    update_remote,
};
use crate::{
    MainWindow,
    gui::batch::remote_dispatch::{DispatcherGroup, RemoteStatus},
    settings::WorkerSettings,
};
use slint::Weak;
use std::{
    panic::{self, AssertUnwindSafe},
    sync::atomic::Ordering,
};
use tracing::warn;

/// Dispatches one claimed design to the remote and returns whether its curves were
/// computed and saved. A panic anywhere in the attempt counts as a failure of this one
/// design. The design is counted in flight on the remote from the moment its title is
/// known (it resolved) until the attempt ends; a design that never resolved was never
/// sent, so it never counts.
fn dispatch_design(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    worker: Option<&WorkerSettings>,
    entry_id: i64,
) -> bool {
    let mut on_remote = false;
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        process_remote_entry(shared.ctx, worker, entry_id, shared.cancel, |title| {
            on_remote = true;
            update_remote(shared.progress, |remote| remote.item_started(title));
            push_progress(
                ui_weak,
                shared.design_total,
                shared.local_lane_total,
                shared.progress,
            );
        })
    }));
    if on_remote {
        update_remote(shared.progress, RemoteStatus::item_ended);
    }
    match result {
        Ok(saved) => saved,
        Err(payload) => {
            warn!(
                "Remote tilt-curve dispatch panicked for entry {entry_id}: {}",
                panic_message(&*payload)
            );
            false
        }
    }
}

/// Accounts for a design the remote has settled: counted computed when it was saved;
/// otherwise handed to the local lane's retry pile (`fallback_to_local`) or counted
/// failed for good.
fn settle_design(shared: &LaneShared<'_>, entry_id: i64, saved: bool, fallback_to_local: bool) {
    if saved {
        shared.tally.computed.fetch_add(1, Ordering::Relaxed);
        increment_completed(shared.progress);
    } else if fallback_to_local {
        // Not accounted for yet -- the local lane will attempt this exact design
        // next, and IT is the one that finally tallies/completes it. See
        // `gui::batch::batch_queue`'s module doc comment for why this never goes
        // back to the shared pool.
        shared.queue.return_to_local(entry_id);
    } else {
        // `RemoteOnly`: no local fallback exists, so this failure is final.
        shared.tally.failed.fetch_add(1, Ordering::Relaxed);
        increment_completed(shared.progress);
    }
}

/// Runs ONE remote dispatcher: claims fresh designs via `WorkQueue::claim_shared` only
/// (never a local-retried one -- that pile is reserved for the local lane, see
/// `gui::batch::batch_queue`'s doc comment) until the shared pool is empty. The batch
/// runs `shared.remote_lane_total` of these at once; when the LAST one ends,
/// `dispatchers` raises `remote_lane_done` so the local lane knows no further requeues
/// are coming.
///
/// `worker` is `None` only for `RemoteOnly` with no worker configured (see
/// [`process_remote_entry`]'s own doc comment); `fallback_to_local` is `true` only for
/// `LiveComputeTarget::Both` -- for `RemoteOnly` every failure is final and tallied
/// `failed` directly: a remote failure is reported as failed and never silently
/// re-rendered locally.
pub(super) fn run_remote_lane(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    worker: Option<&WorkerSettings>,
    fallback_to_local: bool,
    dispatchers: &DispatcherGroup<'_>,
) {
    // Declared first, so it drops LAST: after every `return_to_local` call below (all
    // inside the loop) and after the final progress push -- see
    // `gui::batch::batch_queue`'s doc comment for why the local lane's own stop
    // condition depends on that ordering.
    let _counted_out_on_drop = dispatchers.guard();
    update_remote(shared.progress, RemoteStatus::dispatcher_started);
    push_progress(
        ui_weak,
        shared.design_total,
        shared.local_lane_total,
        shared.progress,
    );

    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(entry_id) = shared.queue.claim_shared() else {
            break;
        };
        let saved = dispatch_design(shared, ui_weak, worker, entry_id);
        settle_design(shared, entry_id, saved, fallback_to_local);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );
    }

    update_remote(shared.progress, RemoteStatus::dispatcher_ended);
    push_progress(
        ui_weak,
        shared.design_total,
        shared.local_lane_total,
        shared.progress,
    );
}
