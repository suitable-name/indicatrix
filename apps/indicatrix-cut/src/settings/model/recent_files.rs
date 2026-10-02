//! The "Open Recent" list's rule for the two native file kinds: a self-contained
//! `.indicatrix` design file and the older `.indicatrix.toml`/`.gemcut.toml` sidecar
//! both belong in it, and a design file replaces the older file it was saved over.

/// The older sidecar suffixes a design file supersedes, lower case.
const OLDER_SIDECAR_SUFFIXES: [&str; 2] = [".indicatrix.toml", ".gemcut.toml"];

/// The design file extension, lower case, with its dot.
const DESIGN_SUFFIX: &str = ".indicatrix";

/// `true` when `existing` is an older sidecar of the very design that the design file
/// at `new_path` now stores: same folder and stem, an older suffix. A save (or open) of
/// `foo.indicatrix` makes `foo.indicatrix.toml` in the Open Recent list redundant, so
/// the list prefers the new extension and drops the older entry. Compared
/// case-insensitively; paths are compared as text, exactly as the list stores them.
#[must_use]
pub(super) fn is_superseded_sidecar(existing: &str, new_path: &str) -> bool {
    let new_lower = new_path.to_ascii_lowercase();
    let Some(stem) = new_lower.strip_suffix(DESIGN_SUFFIX) else {
        return false;
    };
    let existing_lower = existing.to_ascii_lowercase();
    OLDER_SIDECAR_SUFFIXES
        .iter()
        .any(|suffix| existing_lower == format!("{stem}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_design_file_supersedes_the_sidecars_of_the_same_stem() {
        assert!(is_superseded_sidecar(
            "C:/d/round.indicatrix.toml",
            "C:/d/round.indicatrix"
        ));
        assert!(is_superseded_sidecar(
            "C:/d/Round.GemCut.toml",
            "C:/d/round.indicatrix"
        ));
    }

    #[test]
    fn other_stems_folders_and_kinds_are_not_superseded() {
        assert!(!is_superseded_sidecar(
            "C:/d/other.indicatrix.toml",
            "C:/d/round.indicatrix"
        ));
        assert!(!is_superseded_sidecar(
            "C:/e/round.indicatrix.toml",
            "C:/d/round.indicatrix"
        ));
        assert!(!is_superseded_sidecar(
            "C:/d/round.indicatrix",
            "C:/d/round.indicatrix"
        ));
        // An older sidecar being recorded never removes anything.
        assert!(!is_superseded_sidecar(
            "C:/d/round.indicatrix",
            "C:/d/round.indicatrix.toml"
        ));
    }
}
