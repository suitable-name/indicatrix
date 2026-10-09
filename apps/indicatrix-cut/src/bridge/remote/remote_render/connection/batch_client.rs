//! The persistent batch connection (protocol v24): [`BatchClient`] keeps ONE mutual-TLS
//! connection to a direct worker for a whole preview run and exchanges
//! `BatchRenderRequest`s for finished PNGs on it.
//!
//! Unlike the one-shot lifecycle (one connection per picture) the caller can send the
//! NEXT batch while the current one is tracing, so the worker always has queued work.
//! The caller's dispatcher thread owns the client and alternates [`BatchClient::poll`]
//! with its own bookkeeping; nothing here spawns a thread, because TLS state cannot be
//! split across threads (see the parent module's doc comment).
//!
//! Liveness is enforced here: a worker heartbeats `PROGRESS` at least every 2 s while any
//! batch is unfinished, so [`BatchClient::poll`] fails with
//! [`RemoteError::WorkerSilent`] once an outstanding batch has been quiet for the usual
//! deadlines. Progress and wall-clock deadlines (is the worker advancing, is a batch
//! taking absurdly long) are the dispatcher's, via [`BatchWatch`].

use super::{
    super::types::{RemoteError, RemoteStream},
    handshake::connect_and_handshake,
    stream_io::try_read_stream_event,
};
use crate::{
    bridge::preview_wait::{PREVIEW_PROGRESS_STALL, PREVIEW_REMOTE_MAX_WALL, RemoteShortfall},
    settings::WorkerSettings,
};
use indicatrix_net::{
    framing::MAX_CONTROL_FRAME_LEN,
    messages::{
        Backend, BatchItem, BatchRenderRequest, BatchReply, ClientMessage, MAX_BATCH_ITEMS,
        StreamEvent,
    },
};
use std::time::{Duration, Instant};

/// The largest encoded `BatchRenderRequest` the client sends: comfortably under the
/// worker's `MAX_CONTROL_FRAME_LEN` read bound.
pub const MAX_BATCH_REQUEST_BYTES: usize = (MAX_CONTROL_FRAME_LEN as usize / 8) * 7;

/// What one decoded reply means to the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchEvent {
    /// An item was traced (its picture is being encoded).
    ItemProgress {
        batch_id: u32,
        item_id: u32,
        samples_done: u32,
    },
    /// An item's finished PNG.
    ItemDone {
        batch_id: u32,
        item_id: u32,
        samples_done: u32,
        png: Vec<u8>,
    },
    /// An item that produced no picture; the rest of its batch carries on.
    ItemFailed {
        batch_id: u32,
        item_id: u32,
        reason: String,
    },
    /// A batch ended. Items of it that were never answered were cancelled.
    BatchDone { batch_id: u32, cancelled: bool },
    /// The worker's liveness heartbeat, carrying its running traced-sample total.
    Heartbeat { samples_done: u32 },
    /// A request or the connection was refused: `UNSUPPORTED_REQUEST`, a validation
    /// failure, a trace panic. `batch_id` names the batch when the error concerns one.
    Refused {
        batch_id: Option<u32>,
        code: u32,
        message: String,
    },
}

/// Turns one decoded stream event into a [`BatchEvent`]; `None` for an event a batch
/// connection has no use for (a `PONG`, a capability change).
fn to_batch_event(event: StreamEvent, payload: Option<Vec<u8>>) -> Option<BatchEvent> {
    match event {
        StreamEvent::BatchItemProgress(p) => Some(BatchEvent::ItemProgress {
            batch_id: p.request_id,
            item_id: p.item_id,
            samples_done: p.samples_done,
        }),
        StreamEvent::BatchItemDone(done) => Some(BatchEvent::ItemDone {
            batch_id: done.request_id,
            item_id: done.item_id,
            samples_done: done.samples_done,
            png: payload.unwrap_or_default(),
        }),
        StreamEvent::BatchItemFailed(failed) => Some(BatchEvent::ItemFailed {
            batch_id: failed.request_id,
            item_id: failed.item_id,
            reason: failed.reason,
        }),
        StreamEvent::BatchDone(done) => Some(BatchEvent::BatchDone {
            batch_id: done.request_id,
            cancelled: done.cancelled,
        }),
        StreamEvent::Progress(progress) => Some(BatchEvent::Heartbeat {
            samples_done: progress.samples_done,
        }),
        StreamEvent::Error(error) => Some(BatchEvent::Refused {
            batch_id: error.request_id,
            code: error.code,
            message: error.message,
        }),
        _ => None,
    }
}

