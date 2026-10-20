//! Coherent global proposals. Every proposal still passes projection and the full gate.
use super::space::settle_relations;
use crate::Design;
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Common y stretch about the coordinate origin. Normals transform by A^-T and
/// offsets by the same normalization, preserving every plane incidence analytically.
/// The real authored solve decides whether derived depths can realize that preference.
/// Radial scale is fixed by the baseline; no independent shrink-to-fit direction.
pub(super) fn axial_preference(design: &Design, factor: f64) -> Option<Design> {
    if !factor.is_finite() || factor <= 0.0 {
        return None;
    }
    let mut candidate = design.clone();
    for (i, tier) in candidate.tiers.iter_mut().enumerate() {
        let angle = tier.angle_deg.abs().to_radians();
        let norm = angle.sin().hypot(angle.cos() / factor);
        if !design.is_tier_driven(i) {
            tier.angle_deg = (angle.sin().atan2(angle.cos() / factor))
                .to_degrees()
                .copysign(tier.angle_deg);
        }
        if design.tier_target(i).is_none()
            && let MeetConstraint::ScaleReference(mast) = &mut tier.constraint
        {
            *mast /= norm;
        }
    }
    settle_relations(&mut candidate).then_some(candidate)
}
