//! [`PeerLink`]: what a sender keeps per peer CONNECTION to adapt its compression.
//!
//! It bundles the bandwidth estimator and tier selector ([`LinkAdapter`]), the encoder
//! ([`AdaptiveEncoder`]), the small-write aggregator and the hand-over to the process-wide
//! peer book.
//!
//! # What is timed
//!
//! Only the blocking socket write of one frame: [`PeerLink::send_payload`] and
//! [`PeerLink::send_display`] encode first (compression time is never part of the
//! sample), start the clock, run the caller's write closure and stop the clock when it
//! returns. The closure must write straight to the socket (a `TcpStream`, or the TLS
//! stream on top of one), so its duration is what the kernel send buffer let through. A
//! sender that writes through a channel or a `BufWriter` must call
//! [`PeerLink::record_write`] at the point that actually blocks on the socket (for a
//! buffered writer: around the completed flush), not around the enqueue.
//!
//! The wire bytes passed to the estimator are the encoded payload plus
//! [`FRAME_OVERHEAD_BYTES`] for the header and the length prefixes.

use super::{
    aggregate::{WriteAggregator, WriteSample},
    bandwidth::MIN_SAMPLE_BYTES,
    peers::{recall_peer_bandwidth, remember_peer_bandwidth},
    policy::{AdaptiveEncoder, AdaptiveEncoderPolicy},
    tier::{BandwidthTier, LinkAdapter},
};
use crate::{
    display::{DisplayError, encode_rgba8},
    messages::DisplayEncoding,
    radiance::EncodedPayload,
};
use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// Bytes a frame's header and length prefixes add to its payload on the wire.
pub const FRAME_OVERHEAD_BYTES: usize = 64;

/// After this many payloads in a row went out `Raw`, the encoder's scratch buffers are
/// released (a link in the `Raw` tiers, or an incompressible stream, does not need them).
const RELEASE_AFTER_RAW_FRAMES: u32 = 8;

/// The adaptive-compression state of one peer connection. Cheap to share by reference:
/// the methods take `&self` and lock a private mutex that only the connection's own
/// emitter thread ever contends for.
#[derive(Debug)]
pub struct PeerLink {
    peer: String,
    remember: bool,
    state: Mutex<LinkState>,
}

#[derive(Debug)]
struct LinkState {
    adapter: LinkAdapter,
    encoder: AdaptiveEncoder,
    aggregate: WriteAggregator,
    track: bool,
    raw_streak: u32,
    last_raw_len: usize,
}

impl PeerLink {
    /// A link to `peer` (the key of the process-wide bandwidth book and the name in log
    /// lines) following `policy`. An adaptive, non-loopback policy starts from the last
    /// bandwidth the book knows for `peer`, if any; the estimator corrects a stale value.
    /// Logs the mode once at `info`.
    #[must_use]
    pub fn new(peer: &str, policy: AdaptiveEncoderPolicy) -> Self {
        let track = policy.is_adaptive() && !policy.is_loopback();
        let mut adapter = LinkAdapter::default();
        let seeded = recall_peer_bandwidth(peer).filter(|_| track);
        if let Some(mbps) = seeded {
            adapter.seed(mbps);
        }
        if policy.is_adaptive() && policy.is_loopback() {
            tracing::info!(peer, "payload compression: raw (loopback peer)");
        } else if track {
            tracing::info!(
                peer,
                seeded_mbps = ?seeded,
                tier = adapter.tier().index(),
                "payload compression: adaptive to the measured link speed"
            );
        } else {
            tracing::info!(peer, mode = ?policy.mode(), "payload compression: fixed");
        }
        Self {
            peer: peer.to_string(),
            remember: track,
            state: Mutex::new(LinkState {
                adapter,
                encoder: AdaptiveEncoder::new(policy),
                aggregate: WriteAggregator::new(),
                track,
                raw_streak: 0,
                last_raw_len: 0,
            }),
        }
    }

