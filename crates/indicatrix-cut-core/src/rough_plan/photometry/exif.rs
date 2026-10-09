//! A small EXIF reader for JPEG files: exposure time, f-number, ISO, white-balance mode and
//! the camera's make and model.
//!
//! It reads only what the consistency check needs and never
//! fails: a file it cannot parse gives an empty [`CaptureMeta`].

use super::linear::CaptureMeta;

/// A TIFF-structured block with its byte order.
struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let bytes: [u8; 2] = self.data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let bytes: [u8; 4] = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    /// A RATIONAL stored at the offset in the entry's value field.
    fn rational(&self, field: usize) -> Option<f64> {
        let at = self.u32(field)? as usize;
        let numerator = f64::from(self.u32(at)?);
        let denominator = f64::from(self.u32(at.checked_add(4)?)?);
        (denominator > 0.0).then(|| numerator / denominator)
    }

    /// An ASCII value: inline when it fits the four bytes of the field, else at the offset.
    fn text(&self, field: usize, count: usize) -> Option<String> {
        let start = if count <= 4 {
            field
        } else {
            self.u32(field)? as usize
        };
        let bytes = self.data.get(start..start.checked_add(count)?)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let text = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    /// Calls `visit(tag, type, count, value_field_offset)` for every entry of the IFD.
    fn each_entry(&self, ifd: usize, mut visit: impl FnMut(u16, u16, usize, usize)) {
        let Some(entries) = self.u16(ifd) else {
            return;
        };
        for i in 0..usize::from(entries) {
            let at = ifd + 2 + 12 * i;
            let (Some(tag), Some(kind), Some(count)) =
                (self.u16(at), self.u16(at + 2), self.u32(at + 4))
            else {
                return;
            };
            visit(tag, kind, count as usize, at + 8);
        }
    }
}

/// The metadata of a TIFF-structured EXIF block (the bytes after `Exif\0\0`).
#[must_use]
pub fn parse_tiff_exif(block: &[u8]) -> CaptureMeta {
    let mut meta = CaptureMeta::default();
    let little = match block.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return meta,
    };
    let tiff = Tiff {
        data: block,
        little,
    };
    if tiff.u16(2) != Some(42) {
        return meta;
    }
    let Some(ifd0) = tiff.u32(4) else {
        return meta;
    };
    let mut exif_ifd = None;
    tiff.each_entry(ifd0 as usize, |tag, _kind, count, field| match tag {
        0x010F => meta.camera_make = tiff.text(field, count),
        0x0110 => meta.camera_model = tiff.text(field, count),
        0x8769 => exif_ifd = tiff.u32(field),
        _ => {}
    });
    if let Some(offset) = exif_ifd {
        tiff.each_entry(offset as usize, |tag, kind, _count, field| match tag {
            0x829A if kind == 5 => meta.exposure_time_s = tiff.rational(field),
            0x829D if kind == 5 => meta.f_number = tiff.rational(field),
            0x8827 if kind == 3 => meta.iso = tiff.u16(field).map(u32::from),
            0xA403 if kind == 3 => meta.white_balance_mode = tiff.u16(field),
            _ => {}
        });
    }
    meta
}

/// The metadata of a JPEG file's APP1 EXIF segment; empty when there is none.
#[must_use]
pub fn parse_jpeg_exif(bytes: &[u8]) -> CaptureMeta {
    if bytes.get(0..2) != Some(&[0xFF, 0xD8]) {
        return CaptureMeta::default();
    }
    let mut at = 2_usize;
    while let (Some(&0xFF), Some(&marker)) = (bytes.get(at), bytes.get(at + 1)) {
        if marker == 0xFF {
            at += 1;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let Some(length) = bytes
            .get(at + 2..at + 4)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
        else {
            break;
        };
        if length < 2 {
            break;
        }
        if marker == 0xE1
            && let Some(segment) = bytes.get(at + 4..at + 2 + length)
            && let Some(block) = segment.strip_prefix(b"Exif\0\0")
        {
            return parse_tiff_exif(block);
        }
        at += 2 + length;
    }
    CaptureMeta::default()
}
