//! Maps a rasterized `facet_id` (a plane index, see `raster::SolidRasterizer::pick_at`)
//! back to the tier/orbit-member it came from, for hover/click/selection and the
//! critical-angle overlay.
//!
//! # Mirroring `Design::planes_from_solved`'s plane order exactly
//!
//! [`FacetMap::from_design`] must assign facet ids that line up 1:1 with
//! `indicatrix_cut_core::Design::planes_from_solved`'s output (`preform.planes()` first,
//! then one plane per (tier, index-on-gear) pair), since that plane slice is exactly
//! what `build_solid_mesh` -- and therefore `pick_at`'s `facet_id` -- indexes into.
//!
//! `StandardGemCuts::from_asc_schedule` is not a simple flatten: its last step is
//! `dedup_planes`, a linear, first-occurrence-wins scan dropping any facet whose
//! normal/offset are within tolerance of one already kept, with no "give me the
//! provenance" entry point, so this module reimplements the same three steps against
//! `Design`'s own tier list: (1) the same crown/pavilion side inheritance for an
//! unsigned-zero `angle_deg` (a running `last_side_is_crown` flag); (2) the same
//! per-(tier, index) normal construction (`sin`/`cos` of `theta = angle_deg.abs()`,
//! azimuth `phi = 2*pi*index/gear_teeth`) at `f32` precision; (3) the same
//! first-occurrence-wins dedup, at the same `1/2048` quantum.
//!
//! A candidate carries its `(tier_index, index_on_gear)` provenance through all
//! three steps, so the facet id a surviving candidate ends up at is the exact index
//! [`Design::planes_from_solved`] gives the same plane, by construction rather than
//! by re-matching floats after the fact.

use glam::Vec3;
use indicatrix::geometry::meet_solver::Block;

mod build;
mod overlay;
#[cfg(test)]
mod tests;

/// Everything this map remembers about one facet beyond its bare plane equation.
///
/// `tier_index` is `None` for a preform plane (the rough's own bounding planes,
/// preceding every schedule-derived facet); other fields are placeholders then.
#[derive(Debug, Clone)]
pub struct FacetInfo {
    pub tier_index: Option<usize>,
    /// The index-wheel position (`ConstraintTier::indices` entry, rounded to the
    /// nearest tooth); `0` for a tier with no listed indices, or a placeholder.
    pub index_on_gear: u32,
    /// This facet's tier's own signed `angle_deg` -- `0.0` for a preform placeholder.
    pub angle_deg: f64,
    pub block: Option<Block>,
    /// This facet's tier's own `name` (empty for unnamed/preform).
    pub name: String,
}

impl FacetInfo {
    const fn preform() -> Self {
        Self {
            tier_index: None,
            index_on_gear: 0,
            angle_deg: 0.0,
            block: None,
            name: String::new(),
        }
    }
}

/// The critical-angle-overlay and selection-tint flags [`FacetMap::overlay_flags`]
/// produces.
///
/// Sized to the full plane count and indexed by `facet_id` -- ready to
/// drop straight into `raster::SolidStyle::flagged`/`pending`/`selected`.
#[derive(Debug, Clone)]
pub struct OverlayFlags {
    pub flagged: Vec<bool>,
    pub pending: Vec<bool>,
    pub selected: Vec<bool>,
}

/// Maps every `facet_id` (plane index) in `Design::planes_from_solved`'s output back
/// to the tier/orbit-member it came from (see this module's doc comment).
#[derive(Debug, Clone)]
pub struct FacetMap {
    preform_plane_count: usize,
    /// One entry per facet id, in `Design::planes_from_solved`'s order: the first
    /// `preform_plane_count` entries are [`FacetInfo::preform`] placeholders.
    facets: Vec<FacetInfo>,
    /// `orbits[tier_index]` is every surviving facet id that tier produced, in
    /// `ConstraintTier::indices` order -- see [`Self::facets_of_tier`].
    orbits: Vec<Vec<u32>>,
}

/// One (tier, index-on-gear) candidate plane, carrying its provenance through the
/// dedup pass.
struct Candidate {
    normal: Vec3,
    offset: f32,
    tier_index: usize,
    index_on_gear: u32,
}

/// Same tolerance `indicatrix::geometry::cuts::dedup_planes` uses (module doc, step 3).
const DEDUP_QUANTUM: f32 = 1.0 / 2048.0;

impl FacetMap {
    /// The number of preform (rough-bounding) planes at the start of the plane
    /// arrangement -- every facet id below this is a preform plane.
    #[must_use]
    pub const fn preform_plane_count(&self) -> usize {
        self.preform_plane_count
    }

    /// The tier a facet id belongs to, or `None` for a preform plane or an out-of-range id.
    #[must_use]
    pub fn tier_of(&self, facet_id: usize) -> Option<usize> {
        self.facets.get(facet_id).and_then(|f| f.tier_index)
    }

    /// The index-wheel tooth this facet sits at (`FacetInfo::index_on_gear`), or
    /// `0` for a preform plane, an out-of-range id, or a tier with no listed
    /// indices. Used by `diagram2d`'s index-wheel radial-line pass to find a
    /// selected facet's own tooth without exposing [`FacetInfo`] itself.
    #[must_use]
    pub fn index_on_gear(&self, facet_id: usize) -> u32 {
        self.facets.get(facet_id).map_or(0, |f| f.index_on_gear)
    }

    /// Every surviving facet id a tier's orbit produced, in `ConstraintTier::indices`
    /// order; can have fewer entries than `indices.len()` (a dedup collision).
    #[must_use]
    pub fn facets_of_tier(&self, tier_index: usize) -> &[u32] {
        self.orbits.get(tier_index).map_or(&[], Vec::as_slice)
    }

    /// Total number of facet ids this map covers (`Design::planes_from_solved`'s
    /// output length) -- every valid `facet_id` for [`Self::tier_of`]/
    /// [`Self::hover_text`]/[`Self::facet_label`] is in `0..self.facet_count()`.
    /// Used by `solid_preview::diagram2d`'s caller to build a facet-id-indexed
    /// label/hover-text table covering every facet, not just the visible ones.
    #[must_use]
    pub const fn facet_count(&self) -> usize {
        self.facets.len()
    }
}
