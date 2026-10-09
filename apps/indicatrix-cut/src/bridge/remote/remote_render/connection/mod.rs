//! The two connection lifecycles themselves: one-shot ([`one_shot::spawn_remote_render`])
//! and persistent ([`persistent::spawn_remote_connection`]/[`dispatch_render`]),
//! sharing [`handshake::connect_and_handshake`] and the timeout-tolerant frame/event
//! readers in [`stream_io`]. See this group's own `mod.rs` doc comment for why one
//! thread must own the stream for both directions, and for the two lifecycles' own
//! tradeoffs.
//!
//! The timeout constants and the pure, socket-free decisions built on them
//! ([`liveness_deadline`], [`check_liveness`], [`route_event`]) stay here rather than
//! moving into either lifecycle's own file: both lifecycles need the timeouts, and
//! keeping the three pure functions alongside them is what lets [`tests`] exercise
//! every one of them directly, with no live TCP/TLS connection.

mod asset_upload;
mod batch_client;
mod handshake;
mod one_shot;
mod payload_setting;
mod persistent;
mod stream_io;
#[cfg(test)]
mod tests;

pub use batch_client::{
    BatchClient, BatchEvent, BatchWatch, MAX_BATCH_REQUEST_BYTES, items_fitting,
};
pub use handshake::{connect_and_handshake, test_connection};
pub use one_shot::{spawn_final_image_request, spawn_remote_render};
pub use payload_setting::set_payload_choice;
pub use persistent::spawn_remote_connection;

use super::types::{CurrentRequest, RemoteError, RemoteUpdate};
use indicatrix_net::{client::ApplyOutcome, messages::StreamEvent};
use std::{
    sync::PoisonError,
    time::{Duration, Instant},
};
use stream_io::to_remote_update;

// `Arc`/`Mutex`/`thread` are unused by this module's own (non-test) code -- every
// function here that touches them (`spawn_remote_render`, `spawn_remote_connection`,
// `run_connection`, `dispatch_render`) lives in a sibling file. Imported here, under
// `cfg(test)`, purely so `tests`' `use super::*;` picks them up the same way it did
// before this module was split out of one file.
#[cfg(test)]
use std::{
    sync::{Arc, Mutex},
    thread,
};

