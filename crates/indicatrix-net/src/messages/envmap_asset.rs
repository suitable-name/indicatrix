//! The float environment-map asset (v25): an exact, versioned interchange for an HDR map
//! that is not a Radiance `.hdr` file.
//!
//! A Radiance file loses nothing a viewer started from, but a viewer that holds only the
//! map it LIGHTS with (the phone keeps a downscaled copy of the imported panorama) has no
//! `.hdr` file to send, and re-encoding to RGBE would round every texel. This asset carries
//! the effective texels as they are: little-endian `f32` triples.
//!
//! | Bytes | Content |
//! |---|---|
//! | 8 | magic `IXENVMAP` |
//! | 4 | version, little endian ([`ENVMAP_VERSION`], currently 1) |
//! | 4 | width in texels, little endian |
//! | 4 | height in texels, little endian |
//! | `width * height * 12` | texels, row-major, each `r g b` as little-endian `f32` |
//!
//! The layout is byte-identical to the phone's stored `current.envmap` file, so that file IS
//! a valid asset. It rides the existing content-addressed asset exchange unchanged
//! ([`super::asset`]): the scene names it by [`super::asset::content_hash`] of these exact
//! bytes, a server that lacks it answers `NEED_ASSET`, and the upload is bounded by
//! [`super::asset::MAX_ASSET_LEN`] and verified against the hash before anything is decoded.
//! A server tells the two formats apart by the magic (Radiance files start with `#?`), and
//! builds the sampler from the texels with `EnvironmentMap::from_rgb` -- the same
//! constructor the phone used -- so both sides hold the bit-identical map.
//!
//! Capability: a v25 render server whose `RenderCapability::hdr` is `true` decodes both
//! formats. A peer on another protocol version never pairs (see
//! [`crate::handshake::verify_compatible`]), so `hdr` at v25 is the advertisement that
//! this asset is understood.
//!
//! Every texel must be finite and non-negative. A header whose declared size does not match
//! the byte length exactly is refused before any texel is read.

use super::asset::{ContentHash, MAX_ASSET_LEN};
use sha2::{Digest, Sha256};

/// The first eight bytes of the asset.
pub const ENVMAP_MAGIC: [u8; 8] = *b"IXENVMAP";
/// The format version this build reads and writes.
pub const ENVMAP_VERSION: u32 = 1;
/// Bytes before the first texel.
pub const ENVMAP_HEADER_LEN: usize = 20;
/// The widest map the format may declare (the same bound as `HdrLimits::DEFAULT`).
pub const ENVMAP_MAX_WIDTH: u32 = 16_384;
/// The tallest map the format may declare (the same bound as `HdrLimits::DEFAULT`).
pub const ENVMAP_MAX_HEIGHT: u32 = 8_192;
/// Bytes of one texel.
const TEXEL_BYTES: usize = 12;
/// Texels written per hashing or copy step.
const BLOCK_TEXELS: usize = 1024;

/// Why an environment-map asset could not be encoded or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvmapAssetError {
    /// The bytes do not start with the `IXENVMAP` magic or are shorter than the header.
    NotAnEnvmap,
    /// The header names a version this build does not read.
    UnknownVersion(u32),
    /// A declared side is zero, over [`ENVMAP_MAX_WIDTH`] x [`ENVMAP_MAX_HEIGHT`], or the
    /// whole asset would exceed [`MAX_ASSET_LEN`].
    BadSize {
        /// Declared width in texels.
        width: u32,
        /// Declared height in texels.
        height: u32,
    },
    /// The byte length (or texel count) differs from what the header declares.
    LengthMismatch {
        /// The length the header implies.
        expected: u64,
        /// The length actually present.
        actual: u64,
    },
    /// A texel channel is not a finite non-negative number.
    BadTexel {
        /// The texel's index in row-major order.
        index: usize,
    },
}

