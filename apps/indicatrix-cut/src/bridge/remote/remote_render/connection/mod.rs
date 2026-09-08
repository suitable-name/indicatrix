//! The two connection lifecycles themselves: one-shot ([`spawn_remote_render`]/[`run`])
//! and persistent ([`spawn_remote_connection`]/[`run_connection`]/[`dispatch_render`]),
//! sharing [`connect_and_handshake`] and the timeout-tolerant frame/event readers. See
//! this group's own `mod.rs` doc comment for why one thread must own the stream for
//! both directions, and for the two lifecycles' own tradeoffs.

use super::types::{
    ConnectionCommand, CurrentRequest, HandleKind, RemoteCommand, RemoteConnectionHandle,
    RemoteError, RemoteRenderHandle, RemoteRenderRequest, RemoteStream, RemoteUpdate,
    RenderCommand, StreamEventFrame, host_from_address,
};
use crate::settings::WorkerSettings;
use indicatrix_net::{
    client::{Accumulator, ApplyOutcome},
    framing::{self, LEN_PREFIX_BYTES, MAX_FRAME_LEN},
    messages::{NetError, RenderRequest, StreamEvent},
};
use socket2::{SockRef, TcpKeepalive};
use std::{
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

/// How often [`run`]'s loop checks for a pending [`RemoteCommand`] between read
/// attempts. Purely a responsiveness/CPU trade-off, not a protocol requirement.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// # Timeouts and liveness
///
/// Every blocking step has a deadline. Without one, a worker that accepted a
/// `RenderRequest` and then stopped responding (crashed mid-render, a NAT entry
/// expired, a laptop slept) would leave [`run_connection`]'s loop polling a dead socket
/// forever with `current` stuck `Some` -- the viewport frozen with no error surfaced.
///
/// - [`CONNECT_TIMEOUT`] bounds [`open_tcp`]'s `TcpStream::connect_timeout`.
/// - [`HANDSHAKE_TIMEOUT`] bounds the TLS handshake and `HELLO`/`WELCOME` exchange in
///   [`connect_and_handshake`].
/// - [`WRITE_TIMEOUT`] bounds every write for the connection's life (a persistent
///   socket option, set once alongside [`HANDSHAKE_TIMEOUT`]) so a write can never
///   block forever into a peer that stopped reading. See [`connect_and_handshake`]'s
///   doc comment for the `ErrorKind::WriteZero` caveat this creates.
/// - [`LIVENESS_TIMEOUT`] bounds how long a request may go with no event at all (not an
///   I/O error) once it has seen at least one event. Applied in [`run`]'s loop and, via
///   [`check_liveness`], in [`run_connection`]'s.
/// - [`FIRST_EVENT_TIMEOUT`] is [`LIVENESS_TIMEOUT`]'s longer-grace counterpart for the
///   first wait after a request is (re)dispatched -- a worker's calibration/warm-up at
///   high resolution with a coarse cadence can legitimately take longer than every tick
///   after it. See [`liveness_deadline`] for which applies when.
/// - [`FRAME_REMAINDER_TIMEOUT`] bounds [`try_read_one_frame`]'s read of the REST of a
///   frame (length-prefix remainder plus payload) once its first byte has arrived --
///   without it, a worker dying mid-frame would hang the reading thread on an unbounded
///   `read_exact` until TCP keepalive fires, well past every deadline above (which never
///   get a chance to run while that read is blocked).
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
/// cannot happen here: both [`run`]'s and [`run_connection`]'s loops call
/// [`try_read_stream_event`] every iteration, and neither loop body does anything else
/// that isn't O(1) -- the `on_update`/`route_event` callback either sends down an
/// unbounded channel or queues a closure via `Weak::upgrade_in_event_loop` and returns
/// immediately, so actual render-tail work always happens on other threads. The
/// liveness check is therefore only ever reached immediately after a real, just-
/// attempted, empty read.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the TLS handshake and following `HELLO`/`WELCOME` exchange may take,
/// applied as a read timeout on the raw socket beforehand. Generous next to the
/// ~1-2ms medians measured on loopback (see [`connect_and_handshake`]'s doc comment)
/// to tolerate a slow WAN link, while still bounding a worker that accepts the TCP
/// connection but never completes the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a single write may block before failing. Set once in
/// [`connect_and_handshake`] alongside [`HANDSHAKE_TIMEOUT`]: a write timeout is a
/// persistent socket option, not one that needs reapplying per call.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// TCP keepalive: idle time before the OS sends the first probe. Paired with
/// [`KEEPALIVE_INTERVAL`] in [`open_tcp`].
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
/// Threaded through [`run`] and [`run_connection`] as a parameter (defaulting to this
/// constant) so [`check_liveness`]'s tests can drive it with a much shorter value.
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

/// How long [`try_read_one_frame`] waits for the REMAINDER of a frame (the rest of the
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

/// Connects to `request.worker`, performs the mutual-TLS handshake and
/// `HELLO`/`WELCOME`, then sends and streams one `RenderRequest` covering
/// `[request.first_sample, request.first_sample + request.samples)` of
/// `request.scene` at the session's `request.width x request.height` resolution.
///
/// `accumulator` is shared with the caller: [`Accumulator::begin_request`] is called
/// here (synchronously, right before the request is sent -- see
/// `indicatrix_net::client::session`'s module docs on why that ordering matters) and every
/// reply is applied into it as it arrives, under a short-held lock each time so the
/// caller can read a consistent snapshot from another thread at any point.
pub fn spawn_remote_render(
    request: RemoteRenderRequest,
    accumulator: Arc<Mutex<Accumulator>>,
    mut on_update: impl FnMut(super::types::RemoteUpdate) + Send + 'static,
) -> RemoteRenderHandle {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let request_id = request.request_id;
        let result = run(&request, &accumulator, &rx, &mut on_update);
        if let Err(e) = result {
            on_update(super::types::RemoteUpdate::Failed {
                request_id,
                message: e.to_string(),
            });
        }
    });

    RemoteRenderHandle(HandleKind::OneShot(tx))
}

