//! Which UUID names the open design.
//!
//! The library database keeps a design's saved variants, cutting progress and lighting
//! choice keyed by the design's UUID (`indicatrix_vault::model::design_key`), so every
//! design open in the editor has one from the moment it opens. The UUID lives in the
//! design's `[meta].id` ([`super::DesignFileExtras`]), which is also what a Save and an
//! autosave write into the file: there is no second copy that could drift.
//!
//! - A `.indicatrix` file that has an id keeps it.
//! - A file without one (written by an older version), an `.asc`/`.gem`/`.gcs` opened
//!   from disk and an older sidecar pair get a deterministic UUID made from the file's
//!   own location ([`file_design_url`]), so opening the same file again finds its side
//!   data again before the first Save writes the id into it. A recovered autosave
//!   snapshot, a remote design and a new design get a fresh random one instead: none of
//!   them names a file that stays where it is.
//! - A design loaded from the local catalogue that carries no id of its own gets the
//!   catalogue entry's deterministic UUID, so reopening the same entry finds its side
//!   data again. A catalogue design whose attached file has an id keeps that id.
//! - Save As keeps the UUID: the copy is the same design and shares its side data.
//!
//! The decision is a pure function ([`resolve_design_uuid`]) so it is tested without a
//! window.

use indicatrix_vault::model::design_key::{catalogue_design_uuid, normalize_design_uuid};
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// A random version-4 UUID as 8-4-4-4-12 lowercase hexadecimal.
///
/// The workspace carries no UUID or random-number crate, so the 122 random bits come from
/// the standard library's per-process random hasher keys, mixed with the clock, the
/// process id and a counter so two ids made in one nanosecond still differ. The id only
/// has to be unique, not unguessable.
pub(in crate::gui::editor) fn fresh_design_uuid() -> String {
    use std::{
        fmt::Write as _,
        hash::{BuildHasher, Hasher, RandomState},
    };
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut bytes = [0u8; 16];
    for (half, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(nanos);
        hasher.write_u32(std::process::id());
        hasher.write_u64(counter);
        hasher.write_usize(half);
        chunk.copy_from_slice(&hasher.finish().to_be_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    });
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The UUID a design opens with.
///
/// `stored_id` is the `[meta].id` the design arrived with (empty when it has none). A
/// valid one is returned exactly as written, so opening and saving a file never rewrites
/// its id (not even its letter case). Otherwise a design loaded from the local catalogue
/// (`catalogue_url` is the entry's `url`) gets the entry's deterministic UUID, and
/// anything else gets `fresh()`, which runs only then.
///
/// A non-empty `stored_id` that is not a UUID cannot come from a parsed design file (the
/// file reader refuses it); it is treated as absent rather than trusted as a key.
pub(in crate::gui::editor) fn resolve_design_uuid(
    stored_id: &str,
    catalogue_url: Option<&str>,
    fresh: impl FnOnce() -> String,
) -> String {
    if normalize_design_uuid(stored_id).is_some() {
        return stored_id.to_string();
    }
    catalogue_url
        .filter(|url| !url.trim().is_empty())
        .map_or_else(fresh, catalogue_design_uuid)
}

/// The UUID a design opened from the file at `path` gets: the file's own `[meta].id`
/// (`stored_id`) when it has a valid one, otherwise the deterministic UUID of the file's
/// location ([`file_design_url`]).
///
/// The same rules as [`resolve_design_uuid`], with the file's `file:///` URL standing in
/// for a catalogue entry's `url`. `fresh` only runs if the URL somehow comes out blank,
/// which a real path never does. A valid stored id is returned without looking at the
/// file system at all.
pub(in crate::gui::editor) fn resolve_file_design_uuid(
    stored_id: &str,
    path: &Path,
    fresh: impl FnOnce() -> String,
) -> String {
    if normalize_design_uuid(stored_id).is_some() {
        return stored_id.to_string();
    }
    resolve_design_uuid(stored_id, Some(&file_design_url(path)), fresh)
}

/// The `file:///` URL that names the design file at `path`, the key a design without an
/// id of its own is filed under (see [`resolve_file_design_uuid`]).
///
/// The path is made canonical first (symbolic links, `.` and `..` components and the
/// spelling of the drive letter resolved), so the same file reached two ways gives one
/// URL. A file that cannot be canonicalised (it is gone) falls back to its absolute path,
/// and as a last resort to the path as given: a stable key matters more than a perfect
/// one.
pub(in crate::gui::editor) fn file_design_url(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    file_url_from_path_text(&resolved.to_string_lossy())
}

/// [`file_design_url`] without the file system: the text of an absolute path as a URL.
///
/// Backslashes become slashes, the Windows verbatim prefixes (`\\?\` and `\\?\UNC\`) that
/// `canonicalize` adds are dropped, and every byte outside the unreserved set (plus `/`
/// and `:`) is percent-encoded, so a path with spaces or accents gives a plain ASCII URL.
fn file_url_from_path_text(text: &str) -> String {
    use std::fmt::Write as _;

    let slashed = text.replace('\\', "/");
    let plain = slashed.strip_prefix("//?/UNC/").map_or_else(
        || slashed.strip_prefix("//?/").unwrap_or(&slashed).to_string(),
        |rest| format!("//{rest}"),
    );
    let mut encoded = String::with_capacity(plain.len() + 8);
    for byte in plain.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    if encoded.starts_with("//") {
        // A network share: the first component is the host.
        format!("file:{encoded}")
    } else if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_formats::native::design::is_uuid;

    const FILE_ID: &str = "0b9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10";

    const fn never() -> String {
        panic!("a fresh id must not be made when one is already decided");
    }

    #[test]
    fn a_file_id_is_kept_exactly_as_written() {
        assert_eq!(resolve_design_uuid(FILE_ID, None, never), FILE_ID);
        let upper = FILE_ID.to_ascii_uppercase();
        assert_eq!(
            resolve_design_uuid(&upper, None, never),
            upper,
            "opening and saving must not rewrite the file's id"
        );
    }

    #[test]
    fn a_file_id_beats_the_catalogue_entry_it_was_loaded_through() {
        assert_eq!(
            resolve_design_uuid(FILE_ID, Some("local://capps.indicatrix"), never),
            FILE_ID
        );
    }

    #[test]
    fn a_design_without_an_id_gets_a_fresh_one() {
        let made = resolve_design_uuid("", None, || "made-here".to_string());
        assert_eq!(made, "made-here");
        let real = resolve_design_uuid("", None, fresh_design_uuid);
        assert!(is_uuid(&real), "{real}");
    }

    #[test]
    fn a_stored_id_that_is_not_a_uuid_is_replaced_not_trusted() {
        assert_eq!(
            resolve_design_uuid("not-a-uuid", None, || "fresh".to_string()),
            "fresh"
        );
        assert_eq!(
            resolve_design_uuid("   ", None, || "fresh".to_string()),
            "fresh"
        );
    }

    #[test]
    fn a_catalogue_design_without_an_id_gets_the_entrys_deterministic_uuid() {
        let url = "local://capps-brilliant.asc";
        let first = resolve_design_uuid("", Some(url), never);
        let again = resolve_design_uuid("", Some(url), never);
        assert_eq!(first, again, "reopening the same entry must find its data");
        assert_eq!(first, catalogue_design_uuid(url));
        assert!(is_uuid(&first), "{first}");
        assert_ne!(
            first,
            resolve_design_uuid("", Some("local://other.asc"), never)
        );
    }

    #[test]
    fn a_blank_catalogue_url_counts_as_no_catalogue_entry() {
        assert_eq!(
            resolve_design_uuid("", Some("  "), || "fresh".to_string()),
            "fresh"
        );
    }

    #[test]
    fn fresh_ids_are_valid_version_4_uuids_and_differ() {
        let first = fresh_design_uuid();
        let second = fresh_design_uuid();
        assert!(is_uuid(&first), "{first}");
        assert_eq!(&first[14..15], "4", "version nibble");
        assert!(
            matches!(&first[19..20], "8" | "9" | "a" | "b"),
            "variant nibble in {first}"
        );
        assert_ne!(first, second);
    }

    #[test]
    fn a_windows_path_becomes_an_encoded_file_url() {
        assert_eq!(
            file_url_from_path_text(r"C:\Users\Anna\My Designs\ring.asc"),
            "file:///C:/Users/Anna/My%20Designs/ring.asc"
        );
        assert_eq!(
            file_url_from_path_text(r"C:\Müller\ring.asc"),
            "file:///C:/M%C3%BCller/ring.asc",
            "accents are percent-encoded byte by byte"
        );
    }

    #[test]
    fn the_windows_verbatim_prefixes_are_dropped() {
        assert_eq!(
            file_url_from_path_text(r"\\?\C:\Users\Anna\ring.asc"),
            file_url_from_path_text(r"C:\Users\Anna\ring.asc"),
            "canonicalize adds the prefix; it must not change the key"
        );
        assert_eq!(
            file_url_from_path_text(r"\\?\UNC\workshop\designs\ring.asc"),
            "file://workshop/designs/ring.asc"
        );
        assert_eq!(
            file_url_from_path_text(r"\\workshop\designs\ring.asc"),
            "file://workshop/designs/ring.asc"
        );
    }

    #[test]
    fn a_unix_path_becomes_a_three_slash_file_url() {
        assert_eq!(
            file_url_from_path_text("/home/anna/designs/ring.asc"),
            "file:///home/anna/designs/ring.asc"
        );
    }

    #[test]
    fn a_design_file_without_an_id_gets_its_locations_deterministic_uuid() {
        let path = Path::new("a-design-that-does-not-exist-here/ring.asc");
        let first = resolve_file_design_uuid("", path, never);
        let again = resolve_file_design_uuid("", path, never);
        assert_eq!(
            first, again,
            "opening the same file again must find its data"
        );
        assert!(is_uuid(&first), "{first}");
        assert_eq!(&first[14..15], "5", "a name-based UUID, not a random one");
        assert_eq!(first, catalogue_design_uuid(&file_design_url(path)));
        assert_ne!(
            first,
            resolve_file_design_uuid("", Path::new("another-folder/ring.asc"), never),
            "a different file is a different design"
        );
    }

    #[test]
    fn a_file_id_beats_the_file_location() {
        let path = Path::new("somewhere/ring.indicatrix");
        assert_eq!(resolve_file_design_uuid(FILE_ID, path, never), FILE_ID);
        let upper = FILE_ID.to_ascii_uppercase();
        assert_eq!(
            resolve_file_design_uuid(&upper, path, never),
            upper,
            "opening and saving must not rewrite the file's id"
        );
    }

    #[test]
    fn a_stored_id_that_is_not_a_uuid_falls_back_to_the_file_location() {
        let path = Path::new("somewhere/ring.asc");
        assert_eq!(
            resolve_file_design_uuid("not-a-uuid", path, never),
            resolve_file_design_uuid("", path, never)
        );
    }

    #[test]
    fn two_spellings_of_one_existing_file_give_one_uuid() {
        let folder = std::env::temp_dir().join(format!(
            "indicatrix-cut-design-identity-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(folder.join("inner")).expect("create the test folder");
        let file = folder.join("ring.asc");
        std::fs::write(&file, "a design").expect("write the test file");

        let direct = resolve_file_design_uuid("", &file, never);
        let roundabout_path = folder.join("inner").join("..").join("ring.asc");
        let roundabout = resolve_file_design_uuid("", &roundabout_path, never);
        let _ = std::fs::remove_dir_all(&folder);

        assert_eq!(
            direct, roundabout,
            "the same file reached through `..` is the same design"
        );
        assert!(is_uuid(&direct), "{direct}");
    }
}
