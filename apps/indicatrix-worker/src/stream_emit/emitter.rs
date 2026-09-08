//! The cadence emitter and delta coalescing: [`run_stream`] owns the socket for the
//! life of one `RenderRequest`, interleaving cadence-paced `FRAME`/`PREVIEW`/`PROGRESS`
//! writes with short-timeout polls for an incoming `CANCEL` or pipelined
//! `RenderRequest`. [`PendingDelta`] is the coalescing buffer un-emitted `FRAME` deltas
//! sum into between emissions -- see `crate::serve`'s module docs for the full
//! architecture.
//!
//! # `PROGRESS` is a liveness signal
//!
//! `StreamEvent::Progress` goes out on every cadence tick regardless of transfer mode or
//! whether new samples landed, and even while `run_stream` waits for a cancelled
//! request's tracer thread to stop. A client treats the absence of any `StreamEvent` for
//! more than a cadence or two as this worker having gone away.
//!
//! `cadence` alone isn't a safe upper bound on that gap: it's client-chosen with only a
//! floor enforced (`super::MIN_CADENCE_FLOOR_MS`). [`super::HEARTBEAT_INTERVAL`] is the
//! hard, cadence-independent ceiling this module additionally guarantees, closing the
//! gap where a wide-cadence 4K export could outrun a client's liveness deadline.

use super::{
    StreamOutcome, TimeoutCache, TimeoutRead, TimeoutWrite,
    downsample::downsample_preview,
    tracer::{SharedState, TracerJob, run_tracer},
};
use crate::cli::ComputeMode;
use glam::Vec3;
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::{
    messages::{
        ClientMessage, Done, FrameHeader, NetError, PreviewConfig, PreviewHeader, Progress,
        RenderRequest, Stats, StreamEvent, TransferMode,
    },
    radiance,
};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// How often the emitter re-checks the socket for a pending `CANCEL` and re-evaluates
/// whether the cadence has elapsed, when the tracer hasn't signalled fresh progress.
/// Independent of `TARGET_SUBBATCH` -- a poll interval, not a batch size.
const EMITTER_POLL: Duration = Duration::from_millis(20);

/// Caps how long any single `write()` inside this request's streaming phase may block --
/// see [`super::TimeoutWrite`] for the stall/stuck-cancellation bug this closes.
///
/// Generous relative to [`EMITTER_POLL`]/[`super::MIN_CADENCE_FLOOR_MS`], which bound
/// how often this worker wants to check in; this bounds the worst case for a genuinely
/// quiet peer, with slack for a merely slow (not stuck) network -- a `FRAME` payload is
/// the full scene resolution regardless of cadence, so a healthy connection over a
/// constrained link can legitimately take real time to drain one.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// A full-resolution radiance delta accumulated (coalesced) since it was last taken --
/// the shared buffer `run_tracer`'s sub-batches fold into and `run_stream`'s emitter
/// drains on the cadence.
///
/// Coalescing two adjacent deltas is exactly `PendingDelta::add` twice before one
/// `take`: contributions sum elementwise and sample ranges merge into one contiguous
/// union, so a delta sent as two separate `FRAME`s or one coalesced one sums identically.
pub(super) struct PendingDelta {
    buffer: Vec<Vec3>,
    /// `(first_sample, samples)` of the buffer's contents so far, or `None` if nothing
    /// has been folded in since the last `take`.
    range: Option<(u32, u32)>,
}

impl PendingDelta {
    pub(super) fn new(pixel_count: usize) -> Self {
        Self {
            buffer: vec![Vec3::ZERO; pixel_count],
            range: None,
        }
    }

    /// Folds `contribution` (the sum for sample sub-range `[first_sample, first_sample +
    /// samples)`) into this delta. Must immediately follow whatever range is already
    /// pending; debug-asserted since a violation would mean `run_tracer` is broken, not
    /// caller-supplied data.
    pub(super) fn add(&mut self, first_sample: u32, samples: u32, contribution: &[Vec3]) {
        debug_assert_eq!(contribution.len(), self.buffer.len());
        match self.range {
            None => {
                self.buffer.copy_from_slice(contribution);
                self.range = Some((first_sample, samples));
            }
            Some((range_first, range_samples)) => {
                debug_assert_eq!(
                    first_sample,
                    range_first + range_samples,
                    "sub-batches must coalesce in contiguous sample order"
                );
                for (acc, c) in self.buffer.iter_mut().zip(contribution) {
                    *acc += *c;
                }
                self.range = Some((range_first, range_samples + samples));
            }
        }
    }

