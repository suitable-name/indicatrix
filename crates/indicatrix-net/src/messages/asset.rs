//! Content-addressed assets (v14): the raw bytes of an HDR environment map a
//! scene references by hash.
//!
//! ```text
//! <- NEED_ASSET { content_hash }                  (StreamEvent::NeedAsset)
//! -> ASSET      { content_hash, len } + payload   (ClientMessage::Asset, then one raw frame)
//! ```
//!
//! A `SceneState` whose environment is `SceneEnvironment::Hdr` names the map only by its
//! [`content_hash`] (SHA-256 of the exact file bytes). A server that has not seen that
//! hash answers the request with `NEED_ASSET`; the client sends the bytes once as an
//! `ASSET` message (a small postcard header, then the raw bytes as a second frame, like
//! a `FRAME` payload); the server verifies the hash, caches the bytes and carries on
//! with the request. A later request naming the same hash is never asked again.
//!
//! The payload is bounded by [`MAX_ASSET_LEN`] (256 MiB), checked against the header's
//! declared `len` BEFORE anything is allocated.

use crate::framing;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// SHA-256 of an asset's exact bytes -- its identity on the wire and in every cache.
pub type ContentHash = [u8; 32];

/// The largest asset payload a peer sends or accepts (256 MiB).
pub const MAX_ASSET_LEN: u32 = 256 * 1024 * 1024;

/// The [`ContentHash`] of `bytes` (SHA-256).
#[must_use]
pub fn content_hash(bytes: &[u8]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Lower-case hex of `hash` (64 characters) -- the only form a hash ever takes in a file
/// name or log line, so a peer-chosen value can never smuggle a path separator.
#[must_use]
pub fn hash_hex(hash: &ContentHash) -> String {
    use std::fmt::Write as _;
    hash.iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The header of an `-> ASSET` message: which asset follows and how long it is. The
/// bytes themselves follow as one raw frame of exactly `len` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetHeader {
    /// The asset's SHA-256, as named by the scene and the server's `NEED_ASSET`.
    pub content_hash: ContentHash,
    /// Byte length of the payload frame that follows (at most [`MAX_ASSET_LEN`]).
    pub len: u32,
}

/// Why an asset payload could not be read.
#[derive(Debug)]
pub enum AssetError {
    /// The header declared more than [`MAX_ASSET_LEN`] bytes (refused before reading).
    TooLarge {
        /// The declared length.
        len: u32,
    },
    /// The payload frame's length differs from the header's `len`.
    LengthMismatch {
        /// The header's `len`.
        declared: u32,
        /// The payload frame's actual length.
        actual: usize,
    },
    /// The payload's SHA-256 is not the header's `content_hash`.
    HashMismatch,
    /// Reading the payload frame failed.
    Framing(framing::FramingError),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { len } => write!(
                f,
                "asset of {len} bytes exceeds the {} MiB limit",
                MAX_ASSET_LEN / (1024 * 1024)
            ),
            Self::LengthMismatch { declared, actual } => write!(
                f,
                "asset header declared {declared} bytes but the payload carried {actual}"
            ),
            Self::HashMismatch => write!(f, "asset bytes do not match their declared SHA-256"),
            Self::Framing(e) => write!(f, "reading the asset payload failed: {e}"),
        }
    }
}

impl std::error::Error for AssetError {}

/// Reads the raw payload frame that follows an [`AssetHeader`] and verifies it.
///
/// The length is bounded by [`MAX_ASSET_LEN`] (from the header, before allocating) and
/// must equal the header's `len`; the bytes must hash to its `content_hash`.
///
/// # Errors
///
/// See [`AssetError`]. After [`AssetError::HashMismatch`]/[`AssetError::LengthMismatch`]
/// the stream is still in sync (the whole frame was consumed); after
/// [`AssetError::TooLarge`]/[`AssetError::Framing`] it is not, and the connection must
/// be dropped.
pub fn read_asset_payload<R: std::io::Read>(
    reader: &mut R,
    header: &AssetHeader,
) -> Result<Vec<u8>, AssetError> {
    if header.len > MAX_ASSET_LEN {
        return Err(AssetError::TooLarge { len: header.len });
    }
    let bytes = framing::read_frame_bounded(reader, header.len).map_err(AssetError::Framing)?;
    if bytes.len() != header.len as usize {
        return Err(AssetError::LengthMismatch {
            declared: header.len,
            actual: bytes.len(),
        });
    }
    if content_hash(&bytes) != header.content_hash {
        return Err(AssetError::HashMismatch);
    }
    Ok(bytes)
}

