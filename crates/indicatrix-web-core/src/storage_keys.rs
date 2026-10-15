//! Pure key and format helpers for the page's browser storage layers: no `web-sys`, no
//! `serde`, so they compile and test on a native host (the page's `storage` module calls
//! them from the wasm-only side).
//!
//! Every key is `indicatrix.<name>.v<version>`. The version is part of the key, so a
//! changed format starts from an empty slot instead of misreading an old entry.

/// The prefix every key this app writes starts with.
pub const PREFIX: &str = "indicatrix.";
/// The `<name>` of the per-stone cutting-mark blobs (`storage::marks`).
pub const MARKS_NAME: &str = "marks";
/// The format version of a marks blob.
pub const MARKS_VERSION: u32 = 1;
/// The `<name>` of the per-adapter GPU admission records (`storage::admission`).
pub const ADMISSION_NAME: &str = "admission";
/// The format version of an admission record.
pub const ADMISSION_VERSION: u32 = 1;
/// The longest sanitized `<segment>` a caller-supplied key part may contribute, so a
/// long adapter description cannot grow a key without bound.
pub const MAX_SEGMENT_CHARS: usize = 96;

/// `indicatrix.<name>.v<version>`.
#[must_use]
pub fn local_key(name: &str, version: u32) -> String {
    format!("{PREFIX}{name}.v{version}")
}

/// A caller-supplied string made safe as one key segment.
///
/// ASCII letters and digits are lower-cased, `-` and `_` kept, every other character (dots
/// included, so a segment can never forge the `.v<version>` suffix) becomes `_`; at most
/// [`MAX_SEGMENT_CHARS`] characters. The empty string becomes `_`.
#[must_use]
pub fn sanitize_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .take(MAX_SEGMENT_CHARS)
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

/// `indicatrix.marks.<stone>.v1`: the cutting-mark blob of one stone.
#[must_use]
pub fn marks_key(stone: &str) -> String {
    local_key(
        &format!("{MARKS_NAME}.{}", sanitize_segment(stone)),
        MARKS_VERSION,
    )
}

/// `indicatrix.admission.<adapter>.v1`: the GPU admission record of one adapter.
#[must_use]
pub fn admission_key(adapter: &str) -> String {
    local_key(
        &format!("{ADMISSION_NAME}.{}", sanitize_segment(adapter)),
        ADMISSION_VERSION,
    )
}

/// Whether `key` is one this app wrote (starts with [`PREFIX`]).
#[must_use]
pub fn is_app_key(key: &str) -> bool {
    key.starts_with(PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_key_has_prefix_and_version() {
        assert_eq!(local_key("settings", 1), "indicatrix.settings.v1");
        assert_eq!(local_key("guide", 12), "indicatrix.guide.v12");
    }

    #[test]
    fn sanitize_replaces_unsafe_characters() {
        assert_eq!(
            sanitize_segment("NVIDIA RTX 4090 (D3D12)"),
            "nvidia_rtx_4090__d3d12_"
        );
        assert_eq!(sanitize_segment("a.b.v9"), "a_b_v9");
        assert_eq!(sanitize_segment("keep-this_one"), "keep-this_one");
    }

    #[test]
    fn sanitize_bounds_length_and_never_returns_empty() {
        assert_eq!(sanitize_segment(""), "_");
        let long = "x".repeat(500);
        assert_eq!(sanitize_segment(&long).chars().count(), MAX_SEGMENT_CHARS);
    }

    #[test]
    fn sanitize_handles_non_ascii_without_panicking() {
        assert_eq!(sanitize_segment("Grüße"), "gr__e");
    }

    #[test]
    fn marks_and_admission_keys_are_distinct_and_versioned() {
        let m = marks_key("Round Brilliant #3");
        let a = admission_key("Round Brilliant #3");
        assert_eq!(m, "indicatrix.marks.round_brilliant__3.v1");
        assert_eq!(a, "indicatrix.admission.round_brilliant__3.v1");
        assert_ne!(m, a);
        assert!(is_app_key(&m) && is_app_key(&a));
        assert!(!is_app_key("other.key"));
    }
}