    /// Exchanges this delta's buffer with `spare` via `mem::swap` -- O(1), no allocation
    /// or per-pixel copy -- resetting this delta to empty and leaving what was pending
    /// in `spare` for the caller. Returns the previous range (`None` if nothing had been
    /// added since the last swap).
    ///
    /// `spare` must be the same length as this delta's buffer (debug-asserted); its
    /// contents on the way in are never inspected, since [`Self::add`]'s first call
    /// after this always overwrites the whole buffer via `copy_from_slice`.
    ///
    /// The O(1) alternative to a take-then-clone: [`EmitterAccum`] is the one caller,
    /// and this is what keeps `emit_tick`'s only work under the shared `Mutex` to the
    /// swap plus reading `samples_done`.
    pub(super) fn swap_with(&mut self, spare: &mut Vec<Vec3>) -> Option<(u32, u32)> {
        debug_assert_eq!(
            spare.len(),
            self.buffer.len(),
            "swap_with's spare buffer must match this delta's pixel count"
        );
        std::mem::swap(&mut self.buffer, spare);
        self.range.take()
    }
}

/// The emitter's own accumulation state: a running total maintained by folding in every
/// delta taken from [`SharedState::pending_delta`], plus the spare buffer that delta
/// gets swapped into. Owned solely by `run_stream`'s emitter -- never behind the shared
/// `Mutex`, never touched by the tracer thread -- so building a `PREVIEW` or the
/// `FinalOnly` final `FRAME` never needs the lock and can never stall `run_tracer`.
///
/// [`Self::swap_and_fold`] is called every cadence tick and once more in `emit_final`,
/// regardless of [`TransferMode`]: `running_total` must stay current for `PREVIEW`
/// under every transfer mode, and for `FinalOnly`'s own final `FRAME` at the end.
/// Whether the swapped-out delta is ALSO written to the wire as a `FRAME` is a separate
/// decision `emit_tick`/`emit_final` make afterward.
pub(super) struct EmitterAccum {
    /// The full-resolution cumulative sum of every delta folded in so far. Read (never
    /// written) by [`write_preview`] and by `emit_final`'s `TransferMode::FinalOnly`
    /// branch.
    running_total: Vec<Vec3>,
    /// Exchanged with [`SharedState::pending_delta`]'s buffer via
    /// [`PendingDelta::swap_with`] on every [`Self::swap_and_fold`] call; holds whatever
    /// delta was most recently swapped out until the next swap overwrites it.
    spare: Vec<Vec3>,
}

impl EmitterAccum {
    pub(super) fn new(pixel_count: usize) -> Self {
        Self {
            running_total: vec![Vec3::ZERO; pixel_count],
            spare: vec![Vec3::ZERO; pixel_count],
        }
    }

    /// Test-only: seeds `running_total` directly, standing in for "several ticks'
    /// worth of deltas have already been folded in", without needing to actually drive
    /// a `SharedState` through that many swaps first.
    #[cfg(test)]
    pub(super) fn from_running_total(running_total: Vec<Vec3>) -> Self {
        let spare = vec![Vec3::ZERO; running_total.len()];
        Self {
            running_total,
            spare,
        }
    }

    /// Locks `state` just long enough to swap [`SharedState::pending_delta`]'s buffer
    /// into `self.spare` and read `samples_done`, then releases the lock before doing
    /// anything else -- no clone or socket write ever happens while this `Mutex` is
    /// held. Once unlocked, if a delta was swapped out, folds it into
    /// `self.running_total` elementwise -- the only place `running_total` is updated,
    /// entirely outside the lock.
    ///
    /// Returns the swapped-out delta's `(first_sample, samples)` range alongside
    /// `samples_done`, read under the same lock acquisition so the two can never
    /// disagree about which tick they describe.
    pub(super) fn swap_and_fold(
        &mut self,
        state: &Mutex<SharedState>,
    ) -> (Option<(u32, u32)>, u32) {
        let (range, samples_done) = {
            let mut guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let range = guard.pending_delta.swap_with(&mut self.spare);
            (range, guard.samples_done)
        };
        if range.is_some() {
            for (total, c) in self.running_total.iter_mut().zip(self.spare.iter()) {
                *total += *c;
            }
        }
        (range, samples_done)
    }

    /// The delta most recently swapped out by [`Self::swap_and_fold`] -- meaningful only
    /// when that call returned `Some`; otherwise stale-but-harmless (already folded into
    /// `running_total`), since no caller reads this without first checking the range.
    pub(super) fn delta(&self) -> &[Vec3] {
        &self.spare
    }

    /// The full-resolution cumulative sum of every delta folded in so far -- this
    /// emitter's own copy, never read from `SharedState`.
    pub(super) fn running_total(&self) -> &[Vec3] {
        &self.running_total
    }
}

