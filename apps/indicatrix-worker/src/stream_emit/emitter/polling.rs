//! Polling the socket for a pending [`indicatrix_net::messages::ClientMessage`] without
//! blocking the emitter's cadence loop: [`ClientPoll`] is what one poll found, and
//! [`poll_for_client_message`] is the poll itself.

use super::super::{TimeoutCache, TimeoutRead};
use indicatrix_net::messages::{ClientMessage, NetError, RenderRequest};
use std::{io::Read, time::Duration};

/// What one poll of the socket for a pending [`ClientMessage`] found.
pub(in crate::stream_emit) enum ClientPoll {
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
    /// currently streaming -- see [`super::run_stream`]'s doc comment for how this is
    /// handled.
    NextRequest(Box<RenderRequest>),
    /// A `PING` (v14) with this nonce: answer with a `PONG` and keep streaming.
    Ping(u64),
}

/// What one [`poll_raw_client_message`] found.
pub enum RawPoll {
    /// Nothing arrived within the poll window.
    Pending,
    /// The peer closed the connection.
    Closed,
    /// One whole, decoded message (an `ASSET`'s payload frame is NOT read yet).
    Message(ClientMessage),
}

/// Reads one pending [`ClientMessage`] frame without blocking past `poll_timeout` before
/// its first byte -- the timeout-tolerant read [`poll_for_client_message`] interprets for
/// a streaming request, shared with `crate::assets`' wait for an asset. See
/// [`poll_for_client_message`]'s doc comment for the first-byte/remainder timeout rules
/// and `timeouts`.
///
/// # Errors
///
/// [`NetError`] for a transport error, an oversized frame, or undecodable bytes.
pub fn poll_raw_client_message<S: Read + TimeoutRead>(
    stream: &mut S,
    poll_timeout: Duration,
    timeouts: &mut TimeoutCache,
) -> Result<RawPoll, NetError> {
    timeouts
        .apply(stream, Some(poll_timeout))
        .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;

    let mut len_bytes = [0u8; indicatrix_net::framing::LEN_PREFIX_BYTES];
    let n = match stream.read(&mut len_bytes) {
        Ok(0) => return Ok(RawPoll::Closed),
        Ok(n) => n,
        Err(e) if super::super::is_stream_timeout(&e) => {
            return Ok(RawPoll::Pending);
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
        .apply(stream, Some(super::super::FRAME_REMAINDER_TIMEOUT))
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
    Ok(RawPoll::Message(postcard::from_bytes(&payload)?))
}

/// Attempts to read one pending message frame, tolerating a `WouldBlock`/`TimedOut`/
/// `WriteZero` `io::Error` as "nothing pending" rather than a fatal error (see
/// [`super::super::is_stream_timeout`] for why `WriteZero` counts here too).
///
/// Reads the length prefix with a single `read` (not `read_exact`) so a timeout can only
/// land before any byte of a new message has arrived. Once a byte is observed, the rest
/// of that message is read under a bounded timeout ([`super::super::FRAME_REMAINDER_TIMEOUT`])
/// rather than an unbounded blocking read; a timeout there is a protocol error tearing
/// down the connection, since a message already in flight never arriving is itself the
/// anomaly.
///
/// # `timeouts` avoids a redundant `set_read_timeout` on every poll
///
/// `timeouts` (see [`TimeoutCache`]) caches the last read-timeout value actually applied
/// to `stream` through it and skips the underlying `set_read_timeout` call when the
/// requested value is unchanged. On an idle streaming connection this function is called
/// every [`super::EMITTER_POLL`] (20ms) and always asks for the same `poll_timeout`, so without
/// this the socket option would be set ~50 times/s for no behavioral effect; the
/// `FRAME_REMAINDER_TIMEOUT` switch after the first byte still goes through normally,
/// since that value genuinely differs from `poll_timeout`. Owned by [`super::run_stream`] for
/// the life of one request, not shared across requests -- see [`TimeoutCache::new`].
///
/// # Pipelining a `RENDER` ahead of `DONE` is supported
///
/// Whatever bytes arrive here while a request is streaming decode as a tagged
/// [`ClientMessage`] (`Cancel` or `RenderRequest`) rather than being assumed to be
/// `Cancel`. [`ClientPoll::NextRequest`] is how a pipelined request surfaces to
/// [`super::run_stream`], which treats it as an implicit cancel of the request currently
/// streaming (matches how the viewer actually behaves, and removes the `DONE` round
/// trip from the drag-to-render responsiveness path).
pub(in crate::stream_emit) fn poll_for_client_message<S: Read + TimeoutRead>(
    stream: &mut S,
    request_id: u32,
    poll_timeout: Duration,
    timeouts: &mut TimeoutCache,
) -> Result<ClientPoll, NetError> {
    let msg = match poll_raw_client_message(stream, poll_timeout, timeouts)? {
        RawPoll::Pending => return Ok(ClientPoll::Pending),
        RawPoll::Closed => return Ok(ClientPoll::Closed),
        RawPoll::Message(msg) => msg,
    };
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
        ClientMessage::Ping { nonce } => ClientPoll::Ping(nonce),
        // Not supported by a plain worker at all (refused with UNSUPPORTED_REQUEST when
        // it arrives between requests); mid-stream it is dropped like a LibraryRequest.
        ClientMessage::FinalImageRequest(_) => {
            tracing::debug!(
                "received a FinalImageRequest while request_id={request_id} was streaming -- \
                 not supported by this server; ignoring"
            );
            ClientPoll::Stale
        }
        // An asset nobody asked for: its payload frame follows and must be
        // consumed, or it would be misread as the next message.
        ClientMessage::Asset(header) => {
            crate::assets::discard_asset(stream, &header)?;
            ClientPoll::Stale
        }
    })
}
