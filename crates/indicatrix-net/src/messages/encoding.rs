//! Payload-encoding negotiation (protocol v14).
//!
//! [`PayloadEncoding`] is how a `FRAME`/`PREVIEW` radiance payload travels on the wire,
//! [`DisplayEncoding`] how an 8-bit `DISPLAY_FRAME` travels.
//!
//! # Negotiation
//!
//! The client lists what it can decode in [`super::Hello::accept_encodings`]; the server
//! walks ITS OWN preference list and picks the first entry the client accepts
//! ([`negotiate`]), then announces the result in [`super::Welcome::payload_encoding`].
//!
//! - Matching is by FAMILY: a client accepting any `ShuffleZstd { .. }` can decode every
//!   zstd level, so the `level` in an accept list is ignored; the server's chosen level
//!   is the one used.
//! - [`PayloadEncoding::Raw`] is always implicitly accepted and is the fallback when the
//!   two lists share nothing.
//! - A server never picks an encoding its own build cannot produce
//!   ([`PayloadEncoding::is_supported`]).
//!
//! The negotiated encoding is the connection's default, not a promise or a cap for every
//! frame: each `FrameHeader`/`PreviewHeader` names its own `encoding`, an encoder may send
//! an individual payload `Raw` when compressing it would not make it smaller, and an
//! adaptive sender ([`super::adaptive`]) may use any encoding the client announced in
//! `accept_encodings`. A decoder must therefore dispatch on the header, never on the
//! handshake result (none in this crate compares the two).
//!
//! # Defaults (measured with `indicatrix-worker`'s `payload_codec_bench` example)
//!
//! [`DEFAULT_SERVER_PREFERENCE`]: `ShuffleZstd { level: 1 }`, then `ShuffleLz4`, then
//! `Raw`. [`LOOPBACK_SERVER_PREFERENCE`]: `Raw` only, since a loopback link is faster than
//! any codec. Both are only defaults; a server passes whatever list it is configured
//! with to [`negotiate`].

use serde::{Deserialize, Serialize};

/// How one `FRAME`/`PREVIEW` radiance payload is encoded on the wire.
///
/// Every variant is lossless: the decoded bytes are bit-identical to the `w * h * 12`
/// raw `f32` bytes the sender started from (NaN payloads and signed zeros included), so
/// summing decoded deltas is exactly summing raw ones. Variant order is wire-load-bearing
/// (`postcard` encodes the declaration index); append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PayloadEncoding {
    /// The raw little-endian `f32` bytes, unchanged. Always supported.
    Raw,
    /// Byte shuffle (4 planes: all byte 0s, then all byte 1s, ...) followed by a zstd
    /// frame at `level`. Needs this crate's `compression` feature.
    ShuffleZstd {
        /// The zstd compression level the ENCODER uses. Irrelevant for decoding and
        /// ignored when matching accept lists (see the module doc comment).
        level: u8,
    },
    /// Byte shuffle followed by an LZ4 block. Needs this crate's `compression` feature.
    ShuffleLz4,
}

impl PayloadEncoding {
    /// The measured default: zstd level 1 after the byte shuffle.
    pub const DEFAULT_ZSTD: Self = Self::ShuffleZstd { level: 1 };

    /// Whether this build can encode AND decode `self`. `Raw` always; the compressed
    /// variants only with this crate's `compression` feature.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        match self {
            Self::Raw => true,
            Self::ShuffleZstd { .. } | Self::ShuffleLz4 => cfg!(feature = "compression"),
        }
    }

    /// Whether `self` and `other` are the same codec family (see the module doc
    /// comment: zstd levels are interchangeable for decoding).
    #[must_use]
    pub const fn same_family(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Raw, Self::Raw)
                | (Self::ShuffleZstd { .. }, Self::ShuffleZstd { .. })
                | (Self::ShuffleLz4, Self::ShuffleLz4)
        )
    }

    /// The accept list a client sends by default: every encoding this build supports,
    /// most compact first.
    #[must_use]
    pub fn default_accept_list() -> Vec<Self> {
        DEFAULT_SERVER_PREFERENCE
            .into_iter()
            .filter(|e| e.is_supported())
            .collect()
    }
}

/// The default server preference, as measured (zstd level 1 first, LZ4 as the
/// faster fallback, raw last).
pub const DEFAULT_SERVER_PREFERENCE: [PayloadEncoding; 3] = [
    PayloadEncoding::DEFAULT_ZSTD,
    PayloadEncoding::ShuffleLz4,
    PayloadEncoding::Raw,
];

/// The preference a server uses for a loopback peer: raw only, since memory bandwidth
/// beats every codec's throughput.
pub const LOOPBACK_SERVER_PREFERENCE: [PayloadEncoding; 1] = [PayloadEncoding::Raw];

/// Picks the payload encoding for one connection.
///
/// The first entry of `server_preference` that this build supports and whose family
/// appears in `client_accepts`, else [`PayloadEncoding::Raw`]. See the module doc comment.
#[must_use]
pub fn negotiate(
    server_preference: &[PayloadEncoding],
    client_accepts: &[PayloadEncoding],
) -> PayloadEncoding {
    server_preference
        .iter()
        .copied()
        .filter(|e| e.is_supported())
        .find(|e| *e == PayloadEncoding::Raw || client_accepts.iter().any(|c| c.same_family(*e)))
        .unwrap_or(PayloadEncoding::Raw)
}

