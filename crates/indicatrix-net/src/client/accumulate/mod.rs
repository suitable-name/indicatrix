//! [`Accumulator`]: the epoch-gated radiance sum a viewer keeps for one connection's
//! worth of remote rendering.
//!
//! # The invariant this exists to enforce
//!
//! A `CANCEL` can be in flight past a worker that's already mid-batch, so
//! `FRAME`/`PREVIEW`/`DONE` payloads for a just-cancelled request may still arrive after
//! the client has moved on to a new `request_id`. The rule that makes this safe is
//! mechanical and applies uniformly to every message: **honor/sum/display a payload iff
//! its `request_id` matches the accumulator's CURRENT epoch; drop everything else.**
//! [`Accumulator::apply`] is the one place a `request_id` is ever compared; every other
//! piece of client code goes through it rather than re-implementing the check.
//!
//! # `FRAME` vs `PREVIEW`, mirrored from `crate::messages`
//!
//! `FRAME` is a full-resolution DELTA: [`Accumulator::apply`] sums it straight into
//! [`Accumulator::buffer`]. `PREVIEW` is a cumulative, reduced-resolution snapshot: it
//! REPLACES [`Accumulator::last_preview`] rather than being summed into anything --
//! summing a reduced-resolution buffer into a full-resolution one isn't even
//! dimensionally sound. See `crate::messages`'s own module docs for the full argument.
//!
//! # Payload encodings (v14)
//!
//! A `FRAME`/`PREVIEW` payload is decoded according to its OWN header's `encoding`
//! (never the handshake's negotiated one), through the accumulator's reusable
//! [`PayloadDecoder`], with every bounded-decode check `crate::radiance::payload`
//! documents: `raw_len` must equal `width * height * 12` for this accumulator's frame
//! size (a `FRAME`) or the header's own size (a `PREVIEW`), decompression writes into
//! exactly `raw_len` bytes, and short or oversized output is an error. A compressed
//! delta sums bit-identically to the same delta sent raw.
//!
//! The v14 picture events (`DISPLAY_FRAME`, `FINAL_IMAGE`) are epoch-gated the same way
//! and kept ENCODED (see [`PictureSnapshot`]); decode them with `crate::display` when
//! they are shown or saved. `PONG` and `CAPABILITY_CHANGED` carry no `request_id` and are
//! reported regardless of the epoch, like `ERROR`.
//!
//! # Expected sample range: containment, not contiguity
//!
//! A caller that knows which absolute sample range it asked for can start the epoch
//! with [`Accumulator::begin_request_for_range`]. From then on every `FRAME` whose
//! `[first_sample, first_sample + samples)` does not lie inside that range is a
//! protocol bug somewhere (a worker tracing indices it was never assigned would
//! silently duplicate samples another backend also traces, biasing the merged
//! average). Such a frame trips a `debug_assert!` in debug builds and is dropped as
//! [`ApplyOutcome::StaleDropped`] (with a `tracing::warn!`) in release builds, so a
//! misbehaving peer can never sum an out-of-range delta into the buffer.
//!
//! The check is CONTAINMENT of each frame in the request range, never contiguity
//! between frames: a coordinator's `FRAME` carries a SET of samples gathered from
//! several lanes (not necessarily contiguous), reports `first_sample =
//! request.first_sample` and an exact `samples` count, so `[first_sample, first_sample +
//! samples)` is still inside the request range while the individual samples need not
//! be that exact interval. The sample COUNT (`samples_done`) is what the divisor uses,
//! and it is exact either way.

#[cfg(test)]
mod tests;

use crate::{
    messages::{
        DisplayEncoding, Done, ErrorMsg, FrameHeader, PreviewHeader, Progress, StreamEvent,
    },
    radiance::{PayloadDecoder, RadianceError},
};
use glam::Vec3;

/// A `PREVIEW`'s payload, decoded and kept as the accumulator's single
/// "most recent preview" slot -- see the module doc comment on why a new one replaces
/// (never sums with) whatever was there before.
#[derive(Debug, Clone, PartialEq)]
// `buffer: Vec<Vec3>` is float data, so `Eq` isn't derivable here.
pub struct PreviewSnapshot {
    pub width: u32,
    pub height: u32,
    pub samples_done: u32,
    pub buffer: Vec<Vec3>,
}

