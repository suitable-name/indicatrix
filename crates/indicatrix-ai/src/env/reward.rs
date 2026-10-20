//! Multi-objective reward calculation and penalty gates for gemstone RL agents.
//!
//! # Shape of the reward
//!
//! The environments are greedy MDPs (the state never gets worse, see `traits`), so the
//! reward of a step is:
//!
//! - an invalid-geometry or safety penalty when the proposal is unusable;
//! - otherwise `step_cost` plus the score term of `Δ = score_before - score_after`, plus the
//!   gate penalties when the girdle or manufacturability gate failed, minus the shape drift
//!   penalty. The score term is `score_improvement_scale * Δ` for `Δ >= 0` (linear: a bigger
//!   gain is a bigger reward) and `-worse_clip * tanh(|Δ| * score_improvement_scale /
//!   worse_clip)` for `Δ < 0`. A proposal that scores worse than the current stone yields a
//!   negative signal while the state stays put, but the loss saturates at `worse_clip`: at a
//!   one-degree joint move of every tier the measured loss is 2 to 11 points in the median
//!   and up to 20, and unbounded it owned every batch (run 1 averaged -4.6 per step). The
//!   map is monotone, so the ORDER of candidates is kept (a 1-point loss is -0.92, a 15-point
//!   loss -2.0 with the default `worse_clip`). A candidate that failed a gate is charged no
//!   `Δ` at all (the environment hands the evaluator `score_before` for it): it pays the
//!   gate penalty and the step cost and nothing else;
//! - plus `new_best_bonus` ONCE, on the last step of an episode that ended below the base
//!   design's score (not on every accepted step, which would reward the count of tiny
//!   improvements instead of their size). The default is `0.0`: the per-step gains already
//!   sum to the episode's improvement, and a flat bonus for "any improvement" only adds
//!   variance.
//!
//! The penalties sit on the scale of a bad `Δ` (a gate failure is worth about one to two
//! points lost): the trainer normalises the returns, so only the ratios matter. The result
//! is clamped to `+-reward_clip`; a non-finite score is treated as invalid geometry.

use serde::{Deserialize, Serialize};

/// The serde default of [`RewardConfig::worse_clip`], so reward configurations saved before
/// the field existed still load.
const fn default_worse_clip() -> f32 {
    2.0
}

/// Hyperparameters configuring reward scalarization and constraint penalties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewardConfig {
    /// Multiplier on optical score improvement `(score_before - score_after)`. `1.0` makes a
    /// one-point score gain worth one reward unit.
    pub score_improvement_scale: f32,
    /// Fixed penalty applied when an action yields non-closing, unsolvable or unscorable
    /// geometry, or is malformed. Default `-2.0`: the scale of a bad score loss.
    pub invalid_geometry_penalty: f32,
    /// Penalty applied when proposed angles violate safety bounds (cross 0 deg or 90 deg).
    /// Default `-2.0`.
    pub safety_bound_penalty: f32,
    /// Penalty applied when the girdle band thins below the configured fraction of the base
    /// design's band or runs to a knife edge. Default `-1.0`.
    pub knife_edge_penalty: f32,
    /// Weight on drift from the base design's silhouette. The drift handed to the evaluator
    /// is in percent (`100 *` the relative drift of table size and crown-to-pavilion ratio).
    pub shape_drift_weight: f32,
    /// Penalty applied when manufacturability checks regress (more vanishing or undersized
    /// facets than the base design). Default `-1.0`; the gate failed on 50 to 80 % of the
    /// random one-degree proposals on a 24-free-tier design and on none at 0.25 degrees.
    pub manufacturability_regression_penalty: f32,
    /// Small cost per step to encourage finding efficient, short improvement trajectories.
    /// Default `-0.02`.
    pub step_cost: f32,
    /// Bonus awarded once, on the last step of an episode that ended with a score below the
    /// base design's. Default `0.0` (off).
    pub new_best_bonus: f32,
    /// Absolute bound on a step reward; keeps one outlier from dominating a PPO batch.
    pub reward_clip: f32,
    /// Soft bound on the loss charged for a closed, gate-passing candidate that scores worse
    /// than the current stone: the loss `L = -Δ * score_improvement_scale` contributes
    /// `-worse_clip * tanh(L / worse_clip)`, so it is close to linear for small losses and
    /// never exceeds `worse_clip` (a 1-point loss -0.924, a 15-point loss -2.0 at the default
    /// `2.0`). A value that is not finite and positive switches the soft bound off (the loss
    /// stays linear). Gains are never bounded here.
    #[serde(default = "default_worse_clip")]
    pub worse_clip: f32,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self {
            score_improvement_scale: 1.0,
            invalid_geometry_penalty: -2.0,
            safety_bound_penalty: -2.0,
            knife_edge_penalty: -1.0,
            shape_drift_weight: 0.05,
            manufacturability_regression_penalty: -1.0,
            step_cost: -0.02,
            new_best_bonus: 0.0,
            reward_clip: 10.0,
            worse_clip: default_worse_clip(),
        }
    }
}

