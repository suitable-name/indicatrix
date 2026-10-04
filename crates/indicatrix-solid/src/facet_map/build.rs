//! Builds a [`FacetMap`] from a [`Design`] by mirroring `Design::planes_from_solved`'s
//! own candidate-normal construction and dedup pass -- see this module's parent doc
//! comment for the full derivation.

use super::{Candidate, FacetInfo, FacetKind, FacetMap, OFFSET_REL_EPSILON};
use glam::Vec3;
use indicatrix::geometry::{
    cuts::{StandardGemCuts, normals_coincide},
    meet_solver::{Block, SolvedTier, classify_blocks},
    plane::tier_is_crown_side,
};
use indicatrix_cut_core::Design;

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
        let gear_reference_angle = design.meta.gear_reference_angle as f32;

        let mut candidates: Vec<Candidate> = Vec::new();
        for (tier_index, tier) in design.tiers.iter().enumerate() {
            let is_crown = tier_is_crown_side(tier.angle_deg);

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
                let phi =
                    StandardGemCuts::index_to_azimuth(idx as f32, gear_teeth, gear_reference_angle);
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

        // First-occurrence-wins dedup under the plane builder's own rule
        // (module doc, step 3): coinciding normals and an offset within a
        // relative tolerance floored at one mast unit.
        let mut kept: Vec<Candidate> = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let is_duplicate = kept.iter().any(|k| {
                let offset_scale = k.offset.abs().max(candidate.offset.abs()).max(1.0);
                normals_coincide(k.normal.as_dvec3(), candidate.normal.as_dvec3())
                    && (k.offset - candidate.offset).abs() <= OFFSET_REL_EPSILON * offset_scale
            });
            if !is_duplicate {
                kept.push(candidate);
            }
        }

        let mut facets = vec![FacetInfo::preform(); preform_plane_count];
        let mut orbits: Vec<Vec<u32>> = vec![Vec::new(); design.tiers.len()];
        let canonical_labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
        for candidate in kept {
            let facet_id = facets.len() as u32;
            let block = blocks.get(candidate.tier_index).copied();
            let angle_deg = design
                .tiers
                .get(candidate.tier_index)
                .map_or(0.0, |t| t.angle_deg);
            let (name, display_name) = canonical_labels
                .get(candidate.tier_index)
                .map_or((String::new(), String::new()), |l| {
                    (l.code.clone(), l.display_name.clone())
                });
            facets.push(FacetInfo {
                tier_index: Some(candidate.tier_index),
                index_on_gear: candidate.index_on_gear,
                angle_deg,
                block,
                name,
                display_name,
                kind: FacetKind::Flat,
            });
            if let Some(orbit) = orbits.get_mut(candidate.tier_index) {
                orbit.push(facet_id);
            }
        }

        Self {
            preform_plane_count,
            flat_facet_count: facets.len(),
            facets,
            orbits,
            concave_hover: Vec::new(),
        }
    }

    /// [`Self::from_design`] plus one facet per concave tool, with ids above every
    /// flat one: tool `k` of `placements` is facet `flat_count + k`.
    ///
    /// `placements` is the `(concave tier, placement)` list
    /// `Design::concave_tools_from_solved` returns next to its primitives, so facet
    /// ids line up with `StoneGeometry::facet_count`'s numbering. A placement naming
    /// a tier or index the design does not have still gets an (unnamed) facet, so the
    /// id range never drifts from the tool list. With no placements this is exactly
    /// `from_design`.
    #[must_use]
    pub fn from_design_with_tools(
        design: &Design,
        solved: &[SolvedTier],
        placements: &[(usize, usize)],
    ) -> Self {
        let mut map = Self::from_design(design, solved);
        for &(tier, placement) in placements {
            let concave = design.concave_tiers.get(tier);
            let name = concave.map_or("", |t| t.name.as_str());
            let angle_deg = concave.map_or(0.0, |t| t.angle_deg);
            let index = concave
                .and_then(|t| t.indices.get(placement))
                .map_or(0, |i| i.round() as u32);
            let label = match concave {
                Some(t) if !name.is_empty() && t.indices.len() > 1 => format!("{name} {index}"),
                _ => name.to_string(),
            };
            let hover = concave.map_or_else(String::new, |t| {
                let shown = if name.is_empty() { "(unnamed)" } else { name };
                format!(
                    "{shown} {} θ {:.1}° D {:.3}",
                    t.tool.code(),
                    t.tool_azimuth_deg,
                    t.diameter_ratio
                )
            });
            let block = concave.map(|t| {
                if t.is_crown_side() {
                    Block::Crown
                } else {
                    Block::Pavilion
                }
            });
            map.facets.push(FacetInfo {
                tier_index: None,
                index_on_gear: index,
                angle_deg,
                block,
                name: label.clone(),
                display_name: label,
                kind: FacetKind::Concave { tier, placement },
            });
            map.concave_hover.push(hover);
        }
        map
    }
}
