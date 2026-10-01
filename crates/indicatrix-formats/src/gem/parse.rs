//! [`parse_gem`]: the structural `.gem` decoder (layout in the module docs).

use super::{
    error::GemParseError,
    model::{GemDesign, GemFacet},
};
use crate::encoding::decode_windows_1252_or_utf8;

/// The `f64` that stands in place of the next facet's plane `x` to end the facet
/// list (bytes `00 00 00 00 f0 69 f8 c0`).
const SENTINEL: f64 = -99_999.0;

/// The tag that opens an embedded preform section: a length byte `7` followed by
/// the seven bytes `preform`.
const PREFORM_TAG: &[u8] = b"\x07preform";

/// How many `preform` sections may be entered, one inside the other: a file holds
/// at most one, directly after the main design, so a second tag is malformed
/// rather than a reason to keep descending.
const MAX_PREFORM_DEPTH: usize = 1;

/// How a string's length prefix is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LengthPrefix {
    /// One byte, `0..=255` (every corpus file).
    Byte,
    /// A .NET 7-bit varint when the first byte has bit 7 set.
    Varint,
}

/// A cursor over the file bytes that turns every short read into a typed error.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    prefix: LengthPrefix,
    /// Set when a length byte with bit 7 set was read, so [`parse_gem`] knows a
    /// varint retry is worth trying.
    saw_high_bit_length: bool,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8], prefix: LengthPrefix) -> Self {
        Self {
            bytes,
            pos: 0,
            prefix,
            saw_high_bit_length: false,
        }
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, n: usize, expected: &'static str) -> Result<&'a [u8], GemParseError> {
        let offset = self.pos;
        let end = offset
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(GemParseError::UnexpectedEof { offset, expected })?;
        self.pos = end;
        Ok(&self.bytes[offset..end])
    }

    fn array<const N: usize>(&mut self, expected: &'static str) -> Result<[u8; N], GemParseError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, expected)?);
        Ok(out)
    }

    fn f64(&mut self, expected: &'static str) -> Result<f64, GemParseError> {
        Ok(f64::from_le_bytes(self.array(expected)?))
    }

    fn i32(&mut self, expected: &'static str) -> Result<i32, GemParseError> {
        Ok(i32::from_le_bytes(self.array(expected)?))
    }

    fn u32(&mut self, expected: &'static str) -> Result<u32, GemParseError> {
        Ok(u32::from_le_bytes(self.array(expected)?))
    }

    /// Reads one length-prefixed Windows-1252 string.
    fn string(&mut self, expected: &'static str) -> Result<String, GemParseError> {
        let offset = self.pos;
        let [first] = self.array::<1>(expected)?;
        let len = if first & 0x80 == 0 {
            usize::from(first)
        } else {
            self.saw_high_bit_length = true;
            match self.prefix {
                LengthPrefix::Byte => usize::from(first),
                LengthPrefix::Varint => self.varint_tail(first, offset, expected)?,
            }
        };
        let raw = self.take(len, expected)?;
        Ok(decode_windows_1252_or_utf8(raw).into_owned())
    }

    /// Finishes a .NET 7-bit varint whose first byte `first` had bit 7 set.
    fn varint_tail(
        &mut self,
        first: u8,
        offset: usize,
        expected: &'static str,
    ) -> Result<usize, GemParseError> {
        let invalid = GemParseError::InvalidStringLength { offset };
        let mut value = u64::from(first & 0x7F);
        let mut shift = 7u32;
        loop {
            let [byte] = self.array::<1>(expected)?;
            value |= u64::from(byte & 0x7F) << shift;
            if value > u64::from(u32::MAX) {
                return Err(invalid);
            }
            if byte & 0x80 == 0 {
                return usize::try_from(value).map_err(|_| invalid);
            }
            shift += 7;
            if shift > 28 {
                return Err(invalid);
            }
        }
    }
}

/// Decodes a `.gem` file. See the module docs for the layout.
///
/// Strings are read with a one-byte length. When that framing fails and a length
/// byte had bit 7 set, the file is read again with .NET 7-bit varint lengths and
/// that reading is accepted only if it frames the whole file; otherwise the first
/// error is returned.
///
/// # Errors
///
/// [`GemParseError::EmptyInput`] for empty input, otherwise the framing error at
/// the first byte where the layout breaks (end of file inside a field, a vertex
/// flag other than 0/1, an invalid plane, a mirror flag other than 0/1, bytes
/// after the trailer that are not a `preform` section, or a `preform` section
/// nested inside another).
pub fn parse_gem(content: &[u8]) -> Result<GemDesign, GemParseError> {
    if content.is_empty() {
        return Err(GemParseError::EmptyInput);
    }
    let mut reader = Reader::new(content, LengthPrefix::Byte);
    match parse_to_end(&mut reader) {
        Err(first_error) if reader.saw_high_bit_length => {
            let mut retry = Reader::new(content, LengthPrefix::Varint);
            parse_to_end(&mut retry).map_err(|_| first_error)
        }
        result => result,
    }
}

/// Parses one design and requires the reader to be at end of file afterwards.
fn parse_to_end(reader: &mut Reader<'_>) -> Result<GemDesign, GemParseError> {
    let design = parse_design(reader, 0)?;
    if reader.remaining() == 0 {
        Ok(design)
    } else {
        Err(GemParseError::TrailingData {
            offset: reader.pos,
            remaining: reader.remaining(),
        })
    }
}

