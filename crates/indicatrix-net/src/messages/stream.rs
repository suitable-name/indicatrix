//! Every message a client may send after the handshake ([`ClientMessage`]), and every
//! reply a worker sends for one `RENDER` request ([`StreamEvent`]) -- plus the small
//! per-message structs both are built from, and [`ErrorMsg`], used well beyond just
//! this file (handshake refusal, library-request errors).
//!
//! ```text
//! -> CANCEL   { request_id }
//! <- FRAME    { request_id, first_sample, samples, payload_len, xyz_bytes }   -- DELTA, full-res
//! <- PREVIEW  { request_id, width, height, samples_done, payload_len, xyz_bytes } -- CUMULATIVE, reduced-res
//! <- PROGRESS { request_id, samples_done }
//! <- DONE     { request_id, cancelled, stats: Stats }
//! <- ERROR    { code, message }
//! ```
//!
//! `TILT_CURVES` (see [`super::tilt`]) shares this file's [`ClientMessage`]/[`Cancel`]
//! on the request side but NOT [`StreamEvent`] on the reply side -- it is a single
//! request/response family, not a stream.
//!
//! `FRAME` and `PREVIEW` carry their `xyz_bytes` radiance payload raw, never through
//! `postcard` (see [`crate::radiance`]): each is sent as two consecutive frames, a small
//! `postcard`-encoded header then the raw bytes verbatim, via [`write_frame_message`]/
//! [`read_frame_message`] and [`write_preview_message`]/[`read_preview_message`], all of
//! which cross-check the header's declared `payload_len` against the actual byte count.
//!
//! # `FRAME` (delta, full resolution) vs `PREVIEW` (cumulative, reduced resolution)
//!
//! Not interchangeable -- the distinction is load-bearing for correctness:
//!
//! - `FRAME` carries a DELTA -- the summed contribution of exactly the sample sub-range
//!   named by [`FrameHeader::first_sample`]/[`FrameHeader::samples`], at full
//!   resolution. A viewer SUMS every `FRAME` it receives (whose `request_id` matches its
//!   current epoch) into its running total. This is what makes deltas coalesce
//!   losslessly under backpressure: two adjacent, not-yet-sent deltas sum to one delta
//!   over their union, identical whether sent as one `FRAME` or several.
//! - `PREVIEW` carries a CUMULATIVE, reduced-resolution snapshot -- the full running
//!   total so far, downsampled to [`PreviewHeader::width`] x [`PreviewHeader::height`].
//!   Each `PREVIEW` SUPERSEDES the previous one; a viewer never sums two `PREVIEW`s, and
//!   never sums one into the full-resolution accumulator (a reduced buffer isn't
//!   additive with a full-resolution one). This also makes `PREVIEW` freely droppable
//!   under backpressure: the next one already reflects everything the dropped one did.
//!
//! # `request_id` and cancellation epochs
//!
//! Every message from `RENDER` onward echoes the `request_id` the client chose for that
//! `RENDER`. This makes "never merge a stale partial into the next render" mechanical: a
//! `CANCEL` can be in flight past a worker already mid-batch, so `FRAME`/`PREVIEW`
//! payloads for the just-cancelled request may still arrive after the client moves on.
//! Rule: sum/display a payload iff its `request_id` matches the current epoch, drop
//! everything else -- no connection-drop-and-reconnect required.
//!
//! # [`ClientMessage`]: why every post-handshake client message is tagged
//!
//! v1 had a fixed reply shape and no tag on the request side. That broke once a client
//! could pipeline its next `RenderRequest` without waiting for `DONE` on the one
//! currently streaming: whatever arrives mid-stream could be either a `CANCEL` for it or
//! the next `RenderRequest`. [`ClientMessage`] resolves this the same way [`StreamEvent`]
//! resolves the analogous reply-side ambiguity -- a tag says which, never inferred from
//! position.
//!
//! [`ClientMessage::Library`] extends the same tagged envelope to the read-only
//! design-library protocol ([`crate::library`]), and [`ClientMessage::TiltCurvesRequest`]
//! extends it again to the `TILT_CURVES` sweep family ([`super::tilt`]) -- one message
//! type for a worker's post-handshake read loop to decode regardless of family. Each
//! replies with its own single response, never a [`StreamEvent`].
//!
//! **Variant order is deliberately NOT source order.** [`ClientMessage::Cancel`] and
//! [`ClientMessage::Library`] come first and are ALWAYS compiled in, at the same
//! `postcard` discriminant regardless of this crate's `render` feature; only
//! [`ClientMessage::RenderRequest`] and [`ClientMessage::TiltCurvesRequest`] are
//! `cfg`-gated, both after them. This keeps a library-only worker and a full worker
//! wire-compatible for everything both support: two differently-featured peers always
//! agree on tags 0/1, and only exchange tag 2/3 with a peer whose `WELCOME` already
//! proved it has the matching capability. Any future `cfg`-gated variant must be
//! appended after [`ClientMessage::TiltCurvesRequest`], for the same reason.

