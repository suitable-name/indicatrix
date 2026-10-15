//! Inference engine for the reinforcement-learned cut optimizer.
//!
//! [`RlOptimizerModel::optimize`] is a counterpart of `indicatrix_cut_core::optimize_design_with`:
//! it returns the same [`OptimizeResult`], so the editor applies it with
//! `apply_optimize_result`. The search is the trained set-attention policy acting greedily in
//! an [`OptimizerEnv`] instead of a coordinate sweep: each step proposes a direction (the
//! network predicts the per-tier sensitivities of the score components and composes the
//! direction from the objective's weight fractions) and the environment's line search picks how
//! far to go along it.
//!
//! The reported before/after figures are measured separately from the rollout, at
//! [`InferenceConfig::report_fidelity`] (`Full` by default). The policy is trained and run at
//! `Fast` fidelity, so the final design is only adopted when it also measures strictly better
//! in the report; otherwise the result proposes no change.
//!
//! The environment scores with the yield term when the weights weigh it and the preform makes
//! the yield measurable (`ObjectiveWeights::score_with_yield`, as the report does), otherwise
//! with the optical `ObjectiveWeights::score`. A tone weight is not supported and is refused
//! with [`InferenceError::UnsupportedObjective`].
//!
//! The optimizer is the retarget task with the target equal to the source material, so one
//! trained model serves both: [`RlOptimizerModel::from_checkpoint`] and
//! [`RlOptimizerModel::load`] accept a retarget checkpoint as well as an optimizer one.

use super::{
    config::InferenceConfig,
    error::InferenceError,
    policy::{
        check_network, ensure_supported_objective, network_from_file, network_of, run_policy,
    },
    report::{DesignReport, angle_changes, mast_changes, report_design},
};
use crate::{
    env::{OptimizerEnv, OptimizerEnvConfig},
    model::{Checkpoint, GemstoneSetAttentionNetwork, TrainingMode},
};
use indicatrix_cut_core::{
    AngleChange, CANONICAL_LIGHTING_PRESET, Design, MastChange, ObjectiveWeights,
    OptimizeCandidate, OptimizeOutcome, OptimizeResult,
};
use std::path::Path;

/// High-level RL inference engine for optimizing a design under its authored material.
#[derive(Debug, Clone)]
pub struct RlOptimizerModel {
    network: GemstoneSetAttentionNetwork,
    config: InferenceConfig,
}

impl RlOptimizerModel {
    /// Wrap a trained optimizer (or retarget) network.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Weights`] when the network is structurally inconsistent or does not
    /// read the current observation layout.
    pub fn new(
        network: GemstoneSetAttentionNetwork,
        config: InferenceConfig,
    ) -> Result<Self, InferenceError> {
        check_network(&network)?;
        Ok(Self { network, config })
    }

    /// Wrap the network of a training checkpoint, an optimizer one or a retarget one.
    ///
    /// A retarget model serves the optimizer because the optimizer is the retarget task with
    /// the target equal to the source material, and retarget training draws exactly that case
    /// in a fraction of its episodes. The reverse is refused by
    /// [`RlRetargetModel`](super::RlRetargetModel).
    ///
    /// # Errors
    ///
    /// [`InferenceError::Weights`] when the network does not fit the current observation
    /// layout.
    pub fn from_checkpoint(
        checkpoint: Checkpoint,
        config: InferenceConfig,
    ) -> Result<Self, InferenceError> {
        Ok(Self {
            network: network_of(checkpoint, TrainingMode::Optimizer)?,
            config,
        })
    }

    /// Load a training checkpoint file, an optimizer one or a retarget one (see
    /// [`Self::from_checkpoint`] for why both serve).
    ///
    /// # Errors
    ///
    /// [`InferenceError::Weights`] for an unreadable or damaged file, another schema or
    /// observation layout version, or a network that does not fit.
    pub fn load(path: &Path, config: InferenceConfig) -> Result<Self, InferenceError> {
        Ok(Self {
            network: network_from_file(path, TrainingMode::Optimizer)?,
            config,
        })
    }

    /// The settings of every [`Self::optimize`] call.
    #[must_use]
    pub const fn config(&self) -> &InferenceConfig {
        &self.config
    }

