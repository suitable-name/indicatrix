//! Standard-alphabet base64 (RFC 4648 section 4) with padding and no line wrapping: the
//! encoding of an attachment's `data` key. Output is a pure function of the bytes, so
//! the file stays deterministic.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `bytes` as padded standard base64 on a single line.
pub(super) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk.first().copied().unwrap_or(0));
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let word = (b0 << 16) | (b1 << 8) | b2;
        for shift in [18u32, 12, 6, 0].into_iter().take(chunk.len() + 1) {
            let index = ((word >> shift) & 0x3f) as usize;
            out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
        }
        for _ in chunk.len()..3 {
            out.push('=');
        }
    }
    out
}

fn sextet(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decodes strict padded standard base64: no whitespace, no URL-safe characters, length
/// a multiple of four, padding only at the end. `None` for anything else.
pub(super) fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let quads = bytes.len() / 4;
    for (position, quad) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let padding = quad.iter().rev().take_while(|&&b| b == b'=').count();
        if padding > 2 || (padding > 0 && position + 1 != quads) {
            return None;
        }
        let mut word = 0u32;
        for &byte in quad.iter().take(4 - padding) {
            word = (word << 6) | sextet(byte)?;
        }
        word <<= 6 * padding as u32;
        // Non-zero bits under the padding would be dropped silently: refuse, so a
        // decoded value always re-encodes to the exact text it came from.
        if padding > 0 && word & ((1u32 << (8 * padding as u32)) - 1) != 0 {
            return None;
        }
        for shift in [16u32, 8, 0].into_iter().take(3 - padding) {
            out.push(((word >> shift) & 0xff) as u8);
        }
    }
    Some(out)
}