use serde::{Deserialize, Serialize};

/// The `postcard`-encoded header half of a `<- FRAME` message: a DELTA, at full
/// resolution.
///
/// See the module docs for the contrast with [`PreviewHeader`] and for why the radiance
/// payload travels as a second, raw (non-`postcard`) frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameHeader {
    pub request_id: u32,
    pub first_sample: u32,
    pub samples: u32,
    pub payload_len: u32,
}

impl FrameHeader {
    /// Builds a header whose `payload_len` is derived from `xyz_bytes`, so it can never
    /// disagree with the payload it's paired with in [`write_frame_message`].
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
        }
    }
}

/// The `postcard`-encoded header half of a `<- PREVIEW` message: a CUMULATIVE,
/// reduced-resolution snapshot -- see the module docs for the contrast with
/// [`FrameHeader`].
///
/// `samples_done` is the whole request's progress so far (not a sub-range), letting a
/// viewer normalize the sum into a displayable average without waiting on `PROGRESS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewHeader {
    pub request_id: u32,
    pub width: u32,
    pub height: u32,
    pub samples_done: u32,
    pub payload_len: u32,
}

impl PreviewHeader {
    /// Builds a header whose `payload_len` is derived from `xyz_bytes`, so it can never
    /// disagree with the payload it's paired with in [`write_preview_message`].
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
        }
    }
}

/// `<- PROGRESS`: a lightweight, cadence-paced progress ping.
///
/// Sent even under `TransferMode::FinalOnly` (where `FRAME` itself only arrives once at
/// the end), so a viewer always has SOME live feedback regardless of transfer mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub request_id: u32,
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
    pub request_id: u32,
}

/// Every message a client may send after the handshake, tagged so a worker always
/// knows which follows -- never left to be inferred from position alone.
///
/// See the module doc comment for why variant ORDER here is load-bearing across
/// differently-`cfg`-feature-flagged builds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
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
    RenderRequest(Box<super::render::RenderRequest>),
    /// `-> TILT_CURVES`: a request for one design's full tilt-performance sweep -- see
    /// [`super::tilt::TiltCurvesRequest`]. Boxed for the same reason `RenderRequest` is.
    ///
    /// `render`-feature only, declared LAST, after `RenderRequest` (module doc comment
    /// explains why a `cfg`-gated variant must always be appended, never inserted).
    #[cfg(feature = "render")]
    TiltCurvesRequest(Box<super::tilt::TiltCurvesRequest>),
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
}

/// `<- DONE`: the terminal message for a `RENDER` request, exactly once.
///
/// Either it finished (`cancelled: false`) or a `CANCEL` was honored (`cancelled: true`,
/// in which case no further `FRAME`/`PREVIEW` payload for this `request_id` follows --
/// an unsent buffer is discarded rather than flushed on cancellation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub request_id: u32,
    pub cancelled: bool,
    pub stats: Stats,
}

/// `<- ERROR`: a worker's rejection of a request (or `HELLO`), with a machine-readable
/// `code` and a human-readable `message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub code: u32,
    pub message: String,
}

/// Every reply a worker sends for one `RENDER` request, from the first one through
/// `Done`/`Error`, tagged so a reader always knows which follows.
///
/// `Frame` and `Preview` carry only the small header here; the raw radiance payload
/// that goes with each follows as a second, separate raw frame -- see
/// [`write_stream_event`]/[`read_stream_event`], the sole way this variant is meant to
/// go over the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEvent {
    Frame(FrameHeader),
    Preview(PreviewHeader),
    /// Doubles as this stream's liveness heartbeat: emitted every cadence tick
    /// regardless of real progress, and the client treats any event (this one included)
    /// as proof of life, dropping the connection after 8s of receiving nothing. See
    /// [`super::PROTOCOL_VERSION`]'s v7 history entry.
    Progress(Progress),
    Done(Done),
    Error(ErrorMsg),
}

