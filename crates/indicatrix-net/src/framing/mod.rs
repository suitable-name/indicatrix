//! Length-prefixed message framing over any [`Read`]/[`Write`].
//!
//! Every message on the wire (see [`crate::messages`]) is preceded by a 4-byte
//! little-endian `u32` payload length. Framing is deliberately generic over `Read`/
//! `Write` rather than tied to a socket type -- that keeps it fully testable against an
//! in-memory [`std::io::Cursor`] with no networking at all, and the exact same functions
//! work unchanged over a `TcpStream` or a TLS stream, since both implement the same
//! traits.
//!
//! [`read_frame`] reads the payload with [`Read::take`] and [`Read::read_to_end`], which
//! loop internally until the declared length arrives or a real error/EOF occurs -- so a
//! reader that only hands back a few bytes per call (a slow socket, or a message that
//! arrives split across two TCP segments) is handled correctly with no special-casing
//! here. The payload buffer grows with the bytes received, so a lying length prefix
//! never commits memory before the data exists. See the `tests` module for a reader that
//! deliberately exercises the partial-read case.

use std::io::{self, Read, Write};

/// Hard cap on a single frame's payload length.
///
/// Guards against a corrupt or hostile length prefix causing an attempted
/// multi-gigabyte allocation before any content has even been read. Comfortably larger
/// than any radiance buffer this protocol is expected to carry (a 4K frame's `Vec3`
/// buffer is ~100 MiB).
pub const MAX_FRAME_LEN: u32 = 512 * 1024 * 1024;

/// Cap for every control message.
///
/// Covers `ClientMessage`, `StreamEvent`, enrollment and handshake messages. Legitimate
/// control messages are well under this; only the raw radiance, asset and contribution
/// payload frames that follow a header are larger, and those carry their own bounds.
pub const MAX_CONTROL_FRAME_LEN: u32 = 1024 * 1024;

/// How long an authenticated connection may idle between requests.
///
/// A connection that sends no message for this long is closed by the server. Peers
/// heartbeat (`PING`) far more often, so only a dead or hostile peer ever reaches it.
pub const IDLE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

/// Number of bytes in the length prefix itself.
pub const LEN_PREFIX_BYTES: usize = 4;

/// Payload capacity reserved up front; larger frames grow as bytes arrive.
const INITIAL_PAYLOAD_CAPACITY: usize = 1024 * 1024;

/// Everything that can go wrong reading or writing one length-prefixed frame.
#[derive(Debug)]
pub enum FramingError {
    /// The underlying reader/writer failed. A reader that ends exactly at a frame
    /// boundary (no byte of the next length prefix) reports `ErrorKind::UnexpectedEof`
    /// here -- the peer closed the connection cleanly between frames.
    Io(io::Error),
    /// The reader ended inside a frame: after part of a length prefix, or before the
    /// declared payload length arrived. Unlike a clean close at a frame boundary this is
    /// never a normal way for a connection to end.
    TruncatedFrame,
    /// The length prefix (read or about to be written) exceeds [`MAX_FRAME_LEN`].
    FrameTooLarge { len: u32, max: u32 },
}

impl std::fmt::Display for FramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "framing I/O error: {e}"),
            Self::TruncatedFrame => write!(f, "the connection closed in the middle of a frame"),
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
/// [`MAX_FRAME_LEN`], [`FramingError::Io`] if the underlying reader fails (an
/// `UnexpectedEof` there means it ended cleanly at a frame boundary), and
/// [`FramingError::TruncatedFrame`] if it ends inside a frame.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, FramingError> {
    read_frame_bounded(reader, MAX_FRAME_LEN)
}