/// Spawns a background thread that owns one persistent mutual-TLS connection to
/// `worker`, reused across every [`RemoteConnectionHandle::render`] call against the
/// returned handle -- see the module doc comment's "Two connection lifecycles" section.
///
/// Connects lazily: no socket is opened, and no handshake cost paid, until the first
/// [`RemoteConnectionHandle::render`] call actually needs one -- a worker configured but
/// never used in a session costs nothing beyond one idle channel and one idle thread.
#[must_use]
pub fn spawn_remote_connection(worker: WorkerSettings) -> RemoteConnectionHandle {
    let (tx, rx) = mpsc::channel();
    let worker_for_thread = worker.clone();
    thread::spawn(move || run_connection(&worker_for_thread, &rx, LIVENESS_TIMEOUT));
    RemoteConnectionHandle {
        worker,
        commands: tx,
    }
}

/// # Known cost, measured, and what was done about it
///
/// This function's full cost (three file reads, building a `rustls::ClientConfig`, TCP
/// connect, TLS 1.3 handshake, `HELLO`/`WELCOME`) measured end-to-end on loopback
/// against a real worker: ~2.5ms median, ~2.6ms mean, dominated by the TCP connect (1
/// RTT) and TLS handshake (1 more RTT) -- so on a link with `R` ms of RTT, expect
/// roughly this total plus `~2R` ms.
///
/// On loopback/LAN this is noise next to the render itself and the 600ms
/// `SETTLE_DEBOUNCE` gating every dispatch. But higher-latency links (a remote worker
/// over a slower connection) are a real deployment target, and on one of those `~2R` ms
/// per settle is a user-visible stall before the first `FRAME`, repeated on every
/// drag-then-pause. That is why [`spawn_remote_connection`] exists: it lets
/// `gui::remote::orchestrator` hold one connection open across a whole live session and
/// pay this cost once. [`spawn_remote_render`] (this function's other caller, along
/// with [`test_connection`]) stays one connection per call, for callers where that
/// repetition doesn't apply.
///
/// Resolves `address` (a `host:port` string) and connects with a bounded deadline
/// ([`CONNECT_TIMEOUT`]), then configures the socket for a long-lived,
/// latency-sensitive, bidirectional protocol connection:
///
/// - `TCP_NODELAY` -- a `RenderRequest`/`CANCEL`/stream-event frame must never sit in
///   Nagle's buffer waiting for more data that isn't coming; this protocol's messages are
///   sent one at a time; not batched.
/// - OS-level TCP keepalive ([`KEEPALIVE_TIME`]/[`KEEPALIVE_INTERVAL`], via the
///   `socket2` crate -- `std::net::TcpStream` has no keepalive-tuning API of its own) so
///   a network-level dead peer is noticed by the kernel even while nothing above this
///   layer is trying to read or write.
///
/// # Errors
///
/// Any I/O error resolving `address` (including "no address found" for a string that
/// doesn't resolve to anything), connecting within [`CONNECT_TIMEOUT`], or configuring
/// the socket.
fn open_tcp(address: &str) -> std::io::Result<TcpStream> {
    let addr = address.to_socket_addrs()?.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("no address found for {address:?}"),
        )
    })?;
    let tcp = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    tcp.set_nodelay(true)?;
    let keepalive = TcpKeepalive::new()
        .with_time(KEEPALIVE_TIME)
        .with_interval(KEEPALIVE_INTERVAL);
    SockRef::from(&tcp).set_tcp_keepalive(&keepalive)?;
    Ok(tcp)
}

