//! The hover-text/label formatting and the critical-angle/pending/selected overlay
//! flags computed from a built [`FacetMap`].

use super::{FacetKind, FacetMap, OverlayFlags};
use indicatrix::geometry::meet_solver::Block;
use indicatrix_cut_core::{
    Design,
    optics_hints::{self, Risk},
};
use std::collections::BTreeSet;

impl FacetMap {
    /// The facet's own short on-diagram label: `"<tier name> <index>"` (the one
    /// number a cutter reads off a `GemCad` diagram), or just the tier name
    /// for a tier with no listed indices (a table/culet, where the index is always
    /// `0` and carries no information). `""` for a preform plane, an unnamed tier,
    /// or an out-of-range id -- unlike the fuller [`Self::hover_text`] tooltip.
    /// Returns an owned `String` since the index has to be formatted in.
    #[must_use]
    pub fn facet_label(&self, facet_id: usize) -> String {
        let Some(info) = self.facets.get(facet_id) else {
            return String::new();
        };
        if matches!(info.kind, FacetKind::Concave { .. }) {
            return info.name.clone();
        }
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
    /// The margin clause is only printed for a pavilion facet:
    /// `optics_hints::tier_margin_and_risk` (the table's own path, `state/mod.rs`)
    /// only has a real windowing answer for `Block::Pavilion` -- a crown or girdle
    /// facet cannot window, so a margin figure there would mean nothing and would
    /// undermine the reading of the pavilion facets' real margins.
    #[must_use]
    pub fn hover_text(&self, facet_id: usize, n_d: f64) -> String {
        let Some(info) = self.facets.get(facet_id) else {
            return String::new();
        };
        if matches!(info.kind, FacetKind::Concave { .. }) {
            return facet_id
                .checked_sub(self.flat_facet_count)
                .and_then(|k| self.concave_hover.get(k))
                .cloned()
                .unwrap_or_default();
        }
        let Some(_tier_index) = info.tier_index else {
            return "Preform".to_string();
        };
        let block_text = match info.block {
            Some(Block::Crown) => "Crown",
            Some(Block::Pavilion) => "Pavilion",
            Some(Block::Girdle) => "Girdle",
            None => "?",
        };
        let name = if !info.display_name.is_empty() {
            info.display_name.as_str()
        } else if !info.name.is_empty() {
            info.name.as_str()
        } else {
            "(unnamed)"
        };
        let angle = info.angle_deg.abs();
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
    /// [`super::FacetInfo::angle_deg`]) so a caller mid-edit still sees the CURRENT authored
    /// angle for the windowing check, even before the mesh itself catches up.
    /// Windowing risk applies only to pavilion tiers (`angle_deg < 0.0`), per
    /// `optics_hints`'s own module doc comment.
    ///
    /// `pending_tiers` marks EVERY tier index it contains, not just one: a batch
    /// nudge/offset dirties several tiers at once, and a cutter watching a
    /// multi-tier edit needs every one of the changed facets outlined as pending,
    /// not just the first.
    #[must_use]
    pub fn overlay_flags(
        &self,
        design: &Design,
        n_d: f64,
        selected_tier: Option<usize>,
        pending_tiers: &BTreeSet<usize>,
    ) -> OverlayFlags {
        let plane_count = self.facets.len();
        let mut flagged = vec![false; plane_count];
        let mut pending = vec![false; plane_count];
        let mut selected = vec![false; plane_count];

        for (facet_id, info) in self.facets.iter().enumerate() {
            let Some(tier_index) = info.tier_index else {
                continue;
            };
            if pending_tiers.contains(&tier_index) {
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

    /// Every facet-id pair that should carry a meet-point marker: for every tier
    /// pair `Design::facet_meets` names as meeting each other, the full cross
    /// product of that pair's surviving facet ids.
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
