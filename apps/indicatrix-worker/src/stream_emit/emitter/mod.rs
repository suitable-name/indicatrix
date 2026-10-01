//! The cadence emitter: [`run_stream`] owns the socket for the life of one
//! `RenderRequest`, interleaving cadence-paced `FRAME`/`PREVIEW`/`PROGRESS` writes (see
//! [`emit`]) with short-timeout polls (see [`polling`]) for an incoming `CANCEL` or
//! pipelined `RenderRequest`. [`accum`] holds the delta-coalescing buffer and running
//! total the emitter folds sub-batches into, and [`display`] a `DisplayOnly` request's
//! off-thread denoiser -- see `crate::serve`'s module docs for the full architecture.
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

mod accum;
mod display;
mod emit;
mod picture;
mod polling;

use super::{
    Output, ProducerSink, StreamOutcome, StreamSpec, TimeoutCache, TimeoutRead, TimeoutWrite,
    tracer::SharedState,
};
use crate::cli::ComputeMode;
use emit::{emit_final, emit_progress_heartbeat, emit_tick_or_heartbeat};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::messages::{
    Done, ErrorMsg, NetError, PayloadEncoding, RenderRequest, Stats, StreamEvent, error_codes,
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

// Re-exported (at their original `pub(super)`-equivalent scope, `crate::stream_emit`)
// so `crate::stream_emit::emitter::{PendingDelta, EmitterAccum, ClientPoll,
// poll_for_client_message, effective_cadence_ms, emit_tick}` keep resolving for
// `stream_emit`'s own tests, exactly as when this was one flat `emitter.rs` file.
pub(in crate::stream_emit) use accum::{EmitterAccum, PendingDelta};
pub(in crate::stream_emit) use emit::effective_cadence_ms;
#[cfg(test)]
pub(in crate::stream_emit) use emit::emit_tick;
pub(in crate::stream_emit) use polling::{ClientPoll, poll_for_client_message};
pub use polling::{RawPoll, poll_raw_client_message};

/// How often the emitter re-checks the socket for a pending `CANCEL` and re-evaluates
/// whether the cadence has elapsed, when the tracer hasn't signalled fresh progress.
/// Independent of `TARGET_SUBBATCH` -- a poll interval, not a batch size.
const EMITTER_POLL: Duration = Duration::from_millis(20);

/// Caps how long any single `write()` inside this request's streaming phase may block --
/// see [`TimeoutWrite`] for the stall/stuck-cancellation bug this closes.
///
/// Generous relative to [`EMITTER_POLL`]/[`super::MIN_CADENCE_FLOOR_MS`], which bound
/// how often this worker wants to check in; this bounds the worst case for a genuinely
/// quiet peer, with slack for a merely slow (not stuck) network -- a `FRAME` payload is
/// the full scene resolution regardless of cadence, so a healthy connection over a
/// constrained link can legitimately take real time to drain one.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawns `producer` on its own thread against a freshly built [`SharedState`], per
/// [`run_stream`]'s doc comment on why the tracer (or any producer) gets its own thread
/// and shared state rather than running inline.
fn spawn_producer(
    request: &RenderRequest,
    producer: impl FnOnce(ProducerSink) + Send + 'static,
) -> (
    Arc<Mutex<SharedState>>,
    Arc<AtomicBool>,
    mpsc::Receiver<()>,
    std::thread::JoinHandle<()>,
) {
    let pixel_count = request.scene.width as usize * request.scene.height as usize;
    let state = Arc::new(Mutex::new(SharedState::new(pixel_count)));
    let cancel = Arc::new(AtomicBool::new(false));
    let (progress_tx, progress_rx) = mpsc::channel::<()>();
    let sink = ProducerSink::new(
        Arc::clone(&state),
        Arc::clone(&cancel),
        progress_tx,
        request.first_sample,
        pixel_count,
    );
    // A real `std::thread::spawn` (needs `'static`), not a scoped thread: the producer
    // owns clones of everything it touches (the renderer is acquired once at
    // `serve::run` startup and shared by every connection).
    let handle = std::thread::spawn(move || producer(sink));
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
            // A cancelled request never reaches the contribution wait/reclaim -- see
            // `run_stream_loop`'s doc comment.
            reclaimed_samples: 0,
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

/// Answers a mid-stream `PING` (v14) with its `PONG` right away, passing every poll
/// result through unchanged so [`run_stream_loop`] needs no extra arm for it.
fn answer_ping<S: Write>(stream: &mut S, polled: ClientPoll) -> Result<ClientPoll, NetError> {
    if let ClientPoll::Ping(nonce) = polled {
        indicatrix_net::messages::write_stream_event(stream, &StreamEvent::Pong { nonce }, None)?;
    }
    Ok(polled)
}

/// Routes one `CONTRIBUTION` (v16, its header already read): into `spec.contribution`
/// when it's this request's own reserved slot, else discarded (a stale/mismatched
/// `request_id`, or a request with no reserved share at all) -- either way the payload
/// frame that follows is always consumed, keeping the stream in sync.
///
/// # Errors
///
/// [`NetError`] only for a transport-level failure -- the connection is out of sync and
/// [`run_stream_loop`] ends the loop on it, exactly like any other read error.
fn route_contribution<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    spec: &StreamSpec<'_>,
    request: &RenderRequest,
    header: &indicatrix_net::messages::ContributionHeader,
    timeouts: &mut TimeoutCache,
) -> Result<(), NetError> {
    match spec.contribution {
        Some(slot) if header.request_id == request.request_id => {
            slot.receive(stream, header, request.request_id, timeouts)
        }
        _ => indicatrix_net::messages::discard_contribution_payload(stream, header),
    }
}

/// The mutable loop-control state [`poll_once`] updates for [`run_stream_loop`] --
/// bundled so passing them through stays under clippy's argument-count limit.
struct LoopFlags<'a> {
    cancelled: &'a mut bool,
    peer_closed: &'a mut bool,
    pipelined_next: &'a mut Option<RenderRequest>,
}

