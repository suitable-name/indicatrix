//! The design-library protocol's client side: a one-shot request plus a held session
//! ([`client`]), the local-vs-remote source abstraction the viewer's library UI reads
//! through ([`source`]), and the pull-mirror sync that copies a remote worker's
//! library into the local database ([`mirror`]). `mirror` and `source` both build on
//! `client`'s connection handling rather than duplicating it.

use indicatrix_net::library::FrostedTierWire;
use indicatrix_vault::model::frosting::StoredTierFrosting;

pub mod client;
pub mod mirror;
pub mod source;

/// A design's frosting as the library protocol carries it ([`FrostedTierWire`], on
/// `DesignRecord::frosting` and `LibraryResponse::DesignSource::frosting`) in the form the
/// local library stores (`Database::set_diagram_frosting`) and the editor applies
/// (`StoredTierFrosting::to_gcs`, then `Design::apply_imported_frosting`).
#[must_use]
pub fn frosting_from_wire(frosting: &[FrostedTierWire]) -> Vec<StoredTierFrosting> {
    frosting
        .iter()
        .map(|row| StoredTierFrosting {
            tier: row.tier,
            facets: row.facets.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_frosting_becomes_the_stored_rows_unchanged() {
        let wire = vec![
            FrostedTierWire {
                tier: 0,
                facets: Vec::new(),
            },
            FrostedTierWire {
                tier: 4,
                facets: vec![2, 5],
            },
        ];
        assert_eq!(
            frosting_from_wire(&wire),
            vec![
                StoredTierFrosting::whole(0),
                StoredTierFrosting {
                    tier: 4,
                    facets: vec![2, 5]
                },
            ]
        );
        assert_eq!(
            frosting_from_wire(&[]),
            [] as [indicatrix_vault::model::frosting::StoredTierFrosting; 0]
        );
    }
}
