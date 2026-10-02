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
//! # A stalled tracer heartbeats but never advances
//!
//! A worker's heartbeat (`Progress` on [`WORKER_CADENCE_MS`]) is traffic like any other
//! byte, so a wedged tracer that keeps emitting it without its `samples_done` ever
//! advancing would otherwise satisfy [`LaneTimeouts::liveness`] forever. [`PatientReader`]
//! tracks the last genuine advance separately (see `note_progress`) and fails the chunk
//! once it has sat still for [`PROGRESS_STALL`], independent of the ordinary liveness
//! deadline.
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
        Cancel, ClientMessage, ContentHash, FrameHeader, RenderRequest, RequestIntent,
        StreamConfig, StreamEvent, TransferMode, hash_hex, write_asset_message,
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

/// How long a worker's `Progress.samples_done` may sit unchanged -- while the connection
/// otherwise stays alive with regular heartbeats -- before the chunk is failed as stalled
/// (about [`WORKER_CADENCE_MS`] * 60: "N cadences"). A wedged tracer that keeps
/// heartbeating without advancing would otherwise pass [`LaneTimeouts::liveness`]
/// forever, since any received byte (heartbeats included) resets that deadline.
const PROGRESS_STALL: Duration = Duration::from_secs(60);

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
    /// The highest `Progress.samples_done` seen so far, and when it last advanced --
    /// distinct from `last_traffic`, which a heartbeat-only `Progress` also bumps. See
    /// [`Self::note_progress`] and [`PROGRESS_STALL`].
    last_progress: Option<u32>,
    last_progress_advance: Instant,
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
            last_progress: None,
            last_progress_advance: Instant::now(),
        }
    }

    /// Records a `Progress.samples_done` reading: bumps [`PROGRESS_STALL`]'s clock only
    /// when it is a genuine advance, so a worker that keeps heartbeating the same count
    /// (a wedged tracer) does not look alive by this measure even though
    /// [`Self::last_traffic`] keeps moving.
    fn note_progress(&mut self, samples_done: u32) {
        if self.last_progress.is_none_or(|prev| samples_done > prev) {
            self.last_progress = Some(samples_done);
            self.last_progress_advance = Instant::now();
        }
    }

    /// One idle tick: send `CANCEL` if due, then fail once a deadline has passed --
    /// [`PROGRESS_STALL`] first (a stalled chunk is failed regardless of how recently a
    /// heartbeat arrived), then the ordinary cancel/liveness/first-reply deadlines.
    ///
    /// `CANCEL` goes out *before* the stall check runs, on the same tick that
    /// `should_cancel` first answers `true` -- otherwise a chunk that stalled and was
    /// then cancelled would fail on `PROGRESS_STALL` before `cancel_sent_at` was ever
    /// set, instead of deferring to [`LaneTimeouts::cancel_wait`] as intended.
    fn on_idle(&mut self) -> std::io::Result<()> {
        if self.cancel_sent_at.is_none() && (self.should_cancel)() {
            let cancel = ClientMessage::Cancel(Cancel {
                request_id: self.request_id,
            });
            indicatrix_net::messages::write_message(&mut self.conn, &cancel)
                .map_err(|e| std::io::Error::other(format!("CANCEL failed: {e}")))?;
            self.cancel_sent_at = Some(Instant::now());
        }
        if self.cancel_sent_at.is_none()
            && self.last_progress.is_some()
            && self.last_progress_advance.elapsed() > PROGRESS_STALL
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "worker reported no sample progress for {PROGRESS_STALL:?} \
                     (heartbeats only, no advance)"
                ),
            ));
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

    /// Sends `CANCEL` right now if it hasn't gone out yet for this request (idempotent),
    /// switching every later deadline to [`LaneTimeouts::cancel_wait`] exactly as
    /// [`Self::on_idle`]'s own cancellation path does -- for a reason internal to THIS
    /// request (an asset the coordinator can no longer forward, see `read_reply`'s
    /// `NeedAsset` handling) rather than the job's cancel token.
    fn cancel_now(&mut self) -> std::io::Result<()> {
        if self.cancel_sent_at.is_some() {
            return Ok(());
        }
        let cancel = ClientMessage::Cancel(Cancel {
            request_id: self.request_id,
        });
        indicatrix_net::messages::write_message(&mut self.conn, &cancel)
            .map_err(|e| std::io::Error::other(format!("CANCEL failed: {e}")))?;
        self.cancel_sent_at = Some(Instant::now());
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
    /// from its start); a frame set that is not a prefix is only usable once complete, and
    /// then only if its frames tile the request exactly (see [`frames_tile`]).
    pub(super) prefix: bool,
    /// The worker's rendering rate, if measurable: the samples between its first and its
    /// last progress report that advanced the count, over the time between those two
    /// reports (see [`rate_from_progress`]). That span ends before the final `FRAME`'s
    /// encode, upload and decode, so it measures tracing alone. A chunk with fewer than
    /// two advancing reports falls back to the first report to `DONE`.
    pub(super) rate: Option<f64>,
    /// Why the request ended short (worker error, protocol problem), if it did.
    pub(super) error: Option<String>,
    /// The worker's own `ERROR` code, when it refused or failed the request.
    pub(super) worker_code: Option<u32>,
    /// `ASSET` transfers (HDR maps) sent to the worker for this request.
    pub(super) assets_sent: u32,
    /// `Some(why)` when the connection itself can no longer be used.
    pub(super) broken: Option<String>,
    /// Every accepted `FRAME`'s `(first_sample, samples)`, for the tiling check.
    spans: Vec<(u32, u32)>,
    /// Pixels skipped across this reply's frames for a non-finite or negative component.
    dropped_pixels: u64,
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
            spans: Vec::new(),
            dropped_pixels: 0,
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

/// Why [`forward_asset`] could not answer a `NEED_ASSET`.
enum ForwardAssetError {
    /// The worker asked for a hash that isn't this job's map: out of sync, the
    /// connection must be discarded.
    Protocol(String),
    /// The coordinator no longer holds the bytes (should not happen since
    /// [`crate::assets::fetch::hold`] pins them for the job, but defensively handled
    /// anyway): only this chunk is lost, not the connection -- see [`read_reply`].
    Missing(String),
    /// The transport itself failed while forwarding: the connection is out of sync and
    /// must be discarded.
    Transport(String),
}

/// Answers the worker's `NEED_ASSET { hash }` from the job's held map (see the module
/// doc comment).
fn forward_asset<F: FnMut() -> bool>(
    reader: &mut PatientReader<'_, F>,
    exchange: Exchange<'_>,
    hash: &ContentHash,
) -> Result<(), ForwardAssetError> {
    let worker_id = exchange.worker_id;
    let Some(asset) = exchange.asset.filter(|a| a.content_hash() == *hash) else {
        return Err(ForwardAssetError::Protocol(format!(
            "worker #{worker_id} asked for asset {}, which is not this job's HDR map (protocol violation)",
            hash_hex(hash)
        )));
    };
    let Some(bytes) = asset.bytes() else {
        return Err(ForwardAssetError::Missing(format!(
            "the coordinator no longer holds HDR map {} to forward to worker #{worker_id}; \
             failing this chunk, not the connection",
            hash_hex(hash)
        )));
    };
    tracing::info!(
        "coordinator: forwarding HDR map {} ({} bytes) to worker #{worker_id}",
        hash_hex(hash),
        bytes.len()
    );
    reader.send_asset(&bytes).map_err(|e| {
        ForwardAssetError::Transport(format!(
            "forwarding HDR map {} to worker #{worker_id}: {e}",
            hash_hex(hash)
        ))
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
    let mut last_progress: Option<(Instant, u32)> = None;
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
                if let Err(why) = fold_frame(&mut reply, &mut decoder, request, &header, payload) {
                    return reply.broke(format!("worker #{worker_id} {why}"));
                }
            }
            StreamEvent::Progress(p) => {
                reader.note_progress(p.samples_done);
                let now = Instant::now();
                if first_progress.is_none() && p.samples_done > 0 {
                    first_progress = Some((now, p.samples_done));
                }
                // Only a genuine advance moves the end of the measured span: a heartbeat
                // repeating the same count, or the quiet while the frame uploads, must not.
                if p.samples_done > 0 && last_progress.is_none_or(|(_, done)| p.samples_done > done)
                {
                    last_progress = Some((now, p.samples_done));
                }
            }
            StreamEvent::Done(done) => {
                if reply.dropped_pixels > 0 {
                    tracing::warn!(
                        "coordinator: worker #{worker_id} sent {} pixels with non-finite or negative \
                         radiance; they were skipped",
                        reply.dropped_pixels
                    );
                }
                return finish_done(
                    reply,
                    request,
                    worker_id,
                    done.cancelled,
                    first_progress,
                    last_progress,
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
                match forward_asset(reader, exchange, &content_hash) {
                    Ok(()) => reply.assets_sent += 1,
                    Err(ForwardAssetError::Missing(why)) => {
                        // Only this chunk is lost, not the connection: send CANCEL
                        // and keep draining events until the worker's DONE, exactly as an
                        // ordinary job cancellation does, so the connection is clean for
                        // its next chunk.
                        reply.error.get_or_insert(why);
                        if let Err(e) = reader.cancel_now() {
                            return reply.broke(format!("worker #{worker_id}: {e}"));
                        }
                    }
                    Err(ForwardAssetError::Protocol(why) | ForwardAssetError::Transport(why)) => {
                        return reply.broke(why);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Checks one `FRAME` against its chunk and sums it into `reply`.
///
/// The frame must be non-empty, lie inside the chunk, and keep the running sample count
/// within the chunk; its payload must decode. Invalid pixels are skipped (and counted in
/// `reply.dropped_pixels`) but the frame's samples still count as done.
///
/// # Errors
///
/// The reason the frame was refused (the worker's connection is then discarded).
fn fold_frame(
    reply: &mut Reply,
    decoder: &mut PayloadDecoder,
    request: &RenderRequest,
    header: &FrameHeader,
    payload: Option<Vec<u8>>,
) -> Result<(), String> {
    let end = u64::from(request.first_sample) + u64::from(request.samples);
    let contained = header.samples > 0
        && header.first_sample >= request.first_sample
        && u64::from(header.first_sample) + u64::from(header.samples) <= end
        && reply
            .done
            .checked_add(header.samples)
            .is_some_and(|total| total <= request.samples);
    if !contained {
        return Err(format!(
            "sent a FRAME [{}, +{}) outside its chunk",
            header.first_sample, header.samples
        ));
    }
    let bytes = payload.unwrap_or_default();
    let dropped = decoder
        .decode_and_add(
            header.encoding,
            header.raw_len,
            &bytes,
            request.scene.width,
            request.scene.height,
            &mut reply.sum,
        )
        .map_err(|e| format!("sent an undecodable FRAME: {e}"))?;
    reply.dropped_pixels = reply.dropped_pixels.saturating_add(u64::from(dropped));
    reply.prefix &= header.first_sample == request.first_sample + reply.done;
    reply.done += header.samples;
    reply.spans.push((header.first_sample, header.samples));
    Ok(())
}

/// Whether `spans` (`(first_sample, samples)` pairs) tile `[first, first + samples)`
/// exactly: sorted by start they are contiguous, non-overlapping, and cover the whole
/// range.
fn frames_tile(spans: &mut [(u32, u32)], first: u32, samples: u32) -> bool {
    spans.sort_unstable();
    let spans: &[(u32, u32)] = spans;
    let mut next = u64::from(first);
    for &(start, len) in spans {
        if u64::from(start) != next {
            return false;
        }
        next += u64::from(len);
    }
    next == u64::from(first) + u64::from(samples)
}

/// The worker's rendering rate for a chunk of `samples`, from its first and its last
/// `Progress` report that advanced the sample count (each as the instant it arrived and the
/// `samples_done` it carried).
///
/// With two distinct advances the rate is the samples between them over the time between
/// them, so the quiet while the final `FRAME` is encoded, uploaded and decoded (it follows
/// the last advance and precedes `DONE`) is not counted as rendering. A chunk too short
/// for two advances falls back to the samples after the first report over the time since
/// it, which does include that tail. `None` when no report arrived or nothing can be
/// measured.
fn rate_from_progress(
    first: Option<(Instant, u32)>,
    last: Option<(Instant, u32)>,
    samples: u32,
) -> Option<f64> {
    let (first_at, first_done) = first?;
    last.filter(|&(_, done)| done > first_done)
        .and_then(|(last_at, last_done)| {
            marginal_rate(
                last_done - first_done,
                last_at.saturating_duration_since(first_at),
            )
        })
        .or_else(|| marginal_rate(samples.saturating_sub(first_done), first_at.elapsed()))
}

/// The reply once the worker's `DONE` arrived.
///
/// A complete-count reply whose frames do not tile the chunk exactly is not merged: it is
/// reported as a failed chunk (nothing usable, connection still in sync).
fn finish_done(
    mut reply: Reply,
    request: &RenderRequest,
    worker_id: u32,
    cancelled: bool,
    first_progress: Option<(Instant, u32)>,
    last_progress: Option<(Instant, u32)>,
) -> Reply {
    let samples = request.samples;
    if reply.done == samples && !frames_tile(&mut reply.spans, request.first_sample, samples) {
        reply.done = 0;
        reply.prefix = false;
        reply.error = Some(format!(
            "worker #{worker_id} sent {samples} samples whose FRAMEs do not tile the chunk exactly; \
             result discarded"
        ));
        return reply;
    }
    if reply.done == samples {
        reply.rate = rate_from_progress(first_progress, last_progress, samples);
        // A chunk that still finished fully overrides any earlier soft warning (e.g. a
        // since-resolved `NeedAsset` miss) -- `reply.broken` is never set on this path,
        // so there is nothing here that a genuinely broken connection needs preserved.
        reply.error = None;
    } else if !cancelled {
        reply.error = Some(format!(
            "worker #{worker_id} reported DONE after {} of {samples} samples",
            reply.done
        ));
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    /// A connected loopback pair for constructing a [`PatientReader`]; neither end is
    /// read from or written to in these tests, which exercise `on_idle`/`note_progress`
    /// directly by manipulating the reader's clocks rather than by any real traffic.
    fn loopback_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback listener");
        let addr = listener
            .local_addr()
            .expect("a bound listener has a local address");
        let client = TcpStream::connect(addr).expect("connect to our own listener");
        let (server, _) = listener.accept().expect("accept the connection just made");
        (client, server)
    }

    /// A heartbeat-only `Progress` (the same `samples_done` repeated) must not push back
    /// [`PatientReader::last_progress_advance`]; only a genuine increase may.
    #[test]
    fn note_progress_only_advances_the_clock_on_a_genuine_increase() {
        let (mut conn, _peer) = loopback_pair();
        let mut reader = PatientReader::new(&mut conn, 1, LaneTimeouts::default(), || false);
        reader.note_progress(10);
        let after_first = reader.last_progress_advance;
        reader.note_progress(10);
        assert_eq!(
            reader.last_progress_advance, after_first,
            "a repeated samples_done (heartbeat only) must not look like an advance"
        );
        reader.note_progress(11);
        assert_eq!(reader.last_progress, Some(11));
    }

    /// A chunk whose `Progress.samples_done` has sat still for longer than
    /// [`PROGRESS_STALL`] fails, even though the connection itself is otherwise fine
    /// (see the module doc comment's "A stalled tracer heartbeats but never advances").
    #[test]
    fn on_idle_fails_a_chunk_whose_progress_has_stalled_past_the_budget() {
        let (mut conn, _peer) = loopback_pair();
        let mut reader = PatientReader::new(&mut conn, 1, LaneTimeouts::default(), || false);
        reader.note_progress(5);
        reader.last_progress_advance = Instant::now()
            .checked_sub(PROGRESS_STALL + Duration::from_secs(1))
            .expect("the process has been running for at least PROGRESS_STALL + 1s");
        let err = reader
            .on_idle()
            .expect_err("a stalled chunk must fail, not idle forever");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(err.to_string().contains("no sample progress"));
    }

    /// Before any `Progress` has ever arrived, [`PROGRESS_STALL`] must not fire on its
    /// own -- a request that simply hasn't started yet is governed only by the ordinary
    /// first-reply/liveness deadlines.
    #[test]
    fn on_idle_ignores_a_stale_clock_before_any_progress_was_seen() {
        let (mut conn, _peer) = loopback_pair();
        let timeouts = LaneTimeouts {
            first_event: Duration::from_secs(3600),
            ..LaneTimeouts::default()
        };
        let mut reader = PatientReader::new(&mut conn, 1, timeouts, || false);
        reader.last_traffic = Instant::now()
            .checked_sub(PROGRESS_STALL + Duration::from_secs(1))
            .expect("the process has been running for at least PROGRESS_STALL + 1s");
        assert!(reader.on_idle().is_ok());
    }

    /// Once `CANCEL` has gone out, the stall check steps aside for
    /// [`LaneTimeouts::cancel_wait`] -- a stalled-then-cancelled chunk is still bounded,
    /// just by the cancel deadline instead.
    #[test]
    fn on_idle_defers_to_cancel_wait_once_cancel_has_been_sent() {
        let (mut conn, _peer) = loopback_pair();
        let timeouts = LaneTimeouts {
            cancel_wait: Duration::from_secs(3600),
            ..LaneTimeouts::default()
        };
        let mut reader = PatientReader::new(&mut conn, 1, timeouts, || true);
        reader.note_progress(5);
        reader.last_progress_advance = Instant::now()
            .checked_sub(PROGRESS_STALL + Duration::from_secs(1))
            .expect("the process has been running for at least PROGRESS_STALL + 1s");
        // The first `on_idle` sends CANCEL (its `should_cancel` always answers true).
        assert!(reader.on_idle().is_ok());
        assert!(reader.cancel_sent_at.is_some());
        // A second call, still long past PROGRESS_STALL, must not re-trigger the stall
        // failure now that a cancel is in flight -- it defers to `cancel_wait` (3600 s).
        assert!(reader.on_idle().is_ok());
    }

    /// Progress 10 at `t0`, 50 at `t0 + 2 s`, then a slow `FRAME` upload until `DONE` at
    /// `t0 + 10 s`: the worker rendered 40 samples in 2 s. The `DONE` instant is not an
    /// input at all, where the first-progress-to-`DONE` window would give 40 / 10 s.
    #[test]
    fn rate_from_progress_ends_at_the_last_advance_not_at_done() {
        let t0 = Instant::now();
        let first = Some((t0, 10));
        let last = Some((t0 + Duration::from_secs(2), 50));
        let rate = rate_from_progress(first, last, 50).expect("two advances are measurable");
        assert!((rate - 20.0).abs() < 1e-9, "{rate}");
        let through_done = marginal_rate(40, Duration::from_secs(10));
        assert_eq!(through_done, Some(4.0), "the rate this replaces");
    }

    /// A chunk with one advancing report (or none past the first) has no span to
    /// measure, so it falls back to the samples after the first report over the time
    /// since it.
    #[test]
    fn rate_from_progress_falls_back_to_the_whole_window_without_two_advances() {
        let started = Instant::now()
            .checked_sub(Duration::from_secs(4))
            .expect("the process has been running for at least 4 s");
        let first = Some((started, 10));
        for last in [None, first, Some((started + Duration::from_secs(1), 10))] {
            let rate = rate_from_progress(first, last, 50).expect("40 samples in 4 s or more");
            assert!(rate > 0.0 && rate <= 10.0, "{rate}");
            assert!(rate > 5.0, "the test's own clock ran far over: {rate}");
        }
    }

    /// No progress report at all: nothing to measure.
    #[test]
    fn rate_from_progress_is_none_without_any_progress() {
        let last = Some((Instant::now(), 50));
        assert_eq!(rate_from_progress(None, None, 50), None);
        assert_eq!(rate_from_progress(None, last, 50), None);
    }
}
