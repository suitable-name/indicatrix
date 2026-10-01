//! The data types: per-message header/payload structs, [`ClientMessage`], and
//! [`StreamEvent`]. Wire encode/decode for these lives in [`super::wire`].

use crate::{
    messages::{
        encoding::{DisplayEncoding, PayloadEncoding},
        hello::RenderCapability,
    },
    radiance::EncodedPayload,
};
use serde::{Deserialize, Serialize};

/// The `postcard`-encoded header half of a `<- FRAME` message: a DELTA, at full
/// resolution.
///
/// See the module docs for the contrast with [`PreviewHeader`] and for why the radiance
/// payload travels as a second, raw (non-`postcard`) frame.
///
/// # Which samples a FRAME carries
///
/// `samples` is EXACT: the delta is the sum of exactly that many samples per pixel, and
/// a client adds exactly that to its count. The samples all lie inside the request's
/// `[first_sample, first_sample + samples)` range. A plain worker's frames are contiguous
/// sub-ranges starting at `first_sample`; a coordinator's frame is a SET of samples
/// gathered from several lanes (not necessarily contiguous), and it reports
/// `first_sample = request.first_sample`. A client therefore checks containment of
/// `[first_sample, first_sample + samples)` in the request range, never contiguity
/// between frames (see `crate::client::accumulate`).
///
/// # Encoding (v14)
///
/// `payload_len` is the ON-WIRE size of the payload frame that follows (what
/// `framing::MAX_FRAME_LEN` bounds); `raw_len` is the decoded size, which must equal
/// `width * height * 12` for the receiver's own frame size -- see
/// `crate::radiance::payload`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameHeader {
    /// The request this delta belongs to.
    pub request_id: u32,
    /// See the "Which samples" section: the start of the carried range (plain worker)
    /// or the request's own `first_sample` (coordinator).
    pub first_sample: u32,
    /// Exact number of samples summed into this delta.
    pub samples: u32,
    /// Size in bytes of the payload frame that follows, as sent.
    pub payload_len: u32,
    /// How the payload is encoded (v14).
    pub encoding: PayloadEncoding,
    /// Size in bytes of the payload once decoded (v14).
    pub raw_len: u32,
}

impl FrameHeader {
    /// Builds a header for a RAW payload whose `payload_len`/`raw_len` are derived from
    /// `xyz_bytes`, so they can never disagree with the payload it's paired with in
    /// [`super::wire::write_frame_message`].
    #[must_use]
    pub const fn for_payload(
        request_id: u32,
        first_sample: u32,
        samples: u32,
        xyz_bytes: &[u8],
    ) -> Self {
        Self {
            request_id,
            first_sample,
            samples,
            payload_len: xyz_bytes.len() as u32,
            encoding: PayloadEncoding::Raw,
            raw_len: xyz_bytes.len() as u32,
        }
    }

    /// Builds a header for an already-encoded payload (see
    /// [`crate::radiance::payload::PayloadEncoder::encode`]).
    #[must_use]
    pub const fn for_encoded(
        request_id: u32,
        first_sample: u32,
        samples: u32,
        encoded: &EncodedPayload<'_>,
    ) -> Self {
        Self {
            request_id,
            first_sample,
            samples,
            payload_len: encoded.bytes.len() as u32,
            encoding: encoded.encoding,
            raw_len: encoded.raw_len,
        }
    }
}

/// The `postcard`-encoded header half of a `<- PREVIEW` message: a CUMULATIVE,
/// reduced-resolution snapshot -- see the module docs for the contrast with
/// [`FrameHeader`].
///
/// `samples_done` is the whole request's progress so far (not a sub-range), letting a
/// viewer normalize the sum into a displayable average without waiting on `PROGRESS`.
/// `payload_len`/`encoding`/`raw_len` follow [`FrameHeader`]'s rules, with `raw_len`
/// checked against this header's own `width * height * 12`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewHeader {
    /// The request this snapshot belongs to.
    pub request_id: u32,
    /// Preview width in pixels.
    pub width: u32,
    /// Preview height in pixels.
    pub height: u32,
    /// Samples folded into the snapshot so far.
    pub samples_done: u32,
    /// Size in bytes of the payload frame that follows, as sent.
    pub payload_len: u32,
    /// How the payload is encoded (v14).
    pub encoding: PayloadEncoding,
    /// Size in bytes of the payload once decoded (v14).
    pub raw_len: u32,
}

impl PreviewHeader {
    /// Builds a header for a RAW payload whose `payload_len`/`raw_len` are derived from
    /// `xyz_bytes`, so they can never disagree with the payload it's paired with in
    /// [`super::wire::write_preview_message`].
    #[must_use]
    pub const fn for_payload(
        request_id: u32,
        width: u32,
        height: u32,
        samples_done: u32,
        xyz_bytes: &[u8],
    ) -> Self {
        Self {
            request_id,
            width,
            height,
            samples_done,
            payload_len: xyz_bytes.len() as u32,
            encoding: PayloadEncoding::Raw,
            raw_len: xyz_bytes.len() as u32,
        }
    }