/// One [`run_stream_loop`] iteration's poll-and-dispatch step: reads whatever's pending
/// on `stream` (`poll_for_client_message`), answers a `PING` inline, and applies the
/// result to `flags`/`cancel`. Returns `Some(outcome)` when the loop must break right
/// away (a transport-level failure only -- cancel/close/pipelining instead set `flags`
/// for the caller's own end-of-iteration checks).
fn poll_once<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    spec: &StreamSpec<'_>,
    request: &RenderRequest,
    cancel: &Arc<AtomicBool>,
    timeouts: &mut TimeoutCache,
    flags: &mut LoopFlags<'_>,
) -> Option<Result<StreamOutcome, NetError>> {
    match poll_for_client_message(stream, request.request_id, EMITTER_POLL, timeouts)
        .and_then(|polled| answer_ping(stream, polled))
    {
        Ok(ClientPoll::Cancelled) => {
            cancel.store(true, Ordering::Relaxed);
            *flags.cancelled = true;
        }
        Ok(ClientPoll::NextRequest(next)) => {
            // Implicit cancel-then-queue: discard the unsent buffer like an explicit
            // CANCEL, and remember `next` for the caller.
            cancel.store(true, Ordering::Relaxed);
            *flags.cancelled = true;
            *flags.pipelined_next = Some(*next);
        }
        Ok(ClientPoll::Closed) => {
            cancel.store(true, Ordering::Relaxed);
            *flags.peer_closed = true;
        }
        // v16: routed into this request's reserved slot, or discarded -- see
        // `route_contribution`.
        Ok(ClientPoll::Contribution(header)) => {
            if let Err(e) = route_contribution(stream, spec, request, &header, timeouts) {
                cancel.store(true, Ordering::Relaxed);
                return Some(Err(e));
            }
        }
        // A PING was already answered by `answer_ping` above.
        Ok(ClientPoll::Pending | ClientPoll::Stale | ClientPoll::Ping(_)) => {}
        Err(e) => {
            cancel.store(true, Ordering::Relaxed);
            return Some(Err(e));
        }
    }
    None
}

/// The bundle [`run_stream_loop`] hands back to [`run_stream`] once its cadence-paced
/// main loop ends -- the loop's own `Result`, plus the flags/values `run_stream`
/// needs afterward to decide between a normal finish, a peer-closed early return, and
/// a cancelled `DONE` write.
struct StreamLoopOutcome {
    result: Result<StreamOutcome, NetError>,
    peer_closed: bool,
    cancelled: bool,
    pipelined_next: Option<RenderRequest>,
    emission_count: u32,
    /// The stall watchdog failed the request: the producer thread may still be wedged, so
    /// [`run_stream_with`] must not join it (the `ERROR` would never be written).
    stalled: bool,
}