/// Writes one [`StreamEvent`] reply.
///
/// `payload` must be `Some` for [`StreamEvent::Frame`]/[`StreamEvent::Preview`] (the raw
/// radiance bytes the header describes) and `None` for every other variant --
/// debug-asserted: a mismatch would silently corrupt the stream for whatever's read next.
///
/// # Errors
///
/// Returns [`super::codec::NetError::Postcard`] or [`super::codec::NetError::Framing`]
/// under the same conditions as [`super::codec::write_message`] (for the event) and
/// [`crate::framing::write_frame`] (for the raw payload, when present).
pub fn write_stream_event<W: std::io::Write>(
    writer: &mut W,
    event: &StreamEvent,
    payload: Option<&[u8]>,
) -> Result<(), super::codec::NetError> {
    debug_assert_eq!(
        matches!(event, StreamEvent::Frame(_) | StreamEvent::Preview(_)),
        payload.is_some(),
        "StreamEvent::Frame/Preview must carry a payload; every other variant must not"
    );
    super::codec::write_message(writer, event)?;
    if let Some(bytes) = payload {
        crate::framing::write_frame(writer, bytes)?;
    }
    Ok(())
}

/// Reads one [`StreamEvent`] reply written by [`write_stream_event`].
///
/// Includes its raw payload frame when the decoded variant is
/// [`StreamEvent::Frame`]/[`StreamEvent::Preview`], validated against that header's
/// declared `payload_len`.
///
/// # Errors
///
/// Returns [`super::codec::NetError::Postcard`] or [`super::codec::NetError::Framing`]
/// under the same conditions as [`super::codec::read_message`] (for the event) and
/// [`crate::framing::read_frame`] (for the raw payload, when present), or
/// [`super::codec::NetError::FramePayloadLenMismatch`] if a `Frame`/`Preview` header's
/// declared `payload_len` disagrees with the raw payload frame's actual length.
pub fn read_stream_event<R: std::io::Read>(
    reader: &mut R,
) -> Result<(StreamEvent, Option<Vec<u8>>), super::codec::NetError> {
    let event: StreamEvent = super::codec::read_message(reader)?;
    let expected_len = match &event {
        StreamEvent::Frame(h) => Some(h.payload_len),
        StreamEvent::Preview(h) => Some(h.payload_len),
        StreamEvent::Progress(_) | StreamEvent::Done(_) | StreamEvent::Error(_) => None,
    };
    let payload = match expected_len {
        Some(expected) => {
            let bytes = crate::framing::read_frame(reader)?;
            if bytes.len() as u32 != expected {
                return Err(super::codec::NetError::FramePayloadLenMismatch {
                    declared: expected,
                    actual: bytes.len(),
                });
            }
            Some(bytes)
        }
        None => None,
    };
    Ok((event, payload))
}

/// Writes a `<- FRAME` message: a `postcard`-encoded [`FrameHeader`] frame, followed by
/// a second frame carrying `xyz_bytes` completely raw.
///
/// `header.payload_len` must equal `xyz_bytes.len()`, asserted by the caller's
/// construction rather than re-derived here -- use [`FrameHeader::for_payload`] to build
/// a consistent pair.
///
/// # Errors
///
/// Returns [`super::codec::NetError::Postcard`] or [`super::codec::NetError::Framing`]
/// under the same conditions as [`super::codec::write_message`] (for the header) and
/// [`crate::framing::write_frame`] (for the raw payload).
pub fn write_frame_message<W: std::io::Write>(
    writer: &mut W,
    header: &FrameHeader,
    xyz_bytes: &[u8],
) -> Result<(), super::codec::NetError> {
    super::codec::write_message(writer, header)?;
    crate::framing::write_frame(writer, xyz_bytes)?;
    Ok(())
}

/// Reads a `<- FRAME` message written by [`write_frame_message`], validating that
/// `payload_len` matches the raw payload frame's actual byte count.
///
/// # Errors
///
/// See [`write_frame_message`]'s errors, plus
/// [`super::codec::NetError::FramePayloadLenMismatch`] if the header's declared
/// `payload_len` disagrees with the raw payload frame's actual length.
pub fn read_frame_message<R: std::io::Read>(
    reader: &mut R,
) -> Result<(FrameHeader, Vec<u8>), super::codec::NetError> {
    let header: FrameHeader = super::codec::read_message(reader)?;
    let payload = crate::framing::read_frame(reader)?;
    if payload.len() as u32 != header.payload_len {
        return Err(super::codec::NetError::FramePayloadLenMismatch {
            declared: header.payload_len,
            actual: payload.len(),
        });
    }
    Ok((header, payload))
}

