//! One coordinator-to-worker request over a checked-out joined connection: write it,
//! read the worker's `StreamEvent`s until `DONE`/`ERROR` under liveness deadlines, send
//! `CANCEL` when asked, and decode the radiance.
//!
//! # Reading with deadlines and cancellation on one socket
//!
//! [`PatientReader`] wraps the connection for the framing layer: the socket gets a short
//! read timeout ([`POLL`]) and every timeout is retried inside `read` -- a timed-out read
//! consumes nothing on a raw socket or a rustls stream, so the framing layer never sees
//! a torn frame. Between retries it writes the `CANCEL` once the job's cancel token is
//! raised, and it gives up with a `TimedOut` error once the applicable deadline has
//! passed with no byte received: [`LaneTimeouts::first_event`] before the first byte,
//! [`LaneTimeouts::liveness`] after it (a worker heartbeats at least every 2 s),
//! [`LaneTimeouts::cancel_wait`] once `CANCEL` went out. Any received byte counts as
//! traffic, so a large `FRAME` crossing a slow link never trips the deadline mid-transfer.
//!
//! # Forwarding an HDR map
//!
//! A worker that lacks the job's map answers the request with `NEED_ASSET { hash }`. If
//! `hash` is the job's map the coordinator writes `ASSET` from its held copy
//! ([`PatientReader::send_asset`]); any other hash (or a request whose scene names no
//! map) is a protocol violation and the connection is discarded. The payload goes out
//! in pieces of at most [`ASSET_WRITE_PIECE`] bytes: every piece written counts as
//! traffic (so the liveness deadline starts afresh once the transfer ends, however long
//! a 256 MiB map takes over a slow link), each socket write is still bounded by
//! [`WRITE_TIMEOUT`] (a link that stops moving fails), and the job's cancel flag is
//! checked between pieces (a cancel mid-transfer abandons the connection, which cannot
//! be resynchronised inside a frame).

use crate::{assets::HeldAsset, stream_emit::is_stream_timeout};
use glam::Vec3;
use indicatrix_dispatch::{CancelToken, marginal_rate};
use indicatrix_net::{
    SceneState,
    messages::{
        Cancel, ClientMessage, ContentHash, RenderRequest, RequestIntent, StreamConfig,
        StreamEvent, TransferMode, hash_hex, write_asset_message,
    },
    radiance::PayloadDecoder,
};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

use super::super::WorkerConn;

/// The socket read timeout while a request is in flight: how often the reader re-checks
/// cancellation and its deadline.
pub(in crate::coordinator) const POLL: Duration = Duration::from_millis(50);

/// How long one `write()` to a worker may block.
pub(in crate::coordinator) const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// The cadence asked of a worker: `PROGRESS` once a second (its heartbeat), well under
/// [`LaneTimeouts::liveness`].
const WORKER_CADENCE_MS: u32 = 1000;

/// The largest piece of an `ASSET` payload handed to the connection in one `write`
/// (see the module doc comment).
const ASSET_WRITE_PIECE: usize = 1024 * 1024;

/// A joined lane's deadlines, mirroring the GUI export's remote lane
/// (`LIVENESS_TIMEOUT` 8 s, `FIRST_EVENT_TIMEOUT` 30 s, `CANCEL_WAIT_TIMEOUT` 10 s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneTimeouts {
    /// Longest wait for the first byte of the reply (scene upload, calibration).
    pub first_event: Duration,
    /// Longest silence once the reply has started.
    pub liveness: Duration,
    /// Longest wait for `DONE { cancelled: true }` after `CANCEL`.
    pub cancel_wait: Duration,
}

impl Default for LaneTimeouts {
    fn default() -> Self {
        Self {
            first_event: Duration::from_secs(30),
            liveness: Duration::from_secs(8),
            cancel_wait: Duration::from_secs(10),
        }
    }
}