    /// Builds a header for an already-encoded payload, like
    /// [`FrameHeader::for_encoded`].
    #[must_use]
    pub const fn for_encoded(
        request_id: u32,
        width: u32,
        height: u32,
        samples_done: u32,
        encoded: &EncodedPayload<'_>,
    ) -> Self {
        Self {
            request_id,
            width,
            height,
            samples_done,
            payload_len: encoded.bytes.len() as u32,
            encoding: encoded.encoding,
            raw_len: encoded.raw_len,
        }
    }
}

/// The header half of a `<- DISPLAY_FRAME` (v14, `TransferMode::DisplayOnly`).
///
/// A tone-mapped, denoised 8-bit picture of the whole request so far. Like `PREVIEW` it
/// is CUMULATIVE and REPLACES the previous one; unlike `PREVIEW` it is never merged with
/// anything (8-bit pixels are not additive). The payload (`payload_len` bytes, encoded
/// per `encoding`) follows as a raw frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayFrameHeader {
    /// The request this picture belongs to.
    pub request_id: u32,
    /// Samples folded into the picture so far.
    pub samples_done: u32,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// How the RGBA8 payload is encoded.
    pub encoding: DisplayEncoding,
    /// Size in bytes of the payload frame that follows, as sent.
    pub payload_len: u32,
}

/// The header half of a `<- FINAL_IMAGE` (v14, the reply to a `FinalImageRequest`).
///
/// The finished picture, sent once, followed by `DONE`. The payload follows as a raw
/// frame; for `FinalOutput::PngRgba8` it is a PNG (`encoding == DisplayEncoding::Png`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalImageHeader {
    /// The request this picture answers.
    pub request_id: u32,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// Samples the picture was averaged over (the request's `samples` on success).
    pub samples_done: u32,
    /// How the payload is encoded.
    pub encoding: DisplayEncoding,
    /// Size in bytes of the payload frame that follows, as sent.
    pub payload_len: u32,
}

/// `<- PROGRESS`: a lightweight, cadence-paced progress ping.
///
/// Sent even under `TransferMode::FinalOnly` (where `FRAME` itself only arrives once at
/// the end), so a viewer always has SOME live feedback regardless of transfer mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The request this progress belongs to.
    pub request_id: u32,
    /// Samples traced so far.
    pub samples_done: u32,
}

/// `-> CANCEL`: asks the worker to stop tracing `request_id` as soon as possible,
/// without dropping the connection.
///
/// A connection drop would force a full mutual-TLS re-handshake (plus `HELLO`/`WELCOME`
/// re-verification) on the very next request -- unacceptable when the triggering
/// gesture is camera manipulation, which can fire several times a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancel {
    /// The request to cancel.
    pub request_id: u32,
}

/// Every message a client may send after the handshake, tagged so a worker always
/// knows which follows -- never left to be inferred from position alone.
///
/// See the module doc comment for why variant ORDER here is load-bearing across
/// differently-`cfg`-feature-flagged builds: every variant after `Library` is
/// `render`-gated and appended in order (indices 2, 3, 4, 5, 6, 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// `-> CANCEL` for one request.
    Cancel(Cancel),
    /// A read-only design-library request -- see [`crate::library::LibraryRequest`].
    /// Always available, regardless of this crate's `render` feature.
    Library(Box<crate::library::LibraryRequest>),
    /// Boxed to keep this enum's stack footprint close to `Cancel`'s tiny one rather
    /// than every `ClientMessage` paying for `RenderRequest`'s much larger `SceneState`.
    ///
    /// `render`-feature only, declared before [`Self::TiltCurvesRequest`] (the module
    /// doc comment explains the ordering).
    #[cfg(feature = "render")]
    RenderRequest(Box<crate::messages::render::RenderRequest>),
    /// `-> TILT_CURVES`: a request for one design's full tilt-performance sweep -- see
    /// [`crate::messages::tilt::TiltCurvesRequest`]. Boxed for the same reason
    /// `RenderRequest` is.
    ///
    /// `render`-feature only, appended after `RenderRequest`.
    #[cfg(feature = "render")]
    TiltCurvesRequest(Box<crate::messages::tilt::TiltCurvesRequest>),
    /// `-> PING` (v14): a liveness probe on an otherwise idle connection (a coordinator
    /// pings each joined worker). The server answers
    /// [`StreamEvent::Pong`] with the same `nonce` as soon as it reads it -- also while a
    /// request is streaming, interleaved with that stream's events. `render`-feature
    /// only (index 4): only render-capable peers join a coordinator.
    #[cfg(feature = "render")]
    Ping {
        /// Opaque value echoed in the `PONG`.
        nonce: u64,
    },
    /// `-> FINAL_IMAGE_REQUEST` (v14): render and tone-map on the server, reply with one
    /// PNG -- see [`crate::messages::final_image`]. Boxed like `RenderRequest`.
    /// `render`-feature only (index 5).
    #[cfg(feature = "render")]
    FinalImageRequest(Box<crate::messages::final_image::FinalImageRequest>),
    /// `-> ASSET` (v14): the bytes a server asked for with
    /// [`StreamEvent::NeedAsset`] -- this header, then the raw bytes as one separate frame
    /// of exactly `len` bytes (see [`crate::messages::asset`]). `render`-feature only
    /// (index 6), like every render-side variant.
    #[cfg(feature = "render")]
    Asset(crate::messages::asset::AssetHeader),
    /// `-> CONTRIBUTION` (v16): the viewer's own share of a `FinalImageRequest`'s
    /// reserved tail -- this header, then one raw payload frame (see
    /// [`crate::messages::contribution`]). `render`-feature only (index 7).
    #[cfg(feature = "render")]
    Contribution(crate::messages::contribution::ContributionHeader),
}