/// Writes a `<- PREVIEW` message: a `postcard`-encoded [`PreviewHeader`] frame, followed
/// by a second frame carrying `xyz_bytes` completely raw.
///
/// `xyz_bytes` is a reduced-resolution radiance buffer, encoded via
/// [`crate::radiance::encode`]. See the module docs for why a `PREVIEW` payload is
/// CUMULATIVE, never summed the way a `FRAME` payload is.
///
/// # Errors
///
/// See [`write_frame_message`]'s errors.
pub fn write_preview_message<W: std::io::Write>(
    writer: &mut W,
    header: &PreviewHeader,
    xyz_bytes: &[u8],
) -> Result<(), super::codec::NetError> {
    super::codec::write_message(writer, header)?;
    crate::framing::write_frame(writer, xyz_bytes)?;
    Ok(())
}

/// Reads a `<- PREVIEW` message written by [`write_preview_message`].
///
/// # Errors
///
/// See [`read_frame_message`]'s errors.
pub fn read_preview_message<R: std::io::Read>(
    reader: &mut R,
) -> Result<(PreviewHeader, Vec<u8>), super::codec::NetError> {
    let header: PreviewHeader = super::codec::read_message(reader)?;
    let payload = crate::framing::read_frame(reader)?;
    if payload.len() as u32 != header.payload_len {
        return Err(super::codec::NetError::FramePayloadLenMismatch {
            declared: header.payload_len,
            actual: payload.len(),
        });
    }
    Ok((header, payload))
}

#[cfg(test)]
mod tests {
    use super::{
        super::codec::{read_message, write_message},
        *,
    };

    #[test]
    fn error_round_trips() {
        let err = ErrorMsg {
            code: 42,
            message: "scene exceeds max_pixels".to_string(),
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &err).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: ErrorMsg = read_message(&mut cursor).unwrap();
        assert_eq!(err, decoded);
    }

    #[test]
    fn frame_message_round_trips_and_validates_payload_len() {
        let xyz_bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let header = FrameHeader::for_payload(7, 64, 32, &xyz_bytes);
        assert_eq!(header.payload_len, xyz_bytes.len() as u32);
        assert_eq!(header.request_id, 7);

        let mut buf = Vec::new();
        write_frame_message(&mut buf, &header, &xyz_bytes).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let (decoded_header, decoded_bytes) = read_frame_message(&mut cursor).unwrap();
        assert_eq!(decoded_header, header);
        assert_eq!(decoded_bytes, xyz_bytes);
    }

    #[test]
    fn frame_message_rejects_a_forged_payload_len() {
        let xyz_bytes = vec![0u8; 12];
        let lying_header = FrameHeader {
            request_id: 1,
            first_sample: 0,
            samples: 1,
            payload_len: 999,
        };

        let mut buf = Vec::new();
        write_frame_message(&mut buf, &lying_header, &xyz_bytes).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let result = read_frame_message(&mut cursor);
        assert!(matches!(
            result,
            Err(super::super::codec::NetError::FramePayloadLenMismatch {
                declared: 999,
                actual: 12
            })
        ));
    }

    #[test]
    fn preview_message_round_trips_and_validates_payload_len() {
        let xyz_bytes = vec![9u8; 24];
        let header = PreviewHeader::for_payload(7, 4, 2, 128, &xyz_bytes);
        assert_eq!(header.payload_len, xyz_bytes.len() as u32);

        let mut buf = Vec::new();
        write_preview_message(&mut buf, &header, &xyz_bytes).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let (decoded_header, decoded_bytes) = read_preview_message(&mut cursor).unwrap();
        assert_eq!(decoded_header, header);
        assert_eq!(decoded_bytes, xyz_bytes);
    }

    #[test]
    fn preview_message_rejects_a_forged_payload_len() {
        let xyz_bytes = vec![0u8; 24];
        let lying_header = PreviewHeader {
            request_id: 1,
            width: 4,
            height: 2,
            samples_done: 8,
            payload_len: 999,
        };

        let mut buf = Vec::new();
        write_preview_message(&mut buf, &lying_header, &xyz_bytes).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let result = read_preview_message(&mut cursor);
        assert!(matches!(
            result,
            Err(super::super::codec::NetError::FramePayloadLenMismatch {
                declared: 999,
                actual: 24
            })
        ));
    }