/// An 8-bit picture event's payload (v14 `DISPLAY_FRAME` or `FINAL_IMAGE`), as received.
///
/// `bytes` is `encoding`-encoded RGBA8 -- decode it with
/// `crate::display::decode_rgba8(encoding, width, height, &bytes)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureSnapshot {
    /// Picture width in pixels, from the event header.
    pub width: u32,
    /// Picture height in pixels, from the event header.
    pub height: u32,
    /// Samples the picture reflects.
    pub samples_done: u32,
    /// How `bytes` is encoded.
    pub encoding: DisplayEncoding,
    /// The payload as received.
    pub bytes: Vec<u8>,
}

/// What [`Accumulator::apply`] did with one [`StreamEvent`].
///
/// `Copy`: every variant only carries small `Copy` tags (`u32`/`u64`/`bool`, a 32-byte
/// hash), so callers can take it by value cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// A `FRAME` delta was summed into [`Accumulator::buffer`].
    FrameSummed { samples_done: u32 },
    /// A `PREVIEW` snapshot replaced [`Accumulator::last_preview`].
    PreviewReplaced,
    /// A `PROGRESS` ping for the current epoch.
    Progress { samples_done: u32 },
    /// `DONE` for the current epoch -- the request finished or was cancelled; no
    /// further payload for this epoch follows.
    Done { cancelled: bool },
    /// `ERROR` -- the worker rejected the request. Not epoch-gated: an error carries no
    /// `request_id` of its own, so it's always surfaced to the caller.
    WorkerError,
    /// The event's `request_id` didn't match [`Accumulator::current_request_id`] --
    /// dropped without touching `buffer` or `last_preview`. See the module doc comment.
    StaleDropped,
    /// v14: a `DISPLAY_FRAME` for the current epoch replaced
    /// [`Accumulator::last_display_frame`].
    DisplayFrameReplaced,
    /// v14: the `FINAL_IMAGE` for the current epoch is in
    /// [`Accumulator::final_image`]; `DONE` follows.
    FinalImageReceived,
    /// v14: a `PONG` (not epoch-gated) answering the `PING` with this `nonce`.
    Pong { nonce: u64 },
    /// v14: `CAPABILITY_CHANGED` (not epoch-gated); the new capability is in the event.
    CapabilityChanged,
    /// v14: `NEED_ASSET` (not epoch-gated): the server asks for the asset with
    /// this SHA-256 before it can proceed. The caller answers with
    /// `crate::client::send_asset` (or drops the connection).
    NeedAsset { content_hash: [u8; 32] },
}

/// The epoch-gated radiance sum for one session's remote render.
///
/// Owns a `width * height` buffer sized once at construction (the render resolution is
/// session-wide, not per-request -- see the crate's `client` module docs), zeroed
/// every time [`begin_request`](Self::begin_request) starts a new epoch.
pub struct Accumulator {
    width: u32,
    height: u32,
    /// `None` before the first [`begin_request`](Self::begin_request) call -- nothing
    /// is ever "current epoch" yet, so every event is dropped as stale until a request
    /// actually starts.
    current_request_id: Option<u32>,
    /// The absolute sample range `[start, end)` the current epoch's request asked
    /// for, when the caller declared one via
    /// [`begin_request_for_range`](Self::begin_request_for_range). `None` accepts any
    /// `FRAME` range (the pre-existing behaviour of [`begin_request`](Self::begin_request)).
    expected_range: Option<(u32, u32)>,
    buffer: Vec<Vec3>,
    samples_done: u32,
    last_preview: Option<PreviewSnapshot>,
    last_display_frame: Option<PictureSnapshot>,
    final_image: Option<PictureSnapshot>,
    /// Reusable decompression scratch for compressed payloads (v14).
    decoder: PayloadDecoder,
}

/// Whether a `FRAME` covering `[first_sample, first_sample + samples)` lies entirely
/// inside `[start, end)`.
///
/// Containment is the only range property a `FRAME` promises (see the module doc
/// comment). Overflow-safe: a header whose end would overflow `u32` is never inside any
/// range.
#[must_use]
pub const fn frame_within_range(first_sample: u32, samples: u32, start: u32, end: u32) -> bool {
    match first_sample.checked_add(samples) {
        Some(frame_end) => first_sample >= start && frame_end <= end,
        None => false,
    }
}