/// Connects to `worker.address` over mutual TLS (certificates from `worker.cert_dir`)
/// and performs the `HELLO`/`WELCOME` handshake, including
/// [`indicatrix_net::handshake::verify_compatible`]'s client-side defense-in-depth
/// check. Shared by [`run`] (a full render), [`test_connection`] (handshake only), and,
/// `pub(crate)`, by `bridge::library::client` (the read-only design-library protocol
/// rides the same authenticated connection) -- so every caller connects identically.
///
/// The returned stream carries [`WRITE_TIMEOUT`] for the rest of its life and, past
/// this function's [`HANDSHAKE_TIMEOUT`]-bounded read, no read timeout at all --
/// callers set their own afterward (`run`'s/`run_connection`'s poll-timeout reads via
/// [`try_read_one_frame`], or `bridge::library::client`'s longer, static one).
///
/// # `ErrorKind::WriteZero` is a real I/O failure here, not a partial-write retry signal
///
/// `rustls::Stream::write`'s `complete_io` can fail (a write timing out against
/// [`WRITE_TIMEOUT`]), but its public signature surfaces that as plain `WriteZero`
/// rather than the underlying `TimedOut`. Every write-error site in this module (and
/// `bridge::library::client`) already treats any error as fatal without matching a
/// specific `ErrorKind`, so this is handled correctly -- noted here so a future reader
/// chasing a `WriteZero` in a log doesn't go looking for an actual zero-length write bug.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn connect_and_handshake(
    worker: &WorkerSettings,
) -> Result<(RemoteStream, indicatrix_net::messages::Welcome), RemoteError> {
    let ca = indicatrix_net::tls::load_ca(&worker.ca_path())?;
    let cert_chain = indicatrix_net::tls::load_certs(&worker.client_cert_path())?;
    let key = indicatrix_net::tls::load_private_key(&worker.client_key_path())?;
    let config = indicatrix_net::tls::client_config(ca, cert_chain, key)?;

    let tcp = open_tcp(&worker.address)?;
    let host = host_from_address(&worker.address);
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| RemoteError::InvalidServerName(host.to_string()))?;
    let conn = rustls::ClientConnection::new(config, server_name)
        .map_err(indicatrix_net::tls::TlsError::Rustls)?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    // Bounds the TLS handshake below AND the HELLO/WELCOME read inside `handshake` --
    // both run before this function returns, and neither had any deadline before.
    stream
        .sock
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(RemoteError::Io)?;
    // Persists on the socket for the connection's whole remaining life -- see this
    // function's own doc comment's `WriteZero` section for what a timed-out write looks
    // like to a caller.
    stream
        .sock
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(RemoteError::Io)?;
    // Force the handshake to complete now, mirroring
    // `apps/indicatrix-worker/src/serve.rs::accept_tls` on the server side, so a TLS
    // failure (wrong CA, expired cert, clock skew, SAN mismatch) is diagnosed here
    // rather than surfacing later as an opaque I/O error out of the handshake below.
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(RemoteError::Io)?;

    let welcome = indicatrix_net::client::handshake::handshake(&mut stream)?;
    // Clear the handshake-scoped read timeout now that it's served its purpose --
    // every read after this point sets its own (see this function's own doc comment),
    // so leaving `HANDSHAKE_TIMEOUT` in place would just be a stale, misleading value
    // sitting on the socket between here and the first such call.
    stream
        .sock
        .set_read_timeout(None)
        .map_err(RemoteError::Io)?;
    Ok((stream, welcome))
}