    #[test]
    fn progress_round_trips() {
        let progress = Progress {
            request_id: 42,
            samples_done: 128,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &progress).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: Progress = read_message(&mut cursor).unwrap();
        assert_eq!(progress, decoded);
    }

    #[test]
    fn cancel_round_trips() {
        let cancel = Cancel { request_id: 42 };
        let mut buf = Vec::new();
        write_message(&mut buf, &cancel).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: Cancel = read_message(&mut cursor).unwrap();
        assert_eq!(cancel, decoded);
    }

    #[test]
    fn done_round_trips_both_cancelled_states() {
        for cancelled in [false, true] {
            let done = Done {
                request_id: 42,
                cancelled,
                stats: Stats {
                    samples_done: 256,
                    requested_cadence_ms: 250,
                    effective_cadence_ms: 1400,
                },
            };
            let mut buf = Vec::new();
            write_message(&mut buf, &done).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let decoded: Done = read_message(&mut cursor).unwrap();
            assert_eq!(done, decoded);
        }
    }

    #[test]
    fn client_message_cancel_round_trips() {
        let msg = ClientMessage::Cancel(Cancel { request_id: 7 });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: ClientMessage = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    #[cfg(feature = "render")]
    #[test]
    fn client_message_render_request_round_trips() {
        use indicatrix::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };

        let scene = crate::scene::SceneState {
            width: 4,
            height: 4,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
        };
        let msg = ClientMessage::RenderRequest(Box::new(super::super::render::RenderRequest {
            request_id: 8,
            scene,
            first_sample: 0,
            samples: 4,
            stream: super::super::render::StreamConfig {
                transfer_mode: super::super::render::TransferMode::FinalOnly,
                cadence_ms: 100,
                preview: None,
            },
        }));
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: ClientMessage = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    #[cfg(feature = "render")]
    #[test]
    fn client_message_tilt_curves_request_round_trips() {
        use indicatrix::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };

        let scene = crate::scene::SceneState {
            width: 4,
            height: 4,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
        };
        let msg =
            ClientMessage::TiltCurvesRequest(Box::new(super::super::tilt::TiltCurvesRequest {
                request_id: 11,
                scene,
            }));
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: ClientMessage = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    /// Pins [`ClientMessage::Cancel`]/[`ClientMessage::Library`] at `postcard` variant
    /// indices 0/1 -- the two always-compiled-in variants a library-only build and a
    /// full (`render`-feature) build must agree on regardless of which one either peer
    /// was built with (see the module doc comment's "Variant order" section). `postcard`
    /// encodes a variant as a one-byte leading varint for any index below 128, so the
    /// first encoded byte is the declaration index this test pins.
    #[test]
    fn cancel_and_library_keep_postcard_discriminants_0_and_1() {
        let cancel = ClientMessage::Cancel(Cancel { request_id: 1 });
        let cancel_bytes = postcard::to_allocvec(&cancel).unwrap();
        assert_eq!(cancel_bytes[0], 0, "Cancel must stay discriminant 0");

        let library =
            ClientMessage::Library(Box::new(crate::library::LibraryRequest::FilterOptions));
        let library_bytes = postcard::to_allocvec(&library).unwrap();
        assert_eq!(library_bytes[0], 1, "Library must stay discriminant 1");
    }

    /// Pins [`ClientMessage::RenderRequest`]/[`ClientMessage::TiltCurvesRequest`] at
    /// `postcard` variant indices 2/3, both after `Cancel`/`Library` and
    /// `TiltCurvesRequest` after `RenderRequest` -- the order the module doc comment
    /// requires. Only meaningful in a `render`-enabled build.
    #[cfg(feature = "render")]
    #[test]
    fn render_gated_variants_are_appended_in_order_after_cancel_and_library() {
        use indicatrix::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };

        let scene = crate::scene::SceneState {
            width: 1,
            height: 1,
            yaw: 0.0,
            pitch: 0.0,
            distance: 1.0,
            light_yaw: 0.0,
            light_pitch: 0.0,
            exposure: 1.0,
            max_bounces: 1,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
        };

        let render_request =
            ClientMessage::RenderRequest(Box::new(super::super::render::RenderRequest {
                request_id: 1,
                scene: scene.clone(),
                first_sample: 0,
                samples: 1,
                stream: super::super::render::StreamConfig {
                    transfer_mode: super::super::render::TransferMode::FinalOnly,
                    cadence_ms: 100,
                    preview: None,
                },
            }));
        let render_bytes = postcard::to_allocvec(&render_request).unwrap();
        assert_eq!(render_bytes[0], 2, "RenderRequest must stay discriminant 2");

        let tilt_request =
            ClientMessage::TiltCurvesRequest(Box::new(super::super::tilt::TiltCurvesRequest {
                request_id: 1,
                scene,
            }));
        let tilt_bytes = postcard::to_allocvec(&tilt_request).unwrap();
        assert_eq!(
            tilt_bytes[0], 3,
            "TiltCurvesRequest must be discriminant 3 -- appended after RenderRequest"
        );
    }

