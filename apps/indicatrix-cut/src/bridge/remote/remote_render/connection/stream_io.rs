//! Reading length-prefixed stream-event frames off a connection with a bounded,
//! timeout-tolerant poll, and translating a decoded [`StreamEvent`] plus its
//! [`ApplyOutcome`] into the [`RemoteUpdate`] the GUI actually consumes.

use super::super::types::{RemoteStream, RemoteUpdate, StreamEventFrame};
use indicatrix_net::{
    client::ApplyOutcome,
    framing::{self, LEN_PREFIX_BYTES, MAX_FRAME_LEN},
    messages::{ErrorMsg, NetError, StreamEvent, error_codes},
};
use std::io::Read;

/// Translates one applied [`StreamEvent`]/[`ApplyOutcome`] pair into the
/// [`RemoteUpdate`] the GUI actually consumes -- shared by `super::one_shot::run`'s
/// inner loop and `super::persistent`'s `route_event`, so a one-shot and a persistent
/// connection report identically. `None` for [`ApplyOutcome::StaleDropped`]: a stale
/// reply is silently dropped, never surfaced as an update of any kind.
pub(super) fn to_remote_update(
    request_id: u32,
    event: &StreamEvent,
    outcome: ApplyOutcome,
) -> Option<RemoteUpdate> {
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
            Some(worker_error_update(request_id, e))
        }
        ApplyOutcome::DisplayFrameReplaced => {
            let StreamEvent::DisplayFrame(h) = event else {
                unreachable!("DisplayFrameReplaced only comes from a DisplayFrame event")
            };
            Some(RemoteUpdate::DisplayFrame {
                request_id,
                samples_done: h.samples_done,
            })
        }
        ApplyOutcome::FinalImageReceived => Some(RemoteUpdate::FinalImage { request_id }),
        ApplyOutcome::CapabilityChanged => {
            let StreamEvent::CapabilityChanged { render } = event else {
                unreachable!("CapabilityChanged only comes from a CapabilityChanged event")
            };
            Some(RemoteUpdate::CapabilityChanged {
                request_id,
                render: render.clone(),
            })
        }
        // No `PING` is ever sent, so a `PONG` is unsolicited; a stale event is dropped.
        // `NEED_ASSET` is answered by the connection loop itself before it gets here
        // (`super::asset_upload`), never surfaced as an update.
        ApplyOutcome::StaleDropped | ApplyOutcome::Pong { .. } | ApplyOutcome::NeedAsset { .. } => {
            None
        }
    }
}

/// The user-facing note for a coordinator's `ALL_WORKERS_LOST` stream error.
const ALL_WORKERS_LOST_NOTE: &str = "All remote workers were lost";

/// Translates a server `ERROR` into the terminal update the GUI acts on:
/// `UNSUPPORTED_REQUEST` (a plain worker asked for a v14 coordinator feature) becomes
/// [`RemoteUpdate::Unsupported`] so callers fall back instead of failing;
/// `ALL_WORKERS_LOST` becomes a [`RemoteUpdate::Failed`] with a clear note (the
/// ordinary remote-failure/fallback paths then apply); anything else is `Failed` with
/// the server's own message.
pub(super) fn worker_error_update(request_id: u32, error: &ErrorMsg) -> RemoteUpdate {
    match error.code {
        error_codes::UNSUPPORTED_REQUEST => RemoteUpdate::Unsupported {
            request_id,
            message: error.message.clone(),
        },
        error_codes::ALL_WORKERS_LOST => RemoteUpdate::Failed {
            request_id,
            message: if error.message.is_empty() {
                ALL_WORKERS_LOST_NOTE.to_string()
            } else {
                format!("{ALL_WORKERS_LOST_NOTE} ({})", error.message)
            },
        },
        _ => RemoteUpdate::Failed {
            request_id,
            message: error.message.clone(),
        },
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
/// plus the whole payload) is read under a bounded [`super::FRAME_REMAINDER_TIMEOUT`]
/// rather than an unbounded blocking read -- a timeout there is a protocol error
/// (surfaced as an `Err`, not `Ok(None)`): a frame already in flight never finishing
/// arriving is itself the anomaly, not "nothing new yet". See
/// [`super::FRAME_REMAINDER_TIMEOUT`]'s own doc comment for the hang this closes.
fn try_read_one_frame(
    stream: &mut RemoteStream,
    poll_timeout: std::time::Duration,
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
        .set_read_timeout(Some(super::FRAME_REMAINDER_TIMEOUT))
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
/// `super::one_shot::run`'s loop interleave checking for a `RemoteCommand` between read
/// attempts without a second thread ever touching `stream`. See the connection
/// module's own doc comment.
pub(super) fn try_read_stream_event(
    stream: &mut RemoteStream,
    poll_timeout: std::time::Duration,
) -> Result<Option<StreamEventFrame>, NetError> {
    let Some(header_bytes) = try_read_one_frame(stream, poll_timeout)? else {
        return Ok(None);
    };
    let event: StreamEvent = postcard::from_bytes(&header_bytes)?;
    let payload = match event.payload_len() {
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
