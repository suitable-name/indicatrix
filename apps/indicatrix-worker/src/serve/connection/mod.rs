//! Per-connection request handling: the `HELLO`/`WELCOME` handshake and the loop
//! dispatching whatever the peer sends -- `ClientMessage::Library` always, and (only on
//! a `worker` build) `ClientMessage::RenderRequest`/`ClientMessage::TiltCurvesRequest`
//! too -- until the peer closes the connection. See `crate::serve`'s module docs for the
//! full architecture this participates in.
//!
//! - this file: what both build modes share (refusals, the library dispatch) and the
//!   library-only build's whole handler;
//! - [`worker`] (`worker` builds): the viewer-side handshake and `WELCOME` (including the
//!   coordinator's `WELCOME` advertisement);
//! - [`requests`] (`worker` builds): the post-`WELCOME` request loop -- independent of
//!   who dialled, so `serve`'s accept path and `join`'s dial path share it.

use super::library::LibraryHandle;
use indicatrix_net::messages::{ClientMessage, ErrorMsg, NetError, error_codes};
#[cfg(not(feature = "worker"))]
use indicatrix_net::messages::{PROTOCOL_VERSION, PayloadEncoding, PeerRole, Welcome};
#[cfg(feature = "worker")]
use std::io::Write;
#[cfg(not(feature = "worker"))]
use std::io::{Read, Write};
use std::net::SocketAddr;

#[cfg(feature = "worker")]
mod requests;
#[cfg(feature = "worker")]
mod worker;

/// Logs how one connection thread's handler ended.
pub fn report_connection_result(
    peer: Option<SocketAddr>,
    result: std::thread::Result<Result<(), NetError>>,
) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("connection {peer:?} ended with an error: {e}"),
        Err(_) => tracing::warn!("connection {peer:?} panicked and was dropped"),
    }
}

/// `<- ERROR` code for a `HELLO` this worker refuses to pair with -- a protocol-version
/// mismatch (either build) or (also) a `indicatrix` build-hash mismatch (`worker`
/// builds). The shared vocabulary lives in [`error_codes`]; these aliases keep this
/// crate's historical names. (The version half of the gate lives in `super::handshake`,
/// shared by both builds.)
#[cfg(feature = "worker")]
pub(super) const BUILD_MISMATCH_CODE: u32 = error_codes::BUILD_MISMATCH;

/// `<- ERROR` code for a `RenderRequest` arriving at a worker with no render capacity.
/// Distinct from `BUILD_MISMATCH_CODE` on purpose: that one means "we cannot pair at
/// all", this one means "we paired fine, but I only serve the library" -- a client can
/// act on the difference (fall back to local rendering rather than dropping the worker).
pub(super) const NO_RENDER_CAPACITY_CODE: u32 = error_codes::NO_RENDER_CAPACITY;

/// `<- ERROR` code for [`refuse_for_capacity`]: this worker is already handling
/// `--max-connections` connections and refuses to accept another. Sent in place of
/// `WELCOME`, exactly like `BUILD_MISMATCH_CODE` -- see that function's doc comment for
/// why no new wire message type was needed for this.
pub(super) const CONNECTION_LIMIT_REACHED_CODE: u32 = error_codes::CONNECTION_LIMIT_REACHED;

#[cfg(feature = "worker")]
pub use requests::{RequestContext, serve_requests};
#[cfg(feature = "worker")]
pub(super) use requests::{TRACE_PANIC_CODE, VALIDATION_FAILED_CODE};
#[cfg(feature = "worker")]
pub use worker::{
    ViewerContext, handle_connection, handle_connection_with_gpu, handle_viewer_connection,
    local_render_capability, refuse_incompatible_handshake,
};

/// Sends a single `<- ERROR` reply in place of `WELCOME`, telling a peer this server is
/// already at `--max-connections` capacity, then lets the caller drop the connection --
/// the accept loops call this instead of ever handing the connection to a handler when
/// [`super::ConnectionLimiter::try_acquire`] reports the cap is reached.
///
/// Deliberately reuses [`ErrorMsg`], the same `HELLO`-phase refusal shape
/// `BUILD_MISMATCH_CODE` already sends in place of `WELCOME`, rather than adding a new
/// wire message: `indicatrix_net::client::handshake` already reads exactly one reply after
/// writing `HELLO` and tries to decode it as `Welcome` first, falling back to `ErrorMsg`.
///
/// Never reads the peer's own `HELLO` off the wire first -- unnecessary, since the reply
/// above is decoded the same way regardless of whether the server ever read what the
/// client sent. For a TLS listener, call this only after [`super::tls::accept_tls`] has
/// already completed (handshake AND allowlist check): an unauthenticated peer must not
/// learn this server's capacity state before authenticating. For the loopback plaintext
/// listener, there is no authentication step to wait for, so this runs immediately.
pub fn refuse_for_capacity<S: Write>(
    stream: &mut S,
    peer: Option<SocketAddr>,
    active: usize,
    max: usize,
) {
    tracing::info!(
        "connection {peer:?}: refusing -- at capacity ({active}/{max} active connections; see --max-connections)"
    );
    let _ = indicatrix_net::messages::write_message(
        stream,
        &ErrorMsg {
            code: CONNECTION_LIMIT_REACHED_CODE,
            message: format!(
                "this worker is already handling {max} concurrent connection(s) (--max-connections); try again \
                 later"
            ),
            // Refused before HELLO -- no request exists yet.
            request_id: None,
        },
    );
}