/// The settings UI's "Test connection" operation: connect, handshake, report worker
/// identity/backend/build compatibility, then disconnect (simply by dropping the
/// stream when this returns) -- no render. Blocking; callers run this on their own
/// worker thread (see `gui::remote::setup_worker_callbacks`) and report the result back
/// to the UI thread themselves.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn test_connection(
    worker: &WorkerSettings,
) -> Result<indicatrix_net::client::ConnectionInfo, RemoteError> {
    let (_stream, welcome) = connect_and_handshake(worker)?;
    Ok(welcome.into())
}

fn run(
    request: &RemoteRenderRequest,
    accumulator: &Arc<Mutex<Accumulator>>,
    commands: &mpsc::Receiver<RemoteCommand>,
    on_update: &mut dyn FnMut(super::types::RemoteUpdate),
) -> Result<(), RemoteError> {
    run_with_liveness_timeout(request, accumulator, commands, on_update, LIVENESS_TIMEOUT)
}

/// [`run`]'s actual body, with the liveness deadline threaded in as a parameter -- see
/// [`LIVENESS_TIMEOUT`]'s own doc comment for why. [`run`] itself is just this with the
/// real constant plugged in; kept as the separate public entry point so every existing
/// caller ([`spawn_remote_render`]) is unaffected.
fn run_with_liveness_timeout(
    request: &RemoteRenderRequest,
    accumulator: &Arc<Mutex<Accumulator>>,
    commands: &mpsc::Receiver<RemoteCommand>,
    on_update: &mut dyn FnMut(super::types::RemoteUpdate),
    liveness_timeout: Duration,
) -> Result<(), RemoteError> {
    let request_id = request.request_id;
    let (mut stream, welcome) = connect_and_handshake(&request.worker)?;
    on_update(RemoteUpdate::Connected {
        request_id,
        info: welcome.clone().into(),
    });

    // Ask before sending: the handshake advertises render capacity precisely so a client
    // never discovers its absence by having a `RenderRequest` rejected downstream.
    let Some(capability) = welcome.render.as_ref() else {
        return Err(RemoteError::NoRenderCapacity);
    };

    let render_request = RenderRequest {
        request_id,
        scene: request.scene.clone(),
        first_sample: request.first_sample,
        samples: request.samples,
        // `export_stream_config`, not `stream_config`: this function is reached only via
        // `spawn_remote_render`, whose only caller is
        // `bridge::export_thread::remote::run_remote_batch` (the live viewport uses
        // `spawn_remote_connection`/`run_connection`/`dispatch_render` instead, with the
        // live `stream_config` below unchanged) -- so the export-specific choices (no
        // preview stream, a larger cadence floor) apply unconditionally here.
        stream: request
            .worker
            .export_stream_config(capability.min_cadence_ms),
    };

    {
        let mut acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
        acc.begin_request(request_id);
    }
    indicatrix_net::client::send_render_request(&mut stream, &render_request)?;

    // Reset right after the request is actually on the wire -- see `LIVENESS_TIMEOUT`'s
    // own doc comment for why the countdown starts from the most recent sign of life,
    // and sending the request counts as one (a fresh TCP+TLS round trip and a write just
    // succeeded), not from whenever this function happened to be called.
    let mut last_event = Instant::now();
    // Whether ANY stream event has been observed yet for this request -- selects
    // `FIRST_EVENT_TIMEOUT` (before) or `liveness_timeout` (after) via
    // `liveness_deadline`. See `FIRST_EVENT_TIMEOUT`'s own doc comment for why the wait
    // for the very first tick needs more slack than every wait after it.
    let mut seen_first_event = false;
    loop {
        match commands.try_recv() {
            Ok(RemoteCommand::Cancel) => {
                indicatrix_net::client::send_cancel(&mut stream, request_id)?;
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                // `Disconnected` means the handle was dropped -- nobody can ever
                // cancel or observe this render again; nothing further to do but keep
                // draining until DONE so the connection ends cleanly rather than being
                // torn down mid-message.
            }
        }

        let Some((event, payload)) = try_read_stream_event(&mut stream, POLL_INTERVAL)? else {
            // Nothing arrived within this poll -- distinct from an actual I/O error
            // (see `RemoteError::WorkerSilent`'s own doc comment): if this has gone on
            // longer than the currently-applicable deadline, the worker has stopped
            // saying anything at all and this gives up rather than polling a dead
            // connection forever.
            let deadline = liveness_deadline(seen_first_event, liveness_timeout);
            if last_event.elapsed() > deadline {
                return Err(RemoteError::WorkerSilent(last_event.elapsed()));
            }
            continue;
        };
        last_event = Instant::now();
        seen_first_event = true;

        let outcome = {
            let mut acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
            acc.apply(&event, payload.as_deref())
                .map_err(|e| RemoteError::Client(e.into()))?
        };

        let is_terminal = matches!(
            outcome,
            ApplyOutcome::Done { .. } | ApplyOutcome::WorkerError
        );
        if let Some(update) = to_remote_update(request_id, &event, outcome) {
            on_update(update);
        }
        if is_terminal {
            return Ok(());
        }
    }
}

