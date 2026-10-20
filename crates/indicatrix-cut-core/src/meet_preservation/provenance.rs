//! Map authored index instances to the exact planes retained by production dedup.
use super::{Plane, PreservationViolation};
use crate::design::{Design, TierId};
use indicatrix::geometry::{cuts::StandardGemCuts, meet_solver::SolvedTier};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SurfaceKey {
    Preform(usize),
    Facet(TierId, usize),
}

/// `planes` are the production planes of `design` for `solved` (`planes_from_solved`),
/// which every caller already holds.
pub(super) fn surfaces(
    design: &Design,
    solved: &[SolvedTier],
    planes: &[Plane],
) -> Result<Vec<(SurfaceKey, Plane)>, PreservationViolation> {
    let mut result: Vec<_> = design
        .preform
        .planes_offset(design.preform_y_offset)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (SurfaceKey::Preform(i), p))
        .collect();
    let schedule = design.to_asc_schedule_from_solved(solved);
    let preform_len = result.len();
    for (row, aliases) in StandardGemCuts::asc_plane_provenance(&schedule)
        .iter()
        .enumerate()
    {
        let id = design.tier_id_at_or_synthetic(row);
        for (instance, &plane) in aliases.iter().enumerate() {
            let plane = planes.get(preform_len + plane).ok_or_else(|| {
                PreservationViolation::InvalidBaseline("unmapped production facet".into())
            })?;
            result.push((SurfaceKey::Facet(id, instance), *plane));
        }
    }
    Ok(result)
}

/// Compare geometry inputs while permitting only angle and existing pinned-mast edits.
pub(super) fn same_authoring(original: &Design, candidate: &Design) -> bool {
    if original.tiers.len() != candidate.tiers.len() {
        return false;
    }
    let mut normalized = candidate.clone();
    normalized.material = original.material.clone();
    normalized.meta.refractive_index = original.meta.refractive_index;
    for (before, after) in original.tiers.iter().zip(&mut normalized.tiers) {
        after.angle_deg = before.angle_deg;
        if matches!(
            before.constraint,
            indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(_)
        ) && matches!(
            after.constraint,
            indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(_)
        ) {
            after.constraint = before.constraint.clone();
        }
    }
    original == &normalized && original.tier_ids_eq(&normalized)
}
