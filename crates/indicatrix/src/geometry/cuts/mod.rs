//! Facet-plane generators for the crate's reference cuts and for
//! reconstructing a schedule's planes from scraped or parsed data.
//!
//! [`StandardGemCuts`] is a zero-sized marker type; its methods are spread
//! across this folder by data source: [`round_brilliant`] (the 57-facet
//! Standard Round Brilliant reference cut and the shared
//! [`StandardGemCuts::index_to_azimuth`] helper), [`emerald`] (the Emerald
//! Cut reference solid), [`database_angles`] (fabricated proportions from
//! scraped `angle_settings` rows, [`FacetSpec`]), and [`asc_schedule`] (real
//! per-tier depths from a parsed `.asc` schedule). [`error`] holds
//! [`CutError`], the failure type for the `.asc` reconstruction path.

mod asc_schedule;
mod database_angles;
mod emerald;
mod error;
mod facet_spec;
mod round_brilliant;
#[cfg(test)]
mod tests;

pub use asc_schedule::normals_coincide;
pub use error::CutError;
pub use facet_spec::FacetSpec;
pub use round_brilliant::STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS;

/// Zero-sized marker type whose associated functions generate or reconstruct
/// a stone's facet planes; see the module docs for how its methods are split
/// across this folder.
pub struct StandardGemCuts;