/// The background loop behind [`spawn_remote_connection`]: owns `worker`'s connection
/// (lazily -- `stream`/`welcome` start `None`) for as long as `commands` stays open,
/// reconnecting transparently whenever it's found dead rather than exiting after one
/// request the way [`run`] does. Mirrors [`run`]'s read-and-interleave-commands loop
/// shape; the difference is entirely at the top level -- no `DONE` here ever ends the
/// loop, only `commands` disconnecting (the handle was dropped) does.
///
/// `liveness_timeout` is [`LIVENESS_TIMEOUT`] via [`spawn_remote_connection`] in
/// production; threaded as a parameter so [`check_liveness`]'s tests can exercise the
/// decision with a much shorter deadline.
fn run_connection(
    worker: &WorkerSettings,
    commands: &mpsc::Receiver<ConnectionCommand>,
    liveness_timeout: Duration,
) {
    let mut stream: Option<RemoteStream> = None;
    let mut welcome: Option<indicatrix_net::messages::Welcome> = None;
    let mut current: Option<CurrentRequest> = None;
    // Reset on every received `StreamEvent` and every fresh dispatch (a reconnect or a
    // request sent on a reused connection both count as a sign of life) -- see
    // `LIVENESS_TIMEOUT`'s own doc comment.
    let mut last_event = Instant::now();
    // Whether the CURRENT request (if any) has produced any stream event yet -- selects
    // `FIRST_EVENT_TIMEOUT` (before) or `liveness_timeout` (after) via
    // `liveness_deadline`, exactly like `run_with_liveness_timeout`'s own
    // `seen_first_event`. Reset alongside `last_event` on every fresh dispatch: a
    // persistent connection serves many requests over its life, and each one gets its
    // own full first-event grace, not whatever was left over from the last one.
    let mut seen_first_event = false;

    loop {
        match commands.try_recv() {
            Ok(ConnectionCommand::Render(cmd)) => {
                dispatch_render(worker, &mut stream, &mut welcome, &mut current, *cmd);
                last_event = Instant::now();
                seen_first_event = false;
            }
            Ok(ConnectionCommand::Cancel { request_id }) => {
                // A no-op if `request_id` isn't (or is no longer) the current request --
                // see `ConnectionCommand::Cancel`'s own doc comment for why that's safe.
                if current.as_ref().is_some_and(|c| c.request_id == request_id)
                    && let Some(s) = stream.as_mut()
                    && indicatrix_net::client::send_cancel(s, request_id).is_err()
                {
                    // The connection is dead -- the next Render dispatch reconnects
                    // fully rather than trying to salvage this write.
                    stream = None;
                    welcome = None;
                }
            }
            Err(TryRecvError::Empty) => {}
            // The handle was dropped -- `RemoteConnectionHandle`'s own doc comment
            // commits to tearing this connection down (window close, a worker
            // address/cert_dir edit, removal from the list), so this exits immediately
            // rather than `run`'s one-shot "drain until DONE" courtesy: there is no
            // caller left to observe a DONE even if this waited for one.
            Err(TryRecvError::Disconnected) => return,
        }

        let Some(s) = stream.as_mut() else {
            // Nothing connected (never dialed yet, or just found dead above/below) and
            // nothing queued to prompt a dial -- avoid busy-looping `try_recv` against a
            // socket that doesn't exist.
            thread::sleep(POLL_INTERVAL);
            continue;
        };

        match try_read_stream_event(s, POLL_INTERVAL) {
            Ok(Some((event, payload))) => {
                last_event = Instant::now();
                seen_first_event = true;
                route_event(&mut current, &event, payload.as_deref());
            }
            Ok(None) => {
                // Nothing pending within this poll -- if a request has been waiting
                // this long for ANY sign of life (the currently-applicable deadline --
                // see `liveness_deadline`'s own doc comment for why that's not always
                // `liveness_timeout`), give up on it (see `check_liveness`'s own doc
                // comment) and drop the connection so the next dispatch reconnects
                // rather than reusing a worker that's gone quiet.
                let deadline = liveness_deadline(seen_first_event, liveness_timeout);
                if check_liveness(&mut current, last_event, deadline) {
                    stream = None;
                    welcome = None;
                }
            }
            Err(e) => {
                // A transport error mid-stream: report it for whatever was in flight
                // (matching `run`'s own top-level `Err` -> `RemoteUpdate::Failed`
                // handling) and drop the connection -- the NEXT Render dispatch
                // reconnects rather than this thread ever retrying on its own; see
                // `RemoteConnectionHandle`'s "Reconnection" doc section.
                if let Some(mut cur) = current.take() {
                    (cur.on_update)(RemoteUpdate::Failed {
                        request_id: cur.request_id,
                        message: RemoteError::from(e).to_string(),
                    });
                }
                stream = None;
                welcome = None;
            }
        }
    }
}