/// Delivery statistics reported on [`Done`].
///
/// Lets a viewer surface something like "requested 250ms, delivering ~1.4s --
/// link-limited" rather than leaving a user confused about why updates feel slower than
/// what they asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Total samples actually traced before this request ended (finished or cancelled).
    pub samples_done: u32,
    /// Echoes the request's own `StreamConfig::cadence_ms`, so a viewer doesn't need to
    /// have kept its own request around to compare against.
    pub requested_cadence_ms: u32,
    /// Actual average interval between emissions over the life of this request, in
    /// milliseconds. Larger than `requested_cadence_ms` when the link or hardware
    /// couldn't sustain the requested cadence (deltas coalesced under backpressure); `0`
    /// if this request never emitted more than once.
    pub effective_cadence_ms: u32,
    /// v16: samples of a `FinalImageRequest`'s viewer-reserved range this server
    /// rendered ITSELF because the `CONTRIBUTION` did not arrive in time (or was
    /// invalid). `0` otherwise, and for every `RENDER`.
    #[serde(default)]
    pub reclaimed_samples: u32,
}

/// `<- DONE`: the terminal message for a `RENDER` request, exactly once.
///
/// Either it finished (`cancelled: false`) or a `CANCEL` was honored (`cancelled: true`,
/// in which case no further `FRAME`/`PREVIEW` payload for this `request_id` follows --
/// an unsent buffer is discarded rather than flushed on cancellation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// The request that ended.
    pub request_id: u32,
    /// Whether it ended because of a `CANCEL` (or a pipelined replacement).
    pub cancelled: bool,
    /// Delivery statistics.
    pub stats: Stats,
}

/// `<- ERROR`: a worker's rejection of a request (or `HELLO`), with a machine-readable
/// `code` (see [`crate::messages::error_codes`]) and a human-readable `message`.
///
/// # `request_id` (v15)
///
/// `Some(id)` when this refusal or failure concerns one specific request -- everything
/// from `serve::connection::requests`'s `VALIDATION_FAILED`/`TRACE_PANIC` refusals
/// through the emitter's own `StreamOutcome::Failed` -- so [`crate::client::accumulate::Accumulator::apply`]
/// can drop it exactly like any other stale `FRAME`/`PREVIEW`/`DONE` when it arrives for
/// an epoch the client has already moved on from (see that module's doc comment: a
/// `CANCEL` can be in flight past a worker that's already mid-batch). `None` for a
/// refusal that precedes any request at all -- a `HELLO`-phase `BUILD_MISMATCH`/
/// `ROLE_REFUSED`, [`super::super::connection::refuse_for_capacity`]'s
/// `CONNECTION_LIMIT_REACHED`, or a `LibraryRequest` failure (the library protocol has no
/// epoch to be stale against) -- which is never epoch-gated and always reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMsg {
    /// One of [`crate::messages::error_codes`].
    pub code: u32,
    /// Human-readable detail.
    pub message: String,
    /// The request this error concerns, when it concerns exactly one -- see the struct
    /// doc comment. New in [`crate::messages::PROTOCOL_VERSION`] v15 (this field did not
    /// exist on the wire before); `#[serde(default)]` is cosmetic here, not a
    /// cross-version compatibility mechanism -- `postcard` decodes struct fields
    /// positionally, so a v14 peer's two-field `ErrorMsg` is never handed to a v15
    /// decoder in the first place: the `HELLO` gate already refuses a
    /// `protocol_version` mismatch before any message shaped by it crosses the wire.
    #[serde(default)]
    pub request_id: Option<u32>,
}