/// Reads one length-prefixed frame exactly like [`read_frame`], but rejects any length
/// prefix greater than `max` instead of [`MAX_FRAME_LEN`] -- before allocating anything
/// for the payload.
///
/// For a listener that hasn't yet authenticated its peer (e.g. the enrollment
/// listener's bare TLS accept, which requires no client certificate), the request
/// shapes actually expected are a few dozen bytes; there is no reason to let an
/// unauthenticated peer's length prefix commit this process to allocating anywhere near
/// [`MAX_FRAME_LEN`] (512 MiB) before a single content byte has even arrived. [`read_frame`]
/// is exactly this function called with `max = MAX_FRAME_LEN`.
///
/// # Errors
///
/// Returns [`FramingError::FrameTooLarge`] (`max` reported as the cap) if the length
/// prefix exceeds `max`, and [`FramingError::Io`] if the underlying reader fails.
pub fn read_frame_bounded<R: Read>(reader: &mut R, max: u32) -> Result<Vec<u8>, FramingError> {
    let len = read_len_prefix(reader)?;
    if len > max {
        return Err(FramingError::FrameTooLarge { len, max });
    }
    read_payload(reader, len)
}

/// Reads one length-prefixed frame and drops its payload without buffering it, returning
/// the payload length that was skipped.
///
/// For a frame the caller has decided to ignore (an unrequested asset): the bound is
/// checked before anything is read, the bytes go straight to a sink, and a reader that
/// ends before the declared length reports [`FramingError::TruncatedFrame`].
///
/// # Errors
///
/// As [`read_frame_bounded`].
pub fn skip_frame_bounded<R: Read>(reader: &mut R, max: u32) -> Result<u32, FramingError> {
    let len = read_len_prefix(reader)?;
    if len > max {
        return Err(FramingError::FrameTooLarge { len, max });
    }
    let skipped = io::copy(&mut reader.take(u64::from(len)), &mut io::sink())?;
    if skipped != u64::from(len) {
        return Err(FramingError::TruncatedFrame);
    }
    Ok(len)
}

/// Reads the rest of a frame whose first length-prefix bytes the caller already consumed.
///
/// `already_read` holds one to four prefix bytes -- the "wait for the first byte with a
/// short timeout, then finish the frame" pattern of a poll loop. Behaves exactly like [`read_frame_bounded`] with `max`: the bound is checked before any
/// payload allocation, the payload buffer grows with the bytes received, and a reader
/// that ends inside the frame reports [`FramingError::TruncatedFrame`].
///
/// # Errors
///
/// As [`read_frame_bounded`].
///
/// # Panics
///
/// Panics if `already_read` is empty or longer than [`LEN_PREFIX_BYTES`].
pub fn read_frame_continuing<R: Read>(
    reader: &mut R,
    already_read: &[u8],
    max: u32,
) -> Result<Vec<u8>, FramingError> {
    assert!(
        (1..=LEN_PREFIX_BYTES).contains(&already_read.len()),
        "a continued frame has between one and four length-prefix bytes already read"
    );
    let mut len_bytes = [0u8; LEN_PREFIX_BYTES];
    len_bytes[..already_read.len()].copy_from_slice(already_read);
    let len = match complete_len_prefix(reader, len_bytes, already_read.len()) {
        Ok(len) => len,
        // A close right after the caller's first bytes is mid-frame, not a clean close.
        Err(FramingError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(FramingError::TruncatedFrame);
        }
        Err(e) => return Err(e),
    };
    if len > max {
        return Err(FramingError::FrameTooLarge { len, max });
    }
    read_payload(reader, len)
}

/// Reads the 4-byte length prefix, telling a clean close from a truncated one: EOF before
/// the first byte is `Io(UnexpectedEof)` (a close at a frame boundary), EOF after one to
/// three bytes is [`FramingError::TruncatedFrame`].
fn read_len_prefix<R: Read>(reader: &mut R) -> Result<u32, FramingError> {
    complete_len_prefix(reader, [0u8; LEN_PREFIX_BYTES], 0)
}