/// Watches the producer's `samples_done` against [`StreamSpec::stall_timeout`].
///
/// The emitter's own `PROGRESS` heartbeat says nothing about the producer, so without
/// this a wedged tracer keeps every client's silence-based liveness deadline satisfied
/// for as long as the connection lives.
struct StallWatch {
    timeout: Option<Duration>,
    samples_done: u32,
    since: Instant,
}

impl StallWatch {
    /// A watch that starts its window now; `None` never reports a stall.
    fn new(timeout: Option<Duration>) -> Self {
        Self {
            timeout,
            samples_done: 0,
            since: Instant::now(),
        }
    }

    /// Records the latest `samples_done` (any change restarts the window) and returns
    /// the window once the producer has gone longer than it without a new sample.
    fn observe(&mut self, samples_done: u32) -> Option<Duration> {
        if samples_done != self.samples_done {
            self.samples_done = samples_done;
            self.since = Instant::now();
        }
        self.timeout.filter(|window| self.since.elapsed() > *window)
    }
}

/// The heartbeat pacing state [`wait_for_tracer_until`] shares with its caller -- bundled
/// so the wait stays under clippy's argument-count limit.
struct HeartbeatPace<'a> {
    /// The longest gap between two heartbeats while waiting.
    bound: Duration,
    last_emit: &'a mut Instant,
    emission_count: &'a mut u32,
}

/// Fails a stalled request: raises `cancel`, gives the producer a short, bounded
/// (`window / 4`) chance to stop while still heartbeating, and builds the
/// `PRODUCER_STALLED` error the caller returns as [`StreamOutcome::Failed`].
///
/// Deliberately does NOT mark the request cancelled, so no `DONE { cancelled: true }` is
/// written: the request did not end because the client asked, it ended because the
/// server gave up on it.
fn fail_stalled<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    cancel: &AtomicBool,
    progress_rx: &mpsc::Receiver<()>,
    window: Duration,
    pace: &mut HeartbeatPace<'_>,
) -> ErrorMsg {
    let samples_done = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .samples_done;
    tracing::warn!(
        request_id = request.request_id,
        samples_done,
        ?window,
        "render lane made no progress; failing the request"
    );
    cancel.store(true, Ordering::Relaxed);
    let give_up = Instant::now() + window / 4;
    wait_for_tracer_until(stream, request, state, progress_rx, pace, Some(give_up));
    ErrorMsg {
        code: error_codes::PRODUCER_STALLED,
        message: format!(
            "the render lane made no progress for {window:.1?} (after {samples_done} samples); \
             the request was abandoned"
        ),
        request_id: Some(request.request_id),
    }
}

/// The outcome once the producer has `finished`: a trace panic, its own failure (stamped
/// with this request's id), or the final payload.
fn finished_outcome<S: Write>(
    stream: &mut S,
    spec: &StreamSpec<'_>,
    state: &Arc<Mutex<SharedState>>,
    emitter_accum: &mut EmitterAccum,
    streaming_start: Instant,
    emission_count: &mut u32,
) -> Result<StreamOutcome, NetError> {
    let (panicked, failed) = {
        let mut guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (guard.panicked, guard.failed.take())
    };
    if panicked {
        return Ok(StreamOutcome::TracePanicked);
    }
    if let Some(error) = failed {
        // Whatever set `guard.failed` (the tracer thread, via
        // `SharedState::failed`) may not have known this request's own id --
        // stamp it here, at the one place every such error passes through on
        // its way to the wire, so a late `ERROR` for a request the client has
        // already moved on from is epoch-gated exactly like a stale `FRAME`
        // (see `crate::client::accumulate::Accumulator::apply`) instead of
        // failing whatever request happens to be current.
        return Ok(StreamOutcome::Failed(ErrorMsg {
            request_id: Some(spec.request.request_id),
            ..error
        }));
    }
    emit_final(
        stream,
        spec,
        state,
        emitter_accum,
        streaming_start,
        emission_count,
    )
}