/// Wraps a worker connection for the framing layer: see the module doc comment.
pub(in crate::coordinator) struct PatientReader<'a, F: FnMut() -> bool> {
    conn: &'a mut dyn WorkerConn,
    request_id: u32,
    timeouts: LaneTimeouts,
    /// Returns `true` once the request should be cancelled.
    should_cancel: F,
    last_traffic: Instant,
    seen_traffic: bool,
    cancel_sent_at: Option<Instant>,
}

impl<'a, F: FnMut() -> bool> PatientReader<'a, F> {
    /// Reads `conn` for request `request_id` under `timeouts`; `should_cancel` is polled
    /// on every idle tick until it first returns `true`, which sends `CANCEL`.
    pub(in crate::coordinator) fn new(
        conn: &'a mut dyn WorkerConn,
        request_id: u32,
        timeouts: LaneTimeouts,
        should_cancel: F,
    ) -> Self {
        Self {
            conn,
            request_id,
            timeouts,
            should_cancel,
            last_traffic: Instant::now(),
            seen_traffic: false,
            cancel_sent_at: None,
        }
    }

    /// One idle tick: send `CANCEL` if due, then fail once the deadline has passed.
    fn on_idle(&mut self) -> std::io::Result<()> {
        if self.cancel_sent_at.is_none() && (self.should_cancel)() {
            let cancel = ClientMessage::Cancel(Cancel {
                request_id: self.request_id,
            });
            indicatrix_net::messages::write_message(&mut self.conn, &cancel)
                .map_err(|e| std::io::Error::other(format!("CANCEL failed: {e}")))?;
            self.cancel_sent_at = Some(Instant::now());
        }
        let (since, limit, what) = match self.cancel_sent_at {
            Some(sent) => (sent, self.timeouts.cancel_wait, "no DONE after CANCEL"),
            None if self.seen_traffic => {
                (self.last_traffic, self.timeouts.liveness, "worker silent")
            }
            None => (self.last_traffic, self.timeouts.first_event, "no reply"),
        };
        if since.elapsed() > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{what} for {limit:?}"),
            ));
        }
        Ok(())
    }

    /// Writes `bytes` to the connection as one `ASSET` message -- see the module doc
    /// comment's "Forwarding an HDR map" for the pacing, deadlines and cancellation.
    ///
    /// # Errors
    ///
    /// The write failure (including a cancel mid-transfer); the connection is then out
    /// of sync and must be discarded.
    pub(in crate::coordinator) fn send_asset(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let mut paced = PacedWriter { reader: self };
        write_asset_message(&mut paced, bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
        paced.flush()?;
        self.last_traffic = Instant::now();
        Ok(())
    }
}

/// The connection's write side during [`PatientReader::send_asset`]: pieces of at most
/// [`ASSET_WRITE_PIECE`], each counted as traffic, the cancel flag checked before each.
struct PacedWriter<'r, 'a, F: FnMut() -> bool> {
    reader: &'r mut PatientReader<'a, F>,
}

impl<F: FnMut() -> bool> Write for PacedWriter<'_, '_, F> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if (self.reader.should_cancel)() {
            return Err(std::io::Error::other(
                "the job was cancelled while forwarding its HDR map",
            ));
        }
        let piece = &buf[..buf.len().min(ASSET_WRITE_PIECE)];
        let written = self.reader.conn.write(piece)?;
        self.reader.last_traffic = Instant::now();
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.reader.conn.flush()
    }
}

impl<F: FnMut() -> bool> Read for PatientReader<'_, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.conn.read(buf) {
                Ok(n) => {
                    if n > 0 {
                        self.last_traffic = Instant::now();
                        self.seen_traffic = true;
                    }
                    return Ok(n);
                }
                Err(e) if is_stream_timeout(&e) => self.on_idle()?,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

