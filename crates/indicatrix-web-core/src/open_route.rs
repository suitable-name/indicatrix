//! What the Open picker offers, and the file names a design is saved under.
//!
//! Which loader a picked or dropped file goes to is decided by
//! [`indicatrix_editor::files::InputFileKind::classify`] (by content where the name
//! cannot tell: a `.toml` or `.indicatrix` file is a self-contained design file or an
//! older overlay sidecar according to its header, never merely because the name ends
//! in `.toml`).

use indicatrix_formats::native::design::{DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED};

/// The extensions the Open picker offers, the design file first, then the older
/// formats it still reads.
pub const OPEN_EXTENSIONS: [&str; 6] = [DESIGN_EXTENSION, "asc", "toml", "gem", "gcs", "hdr"];

/// The `.asc` name a design file called `file_name` is recorded under, so a later
/// save proposes the same stem (`round.indicatrix` -> `round.asc`).
#[must_use]
pub fn asc_name_for_design_file(file_name: &str) -> String {
    indicatrix_editor::files::converted_asc_file_name(file_name)
}

/// The design file name for a design recorded under `asc_name`: the extension replaced
/// by `.indicatrix` (`round.asc` -> `round.indicatrix`), or appended to a name with no
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
    use indicatrix_formats::native::design::DESIGN_VERSION_CONCAVE;

    #[test]
    fn a_file_from_a_newer_version_is_refused_with_a_clear_message() {
        let text = format!(
            "format = \"indicatrix-design\"\nversion = {}\n",
            DESIGN_VERSION_CONCAVE + 1
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
    fn the_picker_lists_the_design_extension_first() {
        assert_eq!(OPEN_EXTENSIONS[0], "indicatrix");
        assert!(OPEN_EXTENSIONS.contains(&"toml"));
    }

    #[test]
    fn file_names_map_between_the_design_file_and_its_asc_name() {
        assert_eq!(asc_name_for_design_file("round.indicatrix"), "round.asc");
        assert_eq!(design_file_name_for_asc("round.asc"), "round.indicatrix");
        assert_eq!(design_file_name_for_asc("round"), "round.indicatrix");
        assert_eq!(design_file_name_for_asc("a.b.asc"), "a.b.indicatrix");
    }
}