impl RewardConfig {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        let values = [
            self.score_improvement_scale,
            self.invalid_geometry_penalty,
            self.safety_bound_penalty,
            self.knife_edge_penalty,
            self.shape_drift_weight,
            self.manufacturability_regression_penalty,
            self.step_cost,
            self.new_best_bonus,
            self.reward_clip,
            self.worse_clip,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            return Err("reward settings must be finite");
        }
        if self.reward_clip <= 0.0 {
            return Err("reward_clip must be positive");
        }
        Ok(())
    }
}

/// Validity, safety and gate outcomes of one step, as the reward evaluator consumes them.
#[allow(
    clippy::struct_excessive_bools,
    reason = "five independent gate outcomes; a flags struct replaces a positional bool list"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepFlags {
    /// The proposal produced a closing, solvable solid.
    pub is_valid_solid: bool,
    /// The proposed angles stayed inside the safety bounds.
    pub is_safe_bounds: bool,
    /// The girdle gate passed.
    pub girdle_ok: bool,
    /// The manufacturability gate passed.
    pub manufacturability_ok: bool,
    /// `true` only on the final step of an episode that ended below the base design's score.
    pub is_new_best: bool,
}

impl StepFlags {
    /// All gates passed, not a new best.
    pub const ALL_OK: Self = Self {
        is_valid_solid: true,
        is_safe_bounds: true,
        girdle_ok: true,
        manufacturability_ok: true,
        is_new_best: false,
    };
}

/// Evaluator computing step rewards from geometric and optical transitions.
#[derive(Debug, Clone)]
pub struct RewardEvaluator {
    config: RewardConfig,
}

impl RewardEvaluator {
    /// Construct evaluator with specific configuration.
    #[must_use]
    pub const fn new(config: RewardConfig) -> Self {
        Self { config }
    }

    /// Calculate step reward given previous state score, new state score, and constraint flags.
    ///
    /// `shape_penalty` is the silhouette drift in percent (`0.0` on the base shape).
    /// `flags.is_new_best` is `true` only on the final step of an episode that ended below the
    /// base design's score.
    #[must_use]
    pub fn compute_step_reward(
        &self,
        score_before: f32,
        score_after: f32,
        flags: StepFlags,
        shape_penalty: f32,
    ) -> f32 {
        let StepFlags {
            is_valid_solid,
            is_safe_bounds,
            girdle_ok,
            manufacturability_ok,
            is_new_best,
        } = flags;
        if !is_safe_bounds {
            return self.clip(self.config.safety_bound_penalty);
        }
        if !is_valid_solid
            || !score_before.is_finite()
            || !score_after.is_finite()
            || !shape_penalty.is_finite()
        {
            return self.clip(self.config.invalid_geometry_penalty);
        }

        // Optical score delta: positive when the score decreased (improved); a loss is
        // soft-bounded, see `score_term`.
        let mut r = self.config.step_cost + self.score_term(score_before - score_after);

        // Constraint penalties
        if !girdle_ok {
            r += self.config.knife_edge_penalty;
        }
        if !manufacturability_ok {
            r += self.config.manufacturability_regression_penalty;
        }

        // Shape preservation penalty
        r = shape_penalty.mul_add(-self.config.shape_drift_weight, r);

        // Terminal bonus for an episode that improved on the base design
        if is_new_best {
            r += self.config.new_best_bonus;
        }

        self.clip(r)
    }