/// Dispatches `ClientMessage::Library` to [`LibraryHandle::handle`] and writes
/// its reply directly (plain request/response, never a `StreamEvent` stream). A
/// `Cancel` with nothing currently streaming is logged and ignored. Shared by both
/// build modes.
///
/// `db` is `None` on a `join`ed worker's connection: a render worker serves no library
/// (the coordinator does), so a `Library` request gets a `LibraryResponse::Error`. Any
/// other connection passes its [`LibraryHandle`], which opens the database here, on the
/// connection's first `Library` request.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure.
#[allow(
    unreachable_patterns,
    clippy::match_wildcard_for_single_variants,
    reason = "the final arm must stay a wildcard: it matches ClientMessage::RenderRequest and \
              ClientMessage::TiltCurvesRequest only when indicatrix-net's `render` feature is on. \
              That is a DIFFERENT crate's flag from this one's `worker` -- this crate has no \
              feature of its own named `render` at all, so `#[cfg(feature = \"render\")]` inside \
              THIS crate is always false regardless of whether indicatrix-net's variants actually \
              exist (cargo unifies features workspace-wide, so `apps/indicatrix-cut` turning \
              indicatrix-net's `render` on can leave this crate's own `worker` feature off while the \
              variants are still genuinely in scope -- confirmed by `cargo check -p indicatrix-worker \
              --features worker`, which fails to compile a two-cfg-arm version of this match with \
              exactly the \"unexpected cfg condition value: render\" + \"non-exhaustive patterns\" \
              pair this comment replaces). Spelling the variants out, as clippy suggests, fails to \
              compile in a true library-only build (no other workspace member pulling `render` in \
              at all) where neither variant exists; the wildcard is correct in every build. Kept \
              as `allow` (not `expect`) because firing is conditional on that feature -- \
              unconditionally expecting it would warn on every build where the variants ARE \
              nameable."
)]
pub fn handle_non_render_message<S: Write>(
    stream: &mut S,
    msg: &ClientMessage,
    db: Option<&LibraryHandle>,
) -> Result<(), NetError> {
    match msg {
        ClientMessage::Cancel(c) => {
            tracing::debug!(
                "received CANCEL for request_id={} with no request currently streaming on this connection -- ignoring",
                c.request_id
            );
            Ok(())
        }
        ClientMessage::Library(req) => {
            let response = db.map_or_else(
                || {
                    indicatrix_net::library::LibraryResponse::Error(ErrorMsg {
                        code: NO_RENDER_CAPACITY_CODE,
                        message: "this connection is a joined render worker's; the design library \
                                  is served by the coordinator"
                            .to_string(),
                        // The library protocol has no request_id/epoch to be stale
                        // against.
                        request_id: None,
                    })
                },
                |library| library.handle(req),
            );
            indicatrix_net::messages::write_message(stream, &response)
        }
        // Wildcard is the only correct choice here (see the `#[allow]` reason above).
        // Always replies `StreamEvent::Error`, even for a `TiltCurvesRequest` -- a
        // mismatch for that family's reader, but acceptable since this arm is only
        // reachable on a build without `worker`, where the peer already has enough
        // signal (a decode failure, or an absent advertised capability) to know
        // something is wrong.
        _ => {
            tracing::warn!(
                "received a RenderRequest or TiltCurvesRequest, but this worker advertises no \
                 render/tilt-curve capacity (built without its `worker` feature) -- replying \
                 with a protocol error"
            );
            indicatrix_net::messages::write_message(
                stream,
                &indicatrix_net::messages::StreamEvent::Error(indicatrix_net::messages::ErrorMsg {
                    code: NO_RENDER_CAPACITY_CODE,
                    message: "this worker serves the design library only and cannot render or \
                              compute tilt curves; its WELCOME advertises both capacities as \
                              absent"
                        .to_string(),
                    // `msg` is only known to be a RenderRequest/TiltCurvesRequest by
                    // exclusion here (see the `#[allow]` reason above); naming its
                    // request_id would need the same feature-gated match this arm
                    // deliberately avoids.
                    request_id: None,
                }),
            )
        }
    }
}

/// True if `e` is a socket read timeout elapsing. The `WriteZero` member is what a TLS
/// stream reports when the inner I/O of a read times out (see
/// `stream_emit::is_stream_timeout`, which this build cannot reach).
#[cfg(not(feature = "worker"))]
fn is_idle_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WriteZero
    )
}