/// How one request on a connection ended.
pub(super) struct Reply {
    /// Summed radiance of `done` samples (`width * height` long).
    pub(super) sum: Vec<Vec3>,
    /// Samples received: a prefix of the request when `prefix` holds.
    pub(super) done: u32,
    /// Whether `done` is a valid PREFIX of the request (every frame so far was contiguous
    /// from its start); a nested coordinator's set-frames are only usable once complete.
    pub(super) prefix: bool,
    /// The worker's steady-state rate (first to last progress report), if measurable.
    pub(super) rate: Option<f64>,
    /// Why the request ended short (worker error, protocol problem), if it did.
    pub(super) error: Option<String>,
    /// The worker's own `ERROR` code, when it refused or failed the request.
    pub(super) worker_code: Option<u32>,
    /// `ASSET` transfers (HDR maps) sent to the worker for this request.
    pub(super) assets_sent: u32,
    /// `Some(why)` when the connection itself can no longer be used.
    pub(super) broken: Option<String>,
}

impl Reply {
    fn new(pixels: usize) -> Self {
        Self {
            sum: vec![Vec3::ZERO; pixels],
            done: 0,
            prefix: true,
            rate: None,
            error: None,
            worker_code: None,
            assets_sent: 0,
            broken: None,
        }
    }

    fn broke(mut self, why: String) -> Self {
        self.error.get_or_insert_with(|| why.clone());
        self.broken = Some(why);
        self
    }
}

/// The per-chunk `RenderRequest` a joined worker gets: the job's scene byte for byte, a
/// coordinator-local `request_id`, `FinalOnly` with no preview (the coordinator makes its
/// own), `Batch` intent.
pub(super) fn worker_request(
    request_id: u32,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
) -> RenderRequest {
    RenderRequest {
        request_id,
        scene: scene.clone(),
        first_sample,
        samples,
        stream: StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: WORKER_CADENCE_MS,
            preview: None,
        },
        intent: RequestIntent::Batch,
    }
}

/// Who runs one request and under which rules (see [`run_request`]).
#[derive(Clone, Copy)]
pub(super) struct Exchange<'a> {
    /// The worker's registry id (for messages).
    pub(super) worker_id: u32,
    /// The job's cancel token.
    pub(super) cancel: &'a CancelToken,
    /// The lane's liveness deadlines.
    pub(super) timeouts: LaneTimeouts,
    /// The HDR map the coordinator holds for the job, if the scene names one.
    pub(super) asset: Option<&'a HeldAsset>,
}

/// Runs `request` on `conn` to its end (see the module doc comment). Never forwards the
/// worker's `ErrorMsg`: it only becomes a coordinator-worded [`Reply::error`].
pub(super) fn run_request(
    mut conn: &mut dyn WorkerConn,
    request: &RenderRequest,
    exchange: Exchange<'_>,
) -> Reply {
    let (width, height) = (request.scene.width, request.scene.height);
    let reply = Reply::new(width as usize * height as usize);
    if let Err(e) = conn.set_timeouts(Some(POLL), Some(WRITE_TIMEOUT)) {
        return reply.broke(format!("could not arm the socket deadlines: {e}"));
    }
    let message = ClientMessage::RenderRequest(Box::new(request.clone()));
    let reply = if let Err(e) = indicatrix_net::messages::write_message(&mut conn, &message) {
        reply.broke(format!("sending the chunk failed: {e}"))
    } else {
        let cancel = exchange.cancel;
        let mut reader = PatientReader::new(conn, request.request_id, exchange.timeouts, || {
            cancel.is_cancelled()
        });
        read_reply(&mut reader, exchange, request, reply)
    };
    if reply.broken.is_none() {
        let _ = conn.set_timeouts(None, None);
    }
    reply
}

/// Answers the worker's `NEED_ASSET { hash }` from the job's held map (see the module
/// doc comment). `Err(why)` breaks the connection: a hash that is not the job's map, a
/// map the coordinator no longer holds, or a failed transfer.
fn forward_asset<F: FnMut() -> bool>(
    reader: &mut PatientReader<'_, F>,
    exchange: Exchange<'_>,
    hash: &ContentHash,
) -> Result<(), String> {
    let worker_id = exchange.worker_id;
    let Some(asset) = exchange.asset.filter(|a| a.content_hash() == *hash) else {
        return Err(format!(
            "worker #{worker_id} asked for asset {}, which is not this job's HDR map (protocol violation)",
            hash_hex(hash)
        ));
    };
    let Some(bytes) = asset.bytes() else {
        return Err(format!(
            "the coordinator no longer holds HDR map {} to forward to worker #{worker_id}",
            hash_hex(hash)
        ));
    };
    tracing::info!(
        "coordinator: forwarding HDR map {} ({} bytes) to worker #{worker_id}",
        hash_hex(hash),
        bytes.len()
    );
    reader.send_asset(&bytes).map_err(|e| {
        format!(
            "forwarding HDR map {} to worker #{worker_id}: {e}",
            hash_hex(hash)
        )
    })
}

