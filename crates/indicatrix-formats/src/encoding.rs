//! Text decoding shared by the binary `.gem` reader and the `.gcs` reader.
//!
//! `GemCAD` for Windows and Gem Cut Studio 1.1 are ANSI applications: real `.gem`
//! labels carry Windows-1252 bytes (`attached_files` id 6430 spells `É` as byte
//! `0xC9`), and one real `.gcs` file (id 2213) carries byte `0xBA` (`º`) inside an
//! attribute value with no encoding declaration.
//!
//! [`crate::asc::decode_asc_bytes`] is a thin wrapper over
//! [`decode_windows_1252_or_utf8`] that additionally rewrites lone carriage
//! returns into `\n` -- a step that must NOT happen to `.gcs` attribute values or
//! `.gem` labels, which is why the line-ending handling lives in the `asc` module
//! and only the byte decoding is shared here.

use std::borrow::Cow;

/// The UTF-8 byte-order mark some editors prepend to a text file.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Windows-1252's mapping for the bytes `0x80..=0x9F`, the only range where it
/// differs from Latin-1 (every other byte maps to the code point of the same
/// value). The five bytes Windows-1252 leaves undefined (`0x81`, `0x8D`, `0x8F`,
/// `0x90`, `0x9D`) map to the C1 control of the same value, as the WHATWG
/// encoding standard does.
pub const WINDOWS_1252_C1: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// Decodes `bytes` as text: a leading UTF-8 byte-order mark is stripped, valid
/// UTF-8 is borrowed as is, and anything else is read byte by byte as
/// Windows-1252. Never fails, and never touches line endings.
pub fn decode_windows_1252_or_utf8(bytes: &[u8]) -> Cow<'_, str> {
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    std::str::from_utf8(bytes).map_or_else(
        |_| Cow::Owned(bytes.iter().map(|&byte| windows_1252_char(byte)).collect()),
        Cow::Borrowed,
    )
}

/// The character Windows-1252 assigns to `byte`.
pub fn windows_1252_char(byte: u8) -> char {
    match byte {
        0x80..=0x9F => WINDOWS_1252_C1[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// The Windows-1252 byte for `ch`, or `None` when the code page has no such
/// character. The inverse of [`windows_1252_char`].
#[cfg(test)]
pub fn windows_1252_byte(ch: char) -> Option<u8> {
    let code = u32::from(ch);
    if code < 0x80 || (0xA0..=0xFF).contains(&code) {
        return u8::try_from(code).ok();
    }
    WINDOWS_1252_C1
        .iter()
        .position(|&c| c == ch)
        .and_then(|i| u8::try_from(0x80 + i).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_is_borrowed_and_bom_stripped() {
        assert!(matches!(
            decode_windows_1252_or_utf8(b"\xEF\xBB\xBFabc"),
            Cow::Borrowed("abc")
        ));
    }

    #[test]
    fn invalid_utf8_reads_as_windows_1252() {
        assert_eq!(
            decode_windows_1252_or_utf8(b"\xC9toile 41.13\xBA \x80"),
            "Étoile 41.13º €"
        );
    }

    #[test]
    fn byte_and_char_mappings_are_inverse() {
        for byte in 0..=u8::MAX {
            assert_eq!(windows_1252_byte(windows_1252_char(byte)), Some(byte));
        }
        assert_eq!(windows_1252_byte('\u{4E2D}'), None);
    }
}
