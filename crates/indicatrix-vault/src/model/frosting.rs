//! The frosting a stored design states, kept beside its rows.
//!
//! The angle-table rows of a library design cannot hold it (`.asc` has no notation for
//! frosting), so a design imported from a `.gcs` file, or saved from the editor, keeps it in
//! the `diagram_frosting` side table -- see `Database::set_diagram_frosting` and
//! `Database::diagram_frosting`.
//!
//! A tier is named by its position in the design's schedule (the order of the angle rows),
//! and a frosted facet by its slot in the tier's index list, which is how
//! `indicatrix_formats::gcs::gcs_frosting` names them. Both are stable for a stored design,
//! whose rows are written from that same schedule.

use indicatrix_formats::gcs::GcsTierFrosting;
use serde::{Deserialize, Serialize};

/// How one tier of a stored design is frosted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTierFrosting {
    /// The tier's position in the design's schedule (0-based).
    pub tier: u32,
    /// The slots, in the tier's index list, of the frosted facets (ascending, without
    /// repeats). Empty when the whole tier is frosted.
    pub facets: Vec<u32>,
}

impl StoredTierFrosting {
    /// A whole-tier frosting of the tier at `tier`.
    #[must_use]
    pub const fn whole(tier: u32) -> Self {
        Self {
            tier,
            facets: Vec::new(),
        }
    }

    /// Whether this frosts the whole tier.
    #[must_use]
    pub const fn is_whole(&self) -> bool {
        self.facets.is_empty()
    }

    /// `frosting` (what `gcs_frosting` reads, or `ConvertedDesign::frosting` carries) in
    /// stored form. A tier position past `u32::MAX` is dropped.
    #[must_use]
    pub fn from_gcs(frosting: &[(usize, GcsTierFrosting)]) -> Vec<Self> {
        let rows = frosting.iter().filter_map(|(position, how)| {
            let tier = u32::try_from(*position).ok()?;
            match how {
                GcsTierFrosting::Whole => Some(Self::whole(tier)),
                GcsTierFrosting::Facets(slots) => {
                    let facets: Vec<u32> = slots
                        .iter()
                        .filter_map(|&s| u32::try_from(s).ok())
                        .collect();
                    // A facet list that names nothing frosts nothing.
                    (!facets.is_empty()).then_some(Self { tier, facets })
                }
            }
        });
        normalised(rows.collect())
    }

    /// `stored` in the form `Design::apply_imported_frosting` takes: by tier position, ascending.
    #[must_use]
    pub fn to_gcs(stored: &[Self]) -> Vec<(usize, GcsTierFrosting)> {
        normalised(stored.to_vec())
            .into_iter()
            .map(|row| {
                let position = row.tier as usize;
                let how = if row.facets.is_empty() {
                    GcsTierFrosting::Whole
                } else {
                    GcsTierFrosting::Facets(row.facets.iter().map(|&s| s as usize).collect())
                };
                (position, how)
            })
            .collect()
    }
}

/// `rows` in canonical form: one row per tier (a later row for the same tier wins), tiers
/// ascending, each tier's slots ascending without repeats.
#[must_use]
pub fn normalised(rows: Vec<StoredTierFrosting>) -> Vec<StoredTierFrosting> {
    let mut by_tier: std::collections::BTreeMap<u32, Vec<u32>> = std::collections::BTreeMap::new();
    for mut row in rows {
        row.facets.sort_unstable();
        row.facets.dedup();
        by_tier.insert(row.tier, row.facets);
    }
    by_tier
        .into_iter()
        .map(|(tier, facets)| StoredTierFrosting { tier, facets })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcs_frosting_round_trips_through_the_stored_form() {
        let gcs = vec![
            (0, GcsTierFrosting::Whole),
            (3, GcsTierFrosting::Facets(vec![1, 4, 5])),
        ];
        let stored = StoredTierFrosting::from_gcs(&gcs);
        assert_eq!(
            stored,
            vec![
                StoredTierFrosting::whole(0),
                StoredTierFrosting {
                    tier: 3,
                    facets: vec![1, 4, 5]
                }
            ]
        );
        assert_eq!(StoredTierFrosting::to_gcs(&stored), gcs);
    }

    #[test]
    fn normalising_orders_tiers_and_slots_and_keeps_the_last_row_of_a_tier() {
        let rows = vec![
            StoredTierFrosting {
                tier: 5,
                facets: vec![3, 1, 3],
            },
            StoredTierFrosting::whole(2),
            StoredTierFrosting {
                tier: 2,
                facets: vec![7],
            },
        ];
        assert_eq!(
            normalised(rows),
            vec![
                StoredTierFrosting {
                    tier: 2,
                    facets: vec![7]
                },
                StoredTierFrosting {
                    tier: 5,
                    facets: vec![1, 3]
                },
            ]
        );
    }
}
