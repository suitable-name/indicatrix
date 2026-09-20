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
use indicatrix::geometry::meet_solver::{Block, SolvedTier, classify_blocks};
use indicatrix_cut_core::{
    Design,
    optics_hints::{self, Risk},
};

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
    /// Builds the map for `design` against an already-solved mast list -- `solved`
    /// must come from the same solve as the plane arrangement this map describes,
    /// or the facet ids will not line up with the rasterizer's `pick_at` results.
    #[must_use]
    pub fn from_design(design: &Design, solved: &[SolvedTier]) -> Self {
        let preform_plane_count = design.preform.planes().len();
        let inputs = design.meet_tier_inputs();
        let blocks = classify_blocks(&inputs);
        let gear_teeth = (design.meta.gear_teeth_abs().max(1)) as f32;

        let mut candidates: Vec<Candidate> = Vec::new();
        let mut last_side_is_crown = true;
        for (tier_index, tier) in design.tiers.iter().enumerate() {
            let is_crown = if tier.angle_deg == 0.0 {
                if tier.angle_deg.is_sign_negative() {
                    false
                } else {
                    last_side_is_crown
                }
            } else {
                tier.angle_deg > 0.0
            };
            last_side_is_crown = is_crown;

            let theta = (tier.angle_deg.abs() as f32).to_radians();
            let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
            let mast = solved.get(tier_index).map_or(0.0, |s| s.mast);
            let offset = -(mast.abs() as f32);

            if tier.indices.is_empty() {
                let normal = if is_crown {
                    Vec3::new(0.0, cos_theta, sin_theta)
                } else {
                    Vec3::new(0.0, -cos_theta, sin_theta)
                };
                candidates.push(Candidate {
                    normal: normal.normalize(),
                    offset,
                    tier_index,
                    index_on_gear: 0,
                });
                continue;
            }

            for &idx in &tier.indices {
                let phi = 2.0 * std::f32::consts::PI * (idx as f32) / gear_teeth;
                let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
                let normal = if is_crown {
                    Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
                } else {
                    Vec3::new(sin_theta * cos_phi, -cos_theta, sin_theta * sin_phi)
                };
                candidates.push(Candidate {
                    normal: normal.normalize(),
                    offset,
                    tier_index,
                    index_on_gear: idx.round() as u32,
                });
            }
        }

        // First-occurrence-wins dedup, mirroring `dedup_planes` (module doc, step 3).
        let mut kept: Vec<Candidate> = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let is_duplicate = kept.iter().any(|k| {
                let dot = k.normal.dot(candidate.normal);
                dot >= 1.0 - DEDUP_QUANTUM && (k.offset - candidate.offset).abs() <= DEDUP_QUANTUM
            });
            if !is_duplicate {
                kept.push(candidate);
            }
        }

        let mut facets = vec![FacetInfo::preform(); preform_plane_count];
        let mut orbits: Vec<Vec<u32>> = vec![Vec::new(); design.tiers.len()];
        for candidate in kept {
            let facet_id = facets.len() as u32;
            let block = blocks.get(candidate.tier_index).copied();
            let (angle_deg, name) = design
                .tiers
                .get(candidate.tier_index)
                .map_or((0.0, String::new()), |t| (t.angle_deg, t.name.clone()));
            facets.push(FacetInfo {
                tier_index: Some(candidate.tier_index),
                index_on_gear: candidate.index_on_gear,
                angle_deg,
                block,
                name,
            });
            if let Some(orbit) = orbits.get_mut(candidate.tier_index) {
                orbit.push(facet_id);
            }
        }

        Self {
            preform_plane_count,
            facets,
            orbits,
        }
    }

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
    /// indices. Used by `diagram2d`'s index-wheel radial-line pass (#121) to find
    /// a selected facet's own tooth without exposing [`FacetInfo`] itself.
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

    /// The facet's own short on-diagram label: `"<tier name> <index>"` (the one
    /// number a cutter reads off a `GemCad` diagram, per #28), or just the tier name
    /// for a tier with no listed indices (a table/culet, where the index is always
    /// `0` and carries no information). `""` for a preform plane, an unnamed tier,
    /// or an out-of-range id -- unlike the fuller [`Self::hover_text`] tooltip.
    /// Returns an owned `String` (rather than the old plain tier-name `&str`)
    /// since the index has to be formatted in; the one caller
    /// (`preview_state::update_diagram_memory_from_design`) already turned the old
    /// `&str` into an owned `String` immediately anyway.
    #[must_use]
    pub fn facet_label(&self, facet_id: usize) -> String {
        let Some(info) = self.facets.get(facet_id) else {
            return String::new();
        };
        let Some(tier_index) = info.tier_index else {
            return String::new();
        };
        if info.name.is_empty() {
            return String::new();
        }
        let has_orbit = self
            .orbits
            .get(tier_index)
            .is_some_and(|orbit| orbit.len() > 1);
        if has_orbit {
            format!("{} {}", info.name, info.index_on_gear)
        } else {
            info.name.clone()
        }
    }

    /// A short hover tooltip for `facet_id`: `"<tier name>  ·  <angle>°  ·  index
    /// <i>  ·  <block>  ·  margin <x>° over critical"` for a pavilion facet, or the
    /// same without the margin clause for a crown/girdle facet, or `"Preform"`.
    /// `n_d` is the design's effective refractive index the margin is computed
    /// against.
    ///
    /// #123: the margin clause used to be printed for every facet, but
    /// `optics_hints::tier_margin_and_risk` (the table's own path, `state/mod.rs`)
    /// only has a real windowing answer for `Block::Pavilion` -- a crown or girdle
    /// facet cannot window, so a margin figure there means nothing and undermines
    /// the reading of the pavilion facets' real margins.
    #[must_use]
    pub fn hover_text(&self, facet_id: usize, n_d: f64) -> String {
        let Some(info) = self.facets.get(facet_id) else {
            return String::new();
        };
        let Some(_tier_index) = info.tier_index else {
            return "Preform".to_string();
        };
        let block_text = match info.block {
            Some(Block::Crown) => "Crown",
            Some(Block::Pavilion) => "Pavilion",
            Some(Block::Girdle) => "Girdle",
            None => "?",
        };
        let name = if info.name.is_empty() {
            "(unnamed)"
        } else {
            info.name.as_str()
        };
        let angle = info.angle_deg;
        let index = info.index_on_gear;
        if info.block == Some(Block::Pavilion) {
            let margin = optics_hints::tier_margin_deg(info.angle_deg, n_d);
            format!(
                "{name}  ·  {angle:.1}°  ·  index {index}  ·  {block_text}  ·  margin {margin:.1}° over critical"
            )
        } else {
            format!("{name}  ·  {angle:.1}°  ·  index {index}  ·  {block_text}")
        }
    }

    /// The critical-angle-risk (`flagged`), pending-resolve (`pending`) and
    /// list-selection (`selected`) overlay flags for every facet id -- see
    /// [`OverlayFlags`]'s doc comment.
    ///
    /// `design` is read fresh here (rather than this map's own cached
    /// [`FacetInfo::angle_deg`]) so a caller mid-edit still sees the CURRENT authored
    /// angle for the windowing check, even before the mesh itself catches up.
    /// Windowing risk applies only to pavilion tiers (`angle_deg < 0.0`), per
    /// `optics_hints`'s own module doc comment.
    #[must_use]
    pub fn overlay_flags(
        &self,
        design: &Design,
        n_d: f64,
        selected_tier: Option<usize>,
        pending_tier: Option<usize>,
    ) -> OverlayFlags {
        let plane_count = self.facets.len();
        let mut flagged = vec![false; plane_count];
        let mut pending = vec![false; plane_count];
        let mut selected = vec![false; plane_count];

        for (facet_id, info) in self.facets.iter().enumerate() {
            let Some(tier_index) = info.tier_index else {
                continue;
            };
            if pending_tier == Some(tier_index) {
                pending[facet_id] = true;
            }
            if selected_tier == Some(tier_index) {
                selected[facet_id] = true;
            }
            let Some(tier) = design.tiers.get(tier_index) else {
                continue;
            };
            if tier.angle_deg < 0.0
                && optics_hints::windowing_risk(tier.angle_deg, n_d) == Risk::Windows
            {
                flagged[facet_id] = true;
            }
        }

        OverlayFlags {
            flagged,
            pending,
            selected,
        }
    }

    /// Every facet-id pair that should carry a meet-point marker (P1 item 29):
    /// for every tier pair `Design::facet_meets` names as meeting each other, the
    /// full cross product of that pair's surviving facet ids.
    ///
    /// `Design::facet_meets` is resolved by the same [`indicatrix::geometry::
    /// meet_solver::MeetNameResolver`] `Design::solve` itself uses (girdle/culet/
    /// table fallbacks, side-prefix and plural stripping, compound vertex specs),
    /// not a naive name match -- see that method's own doc comment. It only names
    /// TIERS, not facets or geometry, so the actual marker POINT is left for
    /// `diagram2d::render_diagram` to find: the world-space vertex the two
    /// facets' mesh rings genuinely share (see that module's `meet_marker_points`).
    /// Cross-producting a tier pair's orbits (rather than trying to line up
    /// indices here) is deliberately generous -- a facet with no real shared
    /// vertex with its candidate partner simply contributes no marker once
    /// `diagram2d` fails to find one, so over-listing costs a few wasted
    /// ring-vs-ring comparisons, never a wrong marker.
    #[must_use]
    pub fn meeting_facet_pairs(&self, design: &Design) -> Vec<(u32, u32)> {
        // A `BTreeSet` rather than a linear-scan `Vec` dedup: both deterministic
        // (a sorted key, not iteration order, decides output order) and avoids an
        // O(pairs^2) scan on a design with many meet-named tiers.
        let mut pairs: std::collections::BTreeSet<(u32, u32)> = std::collections::BTreeSet::new();
        for tier_index in 0..design.tiers.len() {
            let Ok(partners) = design.facet_meets(tier_index) else {
                continue;
            };
            for partner_tier in partners {
                if partner_tier == tier_index {
                    continue;
                }
                for &facet_a in self.facets_of_tier(tier_index) {
                    for &facet_b in self.facets_of_tier(partner_tier) {
                        pairs.insert((facet_a.min(facet_b), facet_a.max(facet_b)));
                    }
                }
            }
        }
        pairs.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    /// A synthetic "RBC-445"-style design: the tier table
    /// `StandardGemCuts::standard_round_brilliant` hardcodes as raw planes,
    /// reauthored as [`ConstraintTier`]s so [`Design::planes_from_solved`] derives
    /// the identical arrangement through the tier -> schedule -> plane path this
    /// module mirrors. Every tier pinned via `ScaleReference`, so the design solves
    /// trivially.
    fn standard_round_brilliant_design() -> Design {
        const GIRDLE_INDICES: [f64; 16] = [
            0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
            90.0,
        ];
        const BREAK_INDICES: [f64; 16] = [
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
        ];
        const MAIN_INDICES: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
        const STAR_INDICES: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

        fn tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
            ConstraintTier {
                angle_deg,
                name: name.to_string(),
                indices: indices.to_vec(),
                constraint: MeetConstraint::ScaleReference(mast),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            }
        }

        let tiers = vec![
            tier("Table", 0.0, &[], 0.32),
            tier("Star", 15.0, &STAR_INDICES, 0.45),
            tier("Crown Main", 34.5, &MAIN_INDICES, 0.59),
            tier("Upper Girdle", 41.0, &BREAK_INDICES, 0.67),
            tier("Girdle", 90.0, &GIRDLE_INDICES, 1.0),
            tier("Pavilion Main", -41.0, &MAIN_INDICES, 0.67),
            tier("Lower Girdle", -42.5, &BREAK_INDICES, 0.68),
            tier("Culet", -0.0, &[], 0.88),
        ];

        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta {
                gemcad_version: "GemCad 5.0".to_string(),
                gear_teeth: 96,
                gear_reference_angle: 0.0,
                symmetry_order: 8,
                mirror: true,
                refractive_index: 1.54,
                headers: Vec::new(),
                footnotes: Vec::new(),
            },
            tiers,
        )
    }

    #[test]
    fn plane_count_matches_design_planes_on_the_standard_round_brilliant() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let planes = design.planes_from_solved(&solved);
        let map = FacetMap::from_design(&design, &solved);

        assert_eq!(map.facets.len(), planes.len());
        assert_eq!(map.preform_plane_count(), design.preform.planes().len());
    }

    #[test]
    fn preform_planes_map_to_no_tier() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let map = FacetMap::from_design(&design, &solved);

        for facet_id in 0..map.preform_plane_count() {
            assert_eq!(map.tier_of(facet_id), None);
        }
    }

    #[test]
    fn orbit_sizes_match_each_tiers_index_count() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let map = FacetMap::from_design(&design, &solved);

        for (tier_index, tier) in design.tiers.iter().enumerate() {
            let expected = tier.indices.len().max(1);
            assert_eq!(
                map.facets_of_tier(tier_index).len(),
                expected,
                "tier {tier_index} ({})",
                tier.name
            );
        }
    }

    #[test]
    fn every_mapped_facets_normal_carries_its_tiers_own_elevation_angle() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let planes = design.planes_from_solved(&solved);
        let map = FacetMap::from_design(&design, &solved);

        for (facet_id, &(normal, _offset)) in
            planes.iter().enumerate().skip(map.preform_plane_count())
        {
            let tier_index = map
                .tier_of(facet_id)
                .expect("every non-preform facet must map to a tier");
            let tier = &design.tiers[tier_index];
            let theta = tier.angle_deg.abs().to_radians();
            // The tier's elevation alone fixes the normal's y-component magnitude to
            // `cos(theta)`, regardless of azimuth (x/z split `sin(theta)` between them).
            assert!(
                (normal.y.abs() - theta.cos()).abs() < 1e-5,
                "facet {facet_id} (tier {tier_index} {}): normal.y={}, expected +-{}",
                tier.name,
                normal.y,
                theta.cos()
            );
            let horizontal = normal.x.hypot(normal.z);
            assert!(
                (horizontal - theta.sin()).abs() < 1e-5,
                "facet {facet_id} (tier {tier_index} {}): horizontal={horizontal}, expected {}",
                tier.name,
                theta.sin()
            );
        }
    }

    #[test]
    fn overlay_flags_mark_the_pending_and_selected_tiers_and_nothing_else_by_default() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let map = FacetMap::from_design(&design, &solved);
        let n_d = design.effective_refractive_index();

        // Neither tier is steep enough to window at diamond's critical angle
        // (~24.4 deg), so `flagged` should be empty -- this only pins the
        // pending/selected wiring.
        let flags = map.overlay_flags(&design, n_d, Some(2), Some(5));

        for &facet_id in map.facets_of_tier(5) {
            assert!(flags.pending[facet_id as usize]);
        }
        for &facet_id in map.facets_of_tier(2) {
            assert!(flags.selected[facet_id as usize]);
        }
        for &facet_id in map.facets_of_tier(3) {
            assert!(!flags.pending[facet_id as usize]);
            assert!(!flags.selected[facet_id as usize]);
        }
        assert!(
            flags.flagged.iter().all(|&f| !f),
            "diamond RBC must not window"
        );
    }

    #[test]
    fn hover_text_omits_the_margin_clause_for_crown_and_girdle_facets() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let map = FacetMap::from_design(&design, &solved);
        let n_d = design.effective_refractive_index();

        // Tier 1 is "Star" (crown), tier 5 is "Pavilion Main" (pavilion) --
        // see `standard_round_brilliant_design`'s tier list.
        let crown_facet = map.facets_of_tier(1)[0] as usize;
        let pavilion_facet = map.facets_of_tier(5)[0] as usize;

        let crown_text = map.hover_text(crown_facet, n_d);
        let pavilion_text = map.hover_text(pavilion_facet, n_d);
        assert!(
            !crown_text.contains("margin"),
            "a crown facet must not claim a windowing margin: {crown_text}"
        );
        assert!(
            pavilion_text.contains("margin"),
            "a pavilion facet must still report its margin: {pavilion_text}"
        );
    }

    #[test]
    fn index_on_gear_matches_the_tiers_own_index_and_is_zero_for_preform() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let map = FacetMap::from_design(&design, &solved);

        assert_eq!(map.index_on_gear(0), 0, "a preform plane carries no index");
        // Tier 2 ("Crown Main") lists index 0.0 first; its first surviving facet
        // must report exactly that tooth.
        let first_main_facet = map.facets_of_tier(2)[0] as usize;
        assert_eq!(map.index_on_gear(first_main_facet), 0);
    }

    #[test]
    fn meeting_facet_pairs_cross_products_a_meet_nameds_two_orbits() {
        // A minimal fabricated design -- not solved for real geometry -- purely to
        // exercise `Design::facet_meets`'s tier-name resolution feeding
        // `meeting_facet_pairs`'s cross product. `solved` is fabricated too (a
        // fixed mast per tier): `facet_meets` never reads it, and `FacetMap::
        // from_design` only reads it for the (here, irrelevant) plane offset.
        fn tier(
            name: &str,
            angle_deg: f64,
            indices: &[f64],
            constraint: MeetConstraint,
        ) -> ConstraintTier {
            ConstraintTier {
                angle_deg,
                name: name.to_string(),
                indices: indices.to_vec(),
                constraint,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            }
        }
        let tiers = vec![
            tier("Table", 0.0, &[], MeetConstraint::ScaleReference(0.3)),
            tier(
                "Star",
                15.0,
                &[6.0, 18.0, 30.0, 42.0],
                MeetConstraint::MeetNamed(vec!["Table".to_string()]),
            ),
        ];
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta {
                gemcad_version: "GemCad 5.0".to_string(),
                gear_teeth: 96,
                gear_reference_angle: 0.0,
                symmetry_order: 8,
                mirror: true,
                refractive_index: 1.54,
                headers: Vec::new(),
                footnotes: Vec::new(),
            },
            tiers,
        );
        let fabricated_solved: Vec<SolvedTier> = design
            .tiers
            .iter()
            .map(|_| SolvedTier {
                mast: 1.0,
                strategy: indicatrix::geometry::meet_solver::SolveStrategy::ScaleReference,
                detail: String::new(),
            })
            .collect();
        let map = FacetMap::from_design(&design, &fabricated_solved);

        let pairs = map.meeting_facet_pairs(&design);
        // Table (orbit size 1) x Star (orbit size 4) = 4 pairs, every one
        // involving Table's single facet id.
        assert_eq!(pairs.len(), 4, "got: {pairs:?}");
        let table_facet = map.facets_of_tier(0)[0];
        for &(a, b) in &pairs {
            assert!(
                a == table_facet || b == table_facet,
                "every pair must involve Table's facet: {a},{b}"
            );
        }
    }

    #[test]
    fn hover_text_reports_a_preform_plane_distinctly_from_a_facet() {
        let design = standard_round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let map = FacetMap::from_design(&design, &solved);
        let n_d = design.effective_refractive_index();

        assert_eq!(map.hover_text(0, n_d), "Preform");
        let facet_text = map.hover_text(map.preform_plane_count(), n_d);
        assert!(facet_text.contains("Table"), "got: {facet_text}");
        assert!(facet_text.contains("index 0"), "got: {facet_text}");
    }
}