/// How often [`one_shot::run`]/[`persistent`]'s loops check for a pending command
/// between read attempts. Purely a responsiveness/CPU trade-off, not a protocol
/// requirement.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// # Timeouts and liveness
///
/// Every blocking step has a deadline. Without one, a worker that accepted a
/// `RenderRequest` and then stopped responding (crashed mid-render, a NAT entry
/// expired, a laptop slept) would leave `persistent::run_connection`'s loop polling a
/// dead socket forever with `current` stuck `Some` -- the viewport frozen with no error
/// surfaced.
///
/// - [`CONNECT_TIMEOUT`] bounds `handshake::open_tcp`'s `TcpStream::connect_timeout`.
/// - [`HANDSHAKE_TIMEOUT`] bounds the TLS handshake and `HELLO`/`WELCOME` exchange in
///   [`handshake::connect_and_handshake`].
/// - [`WRITE_TIMEOUT`] bounds every write for the connection's life (a persistent
///   socket option, set once alongside [`HANDSHAKE_TIMEOUT`]) so a write can never
///   block forever into a peer that stopped reading. See
///   [`handshake::connect_and_handshake`]'s doc comment for the `ErrorKind::WriteZero`
///   caveat this creates.
/// - [`LIVENESS_TIMEOUT`] bounds how long a request may go with no event at all (not an
///   I/O error) once it has seen at least one event. Applied in [`one_shot::run`]'s loop
///   and, via [`check_liveness`], in [`persistent::run_connection`]'s.
/// - [`FIRST_EVENT_TIMEOUT`] is [`LIVENESS_TIMEOUT`]'s longer-grace counterpart for the
///   first wait after a request is (re)dispatched -- a worker's calibration/warm-up at
///   high resolution with a coarse cadence can legitimately take longer than every tick
///   after it. See [`liveness_deadline`] for which applies when.
/// - [`FRAME_REMAINDER_TIMEOUT`] bounds [`stream_io`]'s read of the REST of a frame
///   (length-prefix remainder plus payload) once its first byte has arrived -- without
///   it, a worker dying mid-frame would hang the reading thread on an unbounded
///   `read_exact` until TCP keepalive fires, well past every deadline above (which
///   never get a chance to run while that read is blocked).
///
/// [`KEEPALIVE_TIME`]/[`KEEPALIVE_INTERVAL`] are a complementary OS-level defense, not a
/// deadline of this module's own: TCP keepalive lets the kernel notice a network-level
/// dead peer well before [`LIVENESS_TIMEOUT`] would otherwise catch it from silence
/// alone.
///
/// # Why a busy consumer can never make either deadline fire early
///
/// A worker whose `Progress` heartbeat is tied to sample cadence rather than wall-clock
/// time can legitimately take longer than [`LIVENESS_TIMEOUT`] on its first tick at a
/// high resolution with a coarse cadence -- [`FIRST_EVENT_TIMEOUT`] is the fix for that
/// case specifically (confirmed against a real 4K/cadence-20 export).
///
/// A second failure mode was investigated and ruled out: a thread doing unrelated
/// CPU-heavy work (denoising, tone-mapping, encoding) between reads, so the deadline
/// elapses against a socket nobody was checking even though bytes were waiting. This
/// cannot happen here: both [`one_shot::run`]'s and [`persistent::run_connection`]'s
/// loops call [`stream_io::try_read_stream_event`] every iteration, and neither loop
/// body does anything else that isn't O(1) -- the `on_update`/`route_event` callback
/// either sends down an unbounded channel or, for a persistent connection's
/// `Frame`/`Preview` events, takes `gui::remote::orchestrator`'s shared `Orchestrator`
/// mutex SYNCHRONOUSLY (right here, before ever queuing anything) purely to
/// rate-limit/gate a redraw -- an O(1) check and mutation, held only that briefly --
/// and otherwise queues a closure via `Weak::upgrade_in_event_loop` and returns
/// immediately, so actual render-tail work (the tonemap/denoise pass) always happens
/// on other threads and is never done while that mutex is held -- the UI thread must
/// never hold this same mutex across the tonemap, or this O(1) claim would become
/// false for however long that redraw's tonemap took; see
/// `gui::remote::orchestrator::tick::update:: redraw_from_epoch`'s own doc
/// comment. The liveness check is therefore only ever reached immediately after a
/// real, just-attempted, empty read.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the TLS handshake and following `HELLO`/`WELCOME` exchange may take,
/// applied as a read timeout on the raw socket beforehand. Generous next to the
/// ~1-2ms medians measured on loopback (see [`handshake::connect_and_handshake`]'s doc
/// comment) to tolerate a slow WAN link, while still bounding a worker that accepts the
/// TCP connection but never completes the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a single write may block before failing. Set once in
/// [`handshake::connect_and_handshake`] alongside [`HANDSHAKE_TIMEOUT`]: a write
/// timeout is a persistent socket option, not one that needs reapplying per call.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// TCP keepalive: idle time before the OS sends the first probe. Paired with
/// [`KEEPALIVE_INTERVAL`] in `handshake::open_tcp`.
const KEEPALIVE_TIME: Duration = Duration::from_secs(30);
/// TCP keepalive: interval between probes after the first, until the peer answers or
/// the OS reports the socket dead.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// How long a request that has already produced at least one event may go without
/// another before this side gives up on it. `indicatrix-worker`'s `Progress` heartbeat
/// is time-based, guaranteed at least every `WORKER_HEARTBEAT_INTERVAL` (2 seconds)
/// regardless of sample cadence or transfer mode -- so steady-state silence longer than
/// that means the worker or network path has stopped. `8` seconds is a generous 4x that
/// interval: two missed heartbeats rounded up to absorb scheduling/network jitter, plus
/// slack for a [`WRITE_TIMEOUT`]-bounded write to fail and unwind worker-side first.
///
/// This is not a bound on a worker's cadence-based `FRAME`/`PREVIEW` emission -- a
/// coarse cadence can still go several heartbeat intervals between radiance deltas;
/// only the bare `Progress` heartbeat guarantee must never exceed
/// `WORKER_HEARTBEAT_INTERVAL`.
///
/// Threaded through [`one_shot::run`] and [`persistent::run_connection`] as a
/// parameter (defaulting to this constant) so [`check_liveness`]'s tests can drive it
/// with a much shorter value.
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(8);