/// What one poll of the socket for a pending [`ClientMessage`] found.
pub(super) enum ClientPoll {
    /// Nothing arrived within the poll window -- not necessarily an error.
    Pending,
    /// A `CANCEL` for the request currently streaming.
    Cancelled,
    /// A `CANCEL` for some other `request_id` (e.g. stale, from an already-ended
    /// request). Not honored.
    Stale,
    /// The peer closed the connection.
    Closed,
    /// The client's next `RenderRequest`, pipelined ahead of `DONE` for the request
    /// currently streaming -- see [`run_stream`]'s doc comment for how this is handled.
    NextRequest(Box<RenderRequest>),
}

/// Attempts to read one pending message frame, tolerating a `WouldBlock`/`TimedOut`/
/// `WriteZero` `io::Error` as "nothing pending" rather than a fatal error (see
/// [`super::is_stream_timeout`] for why `WriteZero` counts here too).
///
/// Reads the length prefix with a single `read` (not `read_exact`) so a timeout can only
/// land before any byte of a new message has arrived. Once a byte is observed, the rest
/// of that message is read under a bounded timeout ([`super::FRAME_REMAINDER_TIMEOUT`])
/// rather than an unbounded blocking read; a timeout there is a protocol error tearing
/// down the connection, since a message already in flight never arriving is itself the
/// anomaly.
///
/// # `timeouts` avoids a redundant `set_read_timeout` on every poll
///
/// `timeouts` (see [`TimeoutCache`]) caches the last read-timeout value actually applied
/// to `stream` through it and skips the underlying `set_read_timeout` call when the
/// requested value is unchanged. On an idle streaming connection this function is called
/// every [`EMITTER_POLL`] (20ms) and always asks for the same `poll_timeout`, so without
/// this the socket option would be set ~50 times/s for no behavioral effect; the
/// `FRAME_REMAINDER_TIMEOUT` switch after the first byte still goes through normally,
/// since that value genuinely differs from `poll_timeout`. Owned by [`run_stream`] for
/// the life of one request, not shared across requests -- see [`TimeoutCache::new`].
///
/// # Pipelining a `RENDER` ahead of `DONE` is supported
///
/// Whatever bytes arrive here while a request is streaming decode as a tagged
/// [`ClientMessage`] (`Cancel` or `RenderRequest`) rather than being assumed to be
/// `Cancel`. [`ClientPoll::NextRequest`] is how a pipelined request surfaces to
/// [`run_stream`], which treats it as an implicit cancel of the request currently
/// streaming (matches how the viewer actually behaves, and removes the `DONE` round
/// trip from the drag-to-render responsiveness path).
pub(super) fn poll_for_client_message<S: Read + TimeoutRead>(
    stream: &mut S,
    request_id: u32,
    poll_timeout: Duration,
    timeouts: &mut TimeoutCache,
) -> Result<ClientPoll, NetError> {
    timeouts
        .apply(stream, Some(poll_timeout))
        .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;

    let mut len_bytes = [0u8; indicatrix_net::framing::LEN_PREFIX_BYTES];
    let n = match stream.read(&mut len_bytes) {
        Ok(0) => return Ok(ClientPoll::Closed),
        Ok(n) => n,
        Err(e) if super::is_stream_timeout(&e) => {
            return Ok(ClientPoll::Pending);
        }
        Err(e) => {
            return Err(NetError::Framing(
                indicatrix_net::framing::FramingError::Io(e),
            ));
        }
    };

    // At least one byte has arrived -- commit to reading the rest under a bounded
    // timeout, not `None`: a stall here is a protocol error, not "nothing new yet".
    timeouts
        .apply(stream, Some(super::FRAME_REMAINDER_TIMEOUT))
        .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;
    if n < len_bytes.len() {
        stream
            .read_exact(&mut len_bytes[n..])
            .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > indicatrix_net::framing::MAX_FRAME_LEN {
        return Err(NetError::Framing(
            indicatrix_net::framing::FramingError::FrameTooLarge {
                len,
                max: indicatrix_net::framing::MAX_FRAME_LEN,
            },
        ));
    }
    let mut payload = vec![0u8; len as usize];
    stream
        .read_exact(&mut payload)
        .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;

    let msg: ClientMessage = postcard::from_bytes(&payload)?;
    Ok(match msg {
        ClientMessage::Cancel(cancel) if cancel.request_id == request_id => ClientPoll::Cancelled,
        ClientMessage::Cancel(_) => ClientPoll::Stale,
        ClientMessage::RenderRequest(next) => ClientPoll::NextRequest(next),
        // Only RenderRequest-vs-RenderRequest pipelining is in scope; a LibraryRequest
        // mid-stream gets no reply, like a stale Cancel. A client should wait for DONE
        // before sending a LibraryRequest on a connection with a render in flight.
        ClientMessage::Library(_) => {
            tracing::debug!(
                "received a LibraryRequest while request_id={request_id} was streaming -- library requests \
                 are not serviced mid-stream in this phase; ignoring"
            );
            ClientPoll::Stale
        }
        // TILT_CURVES supports no pipelining of its own; dropped for the same reason as
        // a mid-stream LibraryRequest above.
        ClientMessage::TiltCurvesRequest(_) => {
            tracing::debug!(
                "received a TiltCurvesRequest while request_id={request_id} was streaming -- \
                 TILT_CURVES is not serviced mid-stream in this phase; ignoring"
            );
            ClientPoll::Stale
        }
    })
}

/// Computes [`Stats::effective_cadence_ms`]: the average wall-clock interval between
/// emissions over `elapsed`, or `0` if fewer than two emissions happened (nothing to
/// average -- see that field's doc comment).
pub(super) fn effective_cadence_ms(elapsed: Duration, emission_count: u32) -> u32 {
    if emission_count < 2 {
        0
    } else {
        (elapsed.as_millis() / u128::from(emission_count - 1)) as u32
    }
}

/// Spawns [`run_tracer`] on its own thread against a freshly built [`SharedState`], per
/// [`run_stream`]'s doc comment on why the tracer gets its own thread and shared state
/// rather than running inline.
fn spawn_tracer(
    request: &RenderRequest,
    threads: usize,
    gpu: &Arc<GpuBackend>,
    compute_mode: ComputeMode,
) -> (
    Arc<Mutex<SharedState>>,
    Arc<AtomicBool>,
    mpsc::Receiver<()>,
    std::thread::JoinHandle<()>,
) {
    let pixel_count = request.scene.width as usize * request.scene.height as usize;
    let state = Arc::new(Mutex::new(SharedState {
        pending_delta: PendingDelta::new(pixel_count),
        samples_done: 0,
        finished: false,
        panicked: false,
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let (progress_tx, progress_rx) = mpsc::channel::<()>();

    let job = TracerJob {
        scene: request.scene.clone(),
        first_sample: request.first_sample,
        samples: request.samples,
        threads,
        compute_mode,
    };
    let tracer_state = Arc::clone(&state);
    let tracer_cancel = Arc::clone(&cancel);
    // Cloning the Arc (not borrowing) lets this run on a real `std::thread::spawn`
    // (needs `'static`) rather than a scoped thread; the renderer is acquired once at
    // `serve::run` startup and shared by every connection.
    let tracer_gpu = Arc::clone(gpu);
    let handle = std::thread::spawn(move || {
        run_tracer(
            &job,
            &tracer_gpu,
            &tracer_state,
            &tracer_cancel,
            &progress_tx,
        );
    });

    (state, cancel, progress_rx, handle)
}

/// Writes the `DONE { cancelled: true, .. }` reply for a cancelled request -- the sole
/// payload it ever carries, per [`run_stream`]'s doc comment on why an unsent buffer is
/// discarded rather than flushed on cancellation.
fn write_cancelled_done<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    streaming_start: Instant,
    emission_count: u32,
) -> Result<(), NetError> {
    let stats = {
        let guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Stats {
            samples_done: guard.samples_done,
            requested_cadence_ms: request.stream.cadence_ms,
            effective_cadence_ms: effective_cadence_ms(streaming_start.elapsed(), emission_count),
        }
    };
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Done(Done {
            request_id: request.request_id,
            cancelled: true,
            stats,
        }),
        None,
    )
}

/// The bundle [`run_stream_loop`] hands back to [`run_stream`] once its cadence-paced
/// main loop ends -- the loop's own `Result`, plus the three flags/values `run_stream`
/// needs afterward to decide between a normal finish, a peer-closed early return, and
/// a cancelled `DONE` write.
struct StreamLoopOutcome {
    result: Result<StreamOutcome, NetError>,
    peer_closed: bool,
    cancelled: bool,
    pipelined_next: Option<RenderRequest>,
    emission_count: u32,
}

/// `run_stream`'s own cadence-paced main loop: polls for a client message, then either
/// winds down (cancel/close), emits a cadence tick or heartbeat, or emits the final
/// payload once the tracer finishes. Split out purely to keep `run_stream` under
/// clippy's function-length limit -- see that function's own doc comment for the full
/// cancellation/heartbeat/pipelining rationale this loop implements.
fn run_stream_loop<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    cancel: &Arc<AtomicBool>,
    progress_rx: &mpsc::Receiver<()>,
    emitter_accum: &mut EmitterAccum,
    streaming_start: Instant,
) -> StreamLoopOutcome {
    let cadence = Duration::from_millis(u64::from(request.stream.cadence_ms));
    let mut last_emit = Instant::now()
        .checked_sub(cadence)
        .unwrap_or_else(Instant::now);
    let mut emission_count: u32 = 0;
    // A connection-closed/transport-error poll, a CANCEL, or a pipelined RenderRequest
    // all set `cancel` and stop the loop; `tracer_handle` is joined once, by the caller,
    // after this returns.
    let mut cancelled = false;
    let mut peer_closed = false;
    // The client's next RenderRequest, if pipelined ahead of DONE (queued, not
    // rejected -- see `run_stream`'s doc comment). Handed back once this call returns.
    let mut pipelined_next: Option<RenderRequest> = None;
    // Fresh per request, never reused -- see `poll_for_client_message`'s doc comment.
    let mut timeouts = TimeoutCache::new();

    let result = loop {
        let _ = progress_rx.recv_timeout(EMITTER_POLL);

        match poll_for_client_message(stream, request.request_id, EMITTER_POLL, &mut timeouts) {
            Ok(ClientPoll::Cancelled) => {
                cancel.store(true, Ordering::Relaxed);
                cancelled = true;
            }
            Ok(ClientPoll::NextRequest(next)) => {
                // Implicit cancel-then-queue: discard the unsent buffer like an
                // explicit CANCEL, and remember `next` for the caller.
                cancel.store(true, Ordering::Relaxed);
                cancelled = true;
                pipelined_next = Some(*next);
            }
            Ok(ClientPoll::Closed) => {
                cancel.store(true, Ordering::Relaxed);
                peer_closed = true;
            }
            Ok(ClientPoll::Pending | ClientPoll::Stale) => {}
            Err(e) => {
                cancel.store(true, Ordering::Relaxed);
                break Err(e);
            }
        }

        if cancelled || peer_closed {
            // This loop's own emission ends here, but the tracer may still be mid
            // sub-batch for up to one sub-batch's wall time -- keep heartbeating while
            // waiting for it (see wait_for_tracer_to_stop and this function's doc
            // comment).
            wait_for_tracer_to_stop(
                stream,
                request,
                state,
                progress_rx,
                cadence.min(super::HEARTBEAT_INTERVAL),
                &mut last_emit,
                &mut emission_count,
            );
            break Ok(StreamOutcome::Completed);
        }

        let finished = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finished;

        if !finished
            && let Err(e) = emit_tick_or_heartbeat(
                stream,
                request,
                state,
                emitter_accum,
                cadence,
                &mut last_emit,
                &mut emission_count,
            )
        {
            cancel.store(true, Ordering::Relaxed);
            break Err(e);
        }

        if finished {
            let panicked = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .panicked;
            if panicked {
                break Ok(StreamOutcome::TracePanicked);
            }
            let r = emit_final(
                stream,
                request,
                state,
                emitter_accum,
                streaming_start,
                &mut emission_count,
            );
            break r.map(|()| StreamOutcome::Completed);
        }
    };

    StreamLoopOutcome {
        result,
        peer_closed,
        cancelled,
        pipelined_next,
        emission_count,
    }
}

/// Streams one `RenderRequest`'s samples over `stream`, per `serve`'s module docs:
/// spawns a tracer thread (via [`run_tracer`]) that never touches `stream`, then this
/// function -- the emitter -- owns `stream` for the rest of the request, interleaving
/// cadence-paced `FRAME`/`PREVIEW`/`PROGRESS` writes with short-timeout polls (via
/// [`poll_for_client_message`]) for an incoming `CANCEL` or pipelined `RenderRequest`.
/// The tracer is decoupled so a slow `write()` never stalls sample production.
///
/// On a normal finish, sends a final `FRAME` (the whole request for
/// [`TransferMode::FinalOnly`]; whatever's left un-coalesced for
/// [`TransferMode::LiveProgressive`]) followed by `DONE { cancelled: false, .. }`. On
/// cancellation (a `CANCEL`, the peer closing, or a pipelined `RenderRequest`), discards
/// whatever hasn't been sent and sends `DONE { cancelled: true, .. }` with no further
/// payload (via [`write_cancelled_done`]). If the tracer panics, sends nothing further
/// and returns [`StreamOutcome::TracePanicked`]. Restores `stream`'s read timeout to
/// blocking before returning either way. Every [`StreamEvent`] written carries
/// `request.request_id`, so a stale reply is identifiable on the reading side.
///
/// # Cancellation still heartbeats while the tracer winds down
///
/// The tracer can take up to one sub-batch's wall time to notice `cancel` and stop after
/// this function's own cadence loop has already ended. Rather than going silent for that
/// window, this keeps sending a bare `Progress` heartbeat at most every
/// `cadence.min(HEARTBEAT_INTERVAL)` (see [`wait_for_tracer_to_stop`]), so a client
/// watching for `DONE` can't confuse "cancelling" with "worker died". The cadence-paced
/// loop has the identical gap: `cadence` is client-chosen with only a floor
/// (`super::MIN_CADENCE_FLOOR_MS`), never a ceiling, so this loop also wakes and
/// heartbeats at least every [`super::HEARTBEAT_INTERVAL`] regardless of `cadence`,
/// whenever a real due tick isn't already covering that instant.
///
/// # The cancelled `DONE` write stays inside `WRITE_TIMEOUT`
///
/// `write_cancelled_done` runs, and its error is propagated, before the write timeout is
/// cleared -- an earlier version cleared both timeouts right after `tracer_handle.join()`
/// and only then attempted this write, leaving it able to block forever against a peer
/// that stopped reading. The read timeout is restored to blocking right after the join
/// regardless of outcome; the write timeout stays live through this one additional write.
///
/// # A pipelined `RenderRequest` is queued, not rejected
///
/// [`poll_for_client_message`] can hand back [`ClientPoll::NextRequest`] -- the client's
/// next `RenderRequest`, sent without waiting for `DONE`. This function treats it as an
/// implicit `CANCEL` of the current request followed by the new one, matching how the
/// viewer actually behaves; forcing a `DONE` round trip or bouncing it back as an error
/// would only add latency. The superseded request's [`SharedState`] is simply dropped
/// once this call returns, since the caller starts the new request with a brand-new
/// [`run_stream`] call and a brand-new [`SharedState`] -- the two never coexist.
///
/// Returns the pipelined request (if any) as the second tuple element, for the caller to
/// start immediately instead of blocking on another read.
///
/// `gpu` threads through to [`spawn_tracer`]/[`run_tracer`], which prefers it over the
/// CPU tracer per adaptively-sized sub-batch; `compute_mode` threads through to
/// [`spawn_tracer`]/[`TracerJob`], one connection-wide choice applied to every request.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure -- the same conditions
/// `handle_connection`'s request loop already propagates for.
pub fn run_stream<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: &RenderRequest,
    threads: usize,
    gpu: &Arc<GpuBackend>,
    compute_mode: ComputeMode,
) -> Result<(StreamOutcome, Option<RenderRequest>), NetError> {
    let (state, cancel, progress_rx, tracer_handle) =
        spawn_tracer(request, threads, gpu, compute_mode);
    let mut emitter_accum =
        EmitterAccum::new(request.scene.width as usize * request.scene.height as usize);

    // Bounds every write this call makes to at most WRITE_TIMEOUT; scoped to the
    // streaming phase and restored before returning.
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let streaming_start = Instant::now();

    let StreamLoopOutcome {
        result,
        peer_closed,
        cancelled,
        pipelined_next,
        emission_count,
    } = run_stream_loop(
        stream,
        request,
        &state,
        &cancel,
        &progress_rx,
        &mut emitter_accum,
        streaming_start,
    );

    let _ = tracer_handle.join();

    // Restore the read timeout to blocking now -- nothing more is read on `stream`
    // regardless of outcome. The write timeout deliberately stays live a little
    // further, through `write_cancelled_done` below (see this function's doc comment).
    let _ = stream.set_read_timeout(None);

    if peer_closed {
        let _ = stream.set_write_timeout(None);
        return Ok((StreamOutcome::Completed, None));
    }

    if cancelled {
        // `?` (not `let _ =`) is deliberate: a write failing under WRITE_TIMEOUT means
        // this connection is dead, and swallowing it would pretend DONE was delivered.
        let cancelled_done_result =
            write_cancelled_done(stream, request, &state, streaming_start, emission_count);
        let _ = stream.set_write_timeout(None);
        cancelled_done_result?;
        // `state` (this request's PendingDelta/running total) is dropped right here
        // either way, never handed to whatever comes next.
        return Ok((StreamOutcome::Completed, pipelined_next));
    }

    let _ = stream.set_write_timeout(None);
    result.map(|outcome| (outcome, None))
}

