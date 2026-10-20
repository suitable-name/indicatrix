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
use crate::env::common::{evaluate_design, preform_volume};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix::{
    color::metrics::FaceUpTone,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_cut_core::{
    AngleChange, Design, MastChange, ObjectiveComponents, ObjectiveFidelity, ObjectiveWeights,
    ShapeTarget,
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
    pub(crate) tone: FaceUpTone,
}

/// Read-only, geometry-aware score for a materialized design; it does not create or recapture an environment.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreReadout {
    /// Exact weighted objective score.
    pub score: f32,
    /// Shared core optical components.
    pub components: ObjectiveComponents,
    /// Measured yield percent, absent when preform or finished volume is unavailable.
    pub yield_pct: Option<f32>,
    /// Measured lighter-tone loss.
    pub tone_lighter_loss_pct: Option<f32>,
    /// Measured deeper-tone loss.
    pub tone_deeper_loss_pct: Option<f32>,
    /// Measured face-up color under the configured lighting and fidelity.
    pub tone: FaceUpTone,
    /// Distance from configured shape target, times 100; absent without a target.
    pub shape_distance_pct: Option<f32>,
    /// Exact solved mast per authored tier.
    pub masts: Vec<f64>,
}

/// Score a finished design with the same shared carved-geometry, lighting and objective path used by environments.
pub fn score_design_at_fidelity(
    design: &Design,
    gem: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    lighting: LightingPreset,
    shape_target: Option<ShapeTarget>,
) -> Result<ScoreReadout, InferenceError> {
    let evaluated = evaluate_design(
        design,
        gem,
        weights,
        fidelity,
        preform_volume(design),
        lighting,
        shape_target,
    )
    .map_err(|error| InferenceError::Unscorable(format!("{error:?}")))?;
    Ok(ScoreReadout {
        score: evaluated.score,
        components: evaluated.components,
        yield_pct: evaluated.yield_pct,
        tone_lighter_loss_pct: evaluated.tone_lighter_loss_pct,
        tone_deeper_loss_pct: evaluated.tone_deeper_loss_pct,
        tone: evaluated.tone,
        shape_distance_pct: shape_target.and_then(|_| {
            evaluated
                .shape_distance_pct
                .is_finite()
                .then_some(evaluated.shape_distance_pct)
        }),
        masts: evaluated.masts,
    })
}

/// Solves `design` and measures it in `gem` at `fidelity`.
///
/// # Errors
///
/// [`InferenceError::Unscorable`] when the design does not solve or a figure is not finite.
#[cfg(test)]
pub(crate) fn report_design(
    design: &Design,
    gem: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
) -> Result<DesignReport, InferenceError> {
    report_design_with_settings(
        design,
        gem,
        weights,
        fidelity,
        LightingPreset::IsoHemisphere,
        None,
    )
}

pub(crate) fn report_design_with_settings(
    design: &Design,
    gem: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    lighting: LightingPreset,
    shape_target: Option<ShapeTarget>,
) -> Result<DesignReport, InferenceError> {
    let evaluated = evaluate_design(
        design,
        gem,
        weights,
        fidelity,
        preform_volume(design),
        lighting,
        shape_target,
    )
    .map_err(|error| InferenceError::Unscorable(format!("{error:?}")))?;
    Ok(DesignReport {
        components: evaluated.components,
        score: evaluated.score,
        yield_loss_pct: evaluated
            .yield_pct
            .map_or(0.0, |yield_pct| 100.0 - yield_pct),
        tone: evaluated.tone,
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
