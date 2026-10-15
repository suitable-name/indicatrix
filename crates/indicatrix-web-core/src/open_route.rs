//! What the Open picker offers, and the file names a design is saved under.
//!
//! Which loader a picked or dropped file goes to is decided by
//! [`indicatrix_editor::files::InputFileKind::classify`] (by content where the name
//! cannot tell: an `.isdd` or legacy `.indicatrix` file is a design file, and a
//! `.toml` is only opened when its header says it is one).

use indicatrix_formats::native::design::{
    DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED, LEGACY_DESIGN_EXTENSION,
};

/// The extensions the Open picker offers, the design file first (`.isdd`, then the
/// legacy `.indicatrix`), then the other formats it reads.
pub const OPEN_EXTENSIONS: [&str; 6] = [
    DESIGN_EXTENSION,
    LEGACY_DESIGN_EXTENSION,
    "asc",
    "gem",
    "gcs",
    "hdr",
];

/// The `.asc` name a design file called `file_name` is recorded under, so a later
/// save proposes the same stem (`round.isdd` -> `round.asc`).
#[must_use]
pub fn asc_name_for_design_file(file_name: &str) -> String {
    indicatrix_editor::files::converted_asc_file_name(file_name)
}

/// The design file name for a design recorded under `asc_name`: the extension replaced
/// by `.isdd` (`round.asc` -> `round.isdd`), or appended to a name with no
/// extension.
#[must_use]
pub fn design_file_name_for_asc(asc_name: &str) -> String {
    let stem = asc_name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or(asc_name, |(stem, _)| stem);
    format!("{stem}{DESIGN_EXTENSION_DOTTED}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_formats::native::design::DESIGN_VERSION_PLANES;

    #[test]
    fn a_file_from_a_newer_version_is_refused_with_a_clear_message() {
        // One past the newest version this build reads (the one with plane-set preforms).
        let text = format!(
            "format = \"indicatrix-design\"\nversion = {}\n",
            DESIGN_VERSION_PLANES + 1
        );
        assert_eq!(
            indicatrix_editor::files::InputFileKind::classify("future.indicatrix", text.as_bytes()),
            Some(indicatrix_editor::files::InputFileKind::Design)
        );
        let error = indicatrix_cut_core::native::design_from_str(&text)
            .expect_err("a newer version is refused");
        assert!(error.to_string().contains("newer Indicatrix"), "{error}");
    }

    #[test]
    fn the_picker_lists_the_design_extension_first_and_still_reads_the_legacy_one() {
        assert_eq!(OPEN_EXTENSIONS[0], "isdd");
        assert_eq!(OPEN_EXTENSIONS[1], "indicatrix");
        assert!(!OPEN_EXTENSIONS.contains(&"toml"));
        for kind in ["a.isdd", "a.indicatrix"] {
            assert_eq!(
                indicatrix_editor::files::InputFileKind::from_file_name(kind),
                Some(indicatrix_editor::files::InputFileKind::Design)
            );
        }
    }

    #[test]
    fn file_names_map_between_the_design_file_and_its_asc_name() {
        assert_eq!(asc_name_for_design_file("round.isdd"), "round.asc");
        assert_eq!(asc_name_for_design_file("round.indicatrix"), "round.asc");
        assert_eq!(design_file_name_for_asc("round.asc"), "round.isdd");
        assert_eq!(design_file_name_for_asc("round"), "round.isdd");
        assert_eq!(design_file_name_for_asc("a.b.asc"), "a.b.isdd");
    }
}