/// Checks whether `current` (if any) has gone longer than `timeout` since `last_event`
/// and, if so, delivers [`RemoteUpdate::Failed`] to it and returns `true` -- telling
/// [`run_connection`]'s caller to drop `stream`/`welcome` too, so the next dispatch
/// reconnects rather than keep polling a worker that has stopped saying anything. A
/// no-op (`false`) when nothing is current or the deadline hasn't passed yet.
///
/// Factored out of `run_connection`'s `Ok(None)` arm so this decision -- pure,
/// independent of any actual socket -- is unit-testable without a live TCP/TLS
/// connection ([`RemoteStream`] is a concrete `rustls::StreamOwned`, with no fake-stream
/// seam the way `bridge::library::client` has). The tests below pin exactly this: a
/// request current longer than `timeout` with no event gets failed and cleared; one
/// whose clock keeps resetting (as `run_connection` does on every real event) never
/// trips.
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

/// Handles one [`ConnectionCommand::Render`]: supersedes whatever was previously
/// current, connects/reconnects on demand, checks render capacity, and sends the
/// request -- everything [`run`] does once per call, except this can run many times
/// against the same `stream`/`welcome` across the life of [`run_connection`]'s loop.
fn dispatch_render(
    worker: &WorkerSettings,
    stream: &mut Option<RemoteStream>,
    welcome: &mut Option<indicatrix_net::messages::Welcome>,
    current: &mut Option<CurrentRequest>,
    cmd: RenderCommand,
) {
    let RenderCommand {
        request,
        accumulator,
        mut on_update,
    } = cmd;
    let request_id = request.request_id;

    // Supersede whatever was still in flight: best-effort CANCEL on the wire (best
    // -effort because a dead connection is about to be recreated below regardless, and
    // because the REAL correctness guard is the new accumulator's own epoch -- see
    // `RemoteConnectionHandle::render`'s doc comment), then drop the old on_update/
    // accumulator so nothing reachable from here ever reports for it again.
    if let Some(previous) = current.take()
        && let Some(s) = stream.as_mut()
    {
        let _ = indicatrix_net::client::send_cancel(s, previous.request_id);
    }

    if stream.is_none() {
        match connect_and_handshake(worker) {
            Ok((s, w)) => {
                *stream = Some(s);
                *welcome = Some(w);
            }
            Err(e) => {
                on_update(RemoteUpdate::Failed {
                    request_id,
                    message: e.to_string(),
                });
                return; // this request never started -- `current` stays `None`
            }
        }
    }
    let w = welcome
        .as_ref()
        .expect("stream is Some at this point, and the two are always set together");
    on_update(RemoteUpdate::Connected {
        request_id,
        info: w.clone().into(),
    });

    let Some(capability) = w.render.as_ref() else {
        on_update(RemoteUpdate::Failed {
            request_id,
            message: RemoteError::NoRenderCapacity.to_string(),
        });
        return;
    };

    let render_request = RenderRequest {
        request_id,
        scene: request.scene,
        first_sample: request.first_sample,
        samples: request.samples,
        stream: worker.stream_config(capability.min_cadence_ms, request.width, request.height),
    };

    {
        let mut acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
        acc.begin_request(request_id);
    }

    let s = stream
        .as_mut()
        .expect("just connected above if this was None");
    match indicatrix_net::client::send_render_request(s, &render_request) {
        Ok(()) => {
            *current = Some(CurrentRequest {
                request_id,
                accumulator,
                on_update,
            });
        }
        Err(e) => {
            // The write failed -- the connection is dead. Drop it (the next dispatch
            // reconnects) and report failure now rather than leaving `current` pointing
            // at a request that was never actually sent.
            *stream = None;
            *welcome = None;
            on_update(RemoteUpdate::Failed {
                request_id,
                message: RemoteError::from(e).to_string(),
            });
        }
    }
}

