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
//! The environment scores optical, tone and shape terms through the shared core path and adds
//! measured yield loss when requested and available. The final proposal is compared using the
//! same configured report fidelity.
//!
//! Checkpoints are task-specific: the optimizer wrapper accepts an optimizer checkpoint only.

use super::{
    config::InferenceConfig,
    error::InferenceError,
    policy::{check_network, network_from_file, network_of, run_policy},
    report::{DesignReport, angle_changes, mast_changes, report_design_with_settings},
};
use crate::{
    env::{OptimizerEnv, OptimizerEnvConfig},
    model::{Checkpoint, GemstoneSetAttentionNetwork, TrainingMode},
};
use indicatrix_cut_core::{
    AngleChange, Design, MastChange, ObjectiveWeights, OptimizeCandidate, OptimizeOutcome,
    OptimizeResult,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    optimize::{OptimizationProvenance, authoring_identity, materialize_preserved_changes},
};
use std::{path::Path, sync::Arc};

/// High-level RL inference engine for optimizing a design under its authored material.
#[derive(Debug, Clone)]
pub struct RlOptimizerModel {
    network: GemstoneSetAttentionNetwork,
    config: InferenceConfig,
}

impl RlOptimizerModel {
    /// Wrap a trained optimizer network.
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

    /// Wrap the network of an optimizer checkpoint.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Weights`] when the checkpoint task/schema/layout does not match or
    /// its network does not fit the current observation layout.
    pub fn from_checkpoint(
        checkpoint: Checkpoint,
        config: InferenceConfig,
    ) -> Result<Self, InferenceError> {
        Ok(Self {
            network: network_of(checkpoint, TrainingMode::Optimizer)?,
            config,
        })
    }

