//! One remote dispatcher's BATCHED mode (protocol v24): a persistent connection, whole
//! designs per request, and the next request queued while the current one traces.
//!
//! The single-picture mode in the parent module opens a fresh connection per picture and
//! lets a GPU worker go idle between pictures. Here the dispatcher
//!
//! 1. claims up to `LaneShared::remote_batch_size` pictures from the shared queue, taking
//!    BOTH views of a design together (`WorkQueue::claim_shared_batch`),
//! 2. resolves each design once and sends the pictures as one `BatchRenderRequest` on its
//!    one [`BatchClient`] connection,
//! 3. keeps up to `MAX_BATCHES_IN_FLIGHT` (two) requests outstanding, claiming and sending
//!    the next the moment a slot frees, and
//! 4. saves each picture as its PNG arrives (`finish_item_at_revision`, the same path a
//!    local render takes).
//!
//! # Failure policy (unchanged per item)
//!
//! A picture that comes back failed, a batch that ends with pictures unanswered, a refused
//! request, a dead connection and a missed deadline all hand the affected pictures back
//! exactly as a failed single request does: to the local pile under `Both`, counted failed
//! under `RemoteOnly` (`dispose_unrendered`). The backoff counts per BATCH failure, not per
//! picture; a picture the worker merely failed (an invalid scene, say) is logged and handed
//! back without touching the backoff, because the connection itself is healthy.
//!
//! # Deadlines and cancel
//!
//! [`BatchWatch`] holds the stall window (no item event or growing heartbeat total for 60
//! s) and a wall allowance per batch scaled by its picture count, started when the batch
//! reaches the front of the worker's queue. A missed deadline cancels every outstanding
//! batch, gives the worker `PREVIEW_CANCEL_ACK_WAIT` to answer, then drops the connection.
//! The batch's Cancel does the same, minus the failure: pictures that finished before it
//! are kept, the rest return to the pool.
//!
//! # Workers that cannot take a batch
//!
//! A coordinator spreads single requests over its joined workers and does not take batches
//! (`BatchClient::takes_batches`); a worker that answers `UNSUPPORTED_REQUEST` is treated
//! the same. Either way the dispatcher returns [`BatchExit::UseSingle`] and the parent
//! module serves the remaining pictures one at a time, exactly as before.

use super::{LaneHealth, dispose_unrendered, note_failure, note_success, wait_out_failures};
use crate::{
    MainWindow,
    bridge::{
        preview_render::{self, PreviewJob},
        preview_wait::{PREVIEW_CANCEL_ACK_WAIT, RemoteShortfall},
        remote::remote_render::{
            BatchClient, BatchEvent, BatchWatch, MAX_BATCH_REQUEST_BYTES, items_fitting,
        },
    },
    gui::batch::preview::engine::{
        LaneShared, PREVIEW_MAX_BOUNCES, PreviewItem, RecordRevision, ResolvedDesign,
        finish_item_at_revision, push_progress, resolve_design, update_remote,
    },
    settings::WorkerSettings,
};
use indicatrix_net::messages::{BatchItem, MAX_BATCHES_IN_FLIGHT, error_codes};
use slint::Weak;
use std::{
    collections::{HashMap, VecDeque},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tracing::{info, warn};

/// How long one poll for the next reply waits.
const POLL: Duration = Duration::from_millis(100);

/// How a dispatcher's batched mode ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BatchExit {
    /// The shared queue is drained (or the batch cancelled); nothing is left to do.
    Finished,
    /// The worker cannot take batches; serve the rest one picture at a time.
    UseSingle,
}

/// Whether `worker` reports a GPU backend in its `WELCOME` -- one handshake probe, used to
/// pick the default dispatcher count. `false` when it cannot be reached (the dispatchers
/// then report the failure the usual way).
pub(in crate::gui::batch::preview) fn remote_is_gpu(worker: &WorkerSettings) -> bool {
    BatchClient::connect(worker).is_ok_and(|client| client.is_gpu())
}

/// One picture of a batch in flight.
struct FlightItem {
    item: PreviewItem,
    item_id: u32,
    revision: RecordRevision,
    /// Answered (saved, failed or handed back) -- never settled twice.
    answered: bool,
}

/// One `BatchRenderRequest` awaiting its replies.
struct Flight {
    batch_id: u32,
    items: Vec<FlightItem>,
}

