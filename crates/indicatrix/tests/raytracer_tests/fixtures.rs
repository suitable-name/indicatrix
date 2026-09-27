//! Shared test fixtures for the girdle-finish and white-furnace tests.

use indicatrix::{
    geometry::cuts::STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, optics::raytracer::FacetFinish,
};

/// Builds a `facet_finishes` slice sized to `planes.len()`, `Polished` everywhere except
/// the girdle band (`STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS`), which is `Frosted`.
pub fn bruted_girdle_finishes(num_planes: usize) -> Vec<FacetFinish> {
    let mut finishes = vec![FacetFinish::Polished; num_planes];
    for i in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
        finishes[i] = FacetFinish::Frosted;
    }
    finishes
}
