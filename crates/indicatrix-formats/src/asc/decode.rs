//! [`decode_asc_bytes`]: turns raw `.asc` file bytes into text, and
//! [`parse_asc_bytes`]: decodes then parses in one step.
//!
//! `GemCAD` for Windows is an ANSI application, and 591 of the 5,759 real catalogue
//! files (measured 2026-09-28) are not valid UTF-8: they carry Windows-1252 bytes
//! such as `0xB0` (`°`) in an `H`, `F` or `G` line. Reading such a file with
//! `read_to_string` fails outright, so every read site hands the raw bytes to
//! [`decode_asc_bytes`] instead.

use super::{error::AscParseError, parse::parse_asc, schedule::AscSchedule};
use crate::encoding::decode_windows_1252_or_utf8;
use std::borrow::Cow;

/// Decodes raw `.asc` file bytes into text.
///
/// In order:
/// 1. a leading UTF-8 byte-order mark is stripped;
/// 2. the rest is read as UTF-8 when it is valid UTF-8 (borrowed, no copy);
/// 3. otherwise every byte is read as Windows-1252, the code page `GemCAD` for
///    Windows writes (a legacy `°` is byte `0xB0`) -- steps 1-3 are
///    `crate::encoding::decode_windows_1252_or_utf8`, shared with the `.gem` and
///    `.gcs` readers;
/// 4. a lone carriage return (classic Mac line ending) becomes `\n`, so
///    [`super::parse_asc`] sees one physical line per record. `\r\n` pairs are
///    left alone: the parser records them as [`super::AscLineEnding::CrLf`].
///
/// Never fails: every byte sequence has a Windows-1252 reading.
#[must_use]
pub fn decode_asc_bytes(bytes: &[u8]) -> Cow<'_, str> {
    normalize_lone_carriage_returns(decode_windows_1252_or_utf8(bytes))
}

/// Decodes raw `.asc` file bytes with [`decode_asc_bytes`], then parses the text
/// with [`super::parse_asc`].
///
/// # Errors
///
/// Exactly [`super::parse_asc`]'s errors; decoding itself cannot fail.
pub fn parse_asc_bytes(bytes: &[u8]) -> Result<AscSchedule, AscParseError> {
    parse_asc(&decode_asc_bytes(bytes))
}

/// Replaces every `\r` that is not the first half of a `\r\n` pair with `\n`.
/// Returns `text` unchanged (no copy) when it has no such lone `\r`.
fn normalize_lone_carriage_returns(text: Cow<'_, str>) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let has_lone_cr = bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'\r' && bytes.get(i + 1) != Some(&b'\n'));
    if !has_lone_cr {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' && chars.peek() != Some(&'\n') {
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    Cow::Owned(out)
}
