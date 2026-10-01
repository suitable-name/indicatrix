//! Read/write helpers for [`super::types::StreamEvent`] and its `FRAME`/`PREVIEW`
//! variants: a `postcard`-encoded header frame plus, for `Frame`/`Preview`, a second raw
//! payload frame -- see the parent module docs for why the payload never goes through
//! `postcard`.

use super::types::{FrameHeader, PreviewHeader, StreamEvent};

/// Writes one [`StreamEvent`] reply.
///
/// `payload` must be `Some` exactly for the payload-carrying variants
/// ([`StreamEvent::payload_len`] is `Some`: `Frame`, `Preview`, `DisplayFrame`,
/// `FinalImage` -- the bytes the header describes, as encoded) and `None` for every
/// other variant -- a mismatch would silently corrupt the stream for whatever's read
/// next, so it is refused before anything is written.
///
/// # Errors
///
/// Returns [`crate::messages::codec::NetError::PayloadPresenceMismatch`] if `payload`'s
/// presence disagrees with the event's variant, and returns [`crate::messages::codec::NetError::Postcard`] or
/// [`crate::messages::codec::NetError::Framing`] under the same conditions as
/// [`crate::messages::codec::write_message`] (for the event) and
/// [`crate::framing::write_frame`] (for the raw payload, when present).
pub fn write_stream_event<W: std::io::Write>(
    writer: &mut W,
    event: &StreamEvent,
    payload: Option<&[u8]>,
) -> Result<(), crate::messages::codec::NetError> {
    if event.payload_len().is_some() != payload.is_some() {
        return Err(crate::messages::codec::NetError::PayloadPresenceMismatch);
    }
    crate::messages::codec::write_message(writer, event)?;
    if let Some(bytes) = payload {
        crate::framing::write_frame(writer, bytes)?;
    }
    Ok(())
}

/// Reads one [`StreamEvent`] reply written by [`write_stream_event`].
///
/// Includes its raw payload frame when the decoded variant carries one
/// ([`StreamEvent::payload_len`]), validated against that header's declared
/// `payload_len`. The payload is returned as sent (possibly compressed); decoding it is
/// the caller's job (`crate::client::Accumulator` does it for FRAME/PREVIEW).
///
/// # Errors
///
/// Returns [`crate::messages::codec::NetError::Postcard`] or
/// [`crate::messages::codec::NetError::Framing`] under the same conditions as
/// [`crate::messages::codec::read_message`] (for the event) and
/// [`crate::framing::read_frame`] (for the raw payload, when present), or
/// [`crate::messages::codec::NetError::FramePayloadLenMismatch`] if a `Frame`/`Preview`
/// header's declared `payload_len` disagrees with the raw payload frame's actual length.
pub fn read_stream_event<R: std::io::Read>(
    reader: &mut R,
) -> Result<(StreamEvent, Option<Vec<u8>>), crate::messages::codec::NetError> {
    let event: StreamEvent = crate::messages::codec::read_control_message(reader)?;
    let payload = match event.payload_len() {
        Some(expected) => {
            let bytes = crate::framing::read_frame(reader)?;
            if bytes.len() as u32 != expected {
                return Err(crate::messages::codec::NetError::FramePayloadLenMismatch {
                    declared: expected,
                    actual: bytes.len(),
                });
            }
            Some(bytes)
        }
        None => None,
    };
    Ok((event, payload))
}

/// Writes a `<- FRAME` message: a `postcard`-encoded [`FrameHeader`] frame, followed by
/// a second frame carrying `xyz_bytes` completely raw.
///
/// `header.payload_len` must equal `xyz_bytes.len()`, asserted by the caller's
/// construction rather than re-derived here -- use [`FrameHeader::for_payload`] to build
/// a consistent pair.
///
/// # Errors
///
/// Returns [`crate::messages::codec::NetError::Postcard`] or
/// [`crate::messages::codec::NetError::Framing`] under the same conditions as
/// [`crate::messages::codec::write_message`] (for the header) and
/// [`crate::framing::write_frame`] (for the raw payload).
pub fn write_frame_message<W: std::io::Write>(
    writer: &mut W,
    header: &FrameHeader,
    xyz_bytes: &[u8],
) -> Result<(), crate::messages::codec::NetError> {
    crate::messages::codec::write_message(writer, header)?;
    crate::framing::write_frame(writer, xyz_bytes)?;
    Ok(())
}

/// Reads a `<- FRAME` message written by [`write_frame_message`], validating that
/// `payload_len` matches the raw payload frame's actual byte count.
///
/// # Errors
///
/// See [`write_frame_message`]'s errors, plus
/// [`crate::messages::codec::NetError::FramePayloadLenMismatch`] if the header's declared
/// `payload_len` disagrees with the raw payload frame's actual length.
pub fn read_frame_message<R: std::io::Read>(
    reader: &mut R,
) -> Result<(FrameHeader, Vec<u8>), crate::messages::codec::NetError> {
    let header: FrameHeader = crate::messages::codec::read_control_message(reader)?;
    let payload = crate::framing::read_frame(reader)?;
    if payload.len() as u32 != header.payload_len {
        return Err(crate::messages::codec::NetError::FramePayloadLenMismatch {
            declared: header.payload_len,
            actual: payload.len(),
        });
    }
    Ok((header, payload))
}

/// Writes a `<- PREVIEW` message: a `postcard`-encoded [`PreviewHeader`] frame, followed
/// by a second frame carrying `xyz_bytes` completely raw.
///
/// `xyz_bytes` is a reduced-resolution radiance buffer, encoded via
/// [`crate::radiance::encode`]. See the module docs for why a `PREVIEW` payload is
/// CUMULATIVE, never summed the way a `FRAME` payload is.
///
/// # Errors
///
/// See [`write_frame_message`]'s errors.
pub fn write_preview_message<W: std::io::Write>(
    writer: &mut W,
    header: &PreviewHeader,
    xyz_bytes: &[u8],
) -> Result<(), crate::messages::codec::NetError> {
    crate::messages::codec::write_message(writer, header)?;
    crate::framing::write_frame(writer, xyz_bytes)?;
    Ok(())
}

/// Reads a `<- PREVIEW` message written by [`write_preview_message`].
///
/// # Errors
///
/// See [`read_frame_message`]'s errors.
pub fn read_preview_message<R: std::io::Read>(
    reader: &mut R,
) -> Result<(PreviewHeader, Vec<u8>), crate::messages::codec::NetError> {
    let header: PreviewHeader = crate::messages::codec::read_control_message(reader)?;
    let payload = crate::framing::read_frame(reader)?;
    if payload.len() as u32 != header.payload_len {
        return Err(crate::messages::codec::NetError::FramePayloadLenMismatch {
            declared: header.payload_len,
            actual: payload.len(),
        });
    }
    Ok((header, payload))
}