/// Applies one [`StreamEvent`] to whatever request is currently in flight on a
/// persistent connection, mirroring [`run`]'s inner-loop body (apply via
/// [`Accumulator::apply`], translate via [`to_remote_update`], clear on a terminal
/// outcome) -- except here `current` may be `None`, in which case the event is simply
/// dropped (a reply for a request this thread already gave up on).
///
/// `event`'s `request_id` is never compared against `current.request_id` directly here
/// -- [`Accumulator::apply`]'s own epoch check already makes that comparison. A stale
/// reply for a request this connection superseded (via [`dispatch_render`]'s supersede
/// step) is therefore dropped correctly even if the best-effort `CANCEL` for it never
/// reached the worker in time -- this is the actual mechanism that keeps a late `FRAME`
/// from settle N-1 out of settle N's accumulator, not the `CANCEL` itself.
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

fn to_remote_update(
    request_id: u32,
    event: &StreamEvent,
    outcome: ApplyOutcome,
) -> Option<super::types::RemoteUpdate> {
    match outcome {
        ApplyOutcome::FrameSummed { samples_done } => Some(RemoteUpdate::Frame {
            request_id,
            samples_done,
        }),
        ApplyOutcome::PreviewReplaced => Some(RemoteUpdate::Preview { request_id }),
        ApplyOutcome::Progress { samples_done } => Some(RemoteUpdate::Progress {
            request_id,
            samples_done,
        }),
        ApplyOutcome::Done { cancelled } => Some(RemoteUpdate::Done {
            request_id,
            cancelled,
        }),
        ApplyOutcome::WorkerError => {
            let StreamEvent::Error(e) = event else {
                unreachable!("WorkerError only ever comes from applying an Error event")
            };
            Some(RemoteUpdate::Failed {
                request_id,
                message: e.message.clone(),
            })
        }
        ApplyOutcome::StaleDropped => None,
    }
}