/// `run_stream`'s own cadence-paced main loop: polls for a client message, then either
/// winds down (cancel/close), emits a cadence tick or heartbeat, or emits the final
/// payload once the tracer finishes. Split out purely to keep `run_stream` under
/// clippy's function-length limit -- see that function's own doc comment for the full
/// cancellation/heartbeat/pipelining rationale this loop implements.
fn run_stream_loop<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    spec: &StreamSpec<'_>,
    state: &Arc<Mutex<SharedState>>,
    cancel: &Arc<AtomicBool>,
    progress_rx: &mpsc::Receiver<()>,
    emitter_accum: &mut EmitterAccum,
    streaming_start: Instant,
) -> StreamLoopOutcome {
    let request = spec.request;
    let cadence = Duration::from_millis(u64::from(request.stream.cadence_ms));
    let mut last_emit = Instant::now()
        .checked_sub(cadence)
        .unwrap_or_else(Instant::now);
    let mut emission_count: u32 = 0;
    // A connection-closed/transport-error poll, a CANCEL, or a pipelined RenderRequest
    // (queued, not rejected -- see `run_stream`'s doc comment) all set `cancel` and stop
    // the loop; `tracer_handle` is joined once, by the caller, after this returns.
    let mut cancelled = false;
    let mut peer_closed = false;
    let mut pipelined_next: Option<RenderRequest> = None;
    // Set only by the stall watchdog below: the producer is presumed wedged.
    let mut stalled = false;
    let mut stall_watch = StallWatch::new(spec.stall_timeout);
    // Fresh per request, never reused -- see `poll_for_client_message`'s doc comment.
    let mut timeouts = TimeoutCache::new();

    let result = loop {
        let _ = progress_rx.recv_timeout(EMITTER_POLL);

        let mut flags = LoopFlags {
            cancelled: &mut cancelled,
            peer_closed: &mut peer_closed,
            pipelined_next: &mut pipelined_next,
        };
        if let Some(outcome) = poll_once(stream, spec, request, cancel, &mut timeouts, &mut flags) {
            break outcome;
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

        let (finished, samples_done) = {
            let guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (guard.finished, guard.samples_done)
        };

        // The producer stopped adding samples: fail the request with an ERROR instead of
        // heartbeating a wedged lane forever (see `fail_stalled`).
        if !finished && let Some(window) = stall_watch.observe(samples_done) {
            stalled = true;
            let mut pace = HeartbeatPace {
                bound: cadence.min(super::HEARTBEAT_INTERVAL),
                last_emit: &mut last_emit,
                emission_count: &mut emission_count,
            };
            let error = fail_stalled(
                stream,
                request,
                state,
                cancel,
                progress_rx,
                window,
                &mut pace,
            );
            break Ok(StreamOutcome::Failed(error));
        }

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
            break finished_outcome(
                stream,
                spec,
                state,
                emitter_accum,
                streaming_start,
                &mut emission_count,
            );
        }
    };

    StreamLoopOutcome {
        result,
        peer_closed,
        cancelled,
        pipelined_next,
        emission_count,
        stalled,
    }
}

/// Streams one `RenderRequest`'s samples over `stream`, per `serve`'s module docs:
/// spawns a tracer thread (`super::tracer::run_tracer`) that never touches `stream`, then this
/// function -- the emitter -- owns `stream` for the rest of the request, interleaving
/// cadence-paced `FRAME`/`PREVIEW`/`PROGRESS` writes with short-timeout polls (via
/// [`poll_for_client_message`]) for an incoming `CANCEL` or pipelined `RenderRequest`.
/// The tracer is decoupled so a slow `write()` never stalls sample production.
///
/// On a normal finish, sends a final `FRAME` (the whole request for
/// `TransferMode::FinalOnly`; whatever's left un-coalesced for
/// `TransferMode::LiveProgressive`) followed by
/// `DONE { cancelled: false, .. }`. On cancellation (a `CANCEL`, the peer closing, or a
/// pipelined `RenderRequest`), discards whatever hasn't been sent and sends
/// `DONE { cancelled: true, .. }` with no further payload (via [`write_cancelled_done`]).
/// If the tracer panics, sends nothing further and returns
/// [`StreamOutcome::TracePanicked`]. Restores `stream`'s read timeout to blocking before
/// returning either way. Every [`StreamEvent`] written carries `request.request_id`, so
/// a stale reply is identifiable on the reading side.
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
/// cleared: clearing both timeouts right after `tracer_handle.join()` and only then
/// attempting this write would leave it able to block forever against a peer that
/// stopped reading. The read timeout is restored to blocking right after the join
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
/// `gpu` threads through to the tracer (`super::tracer::run_tracer`, via
/// [`run_stream_with`]'s producer thread), which prefers it over the CPU tracer per
/// adaptively-sized sub-batch; `compute_mode` threads through to the tracer job, one
/// connection-wide choice applied to every request.
///
/// `payload_encoding` is the connection's negotiated encoding (v14): every `FRAME` and
/// `PREVIEW` payload is encoded with it on this (emitter) thread, never under the
/// tracer's lock; a payload that would not shrink goes out `Raw`, as its header says.
///
/// A `PING` arriving mid-stream is answered with a `PONG` between this request's own
/// events.
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
    payload_encoding: PayloadEncoding,
) -> Result<(StreamOutcome, Option<RenderRequest>), NetError> {
    run_stream_with(
        stream,
        &StreamSpec {
            request,
            payload_encoding,
            output: Output::Radiance,
            // A plain `RENDER` never reserves a viewer contribution -- only a
            // coordinator's `FinalImageRequest` job does (see `coordinator::job::mod`).
            contribution: None,
            // A plain worker's tracer is its only producer, so no progress for the
            // whole window means it is wedged (see `StreamSpec::stall_timeout`).
            stall_timeout: Some(super::PRODUCER_STALL_TIMEOUT),
        },
        super::local_tracer(request, threads, gpu, compute_mode),
    )
}