impl std::fmt::Display for EnvmapAssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnEnvmap => write!(f, "not an IXENVMAP environment-map asset"),
            Self::UnknownVersion(version) => write!(
                f,
                "IXENVMAP version {version} is not supported (this build reads {ENVMAP_VERSION})"
            ),
            Self::BadSize { width, height } => write!(
                f,
                "IXENVMAP declares {width}x{height} texels; it must be non-empty, at most \
                 {ENVMAP_MAX_WIDTH}x{ENVMAP_MAX_HEIGHT} and within the {} MiB asset limit",
                MAX_ASSET_LEN / (1024 * 1024)
            ),
            Self::LengthMismatch { expected, actual } => write!(
                f,
                "IXENVMAP has {actual} bytes but its header implies {expected}"
            ),
            Self::BadTexel { index } => write!(
                f,
                "IXENVMAP texel {index} is not a finite non-negative radiance"
            ),
        }
    }
}

impl std::error::Error for EnvmapAssetError {}

/// Whether `bytes` start with the `IXENVMAP` magic (a cheap sniff; the rest is not checked).
#[must_use]
pub fn is_envmap_asset(bytes: &[u8]) -> bool {
    bytes.len() >= ENVMAP_MAGIC.len() && bytes[..ENVMAP_MAGIC.len()] == ENVMAP_MAGIC
}

/// The exact byte length of an asset of `width` x `height` texels, or `None` for a size
/// the format refuses (see [`EnvmapAssetError::BadSize`]).
#[must_use]
pub fn envmap_asset_len(width: u32, height: u32) -> Option<u64> {
    if width == 0 || height == 0 || width > ENVMAP_MAX_WIDTH || height > ENVMAP_MAX_HEIGHT {
        return None;
    }
    let texels = u64::from(width) * u64::from(height);
    let len = texels * TEXEL_BYTES as u64 + ENVMAP_HEADER_LEN as u64;
    (len <= u64::from(MAX_ASSET_LEN)).then_some(len)
}