    /// Optimize `design` in its own material with greedy policy rollouts.
    ///
    /// The run ends after [`InferenceConfig::steps`] steps, when the environment ends the
    /// episode, or after [`InferenceConfig::stall_steps`] consecutive steps without an accepted
    /// candidate.
    ///
    /// The result follows the cut-core contract: `outcome.changes` and `outcome.after`
    /// describe the single entry of `candidates`; `mast_changes` holds the masts of the
    /// hinge-anchored tiers the policy turned; with nothing to improve, `candidates` and the
    /// changes are empty and `after` equals `before`. Angles of tiers that follow a relation
    /// are not listed (the apply path works them out). `evaluations` counts the candidate
    /// evaluations the rollout spent (between one and the number of line-search scales per
    /// step; the before and after report evaluations are not counted). The tone fields are
    /// `None`: this engine does not measure the face-up tone.
    ///
    /// # Errors
    ///
    /// [`InferenceError::UnsupportedObjective`] when `weights` weigh the face-up tone (checked
    /// first, before any environment work), [`InferenceError::Env`] when the configuration is
    /// out of range or the design does not solve into a closed stone with a movable tier,
    /// [`InferenceError::Model`] when the network rejects an observation,
    /// [`InferenceError::Unscorable`] when the report cannot be measured.
    pub fn optimize(
        &self,
        design: &Design,
        weights: ObjectiveWeights,
    ) -> Result<OptimizeResult, InferenceError> {
        ensure_supported_objective(&weights)?;
        let config = &self.config;
        let mut env = OptimizerEnv::new(
            design.clone(),
            OptimizerEnvConfig {
                max_steps: config.steps,
                max_step_deg: config.max_step_deg,
                weights,
                fidelity: config.rollout_fidelity,
                ..OptimizerEnvConfig::default()
            },
        )?;
        let observation = env.build_observation();
        let run = run_policy(
            &mut env,
            observation,
            &self.network,
            config.steps,
            config.max_step_deg,
            &config.line_search,
            config.stall_steps,
        )?;

        let gem = &env.source_material().gem;
        let end = env.current_design();
        let before = report_design(design, gem, &weights, config.report_fidelity)?;
        let changes = angle_changes(design, end);
        let adopted = if changes.is_empty() {
            None
        } else {
            let after = report_design(end, gem, &weights, config.report_fidelity)?;
            (after.score < before.score).then(|| Adopted {
                after,
                changes,
                masts: mast_changes(design, end),
            })
        };
        Ok(build_result(before, adopted, run.evaluations))
    }
}

/// The final design's figures and changes, when it measures strictly better than the start.
struct Adopted {
    after: DesignReport,
    changes: Vec<AngleChange>,
    masts: Vec<MastChange>,
}

