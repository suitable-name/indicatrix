//! The "Open Recent" list's load-time rule: the list holds design-file paths only
//! (`.isdd`, or a legacy `.indicatrix` one). An entry that names a removed
//! sidecar (`.indicatrix.toml`, or the older `.gemcut.toml`) can no longer be opened, so
//! it is dropped silently when the settings file is read.

use serde::{Deserialize, Deserializer};

/// The removed sidecar suffixes, lower case; a recent-file entry ending in one is pruned.
const REMOVED_SIDECAR_SUFFIXES: [&str; 2] = [".indicatrix.toml", ".gemcut.toml"];

/// `true` when `path` names a sidecar file, which can no longer be opened. Compared
/// case-insensitively, on the path text exactly as the list stores it.
#[must_use]
fn is_removed_sidecar_entry(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    REMOVED_SIDECAR_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// Keeps the entries of `paths` that are not removed sidecars, in order.
#[must_use]
fn prune_removed_sidecars(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| !is_removed_sidecar_entry(path))
        .collect()
}

/// `serde` hook for `AppSettings::recent_native_files`: reads the list and prunes the
/// entries that name a removed sidecar, so a settings file written by an earlier version
/// opens without them and the next save drops them for good.
///
/// # Errors
///
/// The deserializer's own error when the value is not a list of strings.
pub(super) fn deserialize_recent_native_files<'de, D>(
    deserializer: D,
) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer).map(prune_removed_sidecars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_entries_are_pruned_and_design_files_kept_in_order() {
        let kept = prune_removed_sidecars(vec![
            "C:/d/round.indicatrix.toml".to_string(),
            "C:/d/round.isdd".to_string(),
            "C:/d/Old.GemCut.toml".to_string(),
            "C:/d/legacy.indicatrix".to_string(),
        ]);
        assert_eq!(kept, vec!["C:/d/round.isdd", "C:/d/legacy.indicatrix"]);
    }

    #[test]
    fn a_plain_toml_file_is_not_a_removed_sidecar() {
        assert!(!is_removed_sidecar_entry("C:/d/settings.toml"));
        assert!(is_removed_sidecar_entry("C:/d/ROUND.INDICATRIX.TOML"));
    }
}