/// Waits for the tracer thread to signal it has stopped (`state.finished`), sending a
/// bare [`StreamEvent::Progress`] heartbeat at `heartbeat_bound` in the meantime -- see
/// [`run_stream`]'s doc comment for the liveness gap this closes.
///
/// `heartbeat_bound` is `cadence.min(HEARTBEAT_INTERVAL)` in production, computed once
/// by [`run_stream`] (keeps this under clippy's argument-count limit, and lets tests
/// inject a short bound instead of waiting on the real interval). Capping at
/// `HEARTBEAT_INTERVAL` keeps this wind-down heartbeat from going quiet longer than the
/// client's liveness deadline even when `cadence` is wider than that deadline.
///
/// Polls `state.finished` via the same `progress_rx` channel the main loop uses, rather
/// than time-bounding `tracer_handle.join()` itself (`JoinHandle` has no
/// join-with-timeout).
///
/// Never sends `FRAME`/`PREVIEW`: that state is about to be discarded wholesale once the
/// request is confirmed cancelled, so sending either would be wasted bytes.
///
/// Best-effort: a heartbeat write failing here just stops this loop early -- the
/// connection is already ending, and a failed write means it's also transport-dead,
/// which `write_cancelled_done` will hit again and report.
pub(super) fn wait_for_tracer_to_stop<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    progress_rx: &mpsc::Receiver<()>,
    heartbeat_bound: Duration,
    last_emit: &mut Instant,
    emission_count: &mut u32,
) {
    while !state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .finished
    {
        let _ = progress_rx.recv_timeout(EMITTER_POLL);
        if last_emit.elapsed() >= heartbeat_bound {
            if emit_progress_heartbeat(stream, request, state, emission_count).is_err() {
                break;
            }
            *last_emit = Instant::now();
        }
    }
}