/// One persistent batch connection -- see the module doc comment.
pub struct BatchClient {
    stream: RemoteStream,
    backend: Backend,
    next_batch_id: u32,
    /// Batches sent and not yet ended by a `BATCH_DONE` or a refusal.
    outstanding: usize,
    last_event: Instant,
    /// Whether any event arrived since the connection last went from idle to busy: the
    /// first wait gets the longer grace for a cold GPU's pipeline compile.
    seen_event: bool,
    /// `zoning` builds: whether the server's `WELCOME` advertised the zoning capability.
    #[cfg(feature = "zoning")]
    zoning: bool,
}

impl BatchClient {
    /// Connects to `worker` and checks it can render.
    ///
    /// # Errors
    ///
    /// Any [`RemoteError`] from the connection or handshake, or
    /// [`RemoteError::NoRenderCapacity`] for a library-only server.
    pub fn connect(worker: &WorkerSettings) -> Result<Self, RemoteError> {
        let (stream, welcome) = connect_and_handshake(worker)?;
        #[cfg(feature = "zoning")]
        let zoning = welcome.zoning;
        let capability = welcome.render.ok_or(RemoteError::NoRenderCapacity)?;
        Ok(Self {
            stream,
            backend: capability.backend,
            next_batch_id: 1,
            outstanding: 0,
            last_event: Instant::now(),
            seen_event: false,
            #[cfg(feature = "zoning")]
            zoning,
        })
    }

    /// `zoning` builds: whether the server accepts zoned materials. A batch item whose
    /// scene has zones must not be sent to a client for which this is `false` (it would render
    /// as its base zone); [`Self::send`] refuses such a batch, and the dispatcher hands those
    /// items to the local lane first.
    #[cfg(feature = "zoning")]
    #[must_use]
    pub const fn takes_zoning(&self) -> bool {
        self.zoning
    }

    /// Whether the server renders on its own hardware. A coordinator does not take
    /// batches (it spreads single requests over its joined workers instead).
    #[must_use]
    pub const fn takes_batches(&self) -> bool {
        !matches!(self.backend, Backend::Coordinator { .. })
    }

    /// Whether the server traces on a GPU (the batching exists for those).
    #[must_use]
    pub const fn is_gpu(&self) -> bool {
        matches!(self.backend, Backend::Gpu { .. })
    }

    /// Sends `items` as one batch and returns its id.
    ///
    /// # Errors
    ///
    /// [`RemoteError::Client`] when writing fails, or when `items` is empty, has more than
    /// `MAX_BATCH_ITEMS`, or encodes to more than [`MAX_BATCH_REQUEST_BYTES`].
    pub fn send(&mut self, items: Vec<BatchItem>) -> Result<u32, RemoteError> {
        let batch_id = self.next_batch_id;
        let request = BatchRenderRequest {
            request_id: batch_id,
            reply: BatchReply::FinalPng,
            items,
        };
        if request.items.is_empty() || request.items.len() > MAX_BATCH_ITEMS {
            return Err(RemoteError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "a batch needs 1 to {MAX_BATCH_ITEMS} items (got {})",
                    request.items.len()
                ),
            )));
        }
        // `zoning` builds: the zones of the zoned items go first (they are `serde(skip)` in
        // the scene); a server without the capability gets no zoned item at all.
        #[cfg(feature = "zoning")]
        if let Some(payload) =
            indicatrix_net::messages::ZoningPayload::for_batch(batch_id, &request.items)
        {
            if !self.zoning {
                return Err(RemoteError::ZoningUnsupported);
            }
            indicatrix_net::messages::write_zoning_payload(&mut self.stream, &payload)?;
        }
        indicatrix_net::messages::write_message(
            &mut self.stream,
            &ClientMessage::BatchRenderRequest(Box::new(request)),
        )?;
        self.next_batch_id = self.next_batch_id.wrapping_add(1).max(1);
        if self.outstanding == 0 {
            // Idle to busy: the silence clock starts at the send.
            self.last_event = Instant::now();
            self.seen_event = false;
        }
        self.outstanding += 1;
        Ok(batch_id)
    }

    /// Asks the worker to stop `batch_id` (best effort; the batch still ends with a
    /// `BATCH_DONE`).
    ///
    /// # Errors
    ///
    /// [`RemoteError::Client`] when writing fails.
    pub fn cancel(&mut self, batch_id: u32) -> Result<(), RemoteError> {
        indicatrix_net::client::send_cancel(&mut self.stream, batch_id)?;
        Ok(())
    }

    /// Batches sent and not yet ended.
    #[must_use]
    pub const fn outstanding(&self) -> usize {
        self.outstanding
    }

    /// Waits up to `timeout` for the next event. `Ok(None)` is "nothing relevant yet".
    ///
    /// # Errors
    ///
    /// A transport error, or [`RemoteError::WorkerSilent`] once an outstanding batch has
    /// gone quiet past the liveness deadline (30 s before the first event after the
    /// connection became busy, 8 s after).
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<BatchEvent>, RemoteError> {
        if let Some((event, payload)) = try_read_stream_event(&mut self.stream, timeout)? {
            self.last_event = Instant::now();
            self.seen_event = true;
            let mapped = to_batch_event(event, payload);
            match &mapped {
                Some(BatchEvent::BatchDone { .. }) => {
                    self.outstanding = self.outstanding.saturating_sub(1);
                }
                Some(BatchEvent::Refused {
                    batch_id: Some(_), ..
                }) => self.outstanding = self.outstanding.saturating_sub(1),
                _ => {}
            }
            Ok(mapped)
        } else {
            if self.outstanding > 0 {
                let deadline = super::liveness_deadline(self.seen_event, super::LIVENESS_TIMEOUT);
                let silent = self.last_event.elapsed();
                if silent > deadline {
                    return Err(RemoteError::WorkerSilent(silent));
                }
            }
            Ok(None)
        }
    }
}

