//! Which file a design's done marks live in.
//!
//! A design that carries an id (a `.isdd` file saved once has a UUID in its `[meta]`
//! table) keeps its marks under that id, so they survive edits and renames. A design
//! without one is keyed by the SHA-256 of its native text without metadata: the marks then
//! belong to exactly this design and are gone after any edit (an undo brings them back).
//! A design that gets its id on the first save adopts the marks it had under its text hash.
//!
//! Both keys name a file, so removing a design's marks means removing both files:
//! [`ProgressKeys::into_stems`] lists them.

use indicatrix_cut_core::{
    Design,
    native::{DesignExtras, design_to_file},
};
use indicatrix_formats::native::design as design_file;

use crate::app_state::AppState;

/// The key a design whose text cannot be written is filed under. Every such design shares
/// it, so it is never something to delete on behalf of one of them.
const UNREADABLE_KEY: &str = "sha-unreadable";

/// The two keys of a design's marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProgressKeys {
    /// The key the marks are stored under now.
    pub primary: String,
    /// The key the marks had before the design got an id, when `primary` is an id key.
    pub legacy: Option<String>,
}

impl ProgressKeys {
    /// Every key a file of this design can carry, the current one first. The shared
    /// fallback key of an unwritable design is left out.
    pub(super) fn into_stems(self) -> Vec<String> {
        let mut stems = vec![self.primary];
        stems.extend(self.legacy);
        stems.retain(|stem| stem != UNREADABLE_KEY);
        stems
    }
}

/// The keys of `state`'s design. See the module docs.
pub(super) fn progress_keys(state: &AppState) -> ProgressKeys {
    keys_of(&state.document.metadata.id, &state.session.design)
}

/// The keys of `design`, whose `[meta]` id is `id` (empty when it has none).
pub(super) fn keys_of(id: &str, design: &Design) -> ProgressKeys {
    let id = sanitize(id);
    if id.is_empty() {
        ProgressKeys {
            primary: text_key(design),
            legacy: None,
        }
    } else {
        ProgressKeys {
            primary: format!("id-{id}"),
            legacy: legacy_key(text_key(design)),
        }
    }
}

/// `text_key` as the legacy key of a design with an id, unless it is the shared fallback
/// of an unwritable design: that file belongs to no design in particular, so binding to it
/// would adopt the marks of another design and a reset would delete them.
fn legacy_key(text_key: String) -> Option<String> {
    (text_key != UNREADABLE_KEY).then_some(text_key)
}

/// The key from the design's native text without metadata.
fn text_key(design: &Design) -> String {
    let file = design_to_file(design, None, &DesignExtras::default());
    match design_file::to_string(&file) {
        Ok(text) => format!("sha-{}", sha256_hex(text.as_bytes())),
        Err(error) => {
            tracing::warn!(%error, "the design's text could not be hashed for its done marks");
            UNREADABLE_KEY.to_owned()
        }
    }
}

/// `id` reduced to the characters of a UUID, so it is safe as a file name.
fn sanitize(id: &str) -> String {
    id.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// SHA-256 of `data` as 64 lowercase hexadecimal digits.
pub(super) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Two blocks of input (the padding spills into a second block).
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sanitize_keeps_only_uuid_characters() {
        assert_eq!(sanitize(" 8f2c-AA01/../x "), "8f2c-AA01x");
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn the_shared_fallback_is_never_the_legacy_key_of_a_design_with_an_id() {
        assert_eq!(legacy_key(UNREADABLE_KEY.to_owned()), None);
        assert_eq!(legacy_key("sha-123".to_owned()), Some("sha-123".to_owned()));
    }

    #[test]
    fn stems_list_the_current_key_first_and_skip_the_shared_fallback() {
        let both = ProgressKeys {
            primary: "id-abc".to_owned(),
            legacy: Some("sha-123".to_owned()),
        };
        assert_eq!(both.into_stems(), ["id-abc", "sha-123"]);
        let only_text = ProgressKeys {
            primary: "sha-123".to_owned(),
            legacy: None,
        };
        assert_eq!(only_text.into_stems(), ["sha-123"]);
        let unreadable = ProgressKeys {
            primary: "id-abc".to_owned(),
            legacy: Some(UNREADABLE_KEY.to_owned()),
        };
        assert_eq!(unreadable.into_stems(), ["id-abc"]);
        let nothing_else = ProgressKeys {
            primary: UNREADABLE_KEY.to_owned(),
            legacy: None,
        };
        assert_eq!(nothing_else.into_stems(), [] as [String; 0]);
    }
}