    /// The reward contribution of `score_delta = score_before - score_after`: linear for a gain,
    /// `-worse_clip * tanh(loss / worse_clip)` for a loss (`loss = |score_delta| *
    /// score_improvement_scale`), so no single bad proposal can dominate a batch while the
    /// ordering of candidates is preserved.
    fn score_term(&self, score_delta: f32) -> f32 {
        let scale = self.config.score_improvement_scale;
        if score_delta >= 0.0 {
            return scale * score_delta;
        }
        let loss = -score_delta * scale;
        let bound = self.config.worse_clip;
        if bound.is_finite() && bound > 0.0 {
            -bound * (loss / bound).tanh()
        } else {
            -loss
        }
    }

    fn clip(&self, reward: f32) -> f32 {
        let bound = self.config.reward_clip.abs();
        reward.clamp(-bound, bound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_on_the_scale_of_a_bad_score_change() {
        let config = RewardConfig::default();
        assert_eq!(config.score_improvement_scale, 1.0);
        assert_eq!(config.invalid_geometry_penalty, -2.0);
        assert_eq!(config.safety_bound_penalty, -2.0);
        assert_eq!(config.knife_edge_penalty, -1.0);
        assert_eq!(config.shape_drift_weight, 0.05);
        assert_eq!(config.manufacturability_regression_penalty, -1.0);
        assert_eq!(config.step_cost, -0.02);
        assert_eq!(config.new_best_bonus, 0.0);
        assert_eq!(config.reward_clip, 10.0);
        assert_eq!(config.worse_clip, 2.0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_reward_scales_are_rejected() {
        assert!(
            RewardConfig {
                reward_clip: 0.0,
                ..RewardConfig::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RewardConfig {
                score_improvement_scale: f32::NAN,
                ..RewardConfig::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RewardConfig {
                worse_clip: f32::INFINITY,
                ..RewardConfig::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn non_finite_score_is_invalid_geometry() {
        let evaluator = RewardEvaluator::new(RewardConfig::default());
        let r = evaluator.compute_step_reward(50.0, f32::NAN, StepFlags::ALL_OK, 0.0);
        assert_eq!(r, RewardConfig::default().invalid_geometry_penalty);
    }

    #[test]
    fn reward_is_clipped() {
        // A 100-point gain is 99.98 after the step cost: the default bound of 10 caps it. The
        // terminal bonus defaults to 0 and no longer adds to it.
        let evaluator = RewardEvaluator::new(RewardConfig::default());
        let best = StepFlags {
            is_new_best: true,
            ..StepFlags::ALL_OK
        };
        let r = evaluator.compute_step_reward(100.0, 0.0, best, 0.0);
        assert_eq!(r, RewardConfig::default().reward_clip);
        assert_eq!(r, 10.0);

        // The negative side clips too: the -2 geometry penalty under a bound of 1.
        let tight = RewardEvaluator::new(RewardConfig {
            reward_clip: 1.0,
            ..RewardConfig::default()
        });
        let invalid = StepFlags {
            is_valid_solid: false,
            ..StepFlags::ALL_OK
        };
        assert_eq!(tight.compute_step_reward(50.0, 45.0, invalid, 0.0), -1.0);
    }

    #[test]
    fn gate_failures_and_drift_lower_the_reward() {
        // Hand values at the defaults (step cost -0.02, knife edge and manufacturability -1,
        // drift weight 0.05 per percent) for a one-point gain: 1 - 0.02 = 0.98; a gate costs
        // exactly its penalty on top; 10 percent of drift costs 0.5.
        let evaluator = RewardEvaluator::new(RewardConfig::default());
        let ok = StepFlags::ALL_OK;
        let clean = evaluator.compute_step_reward(50.0, 49.0, ok, 0.0);
        let girdle_flags = StepFlags {
            girdle_ok: false,
            ..ok
        };
        let girdle = evaluator.compute_step_reward(50.0, 49.0, girdle_flags, 0.0);
        let manufacturing_flags = StepFlags {
            manufacturability_ok: false,
            ..ok
        };
        let manufacturing = evaluator.compute_step_reward(50.0, 49.0, manufacturing_flags, 0.0);
        let drifted = evaluator.compute_step_reward(50.0, 49.0, ok, 10.0);
        assert!(girdle < clean && manufacturing < clean && drifted < clean);
        assert!((clean - 0.98).abs() < 1e-6, "{clean}");
        assert!((girdle - (-0.02)).abs() < 1e-6, "{girdle}");
        assert!((manufacturing - (-0.02)).abs() < 1e-6, "{manufacturing}");
        assert!((drifted - 0.48).abs() < 1e-6, "{drifted}");
    }

    #[test]
    fn a_gate_failed_candidate_pays_only_the_gate_and_the_step_cost() {
        // The environment hands a gate-failed candidate `score_after = score_before`, so the
        // reward is the penalty plus the step cost: -1.0 - 0.02.
        let evaluator = RewardEvaluator::new(RewardConfig::default());
        let flags = StepFlags {
            manufacturability_ok: false,
            ..StepFlags::ALL_OK
        };
        let r = evaluator.compute_step_reward(42.0, 42.0, flags, 0.0);
        assert!((r - (-1.02)).abs() < 1e-6, "{r}");
    }

    #[test]
    fn a_large_loss_saturates_at_worse_clip_and_a_small_one_stays_linear() {
        let evaluator = RewardEvaluator::new(RewardConfig::default());
        let ok = StepFlags::ALL_OK;
        // 1-point loss: -2 * tanh(0.5) = -2 * 0.4621171573 = -0.9242343, plus the step cost.
        let one = evaluator.compute_step_reward(50.0, 51.0, ok, 0.0);
        assert!((one - (-0.944_234)).abs() < 2e-6, "{one}");
        // 15-point loss: -2 * tanh(7.5) = -2 * (1 - 6.1e-7) = -1.9999988, plus the step cost.
        // f32 resolves tanh(7.5) to a few 1e-8, so the figure is -2.02 within 5e-6.
        let fifteen = evaluator.compute_step_reward(50.0, 65.0, ok, 0.0);
        assert!((fifteen - (-2.02)).abs() < 5e-6, "{fifteen}");
        // A 0.1-point loss is -2 * tanh(0.05) = -0.0999167 (the linear value is -0.1), plus the
        // step cost: -0.1199167.
        let small = evaluator.compute_step_reward(50.0, 50.1, ok, 0.0);
        assert!((small - (-0.119_917)).abs() < 1e-5, "{small}");
        // Monotone: a bigger loss never pays more, and the gain side is untouched (linear).
        let three = evaluator.compute_step_reward(50.0, 53.0, ok, 0.0);
        assert!(fifteen < three && three < one && one < small);
        let gain = evaluator.compute_step_reward(50.0, 45.0, ok, 0.0);
        assert!((gain - 4.98).abs() < 1e-5, "{gain}");
    }

    #[test]
    fn a_non_positive_worse_clip_leaves_the_loss_linear() {
        let evaluator = RewardEvaluator::new(RewardConfig {
            worse_clip: 0.0,
            ..RewardConfig::default()
        });
        let r = evaluator.compute_step_reward(50.0, 55.0, StepFlags::ALL_OK, 0.0);
        assert!((r - (-5.02)).abs() < 1e-6, "{r}");
    }

    #[test]
    fn an_old_reward_config_without_worse_clip_still_loads() {
        let json = r#"{"score_improvement_scale":1.0,"invalid_geometry_penalty":-10.0,
            "safety_bound_penalty":-8.0,"knife_edge_penalty":-5.0,"shape_drift_weight":0.05,
            "manufacturability_regression_penalty":-4.0,"step_cost":-0.05,
            "new_best_bonus":5.0,"reward_clip":20.0}"#;
        let config: RewardConfig = serde_json::from_str(json).expect("old file");
        assert_eq!(config.worse_clip, 2.0);
        assert_eq!(config.step_cost, -0.05);
    }
}
