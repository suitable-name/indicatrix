//! The per-frame encoding chooser ([`AdaptiveEncoderPolicy`]) and a sender-side encoder that
//! applies it ([`AdaptiveEncoder`]).

use super::{
    matrix::{EncodingMatrix, SizeClass},
    tier::BandwidthTier,
};
use crate::{
    display::{DisplayError, encode_rgba8_smallest},
    messages::{DisplayEncoding, PayloadEncoding, negotiate},
    radiance::{EncodedPayload, PayloadEncoder},
};

/// Whether the encoding follows the measured bandwidth or is pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdaptiveMode {
    /// Choose per frame from the encoding matrix by size class and bandwidth tier.
    Adaptive,
    /// Today's behaviour: the first entry of this server preference list the peer accepts
    /// ([`negotiate`]), for every frame, whatever the size or the link. Loopback is NOT
    /// special-cased here; a caller wanting raw on loopback passes
    /// `LOOPBACK_SERVER_PREFERENCE`.
    Fixed(Vec<PayloadEncoding>),
}

/// Chooses the payload and display encoding of each frame for one peer.
///
/// The choice never names an encoding the peer did not announce in its `HELLO`
/// `accept_encodings` (matched by family, so the level is the matrix's), nor one this
/// build cannot produce; the answer degrades to `Raw`. The per-frame header names its
/// encoding, so a decoder needs no knowledge of the policy (see the module docs of
/// [`super`]).
#[derive(Debug, Clone)]
pub struct AdaptiveEncoderPolicy {
    mode: AdaptiveMode,
    accepts: Vec<PayloadEncoding>,
    loopback: bool,
    matrix: EncodingMatrix,
}

impl AdaptiveEncoderPolicy {
    /// An adaptive policy for a peer that announced `accepts`; `loopback` peers always
    /// get `Raw` payloads and RGBA8 pictures.
    #[must_use]
    pub fn adaptive(accepts: &[PayloadEncoding], loopback: bool) -> Self {
        Self {
            mode: AdaptiveMode::Adaptive,
            accepts: accepts.to_vec(),
            loopback,
            matrix: EncodingMatrix::GENERATED,
        }
    }

    /// A policy with adaptation disabled: `preference` is walked like the negotiation
    /// does, once per frame, so the result is the connection's negotiated encoding.
    #[must_use]
    pub fn fixed(preference: &[PayloadEncoding], accepts: &[PayloadEncoding]) -> Self {
        Self {
            mode: AdaptiveMode::Fixed(preference.to_vec()),
            accepts: accepts.to_vec(),
            loopback: false,
            matrix: EncodingMatrix::GENERATED,
        }
    }

    /// The same policy over a different matrix (tests, experiments).
    #[must_use]
    pub const fn with_matrix(mut self, matrix: EncodingMatrix) -> Self {
        self.matrix = matrix;
        self
    }

    /// The mode.
    #[must_use]
    pub const fn mode(&self) -> &AdaptiveMode {
        &self.mode
    }

    /// Whether the policy follows the link (as opposed to [`AdaptiveMode::Fixed`]).
    #[must_use]
    pub const fn is_adaptive(&self) -> bool {
        matches!(self.mode, AdaptiveMode::Adaptive)
    }

    /// Whether the peer was declared a loopback peer ([`Self::adaptive`]'s argument); a
    /// fixed policy never is.
    #[must_use]
    pub const fn is_loopback(&self) -> bool {
        self.loopback
    }

    /// The encoding for a `FRAME`/`PREVIEW` payload of `raw_bytes` uncompressed bytes on
    /// a link at `tier`. The encoder still sends the payload `Raw` when compressing does
    /// not make it smaller.
    #[must_use]
    pub fn choose_payload(&self, raw_bytes: usize, tier: BandwidthTier) -> PayloadEncoding {
        match &self.mode {
            AdaptiveMode::Fixed(preference) => negotiate(preference, &self.accepts),
            AdaptiveMode::Adaptive if self.loopback => PayloadEncoding::Raw,
            AdaptiveMode::Adaptive => {
                let class = SizeClass::of_payload_bytes(raw_bytes);
                negotiate(self.matrix.payload_preference(class, tier), &self.accepts)
            }
        }
    }