/// Consumes the raw payload frame that follows an [`AssetHeader`] nobody asked for,
/// without buffering or hashing it.
///
/// The bound is [`MAX_ASSET_LEN`], checked against the header before reading; the frame
/// must be exactly `header.len` bytes. Returns `Ok(())` for a well-formed frame whatever
/// it contains.
///
/// # Errors
///
/// [`AssetError::TooLarge`] or [`AssetError::Framing`] leave the stream out of sync (drop
/// the connection); [`AssetError::LengthMismatch`] means the whole frame was consumed.
pub fn discard_asset_payload<R: std::io::Read>(
    reader: &mut R,
    header: &AssetHeader,
) -> Result<(), AssetError> {
    if header.len > MAX_ASSET_LEN {
        return Err(AssetError::TooLarge { len: header.len });
    }
    let skipped = framing::skip_frame_bounded(reader, header.len).map_err(AssetError::Framing)?;
    if skipped != header.len {
        return Err(AssetError::LengthMismatch {
            declared: header.len,
            actual: skipped as usize,
        });
    }
    Ok(())
}

/// Writes one `-> ASSET` message: the tagged `ClientMessage::Asset` header, then `bytes`
/// as one raw frame. `render`-feature only, like the variant.
///
/// # Errors
///
/// [`super::NetError::Framing`] with `FrameTooLarge` when `bytes` exceeds
/// [`MAX_ASSET_LEN`], else whatever writing fails with.
#[cfg(feature = "render")]
pub fn write_asset_message<W: std::io::Write>(
    writer: &mut W,
    bytes: &[u8],
) -> Result<(), super::NetError> {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    if len > MAX_ASSET_LEN {
        return Err(framing::FramingError::FrameTooLarge {
            len,
            max: MAX_ASSET_LEN,
        }
        .into());
    }
    let header = AssetHeader {
        content_hash: content_hash(bytes),
        len,
    };
    super::write_message(writer, &super::ClientMessage::Asset(header))?;
    framing::write_frame(writer, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_sha256_and_hex_is_64_lowercase_chars() {
        // SHA-256("abc"), FIPS 180-2 appendix B.1.
        let hex = hash_hex(&content_hash(b"abc"));
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn a_payload_round_trips_and_is_verified() {
        let bytes = b"#?RADIANCE fake payload".to_vec();
        let header = AssetHeader {
            content_hash: content_hash(&bytes),
            len: bytes.len() as u32,
        };
        let mut wire = Vec::new();
        framing::write_frame(&mut wire, &bytes).unwrap();
        let read = read_asset_payload(&mut std::io::Cursor::new(&wire), &header).unwrap();
        assert_eq!(read, bytes);

        let wrong = AssetHeader {
            content_hash: [7; 32],
            ..header
        };
        assert!(matches!(
            read_asset_payload(&mut std::io::Cursor::new(&wire), &wrong),
            Err(AssetError::HashMismatch)
        ));
        let shorter = AssetHeader {
            len: header.len + 1,
            ..header
        };
        assert!(matches!(
            read_asset_payload(&mut std::io::Cursor::new(&wire), &shorter),
            Err(AssetError::LengthMismatch { .. })
        ));
    }

    /// An oversized declared length is refused from the header alone: no payload bytes
    /// exist, so reading first would report an I/O error instead.
    #[test]
    fn an_oversized_asset_is_refused_before_reading() {
        let header = AssetHeader {
            content_hash: [0; 32],
            len: MAX_ASSET_LEN + 1,
        };
        assert!(matches!(
            read_asset_payload(&mut std::io::Cursor::new(Vec::new()), &header),
            Err(AssetError::TooLarge { .. })
        ));
    }

    /// The frame's own length prefix may not exceed the header's `len` either.
    #[test]
    fn a_frame_longer_than_its_header_is_refused() {
        let bytes = vec![1u8; 64];
        let header = AssetHeader {
            content_hash: content_hash(&bytes),
            len: 8,
        };
        let mut wire = Vec::new();
        framing::write_frame(&mut wire, &bytes).unwrap();
        assert!(matches!(
            read_asset_payload(&mut std::io::Cursor::new(&wire), &header),
            Err(AssetError::Framing(
                framing::FramingError::FrameTooLarge { .. }
            ))
        ));
    }

    #[cfg(feature = "render")]
    #[test]
    fn write_asset_message_round_trips_through_the_tagged_envelope() {
        let bytes = b"hdr bytes".to_vec();
        let mut wire = Vec::new();
        write_asset_message(&mut wire, &bytes).unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let msg: super::super::ClientMessage = super::super::read_message(&mut cursor).unwrap();
        let super::super::ClientMessage::Asset(header) = msg else {
            panic!("expected ClientMessage::Asset, got {msg:?}");
        };
        assert_eq!(header.content_hash, content_hash(&bytes));
        assert_eq!(read_asset_payload(&mut cursor, &header).unwrap(), bytes);
    }
}