/// Reads events for `request` until its `DONE`/`ERROR`, folding `FRAME`s into `reply`
/// and answering `NEED_ASSET` from the job's held HDR map.
fn read_reply<F: FnMut() -> bool>(
    reader: &mut PatientReader<'_, F>,
    exchange: Exchange<'_>,
    request: &RenderRequest,
    mut reply: Reply,
) -> Reply {
    let worker_id = exchange.worker_id;
    let mut decoder = PayloadDecoder::new();
    let mut first_progress: Option<(Instant, u32)> = None;
    let end = request.first_sample + request.samples;
    loop {
        let (event, payload) = match indicatrix_net::messages::read_stream_event(reader) {
            Ok(read) => read,
            Err(e) => return reply.broke(format!("worker #{worker_id}: {e}")),
        };
        if event
            .request_id()
            .is_some_and(|id| id != request.request_id)
        {
            continue; // a late event of an earlier request on this connection
        }
        match event {
            StreamEvent::Frame(header) => {
                let contained = header.samples > 0
                    && header.first_sample >= request.first_sample
                    && u64::from(header.first_sample) + u64::from(header.samples) <= u64::from(end)
                    && reply
                        .done
                        .checked_add(header.samples)
                        .is_some_and(|total| total <= request.samples);
                if !contained {
                    return reply.broke(format!(
                        "worker #{worker_id} sent a FRAME [{}, +{}) outside its chunk",
                        header.first_sample, header.samples
                    ));
                }
                let bytes = payload.unwrap_or_default();
                if let Err(e) = decoder.decode_and_add(
                    header.encoding,
                    header.raw_len,
                    &bytes,
                    request.scene.width,
                    request.scene.height,
                    &mut reply.sum,
                ) {
                    return reply.broke(format!(
                        "worker #{worker_id} sent an undecodable FRAME: {e}"
                    ));
                }
                reply.prefix &= header.first_sample == request.first_sample + reply.done;
                reply.done += header.samples;
            }
            StreamEvent::Progress(p) if first_progress.is_none() && p.samples_done > 0 => {
                first_progress = Some((Instant::now(), p.samples_done));
            }
            StreamEvent::Done(done) => {
                return finish_done(
                    reply,
                    worker_id,
                    request.samples,
                    done.cancelled,
                    first_progress,
                );
            }
            StreamEvent::Error(e) => {
                reply.error = Some(format!(
                    "worker #{worker_id} refused or failed the chunk (worker error code {})",
                    e.code
                ));
                reply.worker_code = Some(e.code);
                return reply;
            }
            StreamEvent::NeedAsset { content_hash } => {
                if let Err(why) = forward_asset(reader, exchange, &content_hash) {
                    return reply.broke(why);
                }
                reply.assets_sent += 1;
            }
            _ => {}
        }
    }
}

/// The reply once the worker's `DONE` arrived.
fn finish_done(
    mut reply: Reply,
    worker_id: u32,
    samples: u32,
    cancelled: bool,
    first_progress: Option<(Instant, u32)>,
) -> Reply {
    if reply.done == samples {
        reply.rate = first_progress
            .and_then(|(at, first)| marginal_rate(samples.saturating_sub(first), at.elapsed()));
    } else if !cancelled {
        reply.error = Some(format!(
            "worker #{worker_id} reported DONE after {} of {samples} samples",
            reply.done
        ));
    }
    reply
}
