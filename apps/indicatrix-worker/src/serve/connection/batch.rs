//! Batched preview requests (protocol v24): [`serve_batches`] answers
//! `ClientMessage::BatchRenderRequest`s on one persistent connection with finished PNGs.
//!
//! # Why this exists
//!
//! A catalogue-preview run used to ask the worker for one small picture per fresh
//! connection and take the full float radiance back; the GPU spent most of its time
//! waiting between pictures. A batch carries up to `MAX_BATCH_ITEMS` pictures, the
//! viewer keeps the NEXT batch queued while the current one traces, and the worker
//! returns the finished, tone-mapped PNG of each picture
//! (`indicatrix::render_setup::encode_preview_png`, the function the viewer's own preview
//! path calls, so remote and local bytes are identical for the same sum).
//!
//! # Threads
//!
//! TLS state cannot be shared, so one thread (the caller's) owns the stream, exactly as
//! in `stream_emit`'s emitter. Two helpers do the work:
//!
//! ```text
//!   connection thread            tracer thread                 encoder thread
//!   -----------------            -------------                 --------------
//!   reads CANCEL / batches  -->  per batch: validate items,
//!   writes every event           GPU items: ONE batch call,
//!   (heartbeat every 2 s)        CPU-only / declined items:
//!        ^                       one CPU trace each       --->  tone-map + PNG each
//!        |                                                      (FIFO: BatchDone is last)
//!        +------------------------------------------------------+
//! ```
//!
//! The tracer takes batches in arrival order and starts the next the moment the previous
//! one has been handed to the encoder, so PNG encoding of batch N overlaps the tracing of
//! batch N + 1 and the GPU queue never waits on the network or the encoder.
//!
//! # Routing per item
//!
//! `render_core::scene_uses_gpu` decides: a GPU-eligible item goes into the batch's one
//! `GpuBackend::try_accumulate_batch_cancellable` call (the backend keeps its queue fed
//! across item boundaries, concave items with their tools); a fluorescent, UV-lit,
//! over-limit-tools or unsupported-material scene, and any
//! item the GPU declines, is traced on the CPU by the plain CPU path. The hybrid
//! CPU+GPU split is deliberately NOT used: a preview is small, and splitting it costs more
//! than it saves. Results leave the tracer in completion order: the GPU items in input
//! order first, then the CPU items in input order.
//!
//! # Failure policy
//!
//! One bad item never sinks its batch: an invalid scene, a size mismatch, an HDR
//! environment (previews are studio-lit; the asset round trip is not offered here), a
//! tracer panic or an encode failure becomes `BATCH_ITEM_FAILED` for that item. A batch
//! the connection cannot take (a third in flight, a duplicate id, an empty or oversized
//! item list) is refused with `VALIDATION_FAILED` and the connection stays usable.