/// How long the wait for the first event of a freshly (re)dispatched request may go
/// before giving up -- longer than [`LIVENESS_TIMEOUT`] on purpose. TLS/application
/// handshake overhead is already paid by the time this clock starts (at "request
/// sent", not "connection opened"), but the worker's own calibration/warm-up has not.
/// Confirmed as a real false positive: at 4K with a cadence of 20 samples/tick, the
/// worker's first tick legitimately ran past [`LIVENESS_TIMEOUT`] while still
/// computing. `30` seconds absorbs that without meaningfully changing the user-visible
/// latency before a genuinely dead worker is reported ([`HANDSHAKE_TIMEOUT`]/
/// [`CONNECT_TIMEOUT`] already bound everything before this clock starts).
const FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long [`stream_io`] waits for the REMAINDER of a frame (the rest of the
/// length prefix, plus the whole payload) once its first byte has arrived, instead of
/// blocking forever. Mirrors `apps/indicatrix-worker/src/stream_emit::FRAME_REMAINDER_TIMEOUT`
/// on the server side -- see that constant's own doc comment for the shared rationale
/// (5s is generous next to a real frame's transfer time, even over a slow link, while
/// still bounding a peer that sends a partial frame and goes silent).
///
/// Without this, a worker that died mid-frame (crashed after writing the length prefix
/// but before the payload) would hang this connection's reading thread on an unbounded
/// `read_exact` until TCP keepalive noticed (30s+, via [`KEEPALIVE_TIME`]/
/// [`KEEPALIVE_INTERVAL`]) -- far past [`LIVENESS_TIMEOUT`]/[`FIRST_EVENT_TIMEOUT`],
/// which never get a chance to fire because the thread isn't polling, it's blocked
/// inside this one read.
const FRAME_REMAINDER_TIMEOUT: Duration = Duration::from_secs(5);

/// Which deadline currently applies to an idle wait for the next event: the generous
/// [`FIRST_EVENT_TIMEOUT`] before `seen_first_event`, or the tighter, heartbeat-derived
/// `liveness_timeout` once at least one event has arrived.
///
/// `liveness_timeout` is threaded in as a parameter so this pure decision is
/// unit-testable with a short value; production callers always pass
/// [`LIVENESS_TIMEOUT`] itself.
const fn liveness_deadline(seen_first_event: bool, liveness_timeout: Duration) -> Duration {
    if seen_first_event {
        liveness_timeout
    } else {
        FIRST_EVENT_TIMEOUT
    }
}

/// How long a one-shot connection waits, after writing `CANCEL`, for the worker to end
/// the request (`DONE`/`ERROR`) or close the stream before giving up on it. Unlike
/// [`LIVENESS_TIMEOUT`] this is a wall-clock bound from the moment the cancel was sent:
/// a coordinator whose tracer is wedged keeps heartbeating `PROGRESS`, which resets the
/// silence clock forever, so only a deadline that heartbeats cannot move ends the wait.
const CANCEL_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether a `CANCEL` written at `cancel_sent_at` has gone unanswered for longer than
/// `timeout` as of `now`. `None` (no cancel sent) is never overdue. Pure, with both
/// instants and the timeout passed in, so it is unit-testable without a socket or sleeping.
fn cancel_ack_overdue(cancel_sent_at: Option<Instant>, now: Instant, timeout: Duration) -> bool {
    cancel_sent_at.is_some_and(|sent| now.saturating_duration_since(sent) > timeout)
}

/// Checks whether `current` (if any) has gone longer than `timeout` since `last_event`
/// and, if so, delivers [`RemoteUpdate::Failed`] to it and returns `true` -- telling
/// [`persistent::run_connection`]'s caller to drop `stream`/`welcome` too, so the next
/// dispatch reconnects rather than keep polling a worker that has stopped saying
/// anything. A no-op (`false`) when nothing is current or the deadline hasn't passed
/// yet.
///
/// Factored out of `run_connection`'s `Ok(None)` arm so this decision -- pure,
/// independent of any actual socket -- is unit-testable without a live TCP/TLS
/// connection ([`super::types::RemoteStream`] is a concrete `rustls::StreamOwned`, with
/// no fake-stream seam the way `bridge::library::client` has). The tests below pin
/// exactly this: a request current longer than `timeout` with no event gets failed and
/// cleared; one whose clock keeps resetting (as `run_connection` does on every real
/// event) never trips.
fn check_liveness(
    current: &mut Option<CurrentRequest>,
    last_event: Instant,
    timeout: Duration,
) -> bool {
    if current.is_none() {
        return false;
    }
    let elapsed = last_event.elapsed();
    if elapsed <= timeout {
        return false;
    }
    let mut cur = current.take().expect("just checked Some above");
    (cur.on_update)(RemoteUpdate::Failed {
        request_id: cur.request_id,
        message: RemoteError::WorkerSilent(elapsed).to_string(),
    });
    true
}

/// Applies one [`StreamEvent`] to whatever request is currently in flight on a
/// persistent connection, mirroring [`one_shot::run`]'s inner-loop body (apply via
/// `Accumulator::apply`, translate via [`stream_io::to_remote_update`], clear on a
/// terminal outcome) -- except here `current` may be `None`, in which case the event is
/// simply dropped (a reply for a request this thread already gave up on).
///
/// `event`'s `request_id` is never compared against `current.request_id` directly here
/// -- `Accumulator::apply`'s own epoch check already makes that comparison. A stale
/// reply for a request this connection superseded (via [`persistent::dispatch_render`]'s
/// supersede step) is therefore dropped correctly even if the best-effort `CANCEL` for
/// it never reached the worker in time -- this is the actual mechanism that keeps a
/// late `FRAME` from settle N-1 out of settle N's accumulator, not the `CANCEL` itself.
fn route_event(current: &mut Option<CurrentRequest>, event: &StreamEvent, payload: Option<&[u8]>) {
    let Some(cur) = current.as_mut() else {
        return;
    };

    let outcome = {
        let mut acc = cur
            .accumulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        acc.apply(event, payload)
    };
    let request_id = cur.request_id;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            (cur.on_update)(RemoteUpdate::Failed {
                request_id,
                message: RemoteError::Client(e.into()).to_string(),
            });
            *current = None;
            return;
        }
    };

    let is_terminal = matches!(
        outcome,
        ApplyOutcome::Done { .. } | ApplyOutcome::WorkerError
    );
    if let Some(update) = to_remote_update(request_id, event, outcome) {
        (cur.on_update)(update);
    }
    if is_terminal {
        *current = None;
    }
}