/// Reads and checks the header of `bytes`: `(width, height)`.
///
/// The declared size is validated and the byte length must equal it exactly, so a
/// truncated or padded asset is refused here, before any texel is touched.
///
/// # Errors
///
/// [`EnvmapAssetError`] for a wrong magic or version, a size the format refuses, or a
/// length that disagrees with the header.
pub fn envmap_asset_header(bytes: &[u8]) -> Result<(u32, u32), EnvmapAssetError> {
    if bytes.len() < ENVMAP_HEADER_LEN || !is_envmap_asset(bytes) {
        return Err(EnvmapAssetError::NotAnEnvmap);
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if word(8) != ENVMAP_VERSION {
        return Err(EnvmapAssetError::UnknownVersion(word(8)));
    }
    let (width, height) = (word(12), word(16));
    let expected =
        envmap_asset_len(width, height).ok_or(EnvmapAssetError::BadSize { width, height })?;
    if bytes.len() as u64 != expected {
        return Err(EnvmapAssetError::LengthMismatch {
            expected,
            actual: bytes.len() as u64,
        });
    }
    Ok((width, height))
}

/// Feeds the asset bytes of `texels` to `sink`, in order, in blocks.
///
/// The one writer behind [`encode_envmap_asset`] and [`envmap_asset_hash`], so the hash is
/// always the hash of exactly the bytes that would be sent.
fn emit(
    width: u32,
    height: u32,
    texels: &[[f32; 3]],
    mut sink: impl FnMut(&[u8]),
) -> Result<(), EnvmapAssetError> {
    envmap_asset_len(width, height).ok_or(EnvmapAssetError::BadSize { width, height })?;
    let want_texels = u64::from(width) * u64::from(height);
    if texels.len() as u64 != want_texels {
        return Err(EnvmapAssetError::LengthMismatch {
            expected: want_texels,
            actual: texels.len() as u64,
        });
    }
    sink(&ENVMAP_MAGIC);
    sink(&ENVMAP_VERSION.to_le_bytes());
    sink(&width.to_le_bytes());
    sink(&height.to_le_bytes());
    let mut block = [0_u8; BLOCK_TEXELS * TEXEL_BYTES];
    for (number, group) in texels.chunks(BLOCK_TEXELS).enumerate() {
        let mut at = 0;
        for (offset, texel) in group.iter().enumerate() {
            for channel in texel {
                if !channel.is_finite() || *channel < 0.0 {
                    return Err(EnvmapAssetError::BadTexel {
                        index: number * BLOCK_TEXELS + offset,
                    });
                }
                block[at..at + 4].copy_from_slice(&channel.to_le_bytes());
                at += 4;
            }
        }
        sink(&block[..at]);
    }
    Ok(())
}

/// Encodes `texels` (row-major, `width * height` entries) as an asset.
///
/// # Errors
///
/// [`EnvmapAssetError::BadSize`] for a size the format refuses,
/// [`EnvmapAssetError::LengthMismatch`] when `texels` is not `width * height` long, and
/// [`EnvmapAssetError::BadTexel`] for a channel that is not finite and non-negative.
pub fn encode_envmap_asset(
    width: u32,
    height: u32,
    texels: &[[f32; 3]],
) -> Result<Vec<u8>, EnvmapAssetError> {
    let len = envmap_asset_len(width, height).ok_or(EnvmapAssetError::BadSize { width, height })?;
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    emit(width, height, texels, |piece| out.extend_from_slice(piece))?;
    Ok(out)
}

/// The [`ContentHash`] [`encode_envmap_asset`]'s bytes would have, without building them.
///
/// Streams the header and texels through SHA-256 in small blocks, so a 25 MiB map costs
/// no 25 MiB allocation. A scene names the asset by this hash.
///
/// # Errors
///
/// As [`encode_envmap_asset`].
pub fn envmap_asset_hash(
    width: u32,
    height: u32,
    texels: &[[f32; 3]],
) -> Result<ContentHash, EnvmapAssetError> {
    let mut hasher = Sha256::new();
    emit(width, height, texels, |piece| hasher.update(piece))?;
    Ok(hasher.finalize().into())
}

/// Decodes an asset into `(width, height, texels)`.
///
/// # Errors
///
/// [`EnvmapAssetError`] from [`envmap_asset_header`], or [`EnvmapAssetError::BadTexel`] for
/// a channel that is not finite and non-negative.
pub fn decode_envmap_asset(bytes: &[u8]) -> Result<(u32, u32, Vec<[f32; 3]>), EnvmapAssetError> {
    let (width, height) = envmap_asset_header(bytes)?;
    // `envmap_asset_header` proved the length equals `20 + width * height * 12`, which the
    // asset bound keeps far below `usize::MAX`.
    let count = width as usize * height as usize;
    let mut texels = Vec::with_capacity(count);
    for (index, raw) in bytes[ENVMAP_HEADER_LEN..]
        .as_chunks::<TEXEL_BYTES>()
        .0
        .iter()
        .enumerate()
    {
        let channel =
            |at: usize| f32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
        let texel = [channel(0), channel(4), channel(8)];
        if texel.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return Err(EnvmapAssetError::BadTexel { index });
        }
        texels.push(texel);
    }
    Ok((width, height, texels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::asset::content_hash;

    fn texels(width: u32, height: u32) -> Vec<[f32; 3]> {
        (0..width * height)
            .map(|i| [i as f32 * 0.25, 1.0 / (1.0 + i as f32), (i % 7) as f32])
            .collect()
    }

    #[test]
    fn an_asset_round_trips_bit_for_bit() {
        let source = texels(5, 3);
        let bytes = encode_envmap_asset(5, 3, &source).unwrap();
        assert_eq!(bytes.len(), 20 + 5 * 3 * 12);
        assert!(is_envmap_asset(&bytes));
        assert_eq!(envmap_asset_header(&bytes), Ok((5, 3)));
        let (width, height, back) = decode_envmap_asset(&bytes).unwrap();
        assert_eq!((width, height), (5, 3));
        for (a, b) in source.iter().zip(&back) {
            assert!(a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()));
        }
    }

    #[test]
    fn the_streaming_hash_is_the_hash_of_the_encoded_bytes() {
        // More texels than one block, so the block boundary is exercised.
        let source = texels(70, 30);
        assert!(source.len() > BLOCK_TEXELS);
        let bytes = encode_envmap_asset(70, 30, &source).unwrap();
        assert_eq!(
            envmap_asset_hash(70, 30, &source).unwrap(),
            content_hash(&bytes)
        );
    }

    #[test]
    fn the_layout_is_the_documented_one() {
        let bytes = encode_envmap_asset(1, 1, &[[1.0, 2.0, 3.0]]).unwrap();
        let mut expected = b"IXENVMAP".to_vec();
        expected.extend_from_slice(&1_u32.to_le_bytes());
        expected.extend_from_slice(&1_u32.to_le_bytes());
        expected.extend_from_slice(&1_u32.to_le_bytes());
        for value in [1.0_f32, 2.0, 3.0] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(bytes, expected);
    }

    #[test]
    fn malformed_assets_are_refused_before_any_texel_is_used() {
        let good = encode_envmap_asset(2, 2, &texels(2, 2)).unwrap();
        assert_eq!(
            envmap_asset_header(b"#?RADIANCE\n"),
            Err(EnvmapAssetError::NotAnEnvmap)
        );
        assert_eq!(
            envmap_asset_header(&good[..10]),
            Err(EnvmapAssetError::NotAnEnvmap)
        );
        // Truncated and padded.
        assert!(matches!(
            envmap_asset_header(&good[..good.len() - 1]),
            Err(EnvmapAssetError::LengthMismatch { .. })
        ));
        let mut padded = good.clone();
        padded.push(0);
        assert!(matches!(
            envmap_asset_header(&padded),
            Err(EnvmapAssetError::LengthMismatch { .. })
        ));
        // Another version.
        let mut other = good.clone();
        other[8] = 2;
        assert_eq!(
            envmap_asset_header(&other),
            Err(EnvmapAssetError::UnknownVersion(2))
        );
        // A zero or oversized side.
        let mut zero = good.clone();
        zero[12..16].copy_from_slice(&0_u32.to_le_bytes());
        assert!(matches!(
            envmap_asset_header(&zero),
            Err(EnvmapAssetError::BadSize { .. })
        ));
        let mut wide = good;
        wide[12..16].copy_from_slice(&(ENVMAP_MAX_WIDTH + 1).to_le_bytes());
        assert!(matches!(
            envmap_asset_header(&wide),
            Err(EnvmapAssetError::BadSize { .. })
        ));
    }

    #[test]
    fn a_declared_size_over_the_asset_limit_is_refused_from_the_header() {
        // 16384 x 8192 x 12 bytes is 1.5 GiB, far over MAX_ASSET_LEN.
        assert_eq!(envmap_asset_len(ENVMAP_MAX_WIDTH, ENVMAP_MAX_HEIGHT), None);
        assert!(envmap_asset_len(2048, 1024).is_some());
    }

    #[test]
    fn non_finite_and_negative_texels_are_refused_both_ways() {
        for bad in [f32::NAN, f32::INFINITY, -0.5] {
            let source = [[0.0, bad, 1.0]];
            assert_eq!(
                encode_envmap_asset(1, 1, &source),
                Err(EnvmapAssetError::BadTexel { index: 0 })
            );
            assert_eq!(
                envmap_asset_hash(1, 1, &source),
                Err(EnvmapAssetError::BadTexel { index: 0 })
            );
            let mut bytes = encode_envmap_asset(1, 1, &[[0.0; 3]]).unwrap();
            bytes[24..28].copy_from_slice(&bad.to_le_bytes());
            assert_eq!(
                decode_envmap_asset(&bytes),
                Err(EnvmapAssetError::BadTexel { index: 0 })
            );
        }
    }

    #[test]
    fn a_texel_count_that_disagrees_with_the_size_is_refused() {
        assert!(matches!(
            encode_envmap_asset(2, 2, &texels(2, 1)),
            Err(EnvmapAssetError::LengthMismatch { .. })
        ));
    }
}
