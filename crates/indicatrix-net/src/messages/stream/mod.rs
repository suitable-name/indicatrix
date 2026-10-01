//! Every message a client may send after the handshake ([`ClientMessage`]), and every
//! reply a worker sends for one `RENDER` request ([`StreamEvent`]) -- plus the small
//! per-message structs both are built from, and [`ErrorMsg`], used well beyond just
//! this file (handshake refusal, library-request errors).
//!
//! ```text
//! -> CANCEL   { request_id }
//! -> PING     { nonce }                                                           (v14)
//! <- FRAME    { request_id, first_sample, samples, payload_len, encoding, raw_len } + payload -- DELTA, full-res
//! <- PREVIEW  { request_id, width, height, samples_done, payload_len, encoding, raw_len } + payload -- CUMULATIVE, reduced-res
//! <- PROGRESS { request_id, samples_done }
//! <- DONE     { request_id, cancelled, stats: Stats }
//! <- ERROR    { code, message }
//! <- PONG     { nonce }                                                           (v14)
//! <- DISPLAY_FRAME { request_id, samples_done, width, height, encoding, payload_len } + payload (v14)
//! <- FINAL_IMAGE   { request_id, width, height, samples_done, encoding, payload_len } + payload (v14)
//! <- CAPABILITY_CHANGED { render: Option<RenderCapability> }                     (v14)
//! <- NEED_ASSET { content_hash }                                                  (v14, E)
//! -> ASSET    { content_hash, len } + payload                                     (v14, E)
//! -> CONTRIBUTION { request_id, first_sample, samples, width, height, encoding,
//!                   payload_len, raw_len } + payload    (v16, see crate::messages::contribution)
//! ```
//!
//! `FRAME`/`PREVIEW` payloads are encoded per their header's `encoding` (v14,
//! `crate::radiance::payload`); `DISPLAY_FRAME`/`FINAL_IMAGE` payloads are RGBA8 or PNG
//! (`crate::display`).
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
//! agree on tags 0/1, and only exchange tags 2+ with a peer whose `WELCOME` already
//! proved it has the matching capability. v14 appended [`ClientMessage::Ping`] (4),
//! [`ClientMessage::FinalImageRequest`] (5) and [`ClientMessage::Asset`] (6), all
//! `render`-gated for the same reason; any future variant must be appended after them.
//!
//! Split across submodules by topic:
//!
//! - [`types`]: the data types -- header/message structs, [`ClientMessage`],
//!   [`StreamEvent`].
//! - [`wire`]: read/write helpers for [`StreamEvent`] and its `FRAME`/`PREVIEW` variants.
//!
//! Every item is re-exported here at its original flat `stream::` path.

#[cfg(test)]
mod tests;
mod types;
mod wire;

pub use types::{
    Cancel, ClientMessage, DisplayFrameHeader, Done, ErrorMsg, FinalImageHeader, FrameHeader,
    PreviewHeader, Progress, Stats, StreamEvent,
};
pub use wire::{
    read_frame_message, read_preview_message, read_stream_event, write_frame_message,
    write_preview_message, write_stream_event,
};
