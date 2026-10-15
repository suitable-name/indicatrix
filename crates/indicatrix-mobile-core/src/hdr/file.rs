//! The stored copy of the imported HDR map: `<files>/hdr/current.envmap`.
//!
//! A small binary format of our own, so the (downscaled) map reloads without the `.hdr`
//! decoder and without the original file:
//!
//! | Bytes | Content |
//! |---|---|
//! | 8 | magic `IXENVMAP` |
//! | 4 | version, little endian (1) |
//! | 4 | width in texels, little endian |
//! | 4 | height in texels, little endian |
//! | `width * height * 12` | texels, row-major, each `r g b` as little-endian `f32` |
//!
//! Writes go to a temporary file first and are renamed into place, so a crash never leaves
//! a half-written map. A file that is truncated, has another magic or version, a size
//! outside the stored limits, or a texel that is not a finite non-negative number is
//! refused as a whole.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
};

use super::scale::{MAX_HEIGHT, MAX_WIDTH};

/// The first eight bytes of the file.
const MAGIC: &[u8; 8] = b"IXENVMAP";
/// The format version this code reads and writes.
const VERSION: u32 = 1;
/// Bytes before the first texel.
const HEADER_LEN: usize = 20;

/// Why a stored map could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FileError {
    /// The file could not be opened or read.
    Io(String),
    /// The file is not a valid stored map.
    Corrupt(&'static str),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(f, "the stored HDR map could not be read: {message}"),
            Self::Corrupt(why) => write!(f, "the stored HDR map is damaged ({why})"),
        }
    }
}

/// Reads and checks the 20-byte header: `(width, height)`.
fn parse_header(header: &[u8; HEADER_LEN]) -> Result<(u32, u32), FileError> {
    if &header[..8] != MAGIC {
        return Err(FileError::Corrupt("wrong magic"));
    }
    let word = |at: usize| {
        u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    if word(8) != VERSION {
        return Err(FileError::Corrupt("unknown version"));
    }
    let (width, height) = (word(12), word(16));
    let in_range =
        |value: u32, max: usize| value > 0 && usize::try_from(value).is_ok_and(|v| v <= max);
    if !in_range(width, MAX_WIDTH) || !in_range(height, MAX_HEIGHT) {
        return Err(FileError::Corrupt("size out of range"));
    }
    Ok((width, height))
}

/// The size recorded in the file's header, without reading the texels.
///
/// # Errors
///
/// [`FileError`] when the file is missing, short, or its header is not valid.
pub(super) fn read_size(path: &Path) -> Result<(u32, u32), FileError> {
    let mut file = File::open(path).map_err(|e| FileError::Io(e.to_string()))?;
    let mut header = [0_u8; HEADER_LEN];
    file.read_exact(&mut header)
        .map_err(|_| FileError::Corrupt("short header"))?;
    parse_header(&header)
}

/// The whole map: `(width, height, texels)`.
///
/// # Errors
///
/// [`FileError`] for a missing, short or damaged file.
pub(super) fn read(path: &Path) -> Result<(u32, u32, Vec<[f32; 3]>), FileError> {
    let file = File::open(path).map_err(|e| FileError::Io(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut header = [0_u8; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|_| FileError::Corrupt("short header"))?;
    let (width, height) = parse_header(&header)?;
    // `parse_header` bounded both by the stored limits, so this fits in memory and `usize`.
    let count = width as usize * height as usize;
    let mut texels = Vec::with_capacity(count);
    let mut raw = [0_u8; 12];
    for _ in 0..count {
        reader
            .read_exact(&mut raw)
            .map_err(|_| FileError::Corrupt("truncated texels"))?;
        let channel =
            |at: usize| f32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
        let texel = [channel(0), channel(4), channel(8)];
        if texel.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(FileError::Corrupt("a texel is not a finite radiance"));
        }
        texels.push(texel);
    }
    // Anything after the last texel means the file is not what the header says.
    let mut extra = [0_u8; 1];
    if reader
        .read(&mut extra)
        .map_err(|e| FileError::Io(e.to_string()))?
        != 0
    {
        return Err(FileError::Corrupt("trailing bytes"));
    }
    Ok((width, height, texels))
}

/// Writes the map atomically (temporary file, then rename).
///
/// # Errors
///
/// The I/O error text, or a sentence when `texels` does not match the size.
pub(super) fn write(
    path: &Path,
    width: u32,
    height: u32,
    texels: &[[f32; 3]],
) -> Result<(), String> {
    if texels.len() != width as usize * height as usize {
        return Err("The HDR map has the wrong number of texels.".to_owned());
    }
    let temporary = path.with_extension("envmap.tmp");
    let written = (|| -> std::io::Result<()> {
        let mut writer = BufWriter::new(File::create(&temporary)?);
        writer.write_all(MAGIC)?;
        writer.write_all(&VERSION.to_le_bytes())?;
        writer.write_all(&width.to_le_bytes())?;
        writer.write_all(&height.to_le_bytes())?;
        for texel in texels {
            for channel in texel {
                writer.write_all(&channel.to_le_bytes())?;
            }
        }
        writer.flush()?;
        writer
            .into_inner()
            .map_err(std::io::IntoInnerError::into_error)?
            .sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    written.map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("The HDR map could not be saved: {e}.")
    })
}