/// One cadence-tick opportunity in `run_stream`'s main loop: the full [`emit_tick`]
/// once `last_emit.elapsed() >= cadence`, otherwise the bare
/// [`maybe_emit_heartbeat_backstop`] heartbeat. Pulled out of `run_stream` to keep that
/// function under clippy's line-count limit.
fn emit_tick_or_heartbeat<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emitter_accum: &mut EmitterAccum,
    cadence: Duration,
    last_emit: &mut Instant,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    if last_emit.elapsed() >= cadence {
        emit_tick(stream, request, state, emitter_accum, emission_count)?;
        *last_emit = Instant::now();
        return Ok(());
    }
    maybe_emit_heartbeat_backstop(
        stream,
        request,
        state,
        super::HEARTBEAT_INTERVAL,
        last_emit,
        emission_count,
    )
}

/// The `HEARTBEAT_INTERVAL` backstop inside `run_stream`'s own cadence-paced loop:
/// writes a bare `Progress` heartbeat and resets `*last_emit` whenever
/// `last_emit.elapsed() >= heartbeat_interval`. Called only when a cadence-due tick
/// isn't already covering this instant, so it never fires for a `cadence` at or under
/// [`super::HEARTBEAT_INTERVAL`] and exists purely to cap the gap for a wider one.
fn maybe_emit_heartbeat_backstop<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    heartbeat_interval: Duration,
    last_emit: &mut Instant,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    if last_emit.elapsed() < heartbeat_interval {
        return Ok(());
    }
    emit_progress_heartbeat(stream, request, state, emission_count)?;
    *last_emit = Instant::now();
    Ok(())
}