    #[test]
    fn stream_event_frame_and_preview_round_trip_with_their_payload() {
        let xyz_bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let frame_header = FrameHeader::for_payload(7, 0, 4, &xyz_bytes);
        let event = StreamEvent::Frame(frame_header);
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, Some(&xyz_bytes)).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let (decoded_event, decoded_payload) = read_stream_event(&mut cursor).unwrap();
        assert_eq!(decoded_event, event);
        assert_eq!(decoded_payload, Some(xyz_bytes.clone()));

        let preview_header = PreviewHeader::for_payload(7, 2, 2, 16, &xyz_bytes);
        let event = StreamEvent::Preview(preview_header);
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, Some(&xyz_bytes)).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let (decoded_event, decoded_payload) = read_stream_event(&mut cursor).unwrap();
        assert_eq!(decoded_event, event);
        assert_eq!(decoded_payload, Some(xyz_bytes));
    }

    #[test]
    fn stream_event_progress_done_and_error_round_trip_with_no_payload() {
        for event in [
            StreamEvent::Progress(Progress {
                request_id: 7,
                samples_done: 64,
            }),
            StreamEvent::Done(Done {
                request_id: 7,
                cancelled: false,
                stats: Stats {
                    samples_done: 64,
                    requested_cadence_ms: 250,
                    effective_cadence_ms: 300,
                },
            }),
            StreamEvent::Error(ErrorMsg {
                code: 3,
                message: "internal error while tracing this request".to_string(),
            }),
        ] {
            let mut buf = Vec::new();
            write_stream_event(&mut buf, &event, None).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let (decoded_event, decoded_payload) = read_stream_event(&mut cursor).unwrap();
            assert_eq!(decoded_event, event);
            assert_eq!(decoded_payload, None);
        }
    }

    #[test]
    fn stream_event_a_sequence_reads_back_in_order() {
        // Two FRAMEs, a PROGRESS, then DONE -- an interleaved reply sequence.
        let bytes_a = vec![0u8; 12];
        let bytes_b = vec![1u8; 12];
        let mut buf = Vec::new();
        write_stream_event(
            &mut buf,
            &StreamEvent::Frame(FrameHeader::for_payload(1, 0, 4, &bytes_a)),
            Some(&bytes_a),
        )
        .unwrap();
        write_stream_event(
            &mut buf,
            &StreamEvent::Frame(FrameHeader::for_payload(1, 4, 4, &bytes_b)),
            Some(&bytes_b),
        )
        .unwrap();
        write_stream_event(
            &mut buf,
            &StreamEvent::Progress(Progress {
                request_id: 1,
                samples_done: 8,
            }),
            None,
        )
        .unwrap();
        write_stream_event(
            &mut buf,
            &StreamEvent::Done(Done {
                request_id: 1,
                cancelled: false,
                stats: Stats {
                    samples_done: 8,
                    requested_cadence_ms: 0,
                    effective_cadence_ms: 0,
                },
            }),
            None,
        )
        .unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let mut events = Vec::new();
        loop {
            let (event, _payload) = read_stream_event(&mut cursor).unwrap();
            let done = matches!(event, StreamEvent::Done(_));
            events.push(event);
            if done {
                break;
            }
        }
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], StreamEvent::Frame(_)));
        assert!(matches!(events[1], StreamEvent::Frame(_)));
        assert!(matches!(events[2], StreamEvent::Progress(_)));
        assert!(matches!(events[3], StreamEvent::Done(_)));
    }
}
