//! Client for the read-only design-library protocol (`indicatrix_net::library`): a
//! one-shot [`request`] that connects, sends exactly one [`LibraryRequest`], reads
//! exactly one [`LibraryResponse`], and disconnects; and [`LibrarySession`], which
//! connects once and holds that connection open across many requests.
//!
//! Unlike [`crate::bridge::remote_render`], the library protocol is plain
//! request/response, not streamed -- no poll-loop/timeout-read machinery needed. Both
//! forms here reuse [`crate::bridge::remote_render::connect_and_handshake`] for the
//! mutual-TLS connect+`HELLO`/`WELCOME` rather than duplicating it.
//!
//! # One-shot vs. one held session
//!
//! [`request`] is for a genuinely single call -- the interactive remote-browse path
//! uses it: a user types a query, gets one page of results back. Reconnecting per call
//! is a non-issue with no tight loop of them.
//!
//! [`LibrarySession`] exists for the opposite shape: `bridge::library_mirror`'s sync
//! issues thousands of requests for a real catalogue. Paying a full connect+handshake
//! for every one turns a sync into minutes of pure handshaking before any data moves.
//! A [`LibrarySession`] connects and handshakes once, then reuses that stream for
//! every request -- see its own doc comment for what happens if it drops mid-sync.
//!
//! # Always off the UI thread
//!
//! Every function here (and every [`LibrarySession::request`] call) is a blocking
//! network call. Callers MUST run them on their own thread and marshal the result back
//! via `Weak::upgrade_in_event_loop`.

use crate::{
    bridge::remote::remote_render::{RemoteError, RemoteStream, connect_and_handshake},
    settings::WorkerSettings,
};
use indicatrix_net::{
    client::{self, ClientError, ConnectionInfo},
    library::{LibraryRequest, LibraryResponse},
    messages,
};
use std::{
    io::{Read, Write},
    sync::{Mutex, PoisonError},
    time::Duration,
};

/// A read timeout applied to every [`RemoteStream`] this module holds, set once right
/// after [`connect_and_handshake`] returns (see [`connect_checked`]). Without it, a
/// worker that accepted the request and then stopped responding would block
/// [`send_and_read`] forever. Considerably longer than
/// `bridge::remote::remote_render::connection`'s `LIVENESS_TIMEOUT`: a library reply is
/// one message, not a stream of ticks, and can legitimately be large -- 30s is generous
/// headroom over a slow link, not a tight per-tick bound.
const LIBRARY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything that can go wrong making one library request against a remote worker.
#[derive(Debug)]
pub enum LibraryClientError {
    /// Connecting or handshaking failed -- see [`RemoteError`]'s variants (TLS, I/O,
    /// handshake refusal/incompatibility, a malformed worker address).
    Connect(RemoteError),
    /// The worker connected and authenticated fine but advertises no library capacity
    /// (`Welcome::library` is `false`) -- the same posture [`RemoteError::NoRenderCapacity`]
    /// takes for rendering: nothing is broken, the server just doesn't serve a library.
    NoLibraryCapacity,
    /// Sending the request or reading the reply frame failed at the transport level.
    Client(ClientError),
    /// The worker replied [`LibraryResponse::Error`] -- a request-level failure it
    /// could still form a normal reply for. Carries the worker's message.
    WorkerError(String),
}