/// Every reply a worker sends after the handshake, tagged so a reader always knows which
/// follows.
///
/// That is one `RENDER` request's events from the first one through `Done`/`Error`,
/// plus the v14 connection-level events (`Pong`, `CapabilityChanged`) and the 8-bit
/// picture events.
///
/// `Frame`, `Preview`, `DisplayFrame` and `FinalImage` carry only the small header
/// here; the payload that goes with each follows as a second, separate raw frame -- see
/// [`super::wire::write_stream_event`]/[`super::wire::read_stream_event`], the sole way
/// these variants are meant to go over the wire, and [`Self::payload_len`]. Variant
/// order is wire-load-bearing; append only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEvent {
    /// A full-resolution radiance delta (sum it) -- see [`FrameHeader`].
    Frame(FrameHeader),
    /// A cumulative reduced-resolution snapshot (replace, never sum).
    Preview(PreviewHeader),
    /// Doubles as this stream's liveness heartbeat: emitted every cadence tick
    /// regardless of real progress, and the client treats any event (this one included)
    /// as proof of life, dropping the connection after 8s of receiving nothing. See the
    /// heartbeat note on [`crate::messages::PROTOCOL_VERSION`].
    Progress(Progress),
    /// The terminal event of a request, exactly once.
    Done(Done),
    /// A rejection or failure. Epoch-gated exactly like `Frame`/`Preview`/`Done` when it
    /// carries a `request_id` (v15, see [`ErrorMsg`]); a connection-level refusal with no
    /// `request_id` (e.g. a `HELLO`-phase refusal) is never gated and always reported.
    Error(ErrorMsg),
    /// `<- PONG` (v14): the answer to `ClientMessage::Ping`, echoing its `nonce`. Not
    /// tied to any request; never epoch-gated.
    Pong {
        /// The nonce from the `PING` being answered.
        nonce: u64,
    },
    /// `<- DISPLAY_FRAME` (v14): a tone-mapped 8-bit picture, `TransferMode::DisplayOnly`
    /// only -- see [`DisplayFrameHeader`].
    DisplayFrame(DisplayFrameHeader),
    /// `<- FINAL_IMAGE` (v14): the reply picture to a `FinalImageRequest` -- see
    /// [`FinalImageHeader`] and [`crate::messages::final_image`].
    FinalImage(FinalImageHeader),
    /// `<- CAPABILITY_CHANGED` (v14): the server's render capacity changed since
    /// `WELCOME` (a coordinator gained its first joined worker, or lost its last). Sent
    /// between requests, never inside one. Not epoch-gated. `render` is what
    /// `WELCOME.render` would say now.
    CapabilityChanged {
        /// The server's current render capability.
        render: Option<RenderCapability>,
    },
    /// `<- NEED_ASSET` (v14): the current request's scene names an asset (an HDR
    /// map) this server does not have; the client answers with `ClientMessage::Asset`
    /// carrying exactly those bytes, and the request then proceeds (or ends with
    /// `ERROR(ASSET_FAILED)`). Sent at most once per asset per request, before any other
    /// event of that request. Carries no `request_id` (it always concerns the request
    /// being started); never epoch-gated.
    NeedAsset {
        /// SHA-256 of the wanted bytes (see `crate::messages::asset`).
        content_hash: [u8; 32],
    },
}

impl StreamEvent {
    /// The declared size of the raw payload frame that follows this event on the wire,
    /// or `None` for a variant that carries no payload.
    #[must_use]
    pub const fn payload_len(&self) -> Option<u32> {
        match self {
            Self::Frame(h) => Some(h.payload_len),
            Self::Preview(h) => Some(h.payload_len),
            Self::DisplayFrame(h) => Some(h.payload_len),
            Self::FinalImage(h) => Some(h.payload_len),
            Self::Progress(_)
            | Self::Done(_)
            | Self::Error(_)
            | Self::Pong { .. }
            | Self::CapabilityChanged { .. }
            | Self::NeedAsset { .. } => None,
        }
    }

    /// The `request_id` this event belongs to, or `None` for a request-less event
    /// (`Pong`, `CapabilityChanged`, `NeedAsset`, or an `Error` that doesn't concern one
    /// specific request -- see [`ErrorMsg`]'s doc comment).
    #[must_use]
    pub const fn request_id(&self) -> Option<u32> {
        match self {
            Self::Frame(h) => Some(h.request_id),
            Self::Preview(h) => Some(h.request_id),
            Self::Progress(p) => Some(p.request_id),
            Self::Done(d) => Some(d.request_id),
            Self::DisplayFrame(h) => Some(h.request_id),
            Self::FinalImage(h) => Some(h.request_id),
            Self::Error(e) => e.request_id,
            Self::Pong { .. } | Self::CapabilityChanged { .. } | Self::NeedAsset { .. } => None,
        }
    }
}