/// [`run_stream`] with any producer in place of the local tracer: `producer` runs on its
/// own thread with a [`ProducerSink`] (add chunks, watch the cancel flag, finish) while
/// this thread emits exactly as [`run_stream`] documents -- cadence-paced events,
/// `PROGRESS` heartbeats at least every [`super::HEARTBEAT_INTERVAL`] however long the
/// producer takes (a coordinator job waiting for a worker included), cancellation and
/// pipelining. `spec.output` selects the final event ([`Output`]); a producer that
/// finishes with `ProducerOutcome::Failed` yields [`StreamOutcome::Failed`] (no `DONE`).
///
/// # Errors
///
/// As [`run_stream`].
pub fn run_stream_with<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    spec: &StreamSpec<'_>,
    producer: impl FnOnce(ProducerSink) + Send + 'static,
) -> Result<(StreamOutcome, Option<RenderRequest>), NetError> {
    let request = spec.request;
    let (state, cancel, progress_rx, tracer_handle) = spawn_producer(request, producer);
    let mut emitter_accum = EmitterAccum::with_encoding(
        request.scene.width as usize * request.scene.height as usize,
        spec.payload_encoding,
    );

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
        stalled,
    } = run_stream_loop(
        stream,
        spec,
        &state,
        &cancel,
        &progress_rx,
        &mut emitter_accum,
        streaming_start,
    );

    if stalled {
        // A wedged producer may never return, and joining it would keep the stall
        // `ERROR` from being written. Detach: it exits on its own once its dispatch
        // returns (`cancel` is raised), and every permit/turnstile guard it holds is
        // released by RAII then.
        drop(tracer_handle);
    } else {
        let _ = tracer_handle.join();
    }

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
pub(in crate::stream_emit) fn wait_for_tracer_to_stop<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    progress_rx: &mpsc::Receiver<()>,
    heartbeat_bound: Duration,
    last_emit: &mut Instant,
    emission_count: &mut u32,
) {
    let mut pace = HeartbeatPace {
        bound: heartbeat_bound,
        last_emit,
        emission_count,
    };
    wait_for_tracer_until(stream, request, state, progress_rx, &mut pace, None);
}

/// [`wait_for_tracer_to_stop`] with an optional deadline: with `give_up` set it stops
/// waiting (tracer or not) once that instant passes, so the stall watchdog can fail a
/// request whose tracer never notices `cancel` instead of waiting on it forever.
fn wait_for_tracer_until<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    progress_rx: &mpsc::Receiver<()>,
    pace: &mut HeartbeatPace<'_>,
    give_up: Option<Instant>,
) {
    while !state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .finished
    {
        if give_up.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        let _ = progress_rx.recv_timeout(EMITTER_POLL);
        if pace.last_emit.elapsed() >= pace.bound {
            if emit_progress_heartbeat(stream, request, state, pace.emission_count).is_err() {
                break;
            }
            *pace.last_emit = Instant::now();
        }
    }
}