use super::{
    NO_RENDER_CAPACITY_CODE,
    requests::{RequestContext, VALIDATION_FAILED_CODE, write_pong},
};
use crate::{
    assets, render_core,
    stream_emit::{
        HEARTBEAT_INTERVAL, RawPoll, TimeoutCache, TimeoutRead, TimeoutWrite,
        poll_raw_client_message,
    },
    validate,
};
use glam::Vec3;
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::messages::{
    BatchDone, BatchItem, BatchItemDoneHeader, BatchItemFailed, BatchItemProgress,
    BatchRenderRequest, ClientMessage, ErrorMsg, MAX_BATCH_ITEMS, MAX_BATCHES_IN_FLIGHT, NetError,
    Progress, StreamEvent, error_codes,
};
use std::{
    io::{Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// How long the connection thread waits for an outgoing event before it looks at the
/// socket again; an event that arrives sooner wakes it at once.
const EVENT_WAIT: Duration = Duration::from_millis(20);

/// How long one look at the socket for a `CANCEL` or the next batch may block.
const SOCKET_POLL: Duration = Duration::from_millis(5);

/// Bounds every write of the session: a peer that stops reading ends the connection
/// instead of hanging it (the same bound `stream_emit` applies to a streaming request).
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// One event for the wire, with its raw payload frame where it has one.
type Outgoing = (StreamEvent, Option<Vec<u8>>);

/// A validated batch waiting for, or running on, the tracer thread.
struct QueuedBatch {
    request: BatchRenderRequest,
    /// Raised by a `CANCEL` (or the peer closing); the tracer stops between and inside
    /// items.
    cancel: Arc<AtomicBool>,
}

/// What the connection thread remembers about a batch until its `BATCH_DONE` is written.
struct Unfinished {
    request_id: u32,
    cancel: Arc<AtomicBool>,
    items: usize,
    started: Instant,
}

/// A traced item on its way to the encoder.
struct Picture {
    request_id: u32,
    item_id: u32,
    samples: u32,
    width: u32,
    height: u32,
    accum: Vec<Vec3>,
}

/// What the tracer hands the encoder. One FIFO channel, so a batch's `End` is always
/// written after every one of its pictures and failures.
enum StageJob {
    Picture(Picture),
    Failed {
        request_id: u32,
        item_id: u32,
        reason: String,
    },
    End {
        request_id: u32,
        cancelled: bool,
    },
}

/// Everything the tracer thread owns.
struct TraceEnv {
    /// The process's shared GPU backend.
    gpu: Arc<GpuBackend>,
    /// A backend that always declines, so [`render_core::trace_samples_with_gpu_cancellable`]
    /// runs the plain CPU tracer (the GPU was already offered the item, or cannot take it).
    cpu: GpuBackend,
    prefer_ray_gpu: bool,
    /// CPU tracer threads (`0` = all cores).
    threads: usize,
    /// Samples traced so far across the session, for the heartbeat.
    traced_total: Arc<AtomicU32>,
    /// Straight to the connection thread (per-item progress).
    out: mpsc::Sender<Outgoing>,
    /// To the encoder (pictures, failures, batch ends).
    stage: mpsc::Sender<StageJob>,
}

impl TraceEnv {
    fn fail(&self, request_id: u32, item_id: u32, reason: impl Into<String>) {
        let reason = reason.into();
        tracing::warn!(request_id, item_id, %reason, "batch item failed");
        let _ = self.stage.send(StageJob::Failed {
            request_id,
            item_id,
            reason,
        });
    }

    /// Reports that `item` is traced (its picture is being encoded).
    fn traced(&self, request_id: u32, item: &BatchItem) {
        self.traced_total.fetch_add(item.samples, Ordering::Relaxed);
        let _ = self.out.send((
            StreamEvent::BatchItemProgress(BatchItemProgress {
                request_id,
                item_id: item.item_id,
                samples_done: item.samples,
            }),
            None,
        ));
    }

    fn picture(&self, request_id: u32, item: &BatchItem, accum: Vec<Vec3>) {
        let _ = self.stage.send(StageJob::Picture(Picture {
            request_id,
            item_id: item.item_id,
            samples: item.samples,
            width: item.width,
            height: item.height,
            accum,
        }));
    }
}

/// Why a batch the connection cannot take is refused, or `Ok` when it fits. `in_flight`
/// holds the `request_id` of every unfinished batch.
fn check_admission(request: &BatchRenderRequest, in_flight: &[u32]) -> Result<(), String> {
    if in_flight.len() >= MAX_BATCHES_IN_FLIGHT {
        return Err(format!(
            "at most {MAX_BATCHES_IN_FLIGHT} batches may be in flight on one connection"
        ));
    }
    if in_flight.contains(&request.request_id) {
        return Err(format!(
            "a batch with request_id {} is already in flight",
            request.request_id
        ));
    }
    if request.items.is_empty() {
        return Err("a batch needs at least one item".to_owned());
    }
    if request.items.len() > MAX_BATCH_ITEMS {
        return Err(format!(
            "a batch carries at most {MAX_BATCH_ITEMS} items (got {})",
            request.items.len()
        ));
    }
    let mut ids: Vec<u32> = request.items.iter().map(|item| item.item_id).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("item_id values must be unique within a batch".to_owned());
    }
    Ok(())
}

/// Why one item cannot be traced, or `Ok`. A failure fails that item only.
fn check_item(item: &BatchItem) -> Result<(), String> {
    if !item.output_matches_scene() {
        return Err(format!(
            "the output size {}x{} must equal the scene's {}x{}",
            item.width, item.height, item.scene.width, item.scene.height
        ));
    }
    if item.scene.hdr().is_some() {
        return Err(
            "batch items are studio-lit; an HDR environment is not served in a batch".to_owned(),
        );
    }
    validate::validate_request(&item.scene, item.first_sample, item.samples)
}

#[path = "batch_trace.rs"]
mod trace_impl;
use trace_impl::trace_batch;

/// The encoder thread's body: turns each [`StageJob`] into its wire event, in order.
fn encode_stage(stage: &mpsc::Receiver<StageJob>, out: &mpsc::Sender<Outgoing>) {
    while let Ok(job) = stage.recv() {
        let event = match job {
            StageJob::Picture(picture) => encode_picture(picture),
            StageJob::Failed {
                request_id,
                item_id,
                reason,
            } => (
                StreamEvent::BatchItemFailed(BatchItemFailed {
                    request_id,
                    item_id,
                    reason,
                }),
                None,
            ),
            StageJob::End {
                request_id,
                cancelled,
            } => (
                StreamEvent::BatchDone(BatchDone {
                    request_id,
                    cancelled,
                }),
                None,
            ),
        };
        if out.send(event).is_err() {
            return;
        }
    }
}

/// Tone-maps and PNG-encodes one traced picture with the function the viewer's own
/// preview path uses (`indicatrix::render_setup::encode_preview_png`).
fn encode_picture(picture: Picture) -> Outgoing {
    let Picture {
        request_id,
        item_id,
        samples,
        width,
        height,
        accum,
    } = picture;
    let png = catch_unwind(AssertUnwindSafe(|| {
        indicatrix::render_setup::encode_preview_png(width, height, &accum, samples)
    }))
    .ok()
    .flatten();
    png.map_or_else(
        || {
            (
                StreamEvent::BatchItemFailed(BatchItemFailed {
                    request_id,
                    item_id,
                    reason: "the picture could not be tone-mapped and encoded".to_owned(),
                }),
                None,
            )
        },
        |bytes| {
            (
                StreamEvent::BatchItemDone(BatchItemDoneHeader {
                    request_id,
                    item_id,
                    samples_done: samples,
                    payload_len: u32::try_from(bytes.len())
                        .expect("a PNG of a pixel-capped picture is far below 4 GiB"),
                }),
                Some(bytes),
            )
        },
    )
}

/// The two helper threads and the channels around them.
struct Pipeline {
    /// To the tracer; dropped to let both helpers end.
    work: Option<mpsc::Sender<QueuedBatch>>,
    /// Everything for the wire, from both helpers.
    outgoing: mpsc::Receiver<Outgoing>,
    traced_total: Arc<AtomicU32>,
    threads: Vec<JoinHandle<()>>,
}

impl Pipeline {
    fn start(ctx: &RequestContext<'_>) -> Self {
        let (work_tx, work_rx) = mpsc::channel::<QueuedBatch>();
        let (stage_tx, stage_rx) = mpsc::channel::<StageJob>();
        let (out_tx, out_rx) = mpsc::channel::<Outgoing>();
        let traced_total = Arc::new(AtomicU32::new(0));
        let env = TraceEnv {
            gpu: Arc::clone(ctx.gpu),
            cpu: GpuBackend::disabled(),
            prefer_ray_gpu: ctx.compute_mode != crate::cli::ComputeMode::OnlyCpu,
            threads: ctx.threads,
            traced_total: Arc::clone(&traced_total),
            out: out_tx.clone(),
            stage: stage_tx,
        };
        let tracer = thread::spawn(move || {
            for batch in work_rx {
                trace_batch(&env, &batch);
            }
        });
        let encoder = thread::spawn(move || encode_stage(&stage_rx, &out_tx));
        Self {
            work: Some(work_tx),
            outgoing: out_rx,
            traced_total,
            threads: vec![tracer, encoder],
        }
    }

    /// Lets the helpers end once their queues drain; waits for them when `join` (a clean
    /// session -- on an error path they are detached, since a wedged tracer must not keep
    /// the connection thread from returning).
    fn shutdown(mut self, join: bool) {
        drop(self.work.take());
        if join {
            for handle in self.threads.drain(..) {
                let _ = handle.join();
            }
        }
    }
}

/// One serving session: the batches in flight on this connection, from the first request
/// until none is unfinished.
struct Session {
    pipeline: Pipeline,
    unfinished: Vec<Unfinished>,
    timeouts: TimeoutCache,
    last_write: Instant,
}

impl Session {
    /// Validates `request`; on success queues it for the tracer, on failure answers
    /// `VALIDATION_FAILED` and carries on.
    fn admit<S: Write>(
        &mut self,
        stream: &mut S,
        request: BatchRenderRequest,
    ) -> Result<(), NetError> {
        let in_flight: Vec<u32> = self.unfinished.iter().map(|b| b.request_id).collect();
        if let Err(message) = check_admission(&request, &in_flight) {
            tracing::info!(request_id = request.request_id, %message, "refusing a batch");
            return write_error(stream, VALIDATION_FAILED_CODE, message, request.request_id);
        }
        // Re-attach the zones sent ahead of this batch. One that does not fit
        // refuses the whole batch (a half-zoned batch would render some items as their base
        // zone without saying so).
        #[cfg(feature = "worker")]
        let request = match crate::serve::zoning::attach_to_batch(request) {
            Ok(request) => request,
            Err((request_id, message)) => {
                tracing::info!(request_id, %message, "refusing a batch");
                return write_error(stream, VALIDATION_FAILED_CODE, message, request_id);
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.unfinished.push(Unfinished {
            request_id: request.request_id,
            cancel: Arc::clone(&cancel),
            items: request.items.len(),
            started: Instant::now(),
        });
        let queued = QueuedBatch { request, cancel };
        if let Some(work) = &self.pipeline.work {
            let _ = work.send(queued);
        }
        Ok(())
    }

    /// Writes one outgoing event, noting a batch's end.
    fn write<S: Write>(
        &mut self,
        stream: &mut S,
        (event, payload): Outgoing,
    ) -> Result<(), NetError> {
        indicatrix_net::messages::write_stream_event(stream, &event, payload.as_deref())?;
        self.last_write = Instant::now();
        if let StreamEvent::BatchDone(done) = &event
            && let Some(position) = self
                .unfinished
                .iter()
                .position(|batch| batch.request_id == done.request_id)
        {
            let batch = self.unfinished.remove(position);
            tracing::info!(
                request_id = batch.request_id,
                items = batch.items,
                cancelled = done.cancelled,
                elapsed = ?batch.started.elapsed(),
                "worker: served a preview batch"
            );
        }
        Ok(())
    }

    /// A bare `PROGRESS` when nothing else went out for [`HEARTBEAT_INTERVAL`], so the
    /// viewer's silence-based liveness deadline holds while a long batch traces.
    fn heartbeat<S: Write>(&mut self, stream: &mut S) -> Result<(), NetError> {
        if self.last_write.elapsed() < HEARTBEAT_INTERVAL {
            return Ok(());
        }
        let Some(front) = self.unfinished.first() else {
            return Ok(());
        };
        indicatrix_net::messages::write_stream_event(
            stream,
            &StreamEvent::Progress(Progress {
                request_id: front.request_id,
                samples_done: self.pipeline.traced_total.load(Ordering::Relaxed),
            }),
            None,
        )?;
        self.last_write = Instant::now();
        Ok(())
    }

    /// Raises the cancel flag of every unfinished batch.
    fn cancel_all(&self) {
        for batch in &self.unfinished {
            batch.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Handles one message that arrived while batches are in flight.
    fn handle<S: Read + Write>(
        &mut self,
        stream: &mut S,
        message: ClientMessage,
    ) -> Result<(), NetError> {
        match message {
            ClientMessage::BatchRenderRequest(request) => self.admit(stream, *request),
            ClientMessage::Cancel(cancel) => {
                if let Some(batch) = self
                    .unfinished
                    .iter()
                    .find(|batch| batch.request_id == cancel.request_id) { batch.cancel.store(true, Ordering::Relaxed) } else { tracing::debug!(
                    request_id = cancel.request_id,
                    "CANCEL for a batch that is not in flight -- ignoring"
                ); }
                Ok(())
            }
            ClientMessage::Ping { nonce } => write_pong(stream, nonce),
            // The zones of the next batch, sent just ahead of it.
            #[cfg(feature = "worker")]
            ClientMessage::ZoningPayload(payload) => {
                crate::serve::zoning::stash(*payload);
                Ok(())
            }
            ClientMessage::Asset(header) => assets::discard_asset(stream, &header),
            ClientMessage::Contribution(header) => {
                indicatrix_net::messages::discard_contribution_payload(stream, &header)
            }
            // v25: not served while batches are in flight; a payload frame is consumed.
            ClientMessage::Job(job) => crate::jobs::ignore_in_flight(stream, &job),
            ClientMessage::RenderRequest(request) => write_error(
                stream,
                error_codes::UNSUPPORTED_REQUEST,
                "a RenderRequest cannot be served while batches are in flight on this connection"
                    .to_owned(),
                request.request_id,
            ),
            ClientMessage::FinalImageRequest(request) => write_error(
                stream,
                error_codes::UNSUPPORTED_REQUEST,
                "a FinalImageRequest cannot be served while batches are in flight on this connection"
                    .to_owned(),
                request.request_id,
            ),
            ClientMessage::Library(_) | ClientMessage::TiltCurvesRequest(_) => {
                tracing::debug!(
                    "ignoring a library or tilt-curves request while batches are in flight"
                );
                Ok(())
            }
        }
    }

    /// The session loop: write what the helpers produce, heartbeat, look at the socket,
    /// until no batch is unfinished.
    fn run<S: Read + Write + TimeoutRead>(
        &mut self,
        stream: &mut S,
        first: BatchRenderRequest,
    ) -> Result<(), NetError> {
        self.admit(stream, first)?;
        loop {
            if self.unfinished.is_empty() {
                return Ok(());
            }
            match self.pipeline.outgoing.recv_timeout(EVENT_WAIT) {
                Ok(event) => {
                    self.write(stream, event)?;
                    while let Ok(event) = self.pipeline.outgoing.try_recv() {
                        self.write(stream, event)?;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return self.helpers_lost(stream),
            }
            if self.unfinished.is_empty() {
                return Ok(());
            }
            self.heartbeat(stream)?;
            match poll_raw_client_message(stream, SOCKET_POLL, &mut self.timeouts)? {
                RawPoll::Pending => {}
                RawPoll::Closed => {
                    self.cancel_all();
                    return Ok(());
                }
                RawPoll::Message(message) => self.handle(stream, message)?,
            }
        }
    }

    /// The helper threads ended with batches unfinished (one panicked outside every
    /// guard): tell the viewer so its deadline does not have to.
    fn helpers_lost<S: Write>(&mut self, stream: &mut S) -> Result<(), NetError> {
        let lost = std::mem::take(&mut self.unfinished);
        for batch in lost {
            write_error(
                stream,
                error_codes::TRACE_PANIC,
                "internal error while tracing this batch".to_owned(),
                batch.request_id,
            )?;
        }
        Ok(())
    }
}

/// Writes `StreamEvent::Error` naming `request_id`.
fn write_error<S: Write>(
    stream: &mut S,
    code: u32,
    message: String,
    request_id: u32,
) -> Result<(), NetError> {
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Error(ErrorMsg {
            code,
            message,
            request_id: Some(request_id),
        }),
        None,
    )
}

/// Serves `first` and every batch that follows on `stream` until none is unfinished;
/// returns to the request loop, which picks up a later batch (or anything else) normally.
///
/// A connection with no render lane of its own refuses: `UNSUPPORTED_REQUEST` on a
/// coordinator (the viewer then sends the pictures one at a time, which the coordinator
/// spreads over its joined workers -- a batch is never forwarded as a whole), otherwise
/// `NO_RENDER_CAPACITY`.
///
/// # Errors
///
/// [`NetError`] for a transport-level failure (the helper threads are then detached; they
/// stop on their own once the cancel flags they check are raised).
pub(super) fn serve_batches<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    first: BatchRenderRequest,
    ctx: &RequestContext<'_>,
) -> Result<(), NetError> {
    if !ctx.own_lane {
        let request_id = first.request_id;
        let (code, message) = if ctx.session.is_some() {
            (
                error_codes::UNSUPPORTED_REQUEST,
                "this coordinator has no render lane of its own and does not forward batches; \
                 send the pictures one at a time",
            )
        } else {
            (
                NO_RENDER_CAPACITY_CODE,
                "this server has no render lane; its WELCOME advertised no render capability",
            )
        };
        tracing::info!(request_id, "{message}");
        return write_error(stream, code, message.to_owned(), request_id);
    }

    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut session = Session {
        pipeline: Pipeline::start(ctx),
        unfinished: Vec::new(),
        timeouts: TimeoutCache::new(),
        last_write: Instant::now(),
    };
    let result = session.run(stream, first);
    // However it ended, nothing still running is wanted any more.
    session.cancel_all();
    session.pipeline.shutdown(result.is_ok());
    let _ = stream.set_write_timeout(None);
    result
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
