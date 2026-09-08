//! Cross-source duplicate detection (see `crate::db::sqlite::Database::find_cross_source_duplicates`).
//!
//! `diagram_entries.url` is `UNIQUE`, which dedupes a re-synced row against itself
//! within one source -- but the same physical design scraped from two different sites
//! naturally has two different URLs. This module only *detects and surfaces* that
//! (title/designer/facet-count match across different `source_id`s); it never merges
//! automatically, since two different designers can genuinely publish same-named
//! designs.

use serde::{Deserialize, Serialize};

/// One candidate cross-source match for a design just synced into the catalogue.
///
/// An entry already in the catalogue, under a *different* `source_id`, whose
/// normalised `(title, designer)` and facet count line up with the new design.
/// Surfaced as data to review, not acted on automatically -- see this module's doc
/// comment for why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossSourceDuplicate {
    /// The `diagram_entries.id` of the already-stored row this collides with.
    pub existing_entry_id: i64,
    /// The `source_id` that existing row was synced from.
    pub existing_source_id: String,
    pub existing_title: String,
    pub existing_designer_info: Option<String>,
}

/// Lowercases, trims, and collapses internal whitespace runs to a single space.
///
/// Enough to match "Barion Heart" against " barion   heart " without needing a full
/// fuzzy-matching library for what is, in practice, mostly whitespace/case noise
/// between two independently-scraped sites.
#[must_use]
pub fn normalize_for_dedup(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace_and_case() {
        assert_eq!(normalize_for_dedup("  Barion   Heart  "), "barion heart");
        assert_eq!(normalize_for_dedup("Barion Heart"), "barion heart");
        assert_eq!(normalize_for_dedup("BARION\tHEART"), "barion heart");
    }

    #[test]
    fn normalize_of_empty_string_is_empty() {
        assert_eq!(normalize_for_dedup("   "), "");
    }
}
