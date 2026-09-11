//! The wire messages, and encode/decode for each over a length-prefixed
//! [`crate::framing`] stream.
//!
//! ```text
//! -> HELLO    { protocol_version, build_hash }
//! <- WELCOME  { protocol_version, build_hash, render: Option<RenderCapability>, library, tilt_curves }
//! -> <ClientMessage>  Cancel | Library(LibraryRequest) | RenderRequest | TiltCurvesRequest  (post-handshake, tagged)
//! <- <StreamEvent>    Frame | Preview | Progress | Done | Error          (RENDER replies, tagged)
//! <- <TiltCurvesResponse>  Curves | Cancelled | Error   (TILT_CURVES's single reply, not a stream)
//! ```
//!
//! Split across submodules by topic:
//!
//! - [`hello`]: handshake messages, `HELLO`/`WELCOME`.
//! - [`render`]: `RENDER`'s request shape (`render` feature only).
//! - [`tilt`]: `TILT_CURVES`'s request/response shapes (`render` feature only).
//! - [`stream`]: everything else post-handshake -- [`stream::ClientMessage`],
//!   [`stream::StreamEvent`], and [`stream::ErrorMsg`].
//! - [`codec`]: the generic framed codec ([`codec::NetError`],
//!   [`codec::write_message`]/[`codec::read_message`]) every one of the above builds on.
//!
//! Every item is re-exported here at its original flat `messages::` path.

mod codec;
mod hello;
#[cfg(feature = "render")]
mod render;
mod stream;
#[cfg(feature = "render")]
mod tilt;

pub use codec::{NetError, read_message, write_message};
pub use hello::{Backend, Hello, RenderCapability, Welcome};
#[cfg(feature = "render")]
pub use render::{PreviewConfig, RenderRequest, StreamConfig, TransferMode};
pub use stream::{
    Cancel, ClientMessage, Done, ErrorMsg, FrameHeader, PreviewHeader, Progress, Stats,
    StreamEvent, read_frame_message, read_preview_message, read_stream_event, write_frame_message,
    write_preview_message, write_stream_event,
};
#[cfg(feature = "render")]
pub use tilt::{
    AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS, TiltCurvesRequest,
    TiltCurvesResponse, TiltCurvesResult,
};

/// The wire protocol version this build of `indicatrix-net` speaks.
///
/// Bumped only for changes to the MESSAGE SHAPES in this module. A change to `indicatrix`'s
/// physics does not touch the wire format and is caught instead by the build-hash check
/// in [`crate::handshake`].
///
/// This is pre-release software: every peer runs its own private build, so there is no
/// deployed fleet to stay wire-compatible with, and this crate carries no compatibility
/// shim or version-conditional decode path -- [`crate::handshake::verify_compatible`]
/// simply refuses to pair peers speaking different versions. The bump's job is making
/// that refusal a clear, diagnosable handshake failure instead of a peer silently
/// mis-decoding garbage.
///
/// Note: [`StreamEvent::Progress`] doubles as the stream's liveness heartbeat, not just
/// real sample progress -- a peer must not assume otherwise.
///
/// # When you MUST bump this
///
/// The wire codec is [`postcard`], which is **not self-describing**:
///
/// - **Enums are encoded by declaration-order index, not name.** An older peer sees an
///   unrecognized index on an appended variant and fails outright -- it cannot skip it.
/// - **Structs are encoded as fields in declaration order, no names, no length prefix.**
///   Appending a field is worse: an older peer silently MISALIGNS every field after it.
///   Applies transitively to anything a wire struct embeds (e.g.
///   [`crate::scene::SceneState`]'s `GemMaterial`, inside [`RenderRequest`]).
///
/// So: append a variant to any wire enum, or a field to any wire struct (or anything it
/// embeds), and bump this -- turning silent corruption into an up-front, diagnosable
/// version-mismatch log line.
///
/// **`#[serde(default)]` does not help here**: it matters only for self-describing
/// formats (e.g. `indicatrix-worker`'s local `scene.json`), not postcard's fixed-layout
/// encoding.
///
/// 9: three lighting models appended to `LightingPreset`.
pub const PROTOCOL_VERSION: u16 = 9;

#[cfg(test)]
mod tests {
    #[test]
    /// Pins the constant so a bump is always a deliberate, reviewed edit.
    ///
    /// 9: three lighting models appended to `LightingPreset`.
    fn protocol_version_matches_constant() {
        assert_eq!(super::PROTOCOL_VERSION, 9);
    }
}