impl Accumulator {
    /// Builds an empty accumulator for a `width * height` render. No request is
    /// current yet -- see [`begin_request`](Self::begin_request).
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let pixel_count = width as usize * height as usize;
        Self {
            width,
            height,
            current_request_id: None,
            expected_range: None,
            buffer: vec![Vec3::ZERO; pixel_count],
            samples_done: 0,
            last_preview: None,
            last_display_frame: None,
            final_image: None,
            decoder: PayloadDecoder::new(),
        }
    }

    /// Starts a new epoch: `request_id` becomes [`current_request_id`](Self::current_request_id),
    /// `buffer` is zeroed, `samples_done` resets to 0, and any pending
    /// [`last_preview`](Self::last_preview) (and v14 picture) is cleared -- a preview from
    /// the previous (now superseded) request is exactly as stale as a `FRAME` from it
    /// would be.
    ///
    /// Call this the moment a new `RenderRequest` is sent, NOT when its reply starts
    /// arriving: any bytes for the OLD epoch still in flight on the wire must see the
    /// new epoch already in place so [`apply`](Self::apply) drops them.
    pub fn begin_request(&mut self, request_id: u32) {
        self.current_request_id = Some(request_id);
        self.expected_range = None;
        self.buffer.fill(Vec3::ZERO);
        self.samples_done = 0;
        self.last_preview = None;
        self.last_display_frame = None;
        self.final_image = None;
    }

    /// [`begin_request`](Self::begin_request), plus declaring the absolute sample
    /// range `[first_sample, first_sample + samples)` this request asked for -- see
    /// the module doc comment's "Expected sample range" section. A range whose end
    /// would overflow `u32` saturates at `u32::MAX`.
    pub fn begin_request_for_range(&mut self, request_id: u32, first_sample: u32, samples: u32) {
        self.begin_request(request_id);
        self.expected_range = Some((first_sample, first_sample.saturating_add(samples)));
    }

    /// The current epoch's declared sample range `[start, end)`, if any -- see
    /// [`begin_request_for_range`](Self::begin_request_for_range).
    #[must_use]
    pub const fn expected_range(&self) -> Option<(u32, u32)> {
        self.expected_range
    }

    #[must_use]
    pub const fn current_request_id(&self) -> Option<u32> {
        self.current_request_id
    }

    #[must_use]
    pub fn buffer(&self) -> &[Vec3] {
        &self.buffer
    }

    #[must_use]
    pub const fn samples_done(&self) -> u32 {
        self.samples_done
    }

    #[must_use]
    pub const fn last_preview(&self) -> Option<&PreviewSnapshot> {
        self.last_preview.as_ref()
    }

    /// The most recent `DISPLAY_FRAME` of the current epoch (v14), still encoded.
    #[must_use]
    pub const fn last_display_frame(&self) -> Option<&PictureSnapshot> {
        self.last_display_frame.as_ref()
    }

    /// The current epoch's `FINAL_IMAGE` (v14), still encoded, once it arrived.
    #[must_use]
    pub const fn final_image(&self) -> Option<&PictureSnapshot> {
        self.final_image.as_ref()
    }

    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Applies one [`StreamEvent`] (as read by [`crate::messages::read_stream_event`],
    /// `payload` being whatever it returned alongside), enforcing the epoch rule
    /// described in the module doc comment.
    ///
    /// [`StreamEvent::Error`], [`StreamEvent::Pong`], [`StreamEvent::CapabilityChanged`]
    /// and [`StreamEvent::NeedAsset`] are never epoch-gated (they carry no `request_id`)
    /// and are always reported, regardless of what's currently current.
    ///
    /// # Errors
    ///
    /// Returns [`crate::radiance::RadianceError`] if a `FRAME`/`PREVIEW` payload fails
    /// to decode against its own header (wrong length, lying `raw_len`, a decompression
    /// bomb, an unsupported encoding), or a raw RGBA8 picture has the wrong length --
    /// a malformed payload, not a stale one (a stale payload is still well-formed; it's
    /// just for the wrong epoch), so it's a hard error rather than a silent drop.
    pub fn apply(
        &mut self,
        event: &StreamEvent,
        payload: Option<&[u8]>,
    ) -> Result<ApplyOutcome, RadianceError> {
        if let Some(request_id) = event.request_id()
            && Some(request_id) != self.current_request_id
        {
            return Ok(ApplyOutcome::StaleDropped);
        }
        let bytes = payload.unwrap_or(&[]);
        match event {
            StreamEvent::Frame(header) => self.apply_frame(header, bytes),
            StreamEvent::Preview(header) => self.apply_preview(header, bytes),
            StreamEvent::Progress(Progress { samples_done, .. }) => Ok(ApplyOutcome::Progress {
                samples_done: *samples_done,
            }),
            StreamEvent::Done(Done { cancelled, .. }) => Ok(ApplyOutcome::Done {
                cancelled: *cancelled,
            }),
            StreamEvent::Error(ErrorMsg { .. }) => Ok(ApplyOutcome::WorkerError),
            StreamEvent::Pong { nonce } => Ok(ApplyOutcome::Pong { nonce: *nonce }),
            StreamEvent::CapabilityChanged { .. } => Ok(ApplyOutcome::CapabilityChanged),
            StreamEvent::NeedAsset { content_hash } => Ok(ApplyOutcome::NeedAsset {
                content_hash: *content_hash,
            }),
            StreamEvent::DisplayFrame(h) => {
                self.last_display_frame = Some(picture(
                    h.width,
                    h.height,
                    h.samples_done,
                    h.encoding,
                    bytes,
                )?);
                Ok(ApplyOutcome::DisplayFrameReplaced)
            }
            StreamEvent::FinalImage(h) => {
                self.final_image = Some(picture(
                    h.width,
                    h.height,
                    h.samples_done,
                    h.encoding,
                    bytes,
                )?);
                Ok(ApplyOutcome::FinalImageReceived)
            }
        }
    }

    /// The current-epoch `FRAME` path: range containment check, then decode-and-sum.
    fn apply_frame(
        &mut self,
        header: &FrameHeader,
        bytes: &[u8],
    ) -> Result<ApplyOutcome, RadianceError> {
        if let Some((start, end)) = self.expected_range {
            let inside = frame_within_range(header.first_sample, header.samples, start, end);
            debug_assert!(
                inside,
                "FRAME [{}, +{}) lies outside the expected range [{start}, {end}) \
                 of request {}",
                header.first_sample, header.samples, header.request_id
            );
            if !inside {
                tracing::warn!(
                    request_id = header.request_id,
                    first_sample = header.first_sample,
                    samples = header.samples,
                    expected_start = start,
                    expected_end = end,
                    "dropping a FRAME outside the requested sample range"
                );
                return Ok(ApplyOutcome::StaleDropped);
            }
        }
        // Decodes straight into the running sum: a raw payload is reinterpreted in
        // place, a compressed one summed from its decompressed planes -- never an owned
        // full-frame copy first.
        self.decoder.decode_and_add(
            header.encoding,
            header.raw_len,
            bytes,
            self.width,
            self.height,
            &mut self.buffer,
        )?;
        self.samples_done = self.samples_done.saturating_add(header.samples);
        Ok(ApplyOutcome::FrameSummed {
            samples_done: self.samples_done,
        })
    }

    /// The current-epoch `PREVIEW` path: decode at the header's own size, replace.
    fn apply_preview(
        &mut self,
        header: &PreviewHeader,
        bytes: &[u8],
    ) -> Result<ApplyOutcome, RadianceError> {
        let buffer = self.decoder.decode_to_vec(
            header.encoding,
            header.raw_len,
            bytes,
            header.width,
            header.height,
        )?;
        self.last_preview = Some(PreviewSnapshot {
            width: header.width,
            height: header.height,
            samples_done: header.samples_done,
            buffer,
        });
        Ok(ApplyOutcome::PreviewReplaced)
    }
}