/// The wall-clock allowance of a batch of `items` pictures:
/// [`PREVIEW_REMOTE_MAX_WALL`] per picture.
#[must_use]
pub fn batch_wall(items: usize) -> Duration {
    PREVIEW_REMOTE_MAX_WALL.saturating_mul(u32::try_from(items.max(1)).unwrap_or(u32::MAX))
}

/// A connection's progress and wall-clock deadlines.
///
/// "Progress" is any item event or a heartbeat whose traced-sample total grew -- never a
/// bare repeated heartbeat, which is exactly what a wedged worker sends. The wall clock is
/// per batch and starts when the batch reaches the front of the worker's queue, so a
/// batch waiting behind its predecessor is not charged for the wait.
#[derive(Debug, Clone)]
pub struct BatchWatch {
    last_progress: Instant,
    heartbeat_total: u32,
    front_started: Instant,
    front_wall: Duration,
}

impl BatchWatch {
    /// Starts watching a front batch of `items` pictures at `now`.
    #[must_use]
    pub fn new(now: Instant, items: usize) -> Self {
        Self {
            last_progress: now,
            heartbeat_total: 0,
            front_started: now,
            front_wall: batch_wall(items),
        }
    }

    /// An item event (progress, picture, failure) or a batch end arrived.
    pub const fn item_event(&mut self, now: Instant) {
        self.last_progress = now;
    }

    /// A heartbeat carrying the worker's traced-sample total arrived.
    pub const fn heartbeat(&mut self, samples_done: u32, now: Instant) {
        if samples_done > self.heartbeat_total {
            self.heartbeat_total = samples_done;
            self.last_progress = now;
        }
    }

    /// The next batch of `items` pictures became the front one at `now`.
    pub fn next_front(&mut self, now: Instant, items: usize) {
        self.front_started = now;
        self.front_wall = batch_wall(items);
        self.last_progress = now;
    }

    /// `Some(shortfall)` once the worker has not advanced for [`PREVIEW_PROGRESS_STALL`] or
    /// the front batch has outlived its wall allowance.
    #[must_use]
    pub fn verdict(&self, now: Instant) -> Option<RemoteShortfall> {
        let elapsed = now.saturating_duration_since(self.front_started);
        if elapsed > self.front_wall {
            return Some(RemoteShortfall::WallClock {
                samples_done: self.heartbeat_total,
                elapsed,
            });
        }
        let stalled_for = now.saturating_duration_since(self.last_progress);
        (stalled_for > PREVIEW_PROGRESS_STALL).then_some(RemoteShortfall::Stalled {
            samples_done: self.heartbeat_total,
            stalled_for,
        })
    }
}

