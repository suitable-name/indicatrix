//! Length-prefixed message framing over any [`Read`]/[`Write`].
//!
//! Every message on the wire (see [`crate::messages`]) is preceded by a 4-byte
//! little-endian `u32` payload length. Framing is deliberately generic over `Read`/
//! `Write` rather than tied to a socket type -- that keeps it fully testable against an
//! in-memory [`std::io::Cursor`] with no networking at all (this crate has none), and
//! the exact same functions will later work unchanged over a `TcpStream` or a TLS
//! stream, since both implement the same traits.
//!
//! [`read_frame`] uses [`Read::read_exact`], which itself loops internally until the
//! buffer is full or a real error/EOF occurs -- so a reader that only hands back a few
//! bytes per call (a slow socket, or a message that arrives split across two TCP
//! segments) is handled correctly with no special-casing here. See the `tests` module
//! for a reader that deliberately exercises this.

use std::io::{self, Read, Write};

/// Hard cap on a single frame's payload length.
///
/// Guards against a corrupt or hostile length prefix causing an attempted
/// multi-gigabyte allocation before any content has even been read. Comfortably larger
/// than any radiance buffer this protocol is expected to carry (a 4K frame's `Vec3`
/// buffer is ~100 MiB).
pub const MAX_FRAME_LEN: u32 = 512 * 1024 * 1024;

/// Number of bytes in the length prefix itself.
pub const LEN_PREFIX_BYTES: usize = 4;

#[derive(Debug)]
pub enum FramingError {
    Io(io::Error),
    /// The length prefix (read or about to be written) exceeds [`MAX_FRAME_LEN`].
    FrameTooLarge {
        len: u32,
        max: u32,
    },
}

impl std::fmt::Display for FramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "framing I/O error: {e}"),
            Self::FrameTooLarge { len, max } => write!(f, "frame length {len} exceeds max {max}"),
        }
    }
}

impl std::error::Error for FramingError {}

impl From<io::Error> for FramingError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Writes one length-prefixed frame: a 4-byte little-endian length, then `payload`
/// verbatim.
///
/// # Errors
///
/// Returns [`FramingError::FrameTooLarge`] if `payload` exceeds [`MAX_FRAME_LEN`] (or
/// `u32::MAX`), and [`FramingError::Io`] if the underlying writer fails.
pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), FramingError> {
    let len = u32::try_from(payload.len()).map_err(|_| FramingError::FrameTooLarge {
        len: u32::MAX,
        max: MAX_FRAME_LEN,
    })?;
    if len > MAX_FRAME_LEN {
        return Err(FramingError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(payload)?;
    Ok(())
}

/// Reads one length-prefixed frame written by [`write_frame`], blocking (looping on
/// partial reads via `read_exact`) until the whole length prefix and payload have
/// arrived.
///
/// # Errors
///
/// Returns [`FramingError::FrameTooLarge`] if the length prefix exceeds
/// [`MAX_FRAME_LEN`], and [`FramingError::Io`] if the underlying reader fails or hits
/// EOF before the declared payload length is filled.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, FramingError> {
    let mut len_bytes = [0u8; LEN_PREFIX_BYTES];
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(FramingError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_a_single_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello world").unwrap();
        let mut cursor = Cursor::new(buf);
        let payload = read_frame(&mut cursor).unwrap();
        assert_eq!(payload, b"hello world");
    }

    #[test]
    fn round_trips_an_empty_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"").unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn reads_several_frames_back_to_back() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"first").unwrap();
        write_frame(&mut buf, b"second-longer").unwrap();
        write_frame(&mut buf, b"3").unwrap();

        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), b"first");
        assert_eq!(read_frame(&mut cursor).unwrap(), b"second-longer");
        assert_eq!(read_frame(&mut cursor).unwrap(), b"3");
    }

    #[test]
    fn errors_on_truncated_length_prefix() {
        let mut cursor = Cursor::new(vec![0u8, 1]); // only 2 of the 4 length bytes
        assert!(matches!(read_frame(&mut cursor), Err(FramingError::Io(_))));
    }

    #[test]
    fn errors_on_truncated_payload() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"0123456789").unwrap();
        buf.truncate(buf.len() - 3); // chop the last 3 payload bytes off
        let mut cursor = Cursor::new(buf);
        assert!(matches!(read_frame(&mut cursor), Err(FramingError::Io(_))));
    }

    #[test]
    fn rejects_a_length_prefix_over_the_cap() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_FRAME_LEN + 1).to_le_bytes());
        let mut cursor = Cursor::new(buf);
        assert!(matches!(
            read_frame(&mut cursor),
            Err(FramingError::FrameTooLarge { .. })
        ));
    }

    /// A `Read` that only ever hands back a handful of bytes per call, however large
    /// the caller's buffer is -- simulates a message arriving split across many small
    /// reads (e.g. TCP segments), which `read_exact`'s internal loop must handle
    /// transparently.
    struct DribbleReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }

    impl Read for DribbleReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let remaining = self.data.len() - self.pos;
            let n = remaining.min(self.chunk).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn survives_partial_reads_split_across_many_small_chunks() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"first-message").unwrap();
        write_frame(&mut buf, b"second-message-a-bit-longer").unwrap();

        for chunk in [1usize, 2, 3, 7] {
            let mut reader = DribbleReader {
                data: buf.clone(),
                pos: 0,
                chunk,
            };
            assert_eq!(
                read_frame(&mut reader).unwrap(),
                b"first-message",
                "chunk size {chunk}"
            );
            assert_eq!(
                read_frame(&mut reader).unwrap(),
                b"second-message-a-bit-longer",
                "chunk size {chunk}"
            );
        }
    }

    #[test]
    fn one_byte_reads_still_round_trip_a_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"exactly-one-byte-at-a-time").unwrap();
        let mut reader = DribbleReader {
            data: buf,
            pos: 0,
            chunk: 1,
        };
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            b"exactly-one-byte-at-a-time"
        );
    }
}