/// How one 8-bit `DISPLAY_FRAME` (or `FINAL_IMAGE`) payload is encoded. Variant order is
/// wire-load-bearing; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisplayEncoding {
    /// Raw RGBA8, `width * height * 4` bytes, row-major, top row first.
    Rgba8,
    /// A PNG (8-bit RGBA, non-interlaced) of the same pixels -- see `crate::display`.
    /// Needs this crate's `compression` feature to encode or decode.
    Png,
}

impl DisplayEncoding {
    /// The display encoding that goes with a negotiated payload encoding: raw RGBA8 when
    /// the connection negotiated `Raw` (loopback, fast LAN) or this build has no PNG
    /// codec, PNG otherwise.
    #[must_use]
    pub const fn for_payload_encoding(encoding: PayloadEncoding) -> Self {
        match encoding {
            PayloadEncoding::Raw => Self::Rgba8,
            PayloadEncoding::ShuffleZstd { .. } | PayloadEncoding::ShuffleLz4 => {
                if cfg!(feature = "compression") {
                    Self::Png
                } else {
                    Self::Rgba8
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZSTD1: PayloadEncoding = PayloadEncoding::ShuffleZstd { level: 1 };
    #[cfg(feature = "compression")]
    const ZSTD3: PayloadEncoding = PayloadEncoding::ShuffleZstd { level: 3 };
    const LZ4: PayloadEncoding = PayloadEncoding::ShuffleLz4;
    const RAW: PayloadEncoding = PayloadEncoding::Raw;

    #[cfg(feature = "compression")]
    #[test]
    fn negotiation_picks_the_first_server_preference_the_client_accepts() {
        assert_eq!(
            negotiate(&DEFAULT_SERVER_PREFERENCE, &[ZSTD1, LZ4, RAW]),
            ZSTD1
        );
        assert_eq!(negotiate(&DEFAULT_SERVER_PREFERENCE, &[LZ4, RAW]), LZ4);
        assert_eq!(negotiate(&DEFAULT_SERVER_PREFERENCE, &[LZ4]), LZ4);
        assert_eq!(negotiate(&[LZ4, ZSTD1], &[ZSTD1, LZ4]), LZ4);
        assert_eq!(negotiate(&LOOPBACK_SERVER_PREFERENCE, &[ZSTD1, LZ4]), RAW);
    }

    #[cfg(feature = "compression")]
    #[test]
    fn negotiation_matches_zstd_by_family_and_keeps_the_servers_level() {
        assert_eq!(negotiate(&[ZSTD3, RAW], &[ZSTD1]), ZSTD3);
    }

    #[test]
    fn negotiation_falls_back_to_raw_when_nothing_is_shared() {
        assert_eq!(negotiate(&[ZSTD1], &[LZ4]), RAW);
        assert_eq!(negotiate(&[ZSTD1, LZ4], &[]), RAW);
        assert_eq!(negotiate(&[], &[ZSTD1, LZ4, RAW]), RAW);
        assert_eq!(negotiate(&[LZ4], &[RAW]), RAW);
    }

    #[test]
    fn raw_is_implicitly_accepted_even_when_the_client_does_not_list_it() {
        assert_eq!(negotiate(&[RAW, ZSTD1], &[ZSTD1]), RAW);
    }

    #[test]
    fn the_default_accept_list_only_names_supported_encodings() {
        let list = PayloadEncoding::default_accept_list();
        assert!(list.iter().all(|e| e.is_supported()));
        assert!(list.contains(&RAW));
        assert_eq!(list.len() == 3, cfg!(feature = "compression"));
    }

    #[cfg(not(feature = "compression"))]
    #[test]
    fn a_build_without_compression_never_negotiates_a_compressed_encoding() {
        assert_eq!(
            negotiate(&DEFAULT_SERVER_PREFERENCE, &[ZSTD1, LZ4, RAW]),
            RAW
        );
    }

    #[test]
    fn display_encoding_follows_the_payload_encoding() {
        assert_eq!(
            DisplayEncoding::for_payload_encoding(RAW),
            DisplayEncoding::Rgba8
        );
        let expected = if cfg!(feature = "compression") {
            DisplayEncoding::Png
        } else {
            DisplayEncoding::Rgba8
        };
        assert_eq!(DisplayEncoding::for_payload_encoding(ZSTD1), expected);
        assert_eq!(DisplayEncoding::for_payload_encoding(LZ4), expected);
    }

    #[test]
    fn payload_encoding_variants_keep_their_postcard_discriminants() {
        assert_eq!(postcard::to_allocvec(&RAW).unwrap(), [0]);
        assert_eq!(postcard::to_allocvec(&ZSTD1).unwrap(), [1, 1]);
        assert_eq!(postcard::to_allocvec(&LZ4).unwrap(), [2]);
    }
}