    /// Load an optimizer training checkpoint file.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Weights`] for an unreadable or damaged file, another schema or
    /// observation layout version, a retarget checkpoint, or a network that does not fit.
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
    /// step; the before and after report evaluations are not counted). The result includes
    /// tones measured under the configured lighting, plus the configured tone goal if weighted.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Env`] when the configuration is out of range or the design does not
    /// solve into a closed stone with a movable tier,
    /// [`InferenceError::Model`] when the network rejects an observation,
    /// [`InferenceError::Unscorable`] when the report cannot be measured.
    pub fn optimize(
        &self,
        design: &Design,
        weights: ObjectiveWeights,
    ) -> Result<OptimizeResult, InferenceError> {
        let config = &self.config;
        let mut env = OptimizerEnv::new(
            design.clone(),
            OptimizerEnvConfig {
                max_steps: config.steps,
                max_step_deg: config.max_step_deg,
                weights,
                fidelity: config.rollout_fidelity,
                lighting: config.lighting,
                shape_target: config.shape_target,
                only_tiers: config.only_tiers.clone(),
                angle_bounds: config.angle_bounds.clone(),
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
        let before = report_design_with_settings(
            design,
            gem,
            &weights,
            config.report_fidelity,
            config.lighting,
            config.shape_target,
        )?;
        let changes = angle_changes(design, end);
        let masts = mast_changes(design, end);
        let adopted = if end == design {
            None
        } else {
            let after = report_design_with_settings(
                end,
                gem,
                &weights,
                config.report_fidelity,
                config.lighting,
                config.shape_target,
            )?;
            (after.score < before.score).then_some(Adopted {
                after,
                changes,
                masts,
            })
        };
        let stamp = match &adopted {
            Some(adopted) => Some(
                stamp_proposal(
                    design,
                    env.current_design(),
                    &adopted.changes,
                    &adopted.masts,
                    &env.preservation_baseline(),
                )
                .map_err(InferenceError::Preservation)?,
            ),
            None => None,
        };
        let tone_goal = (weights.tone_weight > 0.0).then_some(weights.tone_goal);
        Ok(build_result(
            before,
            adopted,
            run.evaluations,
            stamp,
            config.lighting,
            tone_goal,
        ))
    }
}

/// The final design's figures and changes, when it measures strictly better than the start.
struct Adopted {
    after: DesignReport,
    changes: Vec<AngleChange>,
    masts: Vec<MastChange>,
}

/// The scoring identity of a result this engine produced. It is not a cut-core scoring key, so
/// `OptimizationProvenance::matches_scoring` never accepts it for a cut-core search.
const AI_SCORING_IDENTITY: &str = "indicatrix-ai:rl-optimizer";

/// The cut-core provenance and baseline of a proposal, which exist only when the proposal went
/// through the meet-preserving gate.
struct Stamp {
    provenance: OptimizationProvenance,
    baseline: Arc<MeetPreservationBaseline>,
}

/// Runs the proposal through cut-core's meet-preserving gate under the Optimize policy, the
/// policy of the cut-core search this engine stands in for.
///
/// Returns an error when exact materialization differs from the certified proposal or the
/// original-relative gate refuses it; no provenance is emitted for an uncertified result.
fn stamp_proposal(
    design: &Design,
    certified_state: &Design,
    changes: &[AngleChange],
    masts: &[MastChange],
    baseline: &MeetPreservationBaseline,
) -> Result<Stamp, String> {
    let policy = PreservationPolicy::OPTIMIZE;
    let (state, _, _) =
        materialize_preserved_changes(design, changes, masts, Some(baseline), policy)
            .map_err(|error| error.to_string())?;
    if &state != certified_state {
        return Err(
            "materializing the result changed its certified angles, masts, or authoring geometry"
                .into(),
        );
    }
    let preservation = baseline
        .validate(&state, policy)
        .map_err(|error| error.to_string())?;
    let provenance = OptimizationProvenance {
        source: authoring_identity(design).map_err(|error| error.to_string())?,
        candidate: authoring_identity(&state).map_err(|error| error.to_string())?,
        scoring: AI_SCORING_IDENTITY.to_owned(),
        policy,
        masts: masts
            .iter()
            .map(|m| (m.index, m.from_mast, m.to_mast))
            .collect(),
        diagnostics: baseline.diagnostics(),
        preservation,
    };
    Ok(Stamp {
        provenance,
        baseline: Arc::new(baseline.clone()),
    })
}

fn build_result(
    before: DesignReport,
    adopted: Option<Adopted>,
    evaluations: usize,
    stamp: Option<Stamp>,
    lighting: indicatrix::optics::raytracer::LightingPreset,
    tone_goal: Option<indicatrix_cut_core::ToneGoal>,
) -> OptimizeResult {
    let (after, changes, masts) = match adopted {
        Some(adopted) => (adopted.after, adopted.changes, adopted.masts),
        None => (before, Vec::new(), Vec::new()),
    };
    let provenance = stamp.as_ref().map(|s| s.provenance.clone());
    let candidates = if changes.is_empty() && masts.is_empty() {
        Vec::new()
    } else {
        vec![OptimizeCandidate {
            provenance: provenance.clone(),
            preservation_baseline: stamp.map(|s| s.baseline),
            changes: changes.clone(),
            mast_changes: masts.clone(),
            after: after.components,
            score: after.score,
            yield_loss_pct: after.yield_loss_pct,
            tone: Some(after.tone),
        }]
    };
    OptimizeResult {
        outcome: OptimizeOutcome {
            provenance,
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
        tone_before: Some(before.tone),
        tone_goal,
        lighting,
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
        network.sens_head.biases.fill(0.0);
        network.sens_head.biases[0] = bias;
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
                lighting: config.lighting,
                shape_target: config.shape_target,
                only_tiers: config.only_tiers.clone(),
                angle_bounds: config.angle_bounds.clone(),
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
        let before = report_design_with_settings(
            &design,
            gem,
            &weights,
            config.report_fidelity,
            config.lighting,
            config.shape_target,
        )
        .expect("the start is scorable");
        let after = report_design_with_settings(
            end,
            gem,
            &weights,
            config.report_fidelity,
            config.lighting,
            config.shape_target,
        )
        .expect("the end is scorable");
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
        // Zero sensitivities give a zero mean: every step is a zero turn, a projection no-op the
        // greedy environment refuses before it reaches the scorer, at every scale of the line
        // search. Nothing changes and nothing is spent.
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
        assert_eq!(result.outcome.evaluations, 0);
    }

    #[test]
    fn a_stalled_run_spends_only_the_steps_up_to_the_stall_limit() {
        // A zero turn costs no evaluation, so the stall rule is measured with the constant turn
        // in the direction that does NOT improve the fixture: every step scores its line search
        // and is refused, the design never changes, and each step therefore costs the same.
        let design = envelope_fixture();
        let run = |bias: f32, stall_steps: usize| {
            let result = RlOptimizerModel::new(
                constant_network(bias),
                fast_config().with_stall_steps(stall_steps),
            )
            .expect("the network fits")
            .optimize(&design, ObjectiveWeights::default())
            .expect("the fixture optimizes");
            (result.candidates.is_empty(), result.outcome.evaluations)
        };
        let bias = PROBE_BIASES
            .iter()
            .copied()
            .find(|&bias| {
                let (unchanged, evaluations) = run(bias, 0);
                unchanged && evaluations > 0
            })
            .expect("a uniform turn in one direction is scored and refused at every scale");
        let (_, one) = run(bias, 1);
        let (_, all) = run(bias, 0);
        assert!((1..=DEFAULT_LINE_SEARCH.len()).contains(&one), "{one}");
        // Four steps of the same refused search; a stall limit of one stops after the first.
        assert_eq!(all, 4 * one, "{all} against {one}");
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
    fn tone_objectives_reach_the_scoring_environment() {
        let mut config = fast_config().with_steps(1);
        config.lighting = indicatrix::optics::raytracer::LightingPreset::Daylight;
        let model = RlOptimizerModel::new(ModelTier::Small.init_network(11), config)
            .expect("the network fits");
        let design = Design::concave_fixture();
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let result = model
                .optimize(&design, preset.weights())
                .unwrap_or_else(|error| panic!("{preset:?}: {error}"));
            assert_eq!(
                result.lighting,
                indicatrix::optics::raytracer::LightingPreset::Daylight
            );
            assert!(result.tone_before.is_some(), "{preset:?}");
            assert_eq!(result.tone_goal, Some(preset.weights().tone_goal));
            if let Some(candidate) = result.candidates.first() {
                assert!(candidate.tone.is_some(), "{preset:?}");
            }
        }
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
            ObjectivePreset::LightenDark,
            ObjectivePreset::IntensifyPale,
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
    fn only_an_optimizer_checkpoint_serves_the_optimizer() {
        let model = RlOptimizerModel::from_checkpoint(
            checkpoint_of(TrainingMode::Optimizer),
            fast_config(),
        )
        .expect("an optimizer checkpoint serves the optimizer");
        assert_eq!(model.config(), &fast_config());
        // Checkpoints are task-specific (schema 4): a retarget model is refused by its mode.
        assert!(matches!(
            RlOptimizerModel::from_checkpoint(checkpoint_of(TrainingMode::Retarget), fast_config()),
            Err(InferenceError::Weights(WeightError::ModeMismatch {
                expected: TrainingMode::Optimizer,
                got: TrainingMode::Retarget,
            }))
        ));
    }

    #[test]
    fn a_damaged_checkpoint_is_still_refused_whatever_its_mode() {
        let damaged = |mode: TrainingMode| {
            let mut checkpoint = checkpoint_of(mode);
            checkpoint.network.tier_proj.weights.pop();
            checkpoint
        };
        assert!(matches!(
            RlOptimizerModel::from_checkpoint(damaged(TrainingMode::Optimizer), fast_config()),
            Err(InferenceError::Weights(
                WeightError::DimensionMismatch { .. }
            ))
        ));
        // The mode is checked before the network: a damaged retarget checkpoint is refused for
        // its mode, and never reaches the optimizer.
        assert!(matches!(
            RlOptimizerModel::from_checkpoint(damaged(TrainingMode::Retarget), fast_config()),
            Err(InferenceError::Weights(WeightError::ModeMismatch { .. }))
        ));
    }

    #[test]
    fn loading_a_file_follows_the_same_mode_rule() {
        let optimizer = checkpoint_file(TrainingMode::Optimizer, "optimizer-file");
        let retarget = checkpoint_file(TrainingMode::Retarget, "retarget-file");
        let from_optimizer = RlOptimizerModel::load(&optimizer, fast_config());
        let from_retarget = RlOptimizerModel::load(&retarget, fast_config());
        let _ = std::fs::remove_file(&optimizer);
        let _ = std::fs::remove_file(&retarget);
        assert!(from_optimizer.is_ok());
        assert!(matches!(
            from_retarget,
            Err(InferenceError::Weights(WeightError::ModeMismatch { .. }))
        ));
    }
}