/// Attempts to read one length-prefixed frame from `stream`, tolerating a
/// `WouldBlock`/`TimedOut` I/O error (from the short read timeout this sets) as
/// "nothing pending yet" rather than a fatal error -- mirrors
/// `apps/indicatrix-worker/src/stream_emit.rs::poll_for_client_message`'s own approach on
/// the server side (see that function's doc comment for why the timeout can only ever
/// land BEFORE any byte of a new frame arrives, never mid-frame).
///
/// Once a byte is observed, the rest of that frame (the remaining length-prefix bytes,
/// plus the whole payload) is read under a bounded [`FRAME_REMAINDER_TIMEOUT`] rather
/// than an unbounded blocking read -- a timeout there is a protocol error (surfaced as
/// an `Err`, not `Ok(None)`): a frame already in flight never finishing arriving is
/// itself the anomaly, not "nothing new yet". See [`FRAME_REMAINDER_TIMEOUT`]'s own doc
/// comment for the hang this closes.
fn try_read_one_frame(
    stream: &mut RemoteStream,
    poll_timeout: Duration,
) -> Result<Option<Vec<u8>>, NetError> {
    stream
        .sock
        .set_read_timeout(Some(poll_timeout))
        .map_err(|e| NetError::Framing(framing::FramingError::Io(e)))?;

    let mut len_bytes = [0u8; LEN_PREFIX_BYTES];
    let n = match stream.read(&mut len_bytes) {
        Ok(0) => {
            return Err(NetError::Framing(framing::FramingError::Io(
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "worker closed the connection",
                ),
            )));
        }
        Ok(n) => n,
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            return Ok(None);
        }
        Err(e) => return Err(NetError::Framing(framing::FramingError::Io(e))),
    };

    // At least one byte has arrived -- commit to reading the rest under a bounded
    // timeout, not `None`: a stall here is a protocol error, not "nothing new yet".
    stream
        .sock
        .set_read_timeout(Some(FRAME_REMAINDER_TIMEOUT))
        .map_err(|e| NetError::Framing(framing::FramingError::Io(e)))?;
    if n < len_bytes.len() {
        stream
            .read_exact(&mut len_bytes[n..])
            .map_err(|e| NetError::Framing(framing::FramingError::Io(e)))?;
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(NetError::Framing(framing::FramingError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        }));
    }
    let mut payload = vec![0u8; len as usize];
    stream
        .read_exact(&mut payload)
        .map_err(|e| NetError::Framing(framing::FramingError::Io(e)))?;
    Ok(Some(payload))
}

/// The timeout-tolerant equivalent of [`indicatrix_net::messages::read_stream_event`]:
/// `Ok(None)` means "nothing arrived within `poll_timeout`", not an error -- lets
/// [`run`]'s loop interleave checking for a [`RemoteCommand`] between read attempts
/// without a second thread ever touching `stream`. See the module doc comment.
fn try_read_stream_event(
    stream: &mut RemoteStream,
    poll_timeout: Duration,
) -> Result<Option<StreamEventFrame>, NetError> {
    let Some(header_bytes) = try_read_one_frame(stream, poll_timeout)? else {
        return Ok(None);
    };
    let event: StreamEvent = postcard::from_bytes(&header_bytes)?;
    let expected_len = match &event {
        StreamEvent::Frame(h) => Some(h.payload_len),
        StreamEvent::Preview(h) => Some(h.payload_len),
        StreamEvent::Progress(_) | StreamEvent::Done(_) | StreamEvent::Error(_) => None,
    };
    let payload = match expected_len {
        Some(expected) => {
            let bytes = framing::read_frame(stream).map_err(NetError::Framing)?;
            if bytes.len() as u32 != expected {
                return Err(NetError::FramePayloadLenMismatch {
                    declared: expected,
                    actual: bytes.len(),
                });
            }
            Some(bytes)
        }
        None => None,
    };
    Ok(Some((event, payload)))
}

#[cfg(test)]
mod tests;