/// How many leading items of `sizes` (each item's encoded size in bytes) fit one request
/// of at most `budget` bytes, keeping at least one. Pure, for the dispatcher's trim.
#[must_use]
pub fn items_fitting(sizes: &[usize], budget: usize) -> usize {
    let mut total = 0_usize;
    let mut kept = 0;
    for &size in sizes {
        if kept > 0 && total + size > budget {
            break;
        }
        total += size;
        kept += 1;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_net::messages::{
        BatchDone, BatchItemDoneHeader, BatchItemFailed, BatchItemProgress, ErrorMsg, Progress,
    };

    #[test]
    fn replies_map_to_dispatcher_events() {
        let png = vec![1, 2, 3];
        assert_eq!(
            to_batch_event(
                StreamEvent::BatchItemDone(BatchItemDoneHeader {
                    request_id: 4,
                    item_id: 2,
                    samples_done: 64,
                    payload_len: 3,
                }),
                Some(png.clone())
            ),
            Some(BatchEvent::ItemDone {
                batch_id: 4,
                item_id: 2,
                samples_done: 64,
                png
            })
        );
        assert_eq!(
            to_batch_event(
                StreamEvent::BatchItemProgress(BatchItemProgress {
                    request_id: 4,
                    item_id: 2,
                    samples_done: 64
                }),
                None
            ),
            Some(BatchEvent::ItemProgress {
                batch_id: 4,
                item_id: 2,
                samples_done: 64
            })
        );
        assert!(matches!(
            to_batch_event(
                StreamEvent::BatchItemFailed(BatchItemFailed {
                    request_id: 4,
                    item_id: 3,
                    reason: "x".into()
                }),
                None
            ),
            Some(BatchEvent::ItemFailed { item_id: 3, .. })
        ));
        assert_eq!(
            to_batch_event(
                StreamEvent::BatchDone(BatchDone {
                    request_id: 4,
                    cancelled: true
                }),
                None
            ),
            Some(BatchEvent::BatchDone {
                batch_id: 4,
                cancelled: true
            })
        );
        assert_eq!(
            to_batch_event(
                StreamEvent::Progress(Progress {
                    request_id: 4,
                    samples_done: 9
                }),
                None
            ),
            Some(BatchEvent::Heartbeat { samples_done: 9 })
        );
        assert!(matches!(
            to_batch_event(
                StreamEvent::Error(ErrorMsg {
                    code: 2,
                    message: "no".into(),
                    request_id: Some(4)
                }),
                None
            ),
            Some(BatchEvent::Refused {
                batch_id: Some(4),
                code: 2,
                ..
            })
        ));
        assert_eq!(to_batch_event(StreamEvent::Pong { nonce: 1 }, None), None);
    }

    #[test]
    fn the_wall_allowance_scales_with_the_item_count() {
        assert_eq!(batch_wall(1), PREVIEW_REMOTE_MAX_WALL);
        assert_eq!(batch_wall(10), PREVIEW_REMOTE_MAX_WALL * 10);
        assert_eq!(batch_wall(0), PREVIEW_REMOTE_MAX_WALL);
    }

    #[test]
    fn a_repeated_heartbeat_does_not_count_as_progress_but_a_grown_total_does() {
        let t0 = Instant::now();
        let secs = Duration::from_secs;
        let mut watch = BatchWatch::new(t0, 10);
        watch.heartbeat(5, t0 + secs(30));
        watch.heartbeat(5, t0 + secs(80));
        // 80 s in, progress last advanced at 30 s: 50 s stalled, still within 60 s.
        assert_eq!(watch.verdict(t0 + secs(80)), None);
        assert!(matches!(
            watch.verdict(t0 + secs(91)),
            Some(RemoteShortfall::Stalled { .. })
        ));
        watch.item_event(t0 + secs(91));
        assert_eq!(watch.verdict(t0 + secs(100)), None);
    }

    #[test]
    fn the_next_front_batch_gets_a_fresh_wall_clock() {
        let t0 = Instant::now();
        let secs = Duration::from_secs;
        let mut watch = BatchWatch::new(t0, 1);
        watch.item_event(t0 + secs(290));
        assert!(matches!(
            watch.verdict(t0 + secs(301)),
            Some(RemoteShortfall::WallClock { .. })
        ));
        watch.next_front(t0 + secs(290), 1);
        assert_eq!(watch.verdict(t0 + secs(301)), None);
    }

    #[test]
    fn items_fitting_keeps_at_least_one_and_stops_at_the_budget() {
        assert_eq!(items_fitting(&[], 100), 0);
        assert_eq!(items_fitting(&[500], 100), 1);
        assert_eq!(items_fitting(&[40, 40, 40], 100), 2);
        assert_eq!(items_fitting(&[40, 40, 40], 120), 3);
    }
}