/// Finishes reading a length prefix whose first `filled` bytes are already in
/// `len_bytes`.
fn complete_len_prefix<R: Read>(
    reader: &mut R,
    mut len_bytes: [u8; LEN_PREFIX_BYTES],
    mut filled: usize,
) -> Result<u32, FramingError> {
    while filled < LEN_PREFIX_BYTES {
        match reader.read(&mut len_bytes[filled..]) {
            Ok(0) if filled == 0 => {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
            }
            Ok(0) => return Err(FramingError::TruncatedFrame),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(u32::from_le_bytes(len_bytes))
}

/// Reads exactly `len` payload bytes, growing the buffer as bytes arrive.
///
/// A peer that declares a large length and then stops sending never makes this process
/// commit more than [`INITIAL_PAYLOAD_CAPACITY`] bytes: the buffer is reserved up to that
/// size and then grows only with data actually received.
fn read_payload<R: Read>(reader: &mut R, len: u32) -> Result<Vec<u8>, FramingError> {
    let wanted = len as usize;
    let mut payload = Vec::with_capacity(wanted.min(INITIAL_PAYLOAD_CAPACITY));
    let got = reader.take(u64::from(len)).read_to_end(&mut payload)?;
    if got != wanted {
        return Err(FramingError::TruncatedFrame);
    }
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

    /// `write_frame`'s length prefix is little-endian, exactly the payload's byte
    /// length -- pinned so a change to the prefix's endianness or width is caught here
    /// rather than only as a cross-version wire mismatch.
    #[test]
    fn write_frame_pins_the_little_endian_length_prefix() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello world").unwrap();
        assert_eq!(&buf[..4], &[11, 0, 0, 0]);
    }

    /// The boundary itself -- `len == max` must succeed (not just `len < max`).
    #[test]
    fn read_frame_bounded_accepts_a_frame_exactly_at_the_bound() {
        let max = 16u32;
        let payload = vec![b'x'; max as usize];
        let mut buf = Vec::new();
        write_frame(&mut buf, &payload).unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame_bounded(&mut cursor, max).unwrap(), payload);
    }

    /// The other side of the boundary -- `len == max + 1` must be refused (not
    /// just some length far past `max`, which `rejects_a_length_prefix_over_the_cap`
    /// already covers for `read_frame`'s own `MAX_FRAME_LEN`).
    #[test]
    fn read_frame_bounded_rejects_exactly_one_byte_over_the_bound() {
        let max = 16u32;
        let mut buf = Vec::new();
        // No payload bytes follow -- the bound check must reject this from the 4-byte
        // prefix alone, before any attempt to read `max + 1` bytes of payload.
        buf.extend_from_slice(&(max + 1).to_le_bytes());
        let mut cursor = Cursor::new(buf);
        let err = read_frame_bounded(&mut cursor, max).unwrap_err();
        assert!(
            matches!(err, FramingError::FrameTooLarge { len, max: m } if len == max + 1 && m == max),
            "{err:?}"
        );
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
        assert!(matches!(
            read_frame(&mut cursor),
            Err(FramingError::TruncatedFrame)
        ));
    }

    /// EOF before the first byte of a prefix is a clean close at a frame boundary,
    /// reported as `Io(UnexpectedEof)` -- distinct from a truncated frame.
    #[test]
    fn eof_at_a_frame_boundary_is_a_clean_close() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"only").unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), b"only");
        let err = read_frame(&mut cursor).unwrap_err();
        assert!(
            matches!(&err, FramingError::Io(e) if e.kind() == io::ErrorKind::UnexpectedEof),
            "{err:?}"
        );
    }

    /// A caller that consumed the first prefix byte itself finishes the frame with
    /// `read_frame_continuing`, bound and truncation rules included.
    #[test]
    fn read_frame_continuing_finishes_a_frame_started_by_the_caller() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"rest of it").unwrap();
        let first = buf[0];
        let mut cursor = Cursor::new(buf[1..].to_vec());
        assert_eq!(
            read_frame_continuing(&mut cursor, &[first], 64).unwrap(),
            b"rest of it"
        );

        let mut oversized = Cursor::new(vec![0, 0, 0]);
        assert!(matches!(
            read_frame_continuing(&mut oversized, &[0x10], 8),
            Err(FramingError::FrameTooLarge { len: 16, max: 8 })
        ));

        let mut truncated = Cursor::new(Vec::new());
        assert!(matches!(
            read_frame_continuing(&mut truncated, &[1], 64),
            Err(FramingError::TruncatedFrame)
        ));
    }

    #[test]
    fn skip_frame_bounded_drops_the_payload_and_keeps_the_stream_in_sync() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"skip me").unwrap();
        write_frame(&mut buf, b"keep me").unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(skip_frame_bounded(&mut cursor, 64).unwrap(), 7);
        assert_eq!(read_frame(&mut cursor).unwrap(), b"keep me");

        let mut big = Vec::new();
        write_frame(&mut big, &[0u8; 32]).unwrap();
        assert!(matches!(
            skip_frame_bounded(&mut Cursor::new(big), 8),
            Err(FramingError::FrameTooLarge { len: 32, max: 8 })
        ));

        let mut short = Vec::new();
        write_frame(&mut short, &[0u8; 32]).unwrap();
        short.truncate(10);
        assert!(matches!(
            skip_frame_bounded(&mut Cursor::new(short), 64),
            Err(FramingError::TruncatedFrame)
        ));
    }

    #[test]
    fn errors_on_truncated_payload() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"0123456789").unwrap();
        buf.truncate(buf.len() - 3); // chop the last 3 payload bytes off
        let mut cursor = Cursor::new(buf);
        assert!(matches!(
            read_frame(&mut cursor),
            Err(FramingError::TruncatedFrame)
        ));
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

    /// A hostile length prefix declaring far more than `max` must be
    /// rejected from the 4-byte prefix alone, before `read_frame_bounded` ever attempts
    /// `vec![0u8; len as usize]` -- proven here by a reader whose payload doesn't
    /// actually contain `0x1FFF_FFFF` bytes at all; a pre-allocation would try to read
    /// them and this test would hang/error on the reader instead of returning cleanly.
    #[test]
    fn read_frame_bounded_rejects_an_oversized_prefix_without_allocating() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0x1FFF_FFFFu32.to_le_bytes());
        // Deliberately no payload bytes follow -- if the length prefix were honored
        // before the bound check, the subsequent `read_exact` would hit EOF instead of
        // `read_frame_bounded` reporting `FrameTooLarge` immediately.
        let mut cursor = Cursor::new(buf);
        let err = read_frame_bounded(&mut cursor, 4096).unwrap_err();
        assert!(
            matches!(
                err,
                FramingError::FrameTooLarge {
                    len: 0x1FFF_FFFF,
                    max: 4096
                }
            ),
            "{err:?}"
        );
    }

    /// A reader that records the largest buffer any `read` call was offered, then hits
    /// EOF once its data is exhausted.
    struct BufferProbe {
        data: Vec<u8>,
        pos: usize,
        largest_offer: usize,
    }

    impl Read for BufferProbe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.largest_offer = self.largest_offer.max(buf.len());
            let n = (self.data.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// A length prefix at the cap followed by EOF fails as an I/O error without the
    /// reader ever being offered more than a small buffer: the payload buffer grows with
    /// the bytes that actually arrive.
    #[test]
    fn a_lying_length_prefix_followed_by_eof_does_not_commit_the_declared_size() {
        let mut probe = BufferProbe {
            data: MAX_FRAME_LEN.to_le_bytes().to_vec(),
            pos: 0,
            largest_offer: 0,
        };
        let err = read_frame(&mut probe).unwrap_err();
        assert!(matches!(err, FramingError::TruncatedFrame), "{err:?}");
        assert!(
            probe.largest_offer <= 2 * INITIAL_PAYLOAD_CAPACITY,
            "reader was offered a {} byte buffer",
            probe.largest_offer
        );
    }

    /// A large frame whose bytes do arrive still round-trips through the incremental
    /// read.
    #[test]
    fn a_frame_larger_than_the_initial_capacity_round_trips() {
        let payload: Vec<u8> = (0..(INITIAL_PAYLOAD_CAPACITY * 2 + 123))
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        let mut buf = Vec::new();
        write_frame(&mut buf, &payload).unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), payload);
    }

    #[test]
    fn read_frame_bounded_accepts_a_frame_within_the_bound() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"small enough").unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(
            read_frame_bounded(&mut cursor, 4096).unwrap(),
            b"small enough"
        );
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