    /// A link pinned to `encoding` (what a connection did before adaptation existed):
    /// never measures, never touches the peer book.
    #[must_use]
    pub fn fixed(encoding: crate::messages::PayloadEncoding) -> Self {
        let policy = AdaptiveEncoderPolicy::fixed(&[encoding], &[encoding]);
        let state = LinkState {
            adapter: LinkAdapter::default(),
            encoder: AdaptiveEncoder::new(policy),
            aggregate: WriteAggregator::new(),
            track: false,
            raw_streak: 0,
            last_raw_len: 0,
        };
        Self {
            peer: String::new(),
            remember: false,
            state: Mutex::new(state),
        }
    }

    fn lock(&self) -> MutexGuard<'_, LinkState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The tier the next frame is encoded for.
    #[must_use]
    pub fn tier(&self) -> BandwidthTier {
        self.lock().adapter.tier()
    }

    /// The bandwidth estimate in Mbit/s, if the link has one.
    #[must_use]
    pub fn estimate(&self) -> Option<f64> {
        self.lock().adapter.estimate()
    }

    /// How many times the tier changed after its first placement.
    #[must_use]
    pub fn tier_switches(&self) -> u32 {
        self.lock().adapter.tier_switches()
    }

    /// Whether this link measures and adapts (an adaptive, non-loopback policy).
    #[must_use]
    pub fn is_tracking(&self) -> bool {
        self.lock().track
    }

    /// The `FRAME`/`PREVIEW` encoding a payload of `raw_bytes` would use right now.
    #[must_use]
    pub fn chosen_payload_encoding(&self, raw_bytes: usize) -> crate::messages::PayloadEncoding {
        let state = self.lock();
        state
            .encoder
            .policy()
            .choose_payload(raw_bytes, state.adapter.tier())
    }

    /// Encodes `raw` for the current tier and hands it to `write`, which must write it to
    /// the socket and return; only that call is timed. A failed write is not measured.
    ///
    /// # Errors
    ///
    /// Whatever `write` returns.
    pub fn send_payload<E>(
        &self,
        raw: &[u8],
        write: impl FnOnce(&EncodedPayload<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut guard = self.lock();
        let state = &mut *guard;
        let tier = state.adapter.tier();
        let encoded = state.encoder.encode(raw, tier);
        let wire = encoded.bytes.len() + FRAME_OVERHEAD_BYTES;
        let raw_fallback = encoded.encoding == crate::messages::PayloadEncoding::Raw;
        let started = Instant::now();
        let result = write(&encoded);
        let elapsed = started.elapsed();
        state.last_raw_len = raw.len();
        state.note_encoding(raw_fallback);
        if result.is_ok() {
            state.record(&self.peer, wire, elapsed);
        }
        drop(guard);
        self.remember_estimate_if_tracking();
        result
    }

    /// Encodes a `width x height` RGBA8 picture for the current tier and hands the
    /// encoding and bytes to `write` (timed like [`Self::send_payload`]). `final_picture`
    /// is the one `FINAL_IMAGE`: the export's product, always a PNG whatever the link (its
    /// write is still timed, a large and useful sample); a `DISPLAY_FRAME` follows the
    /// matrix instead.
    ///
    /// Returns `Ok(false)` (logged, nothing written) if the encoder refuses the picture,
    /// `Ok(true)` once written.
    ///
    /// # Errors
    ///
    /// Whatever `write` returns.
    pub fn send_display<E>(
        &self,
        (width, height): (u32, u32),
        rgba: &[u8],
        final_picture: bool,
        write: impl FnOnce(DisplayEncoding, &[u8]) -> Result<(), E>,
    ) -> Result<bool, E> {
        let mut guard = self.lock();
        let state = &mut *guard;
        let encoded = state.encode_picture(width, height, rgba, final_picture);
        let (encoding, bytes) = match encoded {
            Ok(picture) => picture,
            Err(e) => {
                tracing::warn!(peer = %self.peer, "could not encode a {width}x{height} picture: {e}");
                return Ok(false);
            }
        };
        let wire = bytes.len() + FRAME_OVERHEAD_BYTES;
        let started = Instant::now();
        let result = write(encoding, &bytes);
        let elapsed = started.elapsed();
        if result.is_ok() {
            state.record(&self.peer, wire, elapsed);
        }
        drop(guard);
        self.remember_estimate_if_tracking();
        result.map(|()| true)
    }

    /// Feeds one blocking write measured by the caller (`wire_bytes` written to the
    /// socket in `elapsed`); for senders that cannot use the `send_*` closures.
    pub fn record_write(&self, wire_bytes: usize, elapsed: Duration) {
        self.lock().record(&self.peer, wire_bytes, elapsed);
        self.remember_estimate_if_tracking();
    }

    /// Runs `write` and records its duration as a write of `wire_bytes` (for payloads the
    /// caller encoded itself); a failed write is not measured.
    ///
    /// # Errors
    ///
    /// Whatever `write` returns.
    pub fn timed_write<T, E>(
        &self,
        wire_bytes: usize,
        write: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let started = Instant::now();
        let result = write();
        if result.is_ok() {
            self.record_write(wire_bytes, started.elapsed());
        }
        result
    }

    /// Releases the encoder's scratch buffers; call when a request ends.
    pub fn release_buffers(&self) {
        let mut state = self.lock();
        state.encoder.release_buffers();
        state.raw_streak = 0;
    }

    /// Stores the current estimate in the process-wide peer book (a new connection to the
    /// same peer starts from it).
    pub fn remember_estimate_if_tracking(&self) {
        if !self.remember {
            return;
        }
        let estimate = self.lock().adapter.estimate();
        if let Some(mbps) = estimate {
            remember_peer_bandwidth(&self.peer, mbps);
        }
    }
}

impl Drop for PeerLink {
    fn drop(&mut self) {
        if !self.remember {
            return;
        }
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(mbps) = state.adapter.estimate() {
            remember_peer_bandwidth(&self.peer, mbps);
        }
    }
}

impl LinkState {
    /// Counts consecutive `Raw` payloads and frees the scratch buffers once there are
    /// enough of them.
    fn note_encoding(&mut self, sent_raw: bool) {
        if !sent_raw {
            self.raw_streak = 0;
            return;
        }
        self.raw_streak = self.raw_streak.saturating_add(1);
        if self.raw_streak == RELEASE_AFTER_RAW_FRAMES {
            self.encoder.release_buffers();
        }
    }