/// What launching a claim came to.
enum Launch {
    /// Sent; the flight is outstanding.
    Sent(Flight),
    /// Nothing was left to send (every picture was handed back already).
    NothingSent,
    /// The worker cannot take batches; the claim went back to the queue.
    UseSingle,
    /// The connection failed; the claim was handed back and the failure noted.
    Failed,
}

/// The state of one dispatcher's batched run.
struct Run<'r, 's> {
    shared: &'r LaneShared<'s>,
    ui_weak: &'r Weak<MainWindow>,
    worker: &'r WorkerSettings,
    fallback_to_local: bool,
    health: &'r mut LaneHealth,
    client: Option<BatchClient>,
    flights: VecDeque<Flight>,
    watch: Option<BatchWatch>,
    /// When the user's Cancel was passed on to the worker.
    cancel_sent: Option<Instant>,
    /// A worker answered `UNSUPPORTED_REQUEST`: finish what is in flight, then go single.
    unsupported: bool,
}

/// Runs the batched mode for one dispatcher until the queue is drained, the batch is
/// cancelled or the worker turns out not to take batches.
pub(super) fn run_batch_loop(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    worker: &WorkerSettings,
    fallback_to_local: bool,
    health: &mut LaneHealth,
) -> BatchExit {
    let mut run = Run {
        shared,
        ui_weak,
        worker,
        fallback_to_local,
        health,
        client: None,
        flights: VecDeque::new(),
        watch: None,
        cancel_sent: None,
        unsupported: false,
    };
    run.run()
}

impl Run<'_, '_> {
    fn push(&self) {
        push_progress(
            self.ui_weak,
            self.shared.design_total,
            self.shared.local_lane_total,
            self.shared.progress,
        );
    }