    /// The encoding for a `width x height` 8-bit `DISPLAY_FRAME`/`FINAL_IMAGE` on a link
    /// at `tier`. PNG is chosen only when this build has the PNG codec and the peer
    /// announced a compressed payload encoding (a peer built without the `compression`
    /// feature announces only `Raw` and cannot decode PNG). Use
    /// [`crate::display::encode_rgba8_smallest`] to send raw RGBA8 when PNG does not help.
    #[must_use]
    pub fn choose_display(&self, width: u32, height: u32, tier: BandwidthTier) -> DisplayEncoding {
        match &self.mode {
            AdaptiveMode::Fixed(preference) => {
                DisplayEncoding::for_payload_encoding(negotiate(preference, &self.accepts))
            }
            AdaptiveMode::Adaptive if self.loopback => DisplayEncoding::Rgba8,
            AdaptiveMode::Adaptive => {
                let pixels = width as usize * height as usize;
                let wanted = self
                    .matrix
                    .display_choice(SizeClass::of_pixels(pixels), tier);
                if wanted == DisplayEncoding::Png && !self.peer_decodes_png() {
                    DisplayEncoding::Rgba8
                } else {
                    wanted
                }
            }
        }
    }

    /// Whether PNG may be sent: compiled in here, and the peer announced compression.
    fn peer_decodes_png(&self) -> bool {
        cfg!(feature = "compression") && self.accepts.iter().any(|e| *e != PayloadEncoding::Raw)
    }
}

/// A sender's encoder: applies an [`AdaptiveEncoderPolicy`] frame by frame.
///
/// It keeps ONE [`PayloadEncoder`] (scratch buffers and zstd context) and rebuilds it when
/// the chosen encoding changes, which the tier hysteresis makes rare.
#[derive(Debug)]
pub struct AdaptiveEncoder {
    policy: AdaptiveEncoderPolicy,
    encoder: PayloadEncoder,
}

impl AdaptiveEncoder {
    /// An encoder following `policy`.
    #[must_use]
    pub const fn new(policy: AdaptiveEncoderPolicy) -> Self {
        Self {
            policy,
            encoder: PayloadEncoder::new(PayloadEncoding::Raw),
        }
    }

    /// The policy.
    #[must_use]
    pub const fn policy(&self) -> &AdaptiveEncoderPolicy {
        &self.policy
    }

    /// Drops the scratch buffers and the compression context (up to about twice the
    /// payload size at 4K) while keeping the chosen encoding; the next compressed payload
    /// allocates them again. Call it when a request ends or the link stays on `Raw`.
    /// Switching to a different encoding already frees them, since the encoder is rebuilt.
    pub fn release_buffers(&mut self) {
        self.encoder = PayloadEncoder::new(self.encoder.encoding());
    }

    /// Encodes one `FRAME`/`PREVIEW` payload for a link at `tier`. The returned
    /// [`EncodedPayload::encoding`] is what the header must carry (`Raw` when the
    /// chosen codec did not shrink this payload).
    pub fn encode<'a>(&'a mut self, raw: &'a [u8], tier: BandwidthTier) -> EncodedPayload<'a> {
        let wanted = self.policy.choose_payload(raw.len(), tier);
        if self.encoder.encoding() != wanted {
            self.encoder = PayloadEncoder::new(wanted);
        }
        self.encoder.encode(raw)
    }

    /// Encodes one RGBA8 picture for a link at `tier`, returning the encoding the header
    /// must carry and the bytes; falls back to raw RGBA8 when PNG is not smaller.
    ///
    /// # Errors
    ///
    /// [`DisplayError::LengthMismatch`]/[`DisplayError::TooLarge`] for a wrongly sized
    /// `rgba`, or [`DisplayError::Codec`] if the PNG encoder fails.
    pub fn encode_display(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        tier: BandwidthTier,
    ) -> Result<(DisplayEncoding, Vec<u8>), DisplayError> {
        let wanted = self.policy.choose_display(width, height, tier);
        encode_rgba8_smallest(wanted, width, height, rgba)
    }
}