    /// The picture's encoding and bytes: PNG for the `FINAL_IMAGE`, else the policy's choice
    /// for the current tier.
    fn encode_picture(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        final_picture: bool,
    ) -> Result<(DisplayEncoding, Vec<u8>), DisplayError> {
        if final_picture {
            let bytes = encode_rgba8(DisplayEncoding::Png, width, height, rgba)?;
            return Ok((DisplayEncoding::Png, bytes));
        }
        self.encoder
            .encode_display(width, height, rgba, self.adapter.tier())
    }

    /// Feeds one measured write to the estimator (when this link measures), logging a
    /// tier change.
    fn record(&mut self, peer: &str, wire_bytes: usize, elapsed: Duration) {
        if !self.track {
            return;
        }
        let Some(sample) = self.aggregate.add(wire_bytes, elapsed) else {
            return;
        };
        if self.discard_flattering(&sample) {
            return;
        }
        let before = self.adapter.tier();
        let after = self.adapter.observe(sample.bytes, sample.elapsed);
        if after != before {
            let estimate = self.adapter.estimate();
            tracing::debug!(
                peer,
                estimate_mbps = ?estimate,
                old_tier = before.index(),
                new_tier = after.index(),
                chosen = ?self
                    .encoder
                    .policy()
                    .choose_payload(self.last_raw_len.max(MIN_SAMPLE_BYTES), after),
                "payload compression: bandwidth tier switched"
            );
        }
    }

