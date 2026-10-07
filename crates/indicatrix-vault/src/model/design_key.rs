//! The key every per-design side table uses: the design's own UUID.
//!
//! A design's `[meta].id` (see `indicatrix_formats::native::design::DesignMetadata`) is
//! stable across machines and across Save As, and exists for a design opened from disk
//! that has never been in the catalogue -- which an `entry_id` does not. The side tables
//! for saved variants, cutting progress and the lighting choice are therefore keyed by
//! it ([`Database::save_variant`](crate::db::sqlite::Database::save_variant) and its
//! siblings).
//!
//! A design that lives only in the catalogue has no file and so no id of its own. It
//! gets a name-based one, [`catalogue_design_uuid`]: the same catalogue entry always
//! yields the same UUID, so reopening it finds its side data again.
//!
//! The database stores UUIDs in lowercase. Every public function that takes one accepts
//! either case and surrounding spaces ([`normalize_design_uuid`]), and refuses anything
//! that is not an `8-4-4-4-12` hexadecimal UUID, so a stray empty string can never
//! become a key.

use anyhow::{Result, anyhow};
use indicatrix_formats::native::design::is_uuid;
use std::fmt::Write as _;

/// The RFC 4122 namespace for URLs, `6ba7b811-9dad-11d1-80b4-00c04fd430c8`.
///
/// [`catalogue_design_uuid`] hashes under it. A catalogue entry's `url` (a web address, or
/// the `local://<file>` key of a local import) is exactly the kind of name this namespace
/// is for, and using the standard one means any UUID tool reproduces the result:
/// `uuid5(NAMESPACE_URL, url)`.
const NAMESPACE_URL: [u8; 16] = [
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
];

/// `text` as the canonical key: trimmed and lowercase, or `None` when it is not an
/// `8-4-4-4-12` hexadecimal UUID.
#[must_use]
pub fn normalize_design_uuid(text: &str) -> Option<String> {
    let trimmed = text.trim();
    is_uuid(trimmed).then(|| trimmed.to_ascii_lowercase())
}

/// The deterministic UUID of the catalogue entry whose `url` is given: a version-5
/// (name-based, SHA-1) UUID under the RFC 4122 URL namespace.
///
/// The same `url` always gives the same UUID, on every machine, so a catalogue design
/// that has no design file finds its side data again when it is reopened. The `url` is
/// used exactly as stored (no case folding): it is the entry's unique key.
///
/// If an entry's `url` is rewritten later, a design that was never saved to a file
/// yields a different UUID afterwards. The first Save writes the UUID into the design
/// file, and from then on the file's own id wins.
#[must_use]
pub fn catalogue_design_uuid(url: &str) -> String {
    uuid_v5(&NAMESPACE_URL, url.as_bytes())
}

/// [`normalize_design_uuid`], as a `Result` for the database functions.
pub(crate) fn require_design_uuid(text: &str) -> Result<String> {
    normalize_design_uuid(text).ok_or_else(|| {
        anyhow!("'{text}' is not a design UUID (expected 8-4-4-4-12 hexadecimal digits)")
    })
}

/// An RFC 4122 version-5 UUID: the first 16 bytes of `SHA-1(namespace || name)` with the
/// version and variant bits set, as lowercase `8-4-4-4-12` hexadecimal.
fn uuid_v5(namespace: &[u8; 16], name: &[u8]) -> String {
    let mut input = Vec::with_capacity(namespace.len() + name.len());
    input.extend_from_slice(namespace);
    input.extend_from_slice(name);
    let digest = sha1(&input);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut text = String::with_capacity(36);
    for (position, byte) in bytes.iter().enumerate() {
        if matches!(position, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// SHA-1 (FIPS 180-4) of `data`.
///
/// Used only to derive a stable name-based UUID, never for anything that needs collision
/// resistance against an attacker: the workspace carries no hashing crate, and the
/// version-5 UUID layout is defined in terms of SHA-1.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_length = u64::try_from(data.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    let (blocks, _) = message.as_chunks::<64>();
    for block in blocks {
        let mut schedule = [0u32; 80];
        for (slot, word) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *slot = u32::from_be_bytes(*word);
        }
        for round in 16..80 {
            schedule[round] = (schedule[round - 3]
                ^ schedule[round - 8]
                ^ schedule[round - 14]
                ^ schedule[round - 16])
                .rotate_left(1);
        }
        let mut reg = state;
        for (round, &word) in schedule.iter().enumerate() {
            let (mix, constant) = match round {
                0..=19 => ((reg[1] & reg[2]) | (!reg[1] & reg[3]), 0x5A82_7999_u32),
                20..=39 => (reg[1] ^ reg[2] ^ reg[3], 0x6ED9_EBA1),
                40..=59 => (
                    (reg[1] & reg[2]) | (reg[1] & reg[3]) | (reg[2] & reg[3]),
                    0x8F1B_BCDC,
                ),
                _ => (reg[1] ^ reg[2] ^ reg[3], 0xCA62_C1D6),
            };
            let next = reg[0]
                .rotate_left(5)
                .wrapping_add(mix)
                .wrapping_add(reg[4])
                .wrapping_add(constant)
                .wrapping_add(word);
            reg[4] = reg[3];
            reg[3] = reg[2];
            reg[2] = reg[1].rotate_left(30);
            reg[1] = reg[0];
            reg[0] = next;
        }
        for (total, part) in state.iter_mut().zip(reg) {
            *total = total.wrapping_add(part);
        }
    }

    let mut digest = [0u8; 20];
    for (chunk, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *chunk = word.to_be_bytes();
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
    }

    #[test]
    fn sha1_matches_the_published_test_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
            "a two-block message"
        );
    }

    #[test]
    fn version_5_matches_the_rfc_example() {
        let dns_namespace = [
            0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4,
            0x30, 0xc8,
        ];
        assert_eq!(
            uuid_v5(&dns_namespace, b"python.org"),
            "886313e1-3b8a-5372-9b90-0c9aee199e5d"
        );
    }

    #[test]
    fn a_catalogue_uuid_is_deterministic_valid_and_specific_to_the_url() {
        let first = catalogue_design_uuid("local://capps-brilliant.asc");
        assert_eq!(first, catalogue_design_uuid("local://capps-brilliant.asc"));
        assert!(is_uuid(&first), "{first}");
        assert_eq!(first, first.to_ascii_lowercase());
        assert_eq!(&first[14..15], "5", "version nibble");
        assert_ne!(first, catalogue_design_uuid("local://capps-brilliant.ASC"));
        assert_ne!(first, catalogue_design_uuid("local://other.asc"));
        assert_ne!(first, catalogue_design_uuid(""));
    }

    #[test]
    fn normalising_accepts_either_case_and_spaces_and_refuses_anything_else() {
        let upper = " 886313E1-3B8A-5372-9B90-0C9AEE199E5D ";
        assert_eq!(
            normalize_design_uuid(upper).as_deref(),
            Some("886313e1-3b8a-5372-9b90-0c9aee199e5d")
        );
        for bad in [
            "",
            "   ",
            "not-a-uuid",
            "886313e13b8a53729b900c9aee199e5d",
            "886313e1-3b8a-5372-9b90-0c9aee199e5",
            "886313e1-3b8a-5372-9b90-0c9aee199e5g",
        ] {
            assert_eq!(normalize_design_uuid(bad), None, "{bad:?}");
            assert!(require_design_uuid(bad).is_err(), "{bad:?}");
        }
    }
}