/// Writes a bare `StreamEvent::Progress` -- no `FRAME`/`PREVIEW` -- for
/// [`wait_for_tracer_to_stop`]'s heartbeat; see that function's and this module's own
/// doc comments for why.
fn emit_progress_heartbeat<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    let samples_done = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .samples_done;
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Progress(Progress {
            request_id: request.request_id,
            samples_done,
        }),
        None,
    )?;
    *emission_count += 1;
    Ok(())
}

/// One periodic (cadence-elapsed) emission: `FRAME` (if [`TransferMode::LiveProgressive`]
/// and there's a pending delta), `PREVIEW` (if configured, at least one sample has
/// landed, and it isn't redundant with a `FRAME` this same tick), then `PROGRESS` --
/// always, even under [`TransferMode::FinalOnly`] and before the first sample.
///
/// # The only work done under `state`'s lock is the swap
///
/// `emitter.swap_and_fold(state)` (see [`EmitterAccum`]) is the only place this function
/// touches `state`: it locks just long enough to exchange `pending_delta`'s buffer and
/// read `samples_done`, releasing the lock before folding, encoding, downsampling, or
/// writing anything. This avoids cloning the full-resolution `running_total` while
/// `state` is locked -- a `memcpy` of the whole frame (about 25 MB at 1080p) that would
/// otherwise stand directly between `run_tracer` and the lock it needs after every
/// sub-batch.
///
/// The swap happens unconditionally regardless of [`TransferMode`], since
/// `running_total` must stay current for `PREVIEW` under every mode; whether the
/// swapped-out delta is also written as a `FRAME` is decided separately below.
pub(super) fn emit_tick<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emitter: &mut EmitterAccum,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    let (range, samples_done) = emitter.swap_and_fold(state);

    // Whether this tick actually wrote a `FRAME` -- consulted below before deciding
    // whether a full-scale `PREVIEW` would be redundant with it. Only
    // `LiveProgressive` ever sends the swapped-out delta as a `FRAME` here;
    // `FinalOnly` still needed the swap to keep `running_total` current, but
    // `emit_final` sends its one and only `FRAME` once tracing completes.
    let frame_sent_this_tick =
        range.is_some() && matches!(request.stream.transfer_mode, TransferMode::LiveProgressive);

    if let Some((first_sample, samples)) = range
        && frame_sent_this_tick
    {
        // `as_bytes`, not `encode`: a reinterpreted view over `emitter`'s own delta
        // buffer, not a fresh per-tick `Vec<u8>` allocation+copy -- see that function's
        // doc comment.
        let xyz_bytes = radiance::as_bytes(emitter.delta());
        let header = FrameHeader::for_payload(request.request_id, first_sample, samples, xyz_bytes);
        indicatrix_net::messages::write_stream_event(
            stream,
            &StreamEvent::Frame(header),
            Some(xyz_bytes),
        )?;
        *emission_count += 1;
    }

    // Skip the PREVIEW when nothing has been folded in yet: without this check the
    // first tick would write a full-size PREVIEW of pure zeros (~99.5 MB at 4K) for a
    // client about to overwrite it. PROGRESS still goes out unconditionally below.
    //
    // Also skip it when `cfg` is exactly the frame's own full resolution and this tick
    // already sent a `FRAME` delta: a client's `Accumulator` rebuilds the identical
    // image from `FRAME` deltas already received, so a full-scale `PREVIEW` riding
    // alongside a `FRAME` would just double that tick's bandwidth. Only applies under
    // `LiveProgressive`; a downscaled `PREVIEW` is never skipped.
    if let Some(cfg) = request.stream.preview
        && samples_done > 0
    {
        let full_scale_and_redundant = frame_sent_this_tick
            && cfg.width == request.scene.width
            && cfg.height == request.scene.height;
        if !full_scale_and_redundant {
            write_preview(stream, request, cfg, emitter.running_total(), samples_done)?;
        }
    }

    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Progress(Progress {
            request_id: request.request_id,
            samples_done,
        }),
        None,
    )?;
    *emission_count += 1;

    Ok(())
}