    /// An aggregate of small writes can only lower the estimate: each part may have
    /// fitted in the send buffer, so a fast result says nothing about the link, while a
    /// slow one is real evidence.
    fn discard_flattering(&self, sample: &WriteSample) -> bool {
        if !sample.aggregated || sample.elapsed.is_zero() {
            return false;
        }
        let Some(estimate) = self.adapter.estimate() else {
            return false;
        };
        let mbps = sample.bytes as f64 * 8.0 / 1.0e6 / sample.elapsed.as_secs_f64();
        mbps >= estimate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::PayloadEncoding;

    fn accepts() -> Vec<PayloadEncoding> {
        PayloadEncoding::default_accept_list()
    }

    fn adaptive(peer: &str) -> PeerLink {
        PeerLink::new(peer, AdaptiveEncoderPolicy::adaptive(&accepts(), false))
    }

    const BIG: usize = 1 << 20;

    #[test]
    fn slow_measured_writes_move_the_tier_down_and_fast_ones_up() {
        let link = adaptive("link-test:tiers");
        assert_eq!(link.tier(), BandwidthTier::DEFAULT);
        link.record_write(BIG, Duration::from_millis(500)); // ~17 Mbit/s
        assert_eq!(link.tier(), BandwidthTier::LOWEST);
        let fast = adaptive("link-test:tiers-fast");
        fast.record_write(BIG, Duration::from_micros(200)); // ~42 Gbit/s, capped
        assert_eq!(fast.tier(), BandwidthTier::HIGHEST);
    }

    #[test]
    fn loopback_and_fixed_links_never_measure_or_remember() {
        let loopback = PeerLink::new(
            "link-test:loopback",
            AdaptiveEncoderPolicy::adaptive(&accepts(), true),
        );
        loopback.record_write(BIG, Duration::from_secs(5));
        assert_eq!(loopback.estimate(), None);
        let fixed = PeerLink::fixed(PayloadEncoding::ShuffleLz4);
        fixed.record_write(BIG, Duration::from_secs(5));
        assert_eq!(fixed.estimate(), None);
        assert!(!fixed.is_tracking());
        drop((loopback, fixed));
        assert_eq!(recall_peer_bandwidth("link-test:loopback"), None);
    }

    #[test]
    fn a_new_connection_is_seeded_from_the_peer_book_and_dropping_stores_the_estimate() {
        let first = adaptive("link-test:seed");
        first.record_write(BIG, Duration::from_millis(500));
        let estimate = first.estimate().unwrap();
        drop(first);
        assert_eq!(recall_peer_bandwidth("link-test:seed"), Some(estimate));
        let second = adaptive("link-test:seed");
        assert_eq!(second.tier(), BandwidthTier::LOWEST);
        assert_eq!(second.estimate(), Some(estimate));
    }

    #[test]
    fn small_writes_are_aggregated_and_may_only_lower_the_estimate() {
        let link = adaptive("link-test:aggregate");
        link.record_write(BIG, Duration::from_millis(100)); // ~84 Mbit/s -> tier 1
        let before = link.estimate().unwrap();
        // Fast aggregate (fits the send buffer): ignored.
        for _ in 0..8 {
            link.record_write(MIN_SAMPLE_BYTES / 8, Duration::from_micros(20));
        }
        assert_eq!(link.estimate(), Some(before));
        // Slow aggregate: counts.
        for _ in 0..8 {
            link.record_write(MIN_SAMPLE_BYTES / 8, Duration::from_millis(100));
        }
        assert!(link.estimate().unwrap() < before);
    }

    #[test]
    fn send_payload_times_only_the_write_and_skips_failed_ones() {
        let link = adaptive("link-test:send");
        let raw = vec![0_u8; 3 * BIG];
        let mut seen = None;
        link.send_payload(&raw, |encoded| -> Result<(), ()> {
            seen = Some((encoded.encoding, encoded.raw_len));
            std::thread::sleep(Duration::from_millis(30));
            Ok(())
        })
        .unwrap();
        let (_, raw_len) = seen.unwrap();
        assert_eq!(raw_len as usize, raw.len());
        // 3 MiB of zeros compresses to far under the threshold: the sample is small and
        // held back, so the estimate is still empty.
        assert_eq!(link.estimate(), None);
        let failed = link.send_payload(&raw, |_| Err::<(), _>("broken pipe"));
        assert_eq!(failed, Err("broken pipe"));
    }
}