/// Reads and dispatches the peer's next post-handshake message. `Cancel`/`Library` are
/// handled inline and the loop continues; a clean EOF ends the connection. Each read is
/// bounded in size ([`indicatrix_net::messages::read_control_message`]) and in time (the
/// [`indicatrix_net::framing::IDLE_READ_TIMEOUT`] that [`ClearHandshakeTimeout`] arms).
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure. `Ok(None)` (not an error) for a
/// clean EOF, and for a connection that sent nothing for the idle timeout, which is
/// closed with a logged reason.
#[cfg(not(feature = "worker"))]
fn serve_until_render_request_or_eof<S: Read + Write>(
    stream: &mut S,
    db: &LibraryHandle,
) -> Result<Option<std::convert::Infallible>, NetError> {
    loop {
        let msg: ClientMessage = match indicatrix_net::messages::read_control_message(stream) {
            Ok(m) => m,
            Err(NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(None);
            }
            Err(NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))
                if is_idle_timeout(&e) =>
            {
                tracing::info!(
                    "closing a library-only connection that sent nothing for {} s (idle timeout)",
                    indicatrix_net::framing::IDLE_READ_TIMEOUT.as_secs()
                );
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        handle_non_render_message(stream, &msg, Some(db))?;
    }
}

/// Ends the pre-`HELLO` deadline once `HELLO` has arrived: releases the deadline the
/// accept loop applied to the raw socket (see `serve::HANDSHAKE_TIMEOUT`), re-arming
/// reads with [`indicatrix_net::framing::IDLE_READ_TIMEOUT`] so a peer that goes silent
/// cannot hold a connection slot forever, and making writes blocking again. The
/// library-only build's counterpart to `stream_emit::TimeoutRead`/`TimeoutWrite`, which
/// the worker handler uses for the same purpose but which don't exist here: `stream_emit`
/// is gated on the `worker` feature this build doesn't have. Implemented only for the two
/// concrete transports the accept loop ever hands to [`handle_connection`] -- a plain
/// `TcpStream`, or one wrapped in mutual TLS.
#[cfg(not(feature = "worker"))]
pub trait ClearHandshakeTimeout {
    /// Best-effort, like `serve::tune_accepted_socket`: a failed `set_*_timeout` call is
    /// swallowed rather than turned into a connection-ending error, since the loop ahead
    /// works fine either way -- it would just keep the previous deadline.
    fn clear_handshake_timeout(&mut self);
}

#[cfg(not(feature = "worker"))]
impl ClearHandshakeTimeout for std::net::TcpStream {
    fn clear_handshake_timeout(&mut self) {
        let _ = self.set_read_timeout(Some(indicatrix_net::framing::IDLE_READ_TIMEOUT));
        let _ = self.set_write_timeout(None);
    }
}

#[cfg(not(feature = "worker"))]
impl ClearHandshakeTimeout for rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream> {
    fn clear_handshake_timeout(&mut self) {
        let _ = self
            .sock
            .set_read_timeout(Some(indicatrix_net::framing::IDLE_READ_TIMEOUT));
        let _ = self.sock.set_write_timeout(None);
    }
}

/// Handles one viewer connection end to end on a library-only build (no `worker`
/// feature).
///
/// `HELLO`/`WELCOME` (`WELCOME::render` always `None`, so no build-compatibility check
/// runs), then a loop dispatching `Cancel`/`Library` messages until the peer closes the
/// connection. Only a `HELLO` protocol-version mismatch or a role this viewer port does
/// not serve (claimed, or certified by `cert_role` -- see `super::handshake`) is refused.
/// No radiance ever flows, so `payload_encoding` is always `Raw`.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure.
#[cfg(not(feature = "worker"))]
pub fn handle_connection<S: Read + Write + ClearHandshakeTimeout>(
    mut stream: S,
    db: &LibraryHandle,
    cert_role: Option<PeerRole>,
) -> Result<(), NetError> {
    let check = super::handshake::read_and_check_hello(&mut stream, PeerRole::Viewer, cert_role)?;
    // HELLO has arrived -- the pre-protocol deadline (`serve::HANDSHAKE_TIMEOUT`) has
    // done its job. Swap it for the idle read deadline before anything below relies on it.
    stream.clear_handshake_timeout();

    match check {
        super::handshake::HelloCheck::Accepted(hello) => tracing::debug!(
            "library-only pairing; the viewer's payload encodings {:?} are moot here \
             (no radiance ever flows)",
            hello.accept_encodings
        ),
        super::handshake::HelloCheck::Refused(refusal) => {
            super::handshake::send_refusal(&mut stream, &refusal);
            return Ok(());
        }
    }

    let welcome = Welcome {
        protocol_version: PROTOCOL_VERSION,
        build_hash: indicatrix_net::handshake::UNKNOWN_BUILD_HASH,
        source_hash: indicatrix_net::handshake::UNKNOWN_BUILD_HASH,
        render: None,
        library: true,
        tilt_curves: false,
        registration: None,
        payload_encoding: PayloadEncoding::Raw,
    };
    indicatrix_net::messages::write_message(&mut stream, &welcome)?;

    serve_until_render_request_or_eof(&mut stream, db)?.map_or(Ok(()), |never| match never {})
}
