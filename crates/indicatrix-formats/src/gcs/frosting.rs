//! What a `.gcs` file states as frosted, in the terms of the `.asc` schedule
//! [`super::gcs_to_asc_schedule`] makes from it.
//!
//! `.asc` has no notation for frosting (and none is invented here), so the schedule drops it.
//! [`gcs_frosting`] reads it from the parsed file instead, by tier position and by facet slot,
//! so a caller that builds a design from the schedule can apply it beside it; the writer
//! ([`super::to_gcs_string_with_frosting`]) takes the same list back, so a round trip keeps it.
//!
//! A tier is named by its position among the `<tier>` elements, which is its position in the
//! converted schedule's tiers; a facet by its slot in the tier, which is its position in that
//! schedule tier's `indices` (one index per `<facet>`, in file order).

use super::{design::GcsDesign, tier::GcsTier};

/// How one tier is frosted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GcsTierFrosting {
    /// Every facet of the tier.
    Whole,
    /// Only the facets in these slots (ascending, never empty).
    Facets(Vec<usize>),
}

/// The frosting of one tier, if it has any.
///
/// The tier-level `frosting` attribute (GCS 1.1's "Frosted" toggle) frosts the whole tier. A
/// tier whose facets all state frosting is whole as well. Otherwise the facets that state it,
/// when there are any, are frosted one by one.
fn tier_frosting(tier: &GcsTier) -> Option<GcsTierFrosting> {
    let frosted: Vec<usize> = tier
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.is_frosted())
        .map(|(slot, _)| slot)
        .collect();
    if tier.is_frosted() || (!tier.facets.is_empty() && frosted.len() == tier.facets.len()) {
        Some(GcsTierFrosting::Whole)
    } else if frosted.is_empty() {
        None
    } else {
        Some(GcsTierFrosting::Facets(frosted))
    }
}

/// Every frosted tier of `design` as `(tier position, frosting)`, ascending by position.
///
/// Empty for a file that frosts nothing, which is what every file Gem Cut Studio writes
/// without the "Frosted" toggle is.
#[must_use]
pub fn gcs_frosting(design: &GcsDesign) -> Vec<(usize, GcsTierFrosting)> {
    design
        .tiers
        .iter()
        .enumerate()
        .filter_map(|(position, tier)| tier_frosting(tier).map(|frosting| (position, frosting)))
        .collect()
}

/// The value the writer gives a frosted tier or facet: `0.5`, as Gem Cut Studio 1.1 writes it.
pub(super) const FROSTED_VALUE: &str = "0.5";

/// How the tier at `position` is frosted in `frosting`, if it is.
pub(super) fn frosting_of(
    frosting: &[(usize, GcsTierFrosting)],
    position: usize,
) -> Option<&GcsTierFrosting> {
    frosting
        .iter()
        .find(|(at, _)| *at == position)
        .map(|(_, how)| how)
}