/// Builds a [`PictureSnapshot`], checking what can be checked without decoding: the
/// dimensions stay under the frame cap, and a raw RGBA8 payload is exactly
/// `width * height * 4` bytes. A PNG is validated when it is decoded.
fn picture(
    width: u32,
    height: u32,
    samples_done: u32,
    encoding: DisplayEncoding,
    bytes: &[u8],
) -> Result<PictureSnapshot, RadianceError> {
    let expected = crate::display::rgba8_len(width, height)
        .map_err(|_| RadianceError::TooLarge { width, height })?;
    if encoding == DisplayEncoding::Rgba8 && bytes.len() != expected {
        return Err(RadianceError::LengthMismatch {
            width,
            height,
            expected_bytes: expected,
            got_bytes: bytes.len(),
        });
    }
    Ok(PictureSnapshot {
        width,
        height,
        samples_done,
        encoding,
        bytes: bytes.to_vec(),
    })
}

/// Decodes a `PREVIEW` header's declared dimensions.
///
/// Exposed purely so callers that only want to know a preview's shape (without
/// decoding its payload) don't have to reach into [`PreviewHeader`] themselves; used by
/// `apps/indicatrix-cut`'s bridge layer when sizing a display buffer ahead of the first
/// preview.
#[must_use]
pub const fn preview_dimensions(header: &PreviewHeader) -> (u32, u32) {
    (header.width, header.height)
}
