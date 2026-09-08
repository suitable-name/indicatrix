//! The generic framed-message codec every other `messages` submodule builds on:
//! [`NetError`], [`write_message`], [`read_message`].
//!
//! Split out of the original single `messages.rs` (see `super`'s module doc comment for
//! why) because these three items are the one part every other submodule -- `hello`,
//! `render`, `stream` -- depends on, none of them depend on each other.

use crate::framing::{self, FramingError};
use serde::{Serialize, de::DeserializeOwned};

/// Errors from sending or receiving any message on the wire.
#[derive(Debug)]
pub enum NetError {
    Framing(FramingError),
    Postcard(postcard::Error),
    /// A `FRAME` message's [`super::stream::FrameHeader::payload_len`] didn't match the
    /// number of bytes actually read for the raw radiance frame that followed it.
    FramePayloadLenMismatch {
        declared: u32,
        actual: usize,
    },
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Framing(e) => write!(f, "{e}"),
            Self::Postcard(e) => write!(f, "postcard error: {e}"),
            Self::FramePayloadLenMismatch { declared, actual } => {
                write!(
                    f,
                    "FRAME header declared payload_len={declared} but the payload frame carried {actual} bytes"
                )
            }
        }
    }
}

impl std::error::Error for NetError {}

impl From<FramingError> for NetError {
    fn from(e: FramingError) -> Self {
        Self::Framing(e)
    }
}

impl From<postcard::Error> for NetError {
    fn from(e: postcard::Error) -> Self {
        Self::Postcard(e)
    }
}

/// Encodes `msg` with `postcard` and writes it as one length-prefixed frame.
///
/// Works for any `Serialize` message that isn't the raw-bytes `FRAME`/`PREVIEW` case
/// (see [`super::stream::write_frame_message`]/[`super::stream::write_preview_message`]
/// for those).
///
/// # Errors
///
/// Returns [`NetError::Postcard`] if `msg` fails to serialize, or
/// [`NetError::Framing`] if writing the resulting frame fails.
pub fn write_message<W: std::io::Write, T: Serialize>(
    writer: &mut W,
    msg: &T,
) -> Result<(), NetError> {
    let bytes = postcard::to_allocvec(msg)?;
    framing::write_frame(writer, &bytes)?;
    Ok(())
}

/// Reads one length-prefixed frame and `postcard`-decodes it as `T`. The inverse of
/// [`write_message`].
///
/// # Errors
///
/// Returns [`NetError::Framing`] if reading the frame fails, or [`NetError::Postcard`]
/// if the frame's bytes don't decode as `T`.
pub fn read_message<R: std::io::Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, NetError> {
    let bytes = framing::read_frame(reader)?;
    let msg = postcard::from_bytes(&bytes)?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{PROTOCOL_VERSION, hello::Hello},
        *,
    };

    /// A `Read` that only ever hands back 2 bytes per call, regardless of the caller's
    /// buffer size -- forces `read_message` to exercise `read_exact`'s partial-read
    /// loop rather than getting everything in one call.
    struct Dribble {
        data: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for Dribble {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = (self.data.len() - self.pos).min(2).min(out.len());
            out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn messages_split_across_a_dribbling_reader_still_decode() {
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            build_hash: [7; 8],
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &hello).unwrap();

        let mut reader = Dribble { data: buf, pos: 0 };
        let decoded: Hello = read_message(&mut reader).unwrap();
        assert_eq!(decoded, hello);
    }
}
