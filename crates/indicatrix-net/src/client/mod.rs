//! Viewer-side (client) protocol driver for talking to a `indicatrix-worker`.
//!
//! Generic over `Read`/`Write` (never `TcpStream` or a TLS type by name), so the whole
//! client protocol -- handshake, request/cancel framing, accumulation -- is unit
//! testable against an in-memory [`std::io::Cursor`]. Wrapping an actual `TcpStream`
//! (optionally inside `rustls::StreamOwned`) happens at the call site.
//!
//! # Modules
//!
//! - [`handshake`]: `HELLO`/`WELCOME`, the client-side defense-in-depth
//!   [`crate::handshake::verify_compatible`] check, and the "test connection" operation
//!   (handshake only, no render).
//! - [`accumulate`]: [`accumulate::Accumulator`], the epoch-gated radiance sum -- FRAME
//!   deltas sum into it, PREVIEW snapshots replace a display-only slot (never summed),
//!   and anything whose `request_id` doesn't match the current epoch is dropped.
//! - [`session`]: wires a `RenderRequest` write and the reply stream together --
//!   [`session::send_render_request`]/[`session::send_cancel`] for the write side,
//!   [`session::run_client_session`] to drive an [`accumulate::Accumulator`] for as
//!   long as the connection stays open. Also carries
//!   [`session::send_tilt_curves_request`]/[`session::recv_tilt_curves_response`],
//!   deliberately NOT routed through [`session::run_client_session`]/[`Accumulator`]
//!   since `TILT_CURVES` is a single request/response family, not a stream.

pub mod accumulate;
pub mod handshake;
pub mod session;

pub use accumulate::{Accumulator, ApplyOutcome, PreviewSnapshot};
pub use handshake::{ConnectionInfo, test_connection};
pub use session::{SessionUpdate, run_client_session, send_cancel, send_library_request};
#[cfg(feature = "render")]
pub use session::{recv_tilt_curves_response, send_render_request, send_tilt_curves_request};

use crate::messages::{ErrorMsg, NetError};

/// Everything that can go wrong on the client side of a `indicatrix-net` connection, from
/// the initial `HELLO`/`WELCOME` handshake through the render stream itself.
#[derive(Debug)]
pub enum ClientError {
    /// A transport-level failure: a malformed frame, or an I/O error other than a
    /// clean EOF (handled separately, see [`session::run_client_session`]).
    Net(NetError),
    /// The worker itself refused to pair, replying `ERROR` instead of `WELCOME`.
    /// Carries the worker's own `ErrorMsg` verbatim.
    Refused(ErrorMsg),
    /// This client's own defense-in-depth check
    /// ([`crate::handshake::verify_compatible`]) refused pairing even though the
    /// worker replied with a `WELCOME`.
    Incompatible(crate::handshake::Incompatible),
    /// The reply to `HELLO` decoded as neither `WELCOME` nor `ERROR`.
    MalformedHandshakeReply,
    /// A `FRAME`/`PREVIEW` payload failed [`crate::radiance::decode`] -- wrong length
    /// (a worker/viewer dimension mismatch) or misaligned bytes.
    Radiance(crate::radiance::RadianceError),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Net(e) => write!(f, "{e}"),
            Self::Refused(e) => write!(f, "worker refused to pair: {} ({})", e.message, e.code),
            Self::Incompatible(e) => write!(f, "refusing to pair: {e}"),
            Self::MalformedHandshakeReply => {
                write!(f, "handshake reply decoded as neither WELCOME nor ERROR")
            }
            Self::Radiance(e) => write!(f, "malformed radiance payload: {e:?}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<NetError> for ClientError {
    fn from(e: NetError) -> Self {
        Self::Net(e)
    }
}

impl From<crate::radiance::RadianceError> for ClientError {
    fn from(e: crate::radiance::RadianceError) -> Self {
        Self::Radiance(e)
    }
}
