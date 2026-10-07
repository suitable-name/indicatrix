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
//! `Design`'s own tier list: (1) the same crown/pavilion side rule, the shared
//! `indicatrix::geometry::plane::tier_is_crown_side` (sign of the angle, including
//! the sign of a zero); (2) the same
//! per-(tier, index) normal construction (`sin`/`cos` of `theta = angle_deg.abs()`,
//! azimuth from `StandardGemCuts::index_to_azimuth`, which applies the schedule's
//! gear reference angle) at `f32` precision; (3) the same first-occurrence-wins
//! dedup: the shared `indicatrix::geometry::cuts::normals_coincide` rule on the
//! normals and an offset tolerance of `1/2048` relative to `max(|d|, 1)`.
//!
//! A candidate carries its `(tier_index, index_on_gear)` provenance through all
//! three steps, so the facet id a surviving candidate ends up at is the exact index
//! [`Design::planes_from_solved`] gives the same plane, by construction rather than
//! by re-matching floats after the fact.
//!
//! # Concave facets
//!
//! A design's concave tiers resolve to tools, and a tool is one more facet of the
//! stone: tool `k` has facet id `plane_count + k`. [`FacetMap::from_design_with_tools`]
//! appends those ids *after* the flat ones from the `(concave tier, placement)` list
//! `Design::concave_tools_from_solved` returns, so the flat construction above is not
//! touched and a design without concave tiers builds the identical map.

use glam::Vec3;
use indicatrix::geometry::meet_solver::Block;

mod build;
mod cut;
mod overlay;
#[cfg(test)]
mod tests;

/// Whether a facet is one of the stone's flat planes or the surface a concave tool
/// cut into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetKind {
    /// A flat facet (or a preform plane): every id below the plane count.
    Flat,
    /// The surface of one concave tool: `tier` indexes `Design::concave_tiers` and
    /// `placement` the tier's `indices`.
    Concave {
        /// Index into `Design::concave_tiers`.
        tier: usize,
        /// Index into that tier's `indices`.
        placement: usize,
    },
}

/// Everything this map remembers about one facet beyond its bare plane equation.
///
/// `tier_index` is `None` for a preform plane (the rough's own bounding planes,
/// preceding every schedule-derived facet); other fields are placeholders then. It is
/// also `None` for a concave facet, whose tier lives in a different list: read
/// [`Self::kind`] for that.
#[derive(Debug, Clone)]
pub struct FacetInfo {
    /// Index of the owning tier, or `None` for a preform plane.
    pub tier_index: Option<usize>,
    /// The index-wheel position (`ConstraintTier::indices` entry, rounded to the
    /// nearest tooth); `0` for a tier with no listed indices, or a placeholder.
    pub index_on_gear: u32,
    /// This facet's tier's own signed `angle_deg` -- `0.0` for a preform placeholder.
    pub angle_deg: f64,
    /// Block classification of the owning tier, if any.
    pub block: Option<Block>,
    /// This facet's tier's code in cutting order (`P1`, `G1`, `C1`, `T`, `Culet`; see
    /// `indicatrix_cut_core::compute_tier_labels`), the label the diagram draws and the cutting
    /// sheet's label column prints; empty for a preform plane. For a concave facet, its
    /// on-diagram label (tier name, plus the placement's index when the tier has several).
    pub name: String,
    /// Canonical display name for hover tooltips.
    pub display_name: String,
    /// Flat plane or concave tool surface.
    pub kind: FacetKind,
}

impl FacetInfo {
    const fn preform() -> Self {
        Self {
            tier_index: None,
            index_on_gear: 0,
            angle_deg: 0.0,
            block: None,
            name: String::new(),
            display_name: String::new(),
            kind: FacetKind::Flat,
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
    /// Per-facet flag, indexed by facet id: facet is flagged by the critical-angle overlay.
    pub flagged: Vec<bool>,
    /// Per-facet flag, indexed by facet id: facet awaits a re-solve.
    pub pending: Vec<bool>,
    /// Per-facet flag, indexed by facet id: facet belongs to a selected tier.
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
    /// How many leading entries of `facets` are flat; the rest are concave facets
    /// in tool order, whose hover texts are `concave_hover` (same order).
    flat_facet_count: usize,
    /// Hover text per concave facet: `"{tier name} {CODE} θ {theta:.1}° D {d:.3}"`.
    concave_hover: Vec<String>,
}

/// One (tier, index-on-gear) candidate plane, carrying its provenance through the
/// dedup pass.
struct Candidate {
    normal: Vec3,
    offset: f32,
    tier_index: usize,
    index_on_gear: u32,
}

/// Relative offset tolerance of the plane builder's dedup (module doc, step 3): two
/// offsets merge when they differ by at most this times `max(|d_a|, |d_b|, 1)`.
const OFFSET_REL_EPSILON: f32 = 1.0 / 2048.0;

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

    /// Whether `facet_id` is a flat facet or a concave tool's surface; `Flat` for an
    /// out-of-range id.
    #[must_use]
    pub fn kind_of(&self, facet_id: usize) -> FacetKind {
        self.facets
            .get(facet_id)
            .map_or(FacetKind::Flat, |info| info.kind)
    }

    /// Facet id -> the block of a concave facet's tier (`None` for a flat facet),
    /// the table `diagram2d::DiagramStyle::tool_facet_block` is filled from.
    #[must_use]
    pub fn tool_facet_blocks(&self) -> Vec<Option<Block>> {
        self.facets
            .iter()
            .map(|info| match info.kind {
                FacetKind::Flat => None,
                FacetKind::Concave { .. } => info.block,
            })
            .collect()
    }

    /// Total number of facet ids this map covers (`Design::planes_from_solved`'s
    /// output length, plus one per concave tool) -- every valid `facet_id` for [`Self::tier_of`]/
    /// [`Self::hover_text`]/[`Self::facet_label`] is in `0..self.facet_count()`.
    /// Used by `solid_preview::diagram2d`'s caller to build a facet-id-indexed
    /// label/hover-text table covering every facet, not just the visible ones.
    #[must_use]
    pub const fn facet_count(&self) -> usize {
        self.facets.len()
    }
}
