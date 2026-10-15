//! The figures an inference result reports, and the diff between the design the caller gave and
//! the design the policy ended on.
//!
//! The rollout scores candidates at the environment's fidelity (`Fast` in practice). A result
//! handed to the user is measured separately, once for the starting design and once for the
//! final one, at [`InferenceConfig::report_fidelity`](super::InferenceConfig::report_fidelity).

#![allow(
    clippy::redundant_pub_crate,
    reason = "private module: `pub(crate)` documents the intended reach of each item"
)]

use super::error::InferenceError;
use crate::env::common::to_gpu_planes;
use indicatrix::{
    geometry::{meet_solver::MeetConstraint, stone_metrics::measure_solid},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    AngleChange, Design, MastChange, ObjectiveComponents, ObjectiveFidelity, ObjectiveWeights,
    evaluate_objective, volumetric_yield,
};

/// What one design measures like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DesignReport {
    /// Windowing, extinction and tilt brilliance.
    pub(crate) components: ObjectiveComponents,
    /// [`ObjectiveWeights::score_with_yield`] of the components and the yield loss (lower is
    /// better).
    pub(crate) score: f32,
    /// `100 -` the volumetric yield percentage; `0.0` when the yield cannot be measured (the
    /// cut-core optimizer's convention).
    pub(crate) yield_loss_pct: f32,
}

/// Solves `design` and measures it in `gem` at `fidelity`.
///
/// # Errors
///
/// [`InferenceError::Unscorable`] when the design does not solve or a figure is not finite.
pub(crate) fn report_design(
    design: &Design,
    gem: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
) -> Result<DesignReport, InferenceError> {
    let solved = design
        .solve()
        .map_err(|error| InferenceError::Unscorable(error.to_string()))?;
    let planes = design.planes_from_solved(&solved);
    let components = evaluate_objective(&to_gpu_planes(&planes), gem, fidelity);

    let finished = measure_solid(&planes);
    let preform = measure_solid(&design.preform.planes());
    let yield_fraction = match (&finished, &preform) {
        (Some(finished), Some(preform)) => volumetric_yield(finished.volume, preform.volume),
        _ => None,
    };
    let yield_loss_pct =
        yield_fraction.map_or(0.0, |fraction| 100.0_f64.mul_add(-fraction, 100.0) as f32);

    let score = weights.score_with_yield(&components, yield_loss_pct);
    let finite = [
        score,
        yield_loss_pct,
        components.windowing_pct,
        components.extinction_pct,
        components.tilt_brilliance_pct,
    ]
    .iter()
    .all(|value| value.is_finite());
    if !finite {
        return Err(InferenceError::Unscorable(
            "a measured figure is not a finite number".to_owned(),
        ));
    }
    Ok(DesignReport {
        components,
        score,
        yield_loss_pct,
    })
}

/// The angle changes between `base` and `end` (same tiers, same order), one per modified tier
/// in tier order.
///
/// Tiers that follow a relation are left out: they take the angle their relation gives once
/// the others have moved, which the apply paths (cut-core's `apply_optimize_result`, the
/// editor's retarget) work out themselves.
pub(crate) fn angle_changes(base: &Design, end: &Design) -> Vec<AngleChange> {
    base.tiers
        .iter()
        .zip(&end.tiers)
        .enumerate()
        .filter(|&(index, (was, now))| {
            was.angle_deg.to_bits() != now.angle_deg.to_bits() && !base.is_tier_driven(index)
        })
        .map(|(index, (was, now))| AngleChange {
            index,
            from_deg: was.angle_deg,
            to_deg: now.angle_deg,
        })
        .collect()
}

/// The mast changes between `base` and `end`: every tier that is a `ScaleReference` tier in
/// both and whose mast differs (the hinge-anchored tiers the policy turned).
pub(crate) fn mast_changes(base: &Design, end: &Design) -> Vec<MastChange> {
    base.tiers
        .iter()
        .zip(&end.tiers)
        .enumerate()
        .filter_map(
            |(index, (was, now))| match (&was.constraint, &now.constraint) {
                (MeetConstraint::ScaleReference(from), MeetConstraint::ScaleReference(to))
                    if from.to_bits() != to.to_bits() =>
                {
                    Some(MastChange {
                        index,
                        from_mast: *from,
                        to_mast: *to,
                    })
                }
                _ => None,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_designs_have_no_changes() {
        let design = Design::concave_fixture();
        assert_eq!(angle_changes(&design, &design).len(), 0);
        assert_eq!(mast_changes(&design, &design).len(), 0);
    }

    #[test]
    fn a_moved_tier_is_listed_with_both_angles() {
        let base = Design::concave_fixture();
        let mut end = base.clone();
        end.tiers[2].angle_deg += 0.5;
        let MeetConstraint::ScaleReference(base_mast) = base.tiers[2].constraint else {
            panic!("an imported design pins every tier to a mast");
        };
        end.tiers[2].constraint = MeetConstraint::ScaleReference(base_mast + 0.125);
        let changes = angle_changes(&base, &end);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].index, 2);
        assert_eq!(changes[0].from_deg, base.tiers[2].angle_deg);
        assert_eq!(changes[0].to_deg, end.tiers[2].angle_deg);
        let masts = mast_changes(&base, &end);
        assert_eq!(masts.len(), 1);
        assert_eq!(masts[0].index, 2);
        assert_eq!(masts[0].from_mast, base_mast);
        assert_eq!(masts[0].to_mast, base_mast + 0.125);
    }
}
