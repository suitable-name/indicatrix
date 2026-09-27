//! [`GcsDesign`]: a fully parsed Gem Cut Studio `.gcs` design.

use super::{
    metadata::{GcsIndex, GcsInfo, GcsRender},
    tier::GcsTier,
};
use std::fmt;

/// A fully parsed Gem Cut Studio `.gcs` design.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsDesign {
    /// The `<GemCutStudio version="...">` root's version string (`"1000"` in
    /// every sampled file).
    pub version: String,
    /// The shared index-wheel setup.
    pub index: GcsIndex,
    /// Every tier, in file order.
    pub tiers: Vec<GcsTier>,
    /// Preview material/display settings, when present.
    pub render: Option<GcsRender>,
    /// Free-text design metadata, when present.
    pub info: Option<GcsInfo>,
}

impl GcsDesign {
    /// Total number of solved facet planes across every tier.
    #[must_use]
    pub fn facet_plane_count(&self) -> usize {
        self.tiers.iter().map(GcsTier::facet_plane_count).sum()
    }
}

impl fmt::Display for GcsDesign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcsDesign(gear={}, tiers={}, facets={})",
            self.index.gear,
            self.tiers.len(),
            self.facet_plane_count()
        )
    }
}
