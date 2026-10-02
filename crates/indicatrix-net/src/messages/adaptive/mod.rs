//! Adaptive payload compression: the codec of each `FRAME`/`PREVIEW`/`DISPLAY_FRAME`
//! follows the measured bandwidth of the connection it is sent on.
//!
//! A link can be slower during one job and faster during the next, and the best codec
//! differs by link speed (a slow link pays for a smaller payload with CPU time, a fast one
//! sends raw floats). The pieces, from the wire inwards:
//!
//! 1. [`BandwidthEstimator`] turns "N bytes took T to write" into an EWMA in Mbit/s. It
//!    holds no clock; the sender times its own blocking write and passes the `Duration`.
//!    Writes under [`MIN_SAMPLE_BYTES`] are ignored (the kernel send buffer hides the link).
//! 2. [`BandwidthTier`] is a rung of the [`TIERS_MBPS`] ladder. [`TierSelector`] maps the
//!    estimate onto it by the 50% rule (the higher tier once the estimate is more than
//!    halfway to it) with hysteresis so a link at the boundary does not flip every frame.
//!    [`LinkAdapter`] bundles estimator and selector: one per peer connection.
//! 3. [`EncodingMatrix`] is the generated table (size class x tier) of preferred encodings;
//!    `messages/encoding_matrix.rs` says how to regenerate it from the benchmark.
//! 4. [`AdaptiveEncoderPolicy`] picks the encoding for one frame (matrix lookup filtered by
//!    what the peer announced and this build supports); [`AdaptiveEncoder`] applies it.
//! 5. [`PeerLink`] is what a sender keeps per peer connection: 1 to 4 plus a
//!    [`WriteAggregator`] for runs of small frames and a hand-over to the process-wide
//!    [`PeerBandwidthBook`], so the next connection to the same peer starts from the last
//!    measured speed. [`PayloadChoice`] is the `auto|raw|lz4|zstd[:LEVEL]` setting that
//!    selects between [`AdaptiveEncoderPolicy::adaptive`] and a fixed policy.
//!
//! # No protocol change
//!
//! Every `FrameHeader`/`PreviewHeader`/`DisplayFrameHeader` already names its own
//! encoding and the decoders ([`crate::radiance::PayloadDecoder`],
//! [`crate::display::decode_rgba8`], the client accumulator) dispatch on that header field
//! alone; nothing on the decode side compares it with the `WELCOME`'s negotiated
//! encoding. So consecutive frames may use different encodings and one decoder instance
//! decodes them all. The only constraint is on the sender: use only encodings the peer
//! listed in its `HELLO` `accept_encodings` (families; the level is the sender's choice),
//! which [`AdaptiveEncoderPolicy`] guarantees.
//!
//! # Sender wiring
//!
//! One [`PeerLink`] per peer connection; the closure is the blocking socket write and the
//! only thing timed:
//!
//! ```ignore
//! let policy = PayloadChoice::Auto.policy(&hello.accept_encodings, loopback);
//! let link = PeerLink::new(&peer_key, policy);
//! // per frame:
//! link.send_payload(raw_bytes, |encoded| {
//!     let header = FrameHeader::for_encoded(request_id, first_sample, samples, encoded);
//!     write_stream_event(&mut socket, &StreamEvent::Frame(header), Some(encoded.bytes))
//! })?;
//! ```
//!
//! The pieces underneath ([`LinkAdapter`], [`AdaptiveEncoder`]) stay usable on their own
//! for a sender with its own write path: call [`LinkAdapter::observe`] with the wire bytes
//! and the duration of the blocking write.

mod aggregate;
mod bandwidth;
mod choice;
mod link;
mod matrix;
mod peers;
mod policy;
mod tier;

#[cfg(test)]
mod bandwidth_tests;
#[cfg(test)]
mod matrix_tests;
#[cfg(test)]
mod policy_tests;
#[cfg(test)]
mod tier_tests;

pub use aggregate::{WriteAggregator, WriteSample};
pub use bandwidth::{BandwidthConfig, BandwidthEstimator, MAX_PLAUSIBLE_MBPS, MIN_SAMPLE_BYTES};
pub use choice::{MAX_ZSTD_LEVEL, PayloadChoice};
pub use link::{FRAME_OVERHEAD_BYTES, PeerLink};
pub use matrix::{
    DisplayRow, EncodingMatrix, MEDIUM_LIMIT_BYTES, PayloadRow, SIZE_CLASS_COUNT,
    SMALL_LIMIT_BYTES, SizeClass,
};
pub use peers::{
    MAX_REMEMBERED_PEERS, PeerBandwidthBook, recall_peer_bandwidth, remember_peer_bandwidth,
};
pub use policy::{AdaptiveEncoder, AdaptiveEncoderPolicy, AdaptiveMode};
pub use tier::{
    BandwidthTier, DEFAULT_HYSTERESIS, DEFAULT_TIER_INDEX, LinkAdapter, TIER_COUNT, TIERS_MBPS,
    TierSelector,
};
