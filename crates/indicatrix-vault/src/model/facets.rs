//! Parsing for the scraped `facets_count` field.
//!
//! It packs two numbers into one string (e.g. `"55+6"`): the main facet count and,
//! after a separator, the girdle facet count. See [`parse_facets_count`] for the exact
//! rules -- real-world data from facetdiagrams.org isn't perfectly uniform, so this is
//! deliberately tolerant rather than strict.

/// Splits a scraped `facets_count` value like `"55+6"` (55 main facets + 6 girdle
/// facets) into its two integer parts.
///
/// Real-world data isn't perfectly uniform: besides the common `+` separator, a
/// handful of rows use `-` (an apparent scraping-era typo, e.g. `"78-7"`), a handful
/// have a non-numeric girdle count (`"45+R"` for a rounded girdle, `"80+FC"` for flat
/// culet), and at least one has a doubled separator (`"65++16"`). None of those panic:
///
/// - `None` / empty / whitespace-only input -> `(None, None)` (unknown -- distinct
///   from a genuine zero).
/// - No separator at all (a bare `"57"`) -> `(Some(57), Some(0))`: the whole string is
///   the main facet count, and there are explicitly zero *additional* girdle facets.
/// - A separator (`'+'` or `'-'`) splits the string: the left side is the main facet
///   count, the right side (after stripping any repeated leading separator) is the
///   girdle facet count. Either side that fails to parse as a plain integer becomes
///   `None` for just that half, rather than failing the whole call.
#[must_use]
pub fn parse_facets_count(raw: Option<&str>) -> (Option<i64>, Option<i64>) {
    let Some(raw) = raw else { return (None, None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (None, None);
    }

    let Some(idx) = trimmed.find(['+', '-']) else {
        // No separator: the whole string is the facet count, zero *additional* girdle facets.
        return (trimmed.parse::<i64>().ok(), Some(0));
    };

    let (left, right) = trimmed.split_at(idx);
    let facets = left.trim().parse::<i64>().ok();
    // Strip any leading run of '+'/'-' (handles a doubled separator like "65++16")
    // before parsing the girdle-facet count; a non-numeric remainder ("R", "FC", ...)
    // leaves it `None` rather than panicking or silently defaulting to 0.
    let girdle_facets = right
        .trim_start_matches(['+', '-'])
        .trim()
        .parse::<i64>()
        .ok();

    (facets, girdle_facets)
}

#[cfg(test)]
mod tests {
    use super::parse_facets_count;

    #[test]
    fn splits_the_common_plus_separated_form() {
        assert_eq!(parse_facets_count(Some("55+6")), (Some(55), Some(6)));
        assert_eq!(parse_facets_count(Some("48+8")), (Some(48), Some(8)));
    }

    #[test]
    fn bare_number_gives_zero_girdle_facets_not_none() {
        assert_eq!(parse_facets_count(Some("57")), (Some(57), Some(0)));
        assert_eq!(parse_facets_count(Some("106")), (Some(106), Some(0)));
    }

    #[test]
    fn none_and_empty_and_blank_do_not_panic_and_yield_unknown() {
        assert_eq!(parse_facets_count(None), (None, None));
        assert_eq!(parse_facets_count(Some("")), (None, None));
        assert_eq!(parse_facets_count(Some("   ")), (None, None));
    }

    #[test]
    fn leading_zero_parses_as_plain_integer() {
        assert_eq!(parse_facets_count(Some("09")), (Some(9), Some(0)));
    }

    #[test]
    fn hyphen_separator_is_treated_like_plus() {
        // Real data has exactly one row like this ("34.007 Seven Pointed Star"): a
        // scraping-era typo for "78+7", not a subtraction.
        assert_eq!(parse_facets_count(Some("78-7")), (Some(78), Some(7)));
    }

    #[test]
    fn non_numeric_girdle_suffix_leaves_girdle_facets_none() {
        // "R" (rounded girdle) and "FC" (flat culet) show up in real data instead of a
        // girdle facet count.
        assert_eq!(parse_facets_count(Some("45+R")), (Some(45), None));
        assert_eq!(parse_facets_count(Some("80+FC")), (Some(80), None));
    }

    #[test]
    fn doubled_separator_is_tolerated() {
        assert_eq!(parse_facets_count(Some("65++16")), (Some(65), Some(16)));
    }

    #[test]
    fn non_numeric_facets_side_leaves_facets_none_without_panicking() {
        assert_eq!(parse_facets_count(Some("abc+6")), (None, Some(6)));
    }
}
