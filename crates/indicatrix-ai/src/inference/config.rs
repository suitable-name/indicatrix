//! Inference settings: the rollout length, the step limit, the line search, the stall rule and
//! the fidelity of the report.

use crate::env::DEFAULT_LINE_SEARCH;
use indicatrix::optics::raytracer::LightingPreset;
use indicatrix_cut_core::ObjectiveFidelity;
use indicatrix_cut_core::ShapeTarget;
use std::collections::{BTreeMap, BTreeSet};

/// Policy steps per inference run (the environments' default episode length: 60 steps of
/// 0.5 degrees reach 30 degrees).
pub const DEFAULT_STEPS: usize = 60;
/// Largest angle change per step, degrees, for the optimizer (the optimizer environment's default).
pub const DEFAULT_OPTIMIZER_STEP_DEG: f64 = 0.5;
/// Largest angle change per step, degrees, for the retargeter (the retarget environment's default).
pub const DEFAULT_RETARGET_STEP_DEG: f64 = 0.5;
/// Consecutive steps without an accepted candidate after which a run ends (the default of
/// [`InferenceConfig::stall_steps`]).
pub const DEFAULT_STALL_STEPS: usize = 8;

/// How an inference run is carried out.
///
/// The step limit, `max_step_deg` and the line search should match what the network was trained
/// with; the environments reject a zero step count and a non-positive or non-finite step limit.
#[derive(Debug, Clone, PartialEq)]
pub struct InferenceConfig {
    /// Policy steps in the run (at least 1). Each step costs between one and
    /// `line_search.len()` candidate evaluations at [`Self::rollout_fidelity`].
    pub steps: usize,
    /// Largest angle change per step, degrees (finite, positive).
    pub max_step_deg: f64,
    /// Fidelity the environment scores candidates at while the policy runs. The networks are
    /// trained under `Fast`; `Full` is about a thousand times slower per step.
    pub rollout_fidelity: ObjectiveFidelity,
    /// Fidelity of the before/after figures in the returned report. `Full` is what the editor
    /// shows the user (cut-core: `Fast` carries no tilt information and is never for a report).
    pub report_fidelity: ObjectiveFidelity,
    /// The line-search scales of every step, tried in this order: the policy proposes a
    /// direction (the steepest tier at 95 % of `max_step_deg`) and the environment keeps the
    /// first scale of it that improves the stone. Default [`DEFAULT_LINE_SEARCH`] (`1`, `1/2`,
    /// `1/4`), the scales the networks are trained with. A scale that is not finite and
    /// positive is skipped, and a list with no usable scale means `[1.0]` (the environment's
    /// rule). A rejected step spends every scale, so the cost of a run is at most
    /// `steps * line_search.len()` evaluations.
    pub line_search: Vec<f64>,
    /// The run ends after this many consecutive steps without an accepted candidate (the
    /// design did not change); `0` never stops early. Default [`DEFAULT_STALL_STEPS`]. The policy
    /// is deterministic and the design only changes on an accepted candidate, so a long run of
    /// refused steps repeats itself and would only burn evaluations.
    pub stall_steps: usize,
    /// Lighting preset used for rollout and endpoint scores.
    pub lighting: LightingPreset,
    /// Optional additive shape target.
    pub shape_target: Option<ShapeTarget>,
    /// Optional allow-list for independent angle coordinates.
    pub only_tiers: Option<BTreeSet<usize>>,
    /// Inclusive authored-angle bounds by tier.
    pub angle_bounds: BTreeMap<usize, (f64, f64)>,
}

impl InferenceConfig {
    /// The optimizer's defaults: 60 steps of at most 0.5 degrees, the default line search, stop
    /// after 8 stalled steps, `Fast` rollout, `Full` report.
    #[must_use]
    pub fn optimizer() -> Self {
        Self::with_step(DEFAULT_OPTIMIZER_STEP_DEG)
    }

    /// The retargeter's defaults: 60 steps of at most 0.5 degrees, the default line search, stop
    /// after 8 stalled steps, `Fast` rollout, `Full` report.
    #[must_use]
    pub fn retarget() -> Self {
        Self::with_step(DEFAULT_RETARGET_STEP_DEG)
    }

    fn with_step(max_step_deg: f64) -> Self {
        Self {
            steps: DEFAULT_STEPS,
            max_step_deg,
            rollout_fidelity: ObjectiveFidelity::Fast,
            report_fidelity: ObjectiveFidelity::Full,
            line_search: DEFAULT_LINE_SEARCH.to_vec(),
            stall_steps: DEFAULT_STALL_STEPS,
            lighting: LightingPreset::IsoHemisphere,
            shape_target: None,
            only_tiers: None,
            angle_bounds: BTreeMap::new(),
        }
    }