/// The final emission once the tracer has finished (without cancellation or a panic):
/// the last `FRAME` -- the whole request for [`TransferMode::FinalOnly`], or whatever's
/// left un-coalesced for [`TransferMode::LiveProgressive`] -- then `DONE { cancelled: false }`.
///
/// Swaps once more before reading `emitter`'s state: the tracer's last sub-batch(es) may
/// have landed after the last cadence tick's swap but before `finished` was noticed, so
/// this folds them in for both branches below.
fn emit_final<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emitter: &mut EmitterAccum,
    streaming_start: Instant,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    let (range, _samples_done) = emitter.swap_and_fold(state);

    match request.stream.transfer_mode {
        TransferMode::FinalOnly => {
            let xyz_bytes = radiance::as_bytes(emitter.running_total());
            let header = FrameHeader::for_payload(
                request.request_id,
                request.first_sample,
                request.samples,
                xyz_bytes,
            );
            indicatrix_net::messages::write_stream_event(
                stream,
                &StreamEvent::Frame(header),
                Some(xyz_bytes),
            )?;
            *emission_count += 1;
        }
        TransferMode::LiveProgressive => {
            if let Some((first_sample, samples)) = range {
                let xyz_bytes = radiance::as_bytes(emitter.delta());
                let header =
                    FrameHeader::for_payload(request.request_id, first_sample, samples, xyz_bytes);
                indicatrix_net::messages::write_stream_event(
                    stream,
                    &StreamEvent::Frame(header),
                    Some(xyz_bytes),
                )?;
                *emission_count += 1;
            }
        }
    }

    let stats = Stats {
        samples_done: request.samples,
        requested_cadence_ms: request.stream.cadence_ms,
        effective_cadence_ms: effective_cadence_ms(streaming_start.elapsed(), *emission_count),
    };
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Done(Done {
            request_id: request.request_id,
            cancelled: false,
            stats,
        }),
        None,
    )?;
    Ok(())
}

fn write_preview<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    cfg: PreviewConfig,
    running_total: &[Vec3],
    samples_done: u32,
) -> Result<(), NetError> {
    let preview_buffer = downsample_preview(
        running_total,
        request.scene.width,
        request.scene.height,
        cfg.width,
        cfg.height,
    );
    // `as_bytes`, not `encode`: `preview_buffer` above is already a fresh allocation
    // (downsampling can't avoid one), so this at least skips a SECOND full copy of it
    // purely to hand the bytes to the writer.
    let xyz_bytes = radiance::as_bytes(&preview_buffer);
    let header = PreviewHeader::for_payload(
        request.request_id,
        cfg.width,
        cfg.height,
        samples_done,
        xyz_bytes,
    );
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Preview(header),
        Some(xyz_bytes),
    )
}