/// Parses a facet list, sentinel, trailer and an optional preform section.
/// `depth` counts the `preform` sections already entered (0 for the main design).
fn parse_design(reader: &mut Reader<'_>, depth: usize) -> Result<GemDesign, GemParseError> {
    let facets = parse_facets(reader)?;
    let symmetry = reader.i32("the trailer's symmetry")?;
    let mirror_offset = reader.pos;
    let mirror = match reader.i32("the trailer's mirror flag")? {
        0 => false,
        1 => true,
        found => {
            return Err(GemParseError::BadMirrorFlag {
                offset: mirror_offset,
                found,
            });
        }
    };
    let gear = reader.i32("the trailer's gear")?;
    let refractive_index = reader.f64("the trailer's refractive index")?;
    let unknown_7fff = reader.u32("the trailer's constant 0x7FFF field")?;
    let gear_offset = reader.f64("the trailer's gear offset")?;
    let mut headings: [String; 4] = Default::default();
    for heading in &mut headings {
        *heading = reader.string("a heading string")?;
    }
    let mut footnotes: [String; 4] = Default::default();
    for footnote in &mut footnotes {
        *footnote = reader.string("a footnote string")?;
    }
    let preform = parse_preform(reader, depth)?;
    let vertex_scale = GemDesign::median_vertex_scale(&facets);
    Ok(GemDesign {
        facets,
        symmetry,
        mirror,
        gear,
        refractive_index,
        gear_offset,
        unknown_7fff,
        headings,
        footnotes,
        preform,
        vertex_scale,
    })
}

/// Reads an optional `preform` section: nothing at end of file, a full nested
/// design after the tag, and [`GemParseError::TrailingData`] for anything else. A
/// tag met while `depth` has already reached [`MAX_PREFORM_DEPTH`] is
/// [`GemParseError::NestedPreform`], so the recursion is bounded.
fn parse_preform(
    reader: &mut Reader<'_>,
    depth: usize,
) -> Result<Option<Box<GemDesign>>, GemParseError> {
    if reader.remaining() == 0 {
        return Ok(None);
    }
    if !reader.bytes[reader.pos..].starts_with(PREFORM_TAG) {
        return Err(GemParseError::TrailingData {
            offset: reader.pos,
            remaining: reader.remaining(),
        });
    }
    if depth >= MAX_PREFORM_DEPTH {
        return Err(GemParseError::NestedPreform { offset: reader.pos });
    }
    reader.pos += PREFORM_TAG.len();
    Ok(Some(Box::new(parse_design(reader, depth + 1)?)))
}

/// Reads facet records until the sentinel.
fn parse_facets(reader: &mut Reader<'_>) -> Result<Vec<GemFacet>, GemParseError> {
    let mut facets = Vec::new();
    loop {
        let offset = reader.pos;
        let px = reader.f64("a facet plane or the -99999.0 end-of-facets sentinel")?;
        if px == SENTINEL {
            return Ok(facets);
        }
        let py = reader.f64("a facet plane's y")?;
        let pz = reader.f64("a facet plane's z")?;
        let plane = [px, py, pz];
        if plane.iter().any(|c| !c.is_finite()) || plane == [0.0; 3] {
            return Err(GemParseError::InvalidPlane { offset });
        }
        let tier = reader.i32("a facet's tier number")?;
        let label = reader.string("a facet label")?;
        let (name, instructions) = split_label(&label);
        let vertices = parse_vertices(reader)?;
        facets.push(GemFacet {
            plane,
            tier,
            name,
            instructions,
            vertices,
        });
    }
}

/// Splits a `name\tinstructions` label. Every corpus label has the tab; a label
/// without one is read as a bare name.
fn split_label(label: &str) -> (Option<String>, String) {
    let (name, instructions) = label.split_once('\t').unwrap_or((label, ""));
    let name = Some(name.to_string()).filter(|n| !n.is_empty());
    (name, instructions.to_string())
}

/// Reads a vertex loop: `{ i32 1, f64 x, y, z }` until `i32 0`.
fn parse_vertices(reader: &mut Reader<'_>) -> Result<Vec<[f64; 3]>, GemParseError> {
    let mut vertices = Vec::new();
    loop {
        let offset = reader.pos;
        match reader.i32("a vertex flag")? {
            0 => return Ok(vertices),
            1 => vertices.push([
                reader.f64("a vertex x")?,
                reader.f64("a vertex y")?,
                reader.f64("a vertex z")?,
            ]),
            found => return Err(GemParseError::BadVertexFlag { offset, found }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GemParseError, LengthPrefix, Reader};

    #[test]
    fn varint_lengths_decode_and_overlong_ones_are_rejected() {
        let mut ok = vec![0xC8, 0x01];
        ok.extend_from_slice(&[b'x'; 200]);
        let mut reader = Reader::new(&ok, LengthPrefix::Varint);
        assert_eq!(reader.string("s").expect("varint 200"), "x".repeat(200));
        assert!(reader.saw_high_bit_length);

        let overlong = [0xFF; 6];
        let mut reader = Reader::new(&overlong, LengthPrefix::Varint);
        assert_eq!(
            reader.string("s"),
            Err(GemParseError::InvalidStringLength { offset: 0 })
        );
    }
}