    /// This configuration with another step count.
    #[must_use]
    pub const fn with_steps(mut self, steps: usize) -> Self {
        self.steps = steps;
        self
    }

    /// This configuration with another per-step angle limit, degrees.
    #[must_use]
    pub const fn with_max_step_deg(mut self, max_step_deg: f64) -> Self {
        self.max_step_deg = max_step_deg;
        self
    }

    /// This configuration with another report fidelity.
    #[must_use]
    pub const fn with_report_fidelity(mut self, fidelity: ObjectiveFidelity) -> Self {
        self.report_fidelity = fidelity;
        self
    }

    /// This configuration with another line search (see [`Self::line_search`]); `&[1.0]` is a
    /// single try per step.
    #[must_use]
    pub fn with_line_search(mut self, scales: &[f64]) -> Self {
        self.line_search = scales.to_vec();
        self
    }

    /// This configuration with another stall rule (see [`Self::stall_steps`]); `0` never stops
    /// early.
    #[must_use]
    pub const fn with_stall_steps(mut self, stall_steps: usize) -> Self {
        self.stall_steps = stall_steps;
        self
    }
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self::optimizer()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_environments() {
        let optimizer = InferenceConfig::optimizer();
        assert_eq!(optimizer.steps, DEFAULT_STEPS);
        assert_eq!(DEFAULT_STEPS, 60);
        assert_eq!(optimizer.max_step_deg, 0.5);
        assert_eq!(InferenceConfig::retarget().max_step_deg, 0.5);
        assert_eq!(InferenceConfig::default(), optimizer);
        // The same figures the training environments default to.
        let optimizer_env = crate::env::OptimizerEnvConfig::default();
        assert_eq!(optimizer.steps, optimizer_env.max_steps);
        assert_eq!(optimizer.max_step_deg, optimizer_env.max_step_deg);
        let retarget_env = crate::env::RetargetEnvConfig::new_with_target(
            indicatrix_cut_core::MaterialSelection::none()
                .resolve(&indicatrix_cut_core::BuiltinMaterials),
        );
        assert_eq!(InferenceConfig::retarget().steps, retarget_env.max_steps);
        assert_eq!(
            InferenceConfig::retarget().max_step_deg,
            retarget_env.max_step_deg
        );
    }

    #[test]
    fn the_line_search_and_the_stall_rule_default_to_the_training_values() {
        for config in [InferenceConfig::optimizer(), InferenceConfig::retarget()] {
            assert_eq!(config.line_search, DEFAULT_LINE_SEARCH.to_vec());
            assert_eq!(config.line_search, [1.0, 0.5, 0.25]);
            assert_eq!(config.stall_steps, 8);
            assert_eq!(config.stall_steps, DEFAULT_STALL_STEPS);
            assert_eq!(config.rollout_fidelity, ObjectiveFidelity::Fast);
            assert_eq!(config.report_fidelity, ObjectiveFidelity::Full);
        }
    }

    #[test]
    fn builders_replace_one_field() {
        let config = InferenceConfig::retarget()
            .with_steps(5)
            .with_max_step_deg(0.5)
            .with_report_fidelity(ObjectiveFidelity::Fast);
        assert_eq!(config.steps, 5);
        assert_eq!(config.max_step_deg, 0.5);
        assert_eq!(config.report_fidelity, ObjectiveFidelity::Fast);
        assert_eq!(config.rollout_fidelity, ObjectiveFidelity::Fast);
        // The two new fields keep their defaults.
        assert_eq!(config.line_search, DEFAULT_LINE_SEARCH.to_vec());
        assert_eq!(config.stall_steps, DEFAULT_STALL_STEPS);
    }

    #[test]
    fn the_line_search_and_stall_builders_replace_only_their_field() {
        let base = InferenceConfig::optimizer();
        let single = base.clone().with_line_search(&[1.0]);
        assert_eq!(single.line_search, [1.0]);
        assert_eq!(
            InferenceConfig {
                line_search: base.line_search.clone(),
                ..single
            },
            base
        );

        let never_stop = base.clone().with_stall_steps(0);
        assert_eq!(never_stop.stall_steps, 0);
        assert_eq!(
            InferenceConfig {
                stall_steps: base.stall_steps,
                ..never_stop
            },
            base
        );

        let wide = base.with_line_search(&[2.0, 1.0, 0.5, 0.25]);
        assert_eq!(wide.line_search, [2.0, 1.0, 0.5, 0.25]);
    }
}
