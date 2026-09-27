//! Builds a [`FacetMap`] from a [`Design`] by mirroring `Design::planes_from_solved`'s
//! own candidate-normal construction and dedup pass -- see this module's parent doc
//! comment for the full derivation.

use super::{Candidate, DEDUP_QUANTUM, FacetInfo, FacetMap};
use glam::Vec3;
use indicatrix::geometry::meet_solver::{SolvedTier, classify_blocks};
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
}