    fn cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::Relaxed)
    }

    fn run(&mut self) -> BatchExit {
        loop {
            if self.flights.is_empty() {
                if self.unsupported {
                    return BatchExit::UseSingle;
                }
                if self.fallback_to_local {
                    wait_out_failures(self.shared, self.ui_weak, &self.health.backoff);
                }
                if self.cancelled() {
                    return BatchExit::Finished;
                }
            }
            if let Some(exit) = self.top_up() {
                return exit;
            }
            if self.flights.is_empty() {
                if self.cancelled() || self.shared.queue.shared_is_empty() {
                    return BatchExit::Finished;
                }
                continue;
            }
            self.pass_on_cancel();
            self.poll_once();
            self.check_deadlines();
        }
    }

    /// Claims and sends batches until two are outstanding. `Some(exit)` ends the run.
    fn top_up(&mut self) -> Option<BatchExit> {
        while !self.cancelled() && !self.unsupported && self.flights.len() < MAX_BATCHES_IN_FLIGHT {
            let claimed = self.shared.queue.claim_shared_batch(
                self.shared.remote_batch_size.max(1),
                |a: &PreviewItem, b: &PreviewItem| a.entry_id == b.entry_id,
            );
            if claimed.is_empty() {
                break;
            }
            match self.launch(claimed) {
                Launch::Sent(flight) => {
                    if self.flights.is_empty() {
                        self.watch = Some(BatchWatch::new(Instant::now(), flight.items.len()));
                    }
                    self.flights.push_back(flight);
                }
                Launch::NothingSent => {}
                Launch::UseSingle => return Some(BatchExit::UseSingle),
                Launch::Failed => break,
            }
        }
        None
    }

    /// Resolves the claim's designs, builds and sends the request.
    fn launch(&mut self, claimed: Vec<PreviewItem>) -> Launch {
        let mut sendable = self.build_sendable(claimed);
        if sendable.is_empty() {
            return Launch::NothingSent;
        }
        self.trim_to_request_cap(&mut sendable);

        if self.client.is_none() {
            match BatchClient::connect(self.worker) {
                Ok(client) if client.takes_batches() => self.client = Some(client),
                Ok(_) => {
                    info!("preview batch: the remote is a coordinator; sending pictures singly");
                    let items: Vec<PreviewItem> = sendable.into_iter().map(|(i, ..)| i).collect();
                    self.shared.queue.return_to_shared_front(items);
                    return Launch::UseSingle;
                }
                Err(e) => {
                    return self.fail_launch(sendable, &RemoteShortfall::Failed(e.to_string()));
                }
            }
        }

        // `zoning` builds: a zoned picture goes only to a worker that advertised the zoning
        // capability (otherwise it would render as its base zone). The others are handed back
        // like any picture the remote cannot take -- to the local pile under `Both`, counted
        // failed under `RemoteOnly` -- without touching the failure backoff: nothing is wrong
        // with the connection.
        #[cfg(feature = "zoning")]
        let sendable = match self.client.as_ref() {
            Some(client) if !client.takes_zoning() => {
                let (zoned, plain): (Vec<_>, Vec<_>) = sendable
                    .into_iter()
                    .partition(|(_, batch_item, ..)| batch_item.scene.material.zoning.is_some());
                for (item, ..) in zoned {
                    warn!(
                        entry_id = item.entry_id,
                        "preview batch: worker has no zoning support; this picture is not sent"
                    );
                    dispose_unrendered(self.shared, self.ui_weak, item, self.fallback_to_local);
                }
                plain
            }
            _ => sendable,
        };
        #[cfg(feature = "zoning")]
        if sendable.is_empty() {
            return Launch::NothingSent;
        }

        self.send_request(sendable)
    }

    /// Resolves each claimed design once and builds the batch items for the pictures that can
    /// be sent; the others are disposed of on the spot.
    fn build_sendable(
        &self,
        claimed: Vec<PreviewItem>,
    ) -> Vec<(PreviewItem, BatchItem, RecordRevision, String)> {
        let ctx = self.shared.ctx;
        // One resolve per design, however many of its views were claimed.
        let mut resolved: HashMap<i64, Option<ResolvedDesign>> = HashMap::new();
        for item in &claimed {
            resolved
                .entry(item.entry_id)
                .or_insert_with(|| resolve_design(ctx, item.entry_id));
        }

        let mut sendable: Vec<(PreviewItem, BatchItem, RecordRevision, String)> = Vec::new();
        for item in claimed {
            let Some(Some(design)) = resolved.get(&item.entry_id) else {
                // No geometry or material: says nothing about the remote.
                dispose_unrendered(self.shared, self.ui_weak, item, self.fallback_to_local);
                continue;
            };
            let job = PreviewJob {
                planes: &design.planes,
                material: &design.material,
                size: ctx.preview_size,
                spp: ctx.preview_spp,
                max_bounces: PREVIEW_MAX_BOUNCES,
            };
            // `item_id` is the picture's index in the request, assigned below.
            match preview_render::remote_batch_item(0, &job, item.view) {
                Ok(batch_item) => {
                    sendable.push((item, batch_item, design.revision, design.title.clone()));
                }
                Err(shortfall) => {
                    warn!(entry_id = item.entry_id, %shortfall, "preview batch: not sent");
                    dispose_unrendered(self.shared, self.ui_weak, item, self.fallback_to_local);
                }
            }
        }
        sendable
    }

    /// Keeps the request under the worker's control-frame cap; the rest goes back to the
    /// front of the queue for the next request.
    fn trim_to_request_cap(
        &self,
        sendable: &mut Vec<(PreviewItem, BatchItem, RecordRevision, String)>,
    ) {
        let sizes: Vec<usize> = sendable
            .iter()
            .map(|(_, batch_item, _, _)| postcard::to_allocvec(batch_item).map_or(0, |v| v.len()))
            .collect();
        let keep = items_fitting(&sizes, MAX_BATCH_REQUEST_BYTES);
        if keep < sendable.len() {
            let trimmed: Vec<PreviewItem> = sendable
                .split_off(keep)
                .into_iter()
                .map(|(item, ..)| item)
                .collect();
            self.shared.queue.return_to_shared_front(trimmed);
        }
    }

    /// Builds the request from the pictures that remain, sends it and records the flight.
    fn send_request(
        &mut self,
        sendable: Vec<(PreviewItem, BatchItem, RecordRevision, String)>,
    ) -> Launch {
        let titles: Vec<String> = sendable
            .iter()
            .map(|(_, _, _, title)| title.clone())
            .collect();
        let mut flight_items = Vec::with_capacity(sendable.len());
        let mut batch_items = Vec::with_capacity(sendable.len());
        let mut handed_back = Vec::new();
        for (index, (item, mut batch_item, revision, _)) in sendable.into_iter().enumerate() {
            let item_id = index as u32;
            batch_item.item_id = item_id;
            batch_items.push(batch_item);
            handed_back.push(item);
            flight_items.push(FlightItem {
                item,
                item_id,
                revision,
                answered: false,
            });
        }
        let Some(client) = self.client.as_mut() else {
            unreachable!("connected above")
        };
        let sent = client.send(batch_items);
        match sent {
            Ok(batch_id) => {
                info!(
                    batch_id,
                    pictures = flight_items.len(),
                    "preview batch: sent"
                );
                for title in &titles {
                    update_remote(self.shared.progress, |remote| remote.item_started(title));
                }
                self.push();
                Launch::Sent(Flight {
                    batch_id,
                    items: flight_items,
                })
            }
            Err(e) => {
                self.client = None;
                let shortfall = RemoteShortfall::Failed(e.to_string());
                note_failure(
                    self.shared,
                    self.health,
                    self.ui_weak,
                    handed_back[0],
                    &shortfall,
                    self.fallback_to_local,
                );
                for item in handed_back {
                    dispose_unrendered(self.shared, self.ui_weak, item, self.fallback_to_local);
                }
                Launch::Failed
            }
        }
    }

    /// A connection that could not be made: notes it once and hands the claim back.
    fn fail_launch(
        &mut self,
        sendable: Vec<(PreviewItem, BatchItem, RecordRevision, String)>,
        shortfall: &RemoteShortfall,
    ) -> Launch {
        self.client = None;
        if let Some((first, ..)) = sendable.first() {
            note_failure(
                self.shared,
                self.health,
                self.ui_weak,
                *first,
                shortfall,
                self.fallback_to_local,
            );
        }
        for (item, ..) in sendable {
            dispose_unrendered(self.shared, self.ui_weak, item, self.fallback_to_local);
        }
        Launch::Failed
    }

    /// Forwards the user's Cancel to the worker once.
    fn pass_on_cancel(&mut self) {
        if self.cancel_sent.is_some() || !self.cancelled() {
            return;
        }
        self.cancel_sent = Some(Instant::now());
        self.cancel_all_flights();
    }

    fn cancel_all_flights(&mut self) {
        let ids: Vec<u32> = self.flights.iter().map(|f| f.batch_id).collect();
        if let Some(client) = self.client.as_mut() {
            for id in ids {
                let _ = client.cancel(id);
            }
        }
    }

    /// Reads at most one reply and applies it.
    fn poll_once(&mut self) {
        let Some(client) = self.client.as_mut() else {
            // No connection but flights outstanding cannot happen; clear defensively.
            self.fail_all(&RemoteShortfall::Failed(
                "the connection was lost".to_owned(),
            ));
            return;
        };
        match client.poll(POLL) {
            Ok(Some(event)) => self.apply(event),
            Ok(None) => {}
            Err(e) => {
                self.fail_all(&RemoteShortfall::Failed(e.to_string()));
            }
        }
    }

    fn apply(&mut self, event: BatchEvent) {
        let now = Instant::now();
        match event {
            BatchEvent::ItemProgress { .. } => {
                if let Some(watch) = self.watch.as_mut() {
                    watch.item_event(now);
                }
            }
            BatchEvent::Heartbeat { samples_done } => {
                if let Some(watch) = self.watch.as_mut() {
                    watch.heartbeat(samples_done, now);
                }
            }
            BatchEvent::ItemDone {
                batch_id,
                item_id,
                png,
                ..
            } => {
                if let Some(watch) = self.watch.as_mut() {
                    watch.item_event(now);
                }
                self.settle_picture(batch_id, item_id, Ok(png));
            }
            BatchEvent::ItemFailed {
                batch_id,
                item_id,
                reason,
            } => {
                if let Some(watch) = self.watch.as_mut() {
                    watch.item_event(now);
                }
                self.settle_picture(batch_id, item_id, Err(reason));
            }
            BatchEvent::BatchDone {
                batch_id,
                cancelled,
            } => {
                let shortfall = if self.cancelled() || cancelled {
                    RemoteShortfall::Cancelled
                } else {
                    RemoteShortfall::Failed("the batch ended with pictures unanswered".to_owned())
                };
                self.finish_flight(batch_id, &shortfall);
            }
            BatchEvent::Refused {
                batch_id: Some(batch_id),
                code,
                message,
            } => {
                let shortfall = if code == error_codes::UNSUPPORTED_REQUEST {
                    self.unsupported = true;
                    RemoteShortfall::Unsupported(message)
                } else {
                    RemoteShortfall::Failed(message)
                };
                self.finish_flight(batch_id, &shortfall);
            }
            BatchEvent::Refused {
                batch_id: None,
                message,
                ..
            } => self.fail_all(&RemoteShortfall::Failed(message)),
        }
    }

    /// Settles one picture: saves its PNG, or hands a failed one back.
    fn settle_picture(&mut self, batch_id: u32, item_id: u32, outcome: Result<Vec<u8>, String>) {
        let Some(flight) = self.flights.iter_mut().find(|f| f.batch_id == batch_id) else {
            return;
        };
        let Some(picture) = flight
            .items
            .iter_mut()
            .find(|i| i.item_id == item_id && !i.answered)
        else {
            return;
        };
        picture.answered = true;
        let (item, revision) = (picture.item, picture.revision);
        update_remote(
            self.shared.progress,
            crate::gui::batch::remote_dispatch::RemoteStatus::item_ended,
        );
        match outcome {
            Ok(png) => {
                note_success(self.health);
                finish_item_at_revision(
                    self.shared,
                    self.ui_weak,
                    item.entry_id,
                    item.view,
                    Some(png),
                    revision,
                );
            }
            Err(reason) => {
                warn!(
                    entry_id = item.entry_id,
                    view = ?item.view,
                    %reason,
                    "preview batch: the remote could not render a picture; handed back"
                );
                dispose_unrendered(self.shared, self.ui_weak, item, self.fallback_to_local);
            }
        }
        self.push();
    }

    /// Ends `batch_id`: whatever it left unanswered is handed back under `shortfall`.
    fn finish_flight(&mut self, batch_id: u32, shortfall: &RemoteShortfall) {
        let Some(position) = self.flights.iter().position(|f| f.batch_id == batch_id) else {
            return;
        };
        let Some(mut flight) = self.flights.remove(position) else {
            return;
        };
        self.hand_back_unanswered(&mut flight, shortfall);
        let now = Instant::now();
        if position == 0
            && let (Some(watch), Some(front)) = (self.watch.as_mut(), self.flights.front())
        {
            watch.next_front(now, front.items.len());
        }
        if self.flights.is_empty() {
            self.watch = None;
            self.cancel_sent = None;
        }
    }

    /// Hands every unanswered picture of `flight` back, noting one failure for the batch
    /// unless it was a cancel.
    fn hand_back_unanswered(&mut self, flight: &mut Flight, shortfall: &RemoteShortfall) {
        let pending: Vec<PreviewItem> = flight
            .items
            .iter_mut()
            .filter(|i| !i.answered)
            .map(|i| {
                i.answered = true;
                i.item
            })
            .collect();
        let Some(&first) = pending.first() else {
            return;
        };
        note_failure(
            self.shared,
            self.health,
            self.ui_weak,
            first,
            shortfall,
            self.fallback_to_local,
        );
        for _ in &pending {
            update_remote(
                self.shared.progress,
                crate::gui::batch::remote_dispatch::RemoteStatus::item_ended,
            );
        }
        for item in pending {
            dispose_unrendered(self.shared, self.ui_weak, item, self.fallback_to_local);
        }
        self.push();
    }

    /// The connection is gone or untrustworthy: every outstanding picture is handed back
    /// and the connection dropped.
    fn fail_all(&mut self, shortfall: &RemoteShortfall) {
        let mut flights = std::mem::take(&mut self.flights);
        for flight in &mut flights {
            self.hand_back_unanswered(flight, shortfall);
        }
        self.client = None;
        self.watch = None;
        self.cancel_sent = None;
    }

    /// Applies the stall, wall-clock and cancel-acknowledgement deadlines.
    fn check_deadlines(&mut self) {
        let now = Instant::now();
        if let Some(sent) = self.cancel_sent
            && now.saturating_duration_since(sent) > PREVIEW_CANCEL_ACK_WAIT
        {
            warn!("preview batch: the remote did not acknowledge the cancel; dropping it");
            self.fail_all(&RemoteShortfall::Cancelled);
            return;
        }
        let verdict = self.watch.as_ref().and_then(|watch| watch.verdict(now));
        if let Some(shortfall) = verdict {
            warn!(%shortfall, "preview batch: the remote missed a deadline; cancelling");
            self.cancel_all_flights();
            self.drain_after_cancel();
            self.fail_all(&shortfall);
        }
    }

    /// After a Cancel: waits up to [`PREVIEW_CANCEL_ACK_WAIT`] for the worker to end its
    /// batches, discarding what arrives -- the caller already knows why it gave up.
    fn drain_after_cancel(&mut self) {
        let deadline = Instant::now() + PREVIEW_CANCEL_ACK_WAIT;
        while Instant::now() < deadline {
            let Some(client) = self.client.as_mut() else {
                return;
            };
            if client.outstanding() == 0 {
                return;
            }
            if client.poll(POLL).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_kinds_are_distinct() {
        assert_ne!(BatchExit::Finished, BatchExit::UseSingle);
    }
}