impl std::fmt::Display for LibraryClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "{e}"),
            Self::NoLibraryCapacity => write!(
                f,
                "this worker does not serve a design library -- it was started without a library database"
            ),
            Self::Client(e) => write!(f, "{e}"),
            Self::WorkerError(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for LibraryClientError {}

impl From<ClientError> for LibraryClientError {
    fn from(e: ClientError) -> Self {
        Self::Client(e)
    }
}

impl From<messages::NetError> for LibraryClientError {
    fn from(e: messages::NetError) -> Self {
        Self::Client(e.into())
    }
}

/// Connects to `worker` and checks it actually advertises library capacity
/// (`Welcome::library`) without sending any [`LibraryRequest`] -- the "is this worker
/// reachable and does it serve a library at all" check a source-switch UI action runs
/// before committing to it.
///
/// # Errors
///
/// See [`LibraryClientError`]'s variants ([`LibraryClientError::WorkerError`] is never
/// returned here -- no request is sent).
pub fn probe(worker: &WorkerSettings) -> Result<ConnectionInfo, LibraryClientError> {
    let (_stream, welcome) = connect_and_handshake(worker).map_err(LibraryClientError::Connect)?;
    if !welcome.library {
        return Err(LibraryClientError::NoLibraryCapacity);
    }
    Ok(welcome.into())
}

/// Connects to `worker`, handshakes, and checks `welcome.library` -- the shared guts of
/// both [`request`]'s one-shot connect and [`LibrarySession`]'s (re)connect step. Unlike
/// [`probe`], the `Welcome` itself is discarded once checked: neither caller needs
/// anything from it beyond the capability bit.
fn connect_checked(worker: &WorkerSettings) -> Result<RemoteStream, LibraryClientError> {
    let (stream, welcome) = connect_and_handshake(worker).map_err(LibraryClientError::Connect)?;
    if !welcome.library {
        return Err(LibraryClientError::NoLibraryCapacity);
    }
    // `connect_and_handshake` leaves no read timeout set past its own handshake --
    // `LIBRARY_READ_TIMEOUT` bounds every request/reply read for the life of this stream.
    stream
        .sock
        .set_read_timeout(Some(LIBRARY_READ_TIMEOUT))
        .map_err(|e| LibraryClientError::Connect(RemoteError::Io(e)))?;
    Ok(stream)
}

/// Sends exactly one [`LibraryRequest`] to `worker` and returns the design-library
/// data it carries back, over a fresh connection made and torn down just for this
/// call. See the module doc comment's "One-shot vs. one held session" section for
/// when to reach for this instead of [`LibrarySession`].
///
/// [`LibraryResponse::Error`] is unwrapped into [`LibraryClientError::WorkerError`]
/// rather than returned as `Ok`, so every caller handles "the worker said no" through
/// the same `Result::Err` path as a transport failure.
///
/// # Errors
///
/// See [`LibraryClientError`]'s variants.
pub fn request(
    worker: &WorkerSettings,
    req: &LibraryRequest,
) -> Result<LibraryResponse, LibraryClientError> {
    let mut stream = connect_checked(worker)?;
    send_and_read(&mut stream, req)
}

/// Writes `req` and reads back exactly one reply, over any `Read + Write`.
///
/// Generic rather than tied to [`RemoteStream`] specifically so
/// [`request_with_reconnect`]'s retry decision is unit-testable against an in-memory
/// duplex buffer standing in for a dropped connection, with no live socket.
fn send_and_read<S: Read + Write>(
    stream: &mut S,
    req: &LibraryRequest,
) -> Result<LibraryResponse, LibraryClientError> {
    client::send_library_request(stream, req)?;
    let response: LibraryResponse = messages::read_message(stream)?;
    match response {
        LibraryResponse::Error(e) => Err(LibraryClientError::WorkerError(e.message)),
        other => Ok(other),
    }
}

/// Whether `e` means the connection itself is broken -- worth reconnecting for -- as
/// opposed to a reply the worker formed just fine (`WorkerError`) or a failure
/// `connect` already reports on its own (`Connect`/`NoLibraryCapacity`).
/// [`ClientError::Net`] is the transport-level case (malformed frame, dropped socket,
/// [`LIBRARY_READ_TIMEOUT`] elapsed, timed-out write). Deliberately blunt -- any
/// [`ClientError::Net`] counts as dead, regardless of the nested error. Treating the
/// connection as suspect and reconnecting before retrying costs one extra handshake at
/// worst and is never wrong.
const fn is_dead_connection(e: &LibraryClientError) -> bool {
    matches!(e, LibraryClientError::Client(ClientError::Net(_)))
}

/// The reconnect-on-dead-connection decision [`LibrarySession::request`] applies to its
/// real held [`RemoteStream`] -- factored out generic over any `Read + Write` stream and
/// a `connect` closure so the decision is unit-testable against a scripted in-memory
/// stream, with no live socket.
///
/// `held` starts (or ends up, after a failed request) as `None` -- `connect()` fills
/// it. [`send_and_read`] is tried against whatever's in `held`; on success that
/// connection is kept for next time. On an [`is_dead_connection`] failure, `held` is
/// dropped, `connect()` is called exactly once more, and the same `req` is retried on
/// the fresh connection. Any other error propagates immediately, with `held` left
/// `None` after a failed reconnect so the next call starts clean.
fn request_with_reconnect<S: Read + Write>(
    held: &mut Option<S>,
    req: &LibraryRequest,
    mut connect: impl FnMut() -> Result<S, LibraryClientError>,
) -> Result<LibraryResponse, LibraryClientError> {
    if held.is_none() {
        *held = Some(connect()?);
    }
    let stream = held.as_mut().expect("just connected above if it was empty");
    match send_and_read(stream, req) {
        Ok(response) => Ok(response),
        Err(e) if is_dead_connection(&e) => {
            *held = None;
            let mut fresh = connect()?;
            let response = send_and_read(&mut fresh, req)?;
            *held = Some(fresh);
            Ok(response)
        }
        Err(e) => Err(e),
    }
}

/// A held mutual-TLS connection to `worker`'s design-library protocol, reused across
/// many [`LibraryRequest`]s for the life of one `bridge::library_mirror` sync. See the
/// module doc comment's "One-shot vs. one held session" section for why this exists
/// alongside [`request`].
///
/// # Connecting, and the `welcome.library` check, happen once -- not per request
///
/// The first [`Self::request`] call connects, handshakes, and checks `welcome.library`;
/// every call after that reuses the same stream, unless it has to be re-established
/// after dropping (below), which re-runs the same check on the fresh connection.
///
/// # What happens when the held connection drops mid-sync: reconnect and retry once
///
/// A long-held connection breaking mid-sync is a certainty over a multi-thousand-design
/// sync. [`Self::request`] (via [`request_with_reconnect`]) drops the dead stream,
/// connects fresh exactly once, and retries the same request -- transparent to the
/// caller, whose per-design loop only writes a design after every network call for it
/// has succeeded, so a reconnect mid-fetch never leaves a half-written design.
///
/// If the reconnect attempt itself fails, [`Self::request`] returns that error rather
/// than retrying further: a failure during catalogue enumeration surfaces as
/// `MirrorOutcome::Failed`; a failure fetching one design is counted in
/// `MirrorCounts::failed` and left unsynced for the next sync to retry.
pub struct LibrarySession {
    worker: WorkerSettings,
    /// `None` before the first request, and again immediately after a dead connection
    /// is dropped -- see [`request_with_reconnect`]. Guarded by a [`Mutex`] purely so
    /// [`Self::request`] can take `&self`; nothing here is actually touched from more
    /// than one thread at a time.
    stream: Mutex<Option<RemoteStream>>,
}

impl LibrarySession {
    #[must_use]
    pub const fn new(worker: WorkerSettings) -> Self {
        Self {
            worker,
            stream: Mutex::new(None),
        }
    }

    /// Sends `req` on the held connection, connecting first if this is the very first
    /// call on this session, and transparently reconnecting-and-retrying once if that
    /// connection turns out to be dead -- see this type's own doc comment for the full
    /// policy and why it preserves every safety guarantee a mirror sync depends on.
    ///
    /// # Errors
    ///
    /// See [`LibraryClientError`]'s variants; a [`LibraryClientError::Connect`] here may
    /// mean either the very first connect failed, or a reconnect attempt after a dropped
    /// connection did.
    pub fn request(&self, req: &LibraryRequest) -> Result<LibraryResponse, LibraryClientError> {
        let mut guard = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        request_with_reconnect(&mut guard, req, || connect_checked(&self.worker))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_client_error_display_is_human_readable() {
        let e = LibraryClientError::NoLibraryCapacity;
        assert!(e.to_string().contains("does not serve a design library"));

        let e = LibraryClientError::WorkerError("bad filter".to_string());
        assert_eq!(e.to_string(), "bad filter");
    }

    /// A `Read + Write` over two independent in-memory buffers, standing in for one
    /// held connection's stream, so [`request_with_reconnect`]'s retry decision is
    /// exercised with no live socket.
    struct FakeStream {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl FakeStream {
        fn new(input: Vec<u8>) -> Self {
            Self {
                input: std::io::Cursor::new(input),
                output: Vec::new(),
            }
        }

        /// A stream with nothing left to read -- `send_and_read` writes the request fine
        /// but then hits an immediate EOF reading the reply frame, exactly how a
        /// dropped connection looks from this side.
        fn dead() -> Self {
            Self::new(Vec::new())
        }
    }

    impl Read for FakeStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for FakeStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn encoded_response(resp: &LibraryResponse) -> Vec<u8> {
        let mut buf = Vec::new();
        messages::write_message(&mut buf, resp).unwrap();
        buf
    }

    #[test]
    fn request_with_reconnect_reconnects_once_after_a_dead_connection_and_succeeds() {
        let good_reply = encoded_response(&LibraryResponse::NotFound);
        let mut remaining = vec![FakeStream::new(good_reply)];
        let connect_calls = std::cell::Cell::new(0);

        // The FIRST connect hands back an already-dead stream (nothing to read), so the
        // request against it fails and a reconnect is expected; the SECOND connect hands
        // back a stream with a real, decodable reply queued up.
        let mut held: Option<FakeStream> = Some(FakeStream::dead());
        let response = request_with_reconnect(&mut held, &LibraryRequest::FilterOptions, || {
            connect_calls.set(connect_calls.get() + 1);
            Ok(remaining.pop().expect("only one reconnect expected"))
        })
        .unwrap();

        assert_eq!(response, LibraryResponse::NotFound);
        assert_eq!(
            connect_calls.get(),
            1,
            "exactly one reconnect after the held connection turned out dead"
        );
        assert!(
            held.is_some(),
            "the freshly reconnected stream is kept in `held` for the next request"
        );
    }

    #[test]
    fn request_with_reconnect_connects_lazily_on_the_first_call() {
        let good_reply = encoded_response(&LibraryResponse::NotFound);
        let mut held: Option<FakeStream> = None;
        let connect_calls = std::cell::Cell::new(0);

        let response = request_with_reconnect(&mut held, &LibraryRequest::FilterOptions, || {
            connect_calls.set(connect_calls.get() + 1);
            Ok(FakeStream::new(good_reply.clone()))
        })
        .unwrap();

        assert_eq!(response, LibraryResponse::NotFound);
        assert_eq!(connect_calls.get(), 1);
    }

    #[test]
    fn request_with_reconnect_does_not_reconnect_for_a_worker_error_reply() {
        // LibraryResponse::Error is a reply the worker formed just fine -- request-level,
        // not a broken connection -- so this must never trigger a reconnect attempt.
        let error_reply = encoded_response(&LibraryResponse::Error(messages::ErrorMsg {
            code: 1,
            message: "bad filter".to_string(),
        }));
        let mut held = Some(FakeStream::new(error_reply));
        let connect_calls = std::cell::Cell::new(0);

        let result = request_with_reconnect(&mut held, &LibraryRequest::FilterOptions, || {
            connect_calls.set(connect_calls.get() + 1);
            Ok::<FakeStream, LibraryClientError>(FakeStream::dead())
        });

        assert!(matches!(result, Err(LibraryClientError::WorkerError(_))));
        assert_eq!(
            connect_calls.get(),
            0,
            "a worker-level error reply must never trigger a reconnect"
        );
    }

    #[test]
    fn request_with_reconnect_propagates_the_error_when_reconnecting_also_fails() {
        // The held connection is dead AND the network is genuinely down (not just a
        // blip) -- the whole point of "reconnect once, don't retry forever".
        let mut held = Some(FakeStream::dead());
        let connect_calls = std::cell::Cell::new(0);

        let result = request_with_reconnect(&mut held, &LibraryRequest::FilterOptions, || {
            connect_calls.set(connect_calls.get() + 1);
            Err::<FakeStream, _>(LibraryClientError::Connect(RemoteError::Io(
                std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"),
            )))
        });

        assert!(matches!(result, Err(LibraryClientError::Connect(_))));
        assert_eq!(
            connect_calls.get(),
            1,
            "exactly one reconnect attempt, not a retry loop"
        );
        assert!(
            held.is_none(),
            "a connection already known dead is never left in `held` after a failed reconnect"
        );
    }

    /// A stream whose every read fails with `TimedOut` -- stands in for a real
    /// `RemoteStream` once [`LIBRARY_READ_TIMEOUT`] elapses with no reply. Reaches
    /// `is_dead_connection` wrapped as `ClientError::Net(...)`, already caught by its
    /// blanket `ClientError::Net(_)` match with no special case needed.
    struct TimingOutStream;

    impl Read for TimingOutStream {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out",
            ))
        }
    }

    impl Write for TimingOutStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn request_with_reconnect_treats_a_timed_out_read_as_a_dead_connection() {
        let mut held = Some(TimingOutStream);
        let connect_calls = std::cell::Cell::new(0);

        let result = request_with_reconnect(&mut held, &LibraryRequest::FilterOptions, || {
            connect_calls.set(connect_calls.get() + 1);
            Ok::<TimingOutStream, LibraryClientError>(TimingOutStream)
        });

        assert!(
            matches!(result, Err(LibraryClientError::Client(ClientError::Net(_)))),
            "a TimedOut read must surface as a transport-level ClientError::Net"
        );
        assert_eq!(
            connect_calls.get(),
            1,
            "a timed-out read on the held connection must trigger exactly one reconnect \
             attempt, same as any other dead-connection error"
        );
    }
}