fn build_result(
    before: DesignReport,
    adopted: Option<Adopted>,
    evaluations: usize,
) -> OptimizeResult {
    let (after, changes, masts) = match adopted {
        Some(adopted) => (adopted.after, adopted.changes, adopted.masts),
        None => (before, Vec::new(), Vec::new()),
    };
    let candidates = if changes.is_empty() {
        Vec::new()
    } else {
        vec![OptimizeCandidate {
            changes: changes.clone(),
            mast_changes: masts.clone(),
            after: after.components,
            score: after.score,
            yield_loss_pct: after.yield_loss_pct,
            tone: None,
        }]
    };
    OptimizeResult {
        outcome: OptimizeOutcome {
            before: before.components,
            before_score: before.score,
            before_yield_loss_pct: before.yield_loss_pct,
            after: after.components,
            after_score: after.score,
            after_yield_loss_pct: after.yield_loss_pct,
            evaluations,
            changes,
            cancelled: false,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        },
        candidates,
        mast_changes: masts,
        tone_before: None,
        tone_goal: None,
        lighting: CANONICAL_LIGHTING_PRESET,
        starts_run: 1,
        best_start: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        env::{DEFAULT_LINE_SEARCH, OBS_LAYOUT_VERSION, observation::projection_overrides_tier},
        model::{ModelTier, WeightError},
    };
    use indicatrix_cut_core::{ObjectiveFidelity, ObjectivePreset};
    use std::path::PathBuf;

    fn fast_config() -> InferenceConfig {
        InferenceConfig::optimizer()
            .with_steps(4)
            .with_report_fidelity(ObjectiveFidelity::Fast)
    }

    fn model() -> RlOptimizerModel {
        RlOptimizerModel::new(ModelTier::Small.init_network(11), fast_config())
            .expect("a freshly initialised network fits the layout")
    }

    #[test]
    fn the_result_describes_real_changes_of_the_given_design() {
        let design = Design::concave_fixture();
        let result = model()
            .optimize(&design, ObjectiveWeights::default())
            .expect("the fixture optimizes");
        let outcome = &result.outcome;
        assert!(outcome.before_score.is_finite());
        assert!(outcome.after_score <= outcome.before_score);
        // Between one and the whole line search per step (four steps).
        assert!(
            (1..=4 * DEFAULT_LINE_SEARCH.len()).contains(&outcome.evaluations),
            "{}",
            outcome.evaluations
        );
        for change in &outcome.changes {
            assert_eq!(change.from_deg, design.tiers[change.index].angle_deg);
            assert_ne!(change.from_deg.to_bits(), change.to_deg.to_bits());
        }
        match result.candidates.as_slice() {
            [] => {
                assert_eq!(outcome.changes.len(), 0);
                assert_eq!(result.mast_changes.len(), 0);
                assert_eq!(outcome.after, outcome.before);
                assert_eq!(outcome.after_score, outcome.before_score);
            }
            [candidate] => {
                assert_ne!(outcome.changes.len(), 0);
                assert_eq!(candidate.changes, outcome.changes);
                assert_eq!(candidate.mast_changes, result.mast_changes);
                assert_eq!(candidate.after, outcome.after);
                assert_eq!(candidate.score, outcome.after_score);
                assert!(outcome.after_score < outcome.before_score);
            }
            many => panic!(
                "one result carries at most one candidate, got {}",
                many.len()
            ),
        }
    }

    /// A network that gives every tier the same sensitivity, whatever it is shown: the
    /// sensitivity head ignores its input (all weights zero) and its bias is `[bias, 0, 0, 0]`,
    /// so every tier has the windowing sensitivity `bias` and none to the other components.
    /// The composed mean is then `-MEAN_BOUND` on every tier for `bias > 0` and `+MEAN_BOUND`
    /// for `bias < 0` (the objective's windowing fraction is positive for the default
    /// weights), and zero for `bias == 0`. The projection then turns the tiers it lets move
    /// by `tanh(+-MEAN_BOUND) * max_step_deg`, 95 % of the step limit, in one direction.
    fn constant_network(bias: f32) -> GemstoneSetAttentionNetwork {
        let mut network = ModelTier::Small.init_network(11);
        network.sens_head.weights.fill(0.0);
        network
            .sens_head
            .biases
            .copy_from_slice(&[bias, 0.0, 0.0, 0.0]);
        network
    }

    fn constant_model(bias: f32) -> RlOptimizerModel {
        RlOptimizerModel::new(constant_network(bias), fast_config())
            .expect("a constant-sensitivity network fits the layout")
    }

    /// Both directions of the turn: a positive bias turns every tier by a negative delta, a
    /// negative bias by a positive one. The size of the bias does not matter (the composed
    /// mean is scale-free), only its sign.
    const PROBE_BIASES: [f32; 2] = [1.0, -1.0];

    #[test]
    fn a_known_improving_policy_has_its_change_adopted_and_reported_exactly() {
        let design = Design::concave_fixture();
        let weights = ObjectiveWeights::default();
        let (bias, result) = PROBE_BIASES
            .iter()
            .find_map(|&bias| {
                let result = constant_model(bias)
                    .optimize(&design, weights)
                    .expect("the fixture optimizes");
                (!result.candidates.is_empty()).then_some((bias, result))
            })
            .expect("a uniform turn of the free tiers in one direction improves the fixture");

        // Replay the same policy by hand and measure its end design directly.
        let config = fast_config();
        let mut env = OptimizerEnv::new(
            design.clone(),
            OptimizerEnvConfig {
                max_steps: config.steps,
                max_step_deg: config.max_step_deg,
                weights,
                fidelity: config.rollout_fidelity,
                ..OptimizerEnvConfig::default()
            },
        )
        .expect("the fixture is a closed, movable stone");
        let observation = env.build_observation();
        let run = run_policy(
            &mut env,
            observation,
            &constant_network(bias),
            config.steps,
            config.max_step_deg,
            &config.line_search,
            config.stall_steps,
        )
        .expect("the policy runs");
        let gem = &env.source_material().gem;
        let end = env.current_design();
        let before = report_design(&design, gem, &weights, config.report_fidelity)
            .expect("the start is scorable");
        let after =
            report_design(end, gem, &weights, config.report_fidelity).expect("the end is scorable");
        assert!(
            after.score < before.score,
            "the probe found an improving policy"
        );

        let outcome = &result.outcome;
        assert_eq!(outcome.evaluations, run.evaluations);
        assert_eq!(outcome.before, before.components);
        assert_eq!(outcome.before_score, before.score);
        assert_eq!(outcome.after, after.components);
        assert_eq!(outcome.after_score, after.score);
        assert_eq!(outcome.after_yield_loss_pct, after.yield_loss_pct);
        assert_eq!(outcome.changes, angle_changes(&design, end));
        assert_ne!(outcome.changes.len(), 0);
        assert_eq!(result.mast_changes, mast_changes(&design, end));
        let [candidate] = result.candidates.as_slice() else {
            panic!("one adopted change is one candidate");
        };
        assert_eq!(candidate.changes, outcome.changes);
        assert_eq!(candidate.mast_changes, result.mast_changes);
        assert_eq!(candidate.after, after.components);
        assert_eq!(candidate.score, after.score);
        assert_eq!(candidate.yield_loss_pct, after.yield_loss_pct);
    }

    /// The fixture at n = 1.70 (critical angle 36.03 degrees), a stone whose free tiers all sit
    /// inside their angle envelope. The projection only keeps a delta of zero for a tier inside
    /// its envelope (a tier outside it is moved one step toward the window whatever the policy
    /// says). The fixture's own index, quartz (critical angle 40.49 degrees), puts its -38
    /// degree pavilion tier outside; at n = 1.70 it is inside.
    fn envelope_fixture() -> Design {
        let mut design = Design::concave_fixture();
        design.material.refractive_index_override = Some(1.70);
        design
    }

    fn assert_free_tiers_inside_their_envelope(design: &Design) {
        let env = OptimizerEnv::new(design.clone(), OptimizerEnvConfig::default())
            .expect("the fixture is a closed, movable stone");
        let observation = env.build_observation();
        let crit = observation.global.target_critical_angle_deg;
        for tier in observation
            .tiers
            .iter()
            .filter(|t| t.is_pinned < 0.5 && t.is_flat < 0.5)
        {
            assert!(
                !projection_overrides_tier(tier, crit),
                "premise: free tier at {} degrees lies inside its envelope (critical angle {crit})",
                tier.angle_deg
            );
        }
    }

    #[test]
    fn a_policy_that_does_not_move_proposes_no_change() {
        // Zero sensitivities give a zero mean: every step is a zero turn, which the greedy
        // environment rejects at every scale of the line search.
        let design = envelope_fixture();
        assert_free_tiers_inside_their_envelope(&design);
        let result = constant_model(0.0)
            .optimize(&design, ObjectiveWeights::default())
            .expect("the fixture optimizes");
        assert_eq!(result.candidates.len(), 0);
        assert_eq!(result.outcome.changes.len(), 0);
        assert_eq!(result.mast_changes.len(), 0);
        assert_eq!(result.outcome.after, result.outcome.before);
        assert_eq!(result.outcome.after_score, result.outcome.before_score);
        assert!((1..=4 * DEFAULT_LINE_SEARCH.len()).contains(&result.outcome.evaluations));
    }

    #[test]
    fn a_stalled_run_spends_only_the_steps_up_to_the_stall_limit() {
        let design = envelope_fixture();
        let evaluations = |stall_steps: usize| {
            RlOptimizerModel::new(
                constant_network(0.0),
                fast_config().with_stall_steps(stall_steps),
            )
            .expect("the network fits")
            .optimize(&design, ObjectiveWeights::default())
            .expect("the fixture optimizes")
            .outcome
            .evaluations
        };
        // A refused step spends the whole line search, so the counts are in steps of it.
        let one = evaluations(1);
        let all = evaluations(0);
        assert!(one <= DEFAULT_LINE_SEARCH.len(), "{one}");
        assert!(all > one, "{all} against {one}");
        assert!(all <= 4 * DEFAULT_LINE_SEARCH.len(), "{all}");
    }

    #[test]
    fn inference_is_deterministic() {
        let design = Design::concave_fixture();
        let first = model()
            .optimize(&design, ObjectiveWeights::default())
            .expect("optimizes");
        let second = model()
            .optimize(&design, ObjectiveWeights::default())
            .expect("optimizes");
        assert_eq!(first, second);
    }

    #[test]
    fn a_zero_step_run_is_an_error_not_an_empty_result() {
        let model = RlOptimizerModel::new(
            ModelTier::Small.init_network(11),
            fast_config().with_steps(0),
        )
        .expect("the network fits");
        assert!(matches!(
            model.optimize(&Design::concave_fixture(), ObjectiveWeights::default()),
            Err(InferenceError::Env(_))
        ));
    }

    #[test]
    fn a_tone_weight_is_refused_before_any_environment_work() {
        // Zero steps make the environment refuse to start (`InferenceError::Env`), so the
        // tone error can only come first.
        let model = RlOptimizerModel::new(
            ModelTier::Small.init_network(11),
            fast_config().with_steps(0),
        )
        .expect("the network fits");
        let design = Design::concave_fixture();
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let result = model.optimize(&design, preset.weights());
            assert!(
                matches!(result, Err(InferenceError::UnsupportedObjective(_))),
                "{preset:?}: {result:?}"
            );
        }
        // The premise: without the tone term the same model fails in the environment.
        assert!(matches!(
            model.optimize(&design, ObjectivePreset::Balanced.weights()),
            Err(InferenceError::Env(_))
        ));
    }

    #[test]
    fn the_supported_presets_all_optimize() {
        let design = Design::concave_fixture();
        let single_step = RlOptimizerModel::new(
            ModelTier::Small.init_network(11),
            fast_config().with_steps(1),
        )
        .expect("the network fits");
        for preset in [
            ObjectivePreset::Balanced,
            ObjectivePreset::Brilliance,
            ObjectivePreset::LowWindowing,
            ObjectivePreset::LowExtinction,
            ObjectivePreset::KeepWeight,
        ] {
            let result = single_step
                .optimize(&design, preset.weights())
                .unwrap_or_else(|error| panic!("{preset:?}: {error}"));
            assert!(result.outcome.before_score.is_finite(), "{preset:?}");
            assert!(
                result.outcome.after_score <= result.outcome.before_score,
                "{preset:?}"
            );
        }
    }

    fn checkpoint_of(mode: TrainingMode) -> Checkpoint {
        let tier = ModelTier::Small;
        Checkpoint::new(
            mode,
            tier,
            OBS_LAYOUT_VERSION,
            16,
            0,
            0,
            tier.init_network(2),
        )
    }

    /// Writes a checkpoint of `mode` to a file of its own in the temp directory.
    fn checkpoint_file(mode: TrainingMode, name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "indicatrix-ai-{}-optimizer-{name}.json",
            std::process::id()
        ));
        checkpoint_of(mode)
            .save(&path)
            .expect("the checkpoint is written");
        path
    }

    #[test]
    fn a_retarget_checkpoint_also_serves_the_optimizer() {
        for mode in [TrainingMode::Optimizer, TrainingMode::Retarget] {
            let model = RlOptimizerModel::from_checkpoint(checkpoint_of(mode), fast_config())
                .unwrap_or_else(|error| panic!("{mode}: {error}"));
            assert_eq!(model.config(), &fast_config(), "{mode}");
        }
    }

    #[test]
    fn a_damaged_checkpoint_is_still_refused_whatever_its_mode() {
        let mut checkpoint = checkpoint_of(TrainingMode::Retarget);
        checkpoint.network.tier_proj.weights.pop();
        assert!(matches!(
            RlOptimizerModel::from_checkpoint(checkpoint, fast_config()),
            Err(InferenceError::Weights(
                WeightError::DimensionMismatch { .. }
            ))
        ));
    }

    #[test]
    fn loading_a_file_accepts_both_modes() {
        let optimizer = checkpoint_file(TrainingMode::Optimizer, "optimizer-file");
        let retarget = checkpoint_file(TrainingMode::Retarget, "retarget-file");
        let from_optimizer = RlOptimizerModel::load(&optimizer, fast_config());
        let from_retarget = RlOptimizerModel::load(&retarget, fast_config());
        let _ = std::fs::remove_file(&optimizer);
        let _ = std::fs::remove_file(&retarget);
        assert!(from_optimizer.is_ok());
        assert!(from_retarget.is_ok());
    }
}
