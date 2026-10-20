//! Inference engine for the reinforcement-learned material retargeter.
//!
//! [`RlRetargetModel::retarget`] runs the trained set-attention policy greedily in a
//! [`RetargetEnv`]: starting from the design it is given, in the target material, it adjusts
//! the movable tiers to lower the optical score there. Each step proposes a direction (the
//! network predicts the per-tier sensitivities of the score components and composes the
//! direction from the objective's weight coefficients) and the environment's line search picks
//! how far to go along it. The caller chooses the starting design. The compatibility editor
//! adapter also starts from that original design and returns a plan only if it reconstructs the
//! exact certified geometry.
//!
//! Both tone goals and optional shape/yield terms use the shared environment scorer. Use
//! [`RlRetargetModel::retarget_certified`] when the exact materialized angles and masts are needed.

#[cfg(test)]
use super::report::report_design;
use super::{
    config::InferenceConfig,
    error::InferenceError,
    policy::{check_network, network_from_file, network_of, run_policy},
    report::{angle_changes, report_design_with_settings},
};
use crate::{
    env::{RetargetEnv, RetargetEnvConfig},
    model::{Checkpoint, GemstoneSetAttentionNetwork, TrainingMode},
};
use indicatrix_cut_core::meet_preservation::PreservationFacts;
use indicatrix_cut_core::{Design, ObjectiveWeights, ResolvedMaterial};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Summary of an AI-driven retarget proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RlRetargetProposal {
    /// Target material the stone was adapted to.
    pub target_material_name: String,
    /// Score of the design the run started from, measured in the target material (lower is
    /// better) at the report fidelity.
    pub base_score: f32,
    /// Score of the proposed design in the target material (lower is better) at the report
    /// fidelity. Equals [`Self::base_score`] when the policy found nothing better.
    pub target_score: f32,
    /// Number of policy steps taken.
    pub steps_taken: usize,
    /// Candidate evaluations the rollout spent (at the rollout fidelity): between one and the
    /// number of line-search scales per step that reached the scorer. The two report
    /// evaluations (before and after) are not counted.
    #[serde(default)]
    pub evaluations: usize,
    /// `(tier_index, angle_deg)` of the tiers whose angle the policy changed, in tier order;
    /// signed degrees (crown positive, pavilion negative). Tiers the policy left alone and
    /// tiers that follow a relation are not listed; empty when nothing improved.
    pub proposed_angles_deg: Vec<(usize, f64)>,
    /// Full solved signed angle vector for the selected certified design.
    #[serde(default)]
    pub solved_angles_deg: Vec<f64>,
    /// Full solved mast vector paired by tier index. For exact application prefer the
    /// certified result's materialized Design, which also retains authoring constraints/tools.
    #[serde(default)]
    pub solved_masts: Vec<f64>,
}

/// Complete retarget output with the exact certified materialized design and solved mast vector.
#[derive(Debug, Clone, PartialEq)]
pub struct CertifiedRetargetResult {
    /// User-facing summary of scores, steps, and changed independent angles.
    pub proposal: RlRetargetProposal,
    /// Exact design returned by the episode and certified against its original baseline.
    pub design: Design,
    /// Exact solved signed angle per authored tier.
    pub angles_deg: Vec<f64>,
    /// Exact solved mast per authored tier.
    pub masts: Vec<f64>,
    /// Core preservation facts measured against the immutable episode baseline.
    pub preservation: PreservationFacts,
}

impl RlRetargetProposal {
    /// The proposed angle of tier `tier_index`, if the policy changed it.
    #[must_use]
    pub fn angle_of(&self, tier_index: usize) -> Option<f64> {
        self.proposed_angles_deg
            .iter()
            .find(|&&(index, _)| index == tier_index)
            .map(|&(_, angle)| angle)
    }
}

/// High-level RL inference engine for gemstone material retargeting.
#[derive(Debug, Clone)]
pub struct RlRetargetModel {
    network: GemstoneSetAttentionNetwork,
    config: InferenceConfig,
}

impl RlRetargetModel {
    /// Wrap a trained retargeting network.
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

    /// Wrap the network of a training checkpoint.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Weights`] when the checkpoint task/schema/layout does not match or its
    /// network does not fit the current observation layout.
    pub fn from_checkpoint(
        checkpoint: Checkpoint,
        config: InferenceConfig,
    ) -> Result<Self, InferenceError> {
        Ok(Self {
            network: network_of(checkpoint, TrainingMode::Retarget)?,
            config,
        })
    }

    /// Load a training checkpoint file.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Weights`] for an unreadable or damaged file, another schema or
    /// observation layout version, an optimizer checkpoint, or a network that does not fit.
    pub fn load(path: &Path, config: InferenceConfig) -> Result<Self, InferenceError> {
        Ok(Self {
            network: network_from_file(path, TrainingMode::Retarget)?,
            config,
        })
    }

    /// The settings of every [`Self::retarget`] call.
    #[must_use]
    pub const fn config(&self) -> &InferenceConfig {
        &self.config
    }

    /// Retarget `design` towards `target_material` with greedy policy rollouts.
    ///
    /// The run ends after [`InferenceConfig::steps`] steps, when the environment ends the
    /// episode, or after [`InferenceConfig::stall_steps`] consecutive steps without an accepted
    /// candidate. The proposal is adopted only when it measures strictly better than `design`
    /// in the target material at report fidelity. `proposed_angles_deg` lists independent angle
    /// changes; solved angle/mast vectors retain the complete certified geometry.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Env`] when the configuration is out of range or the design does not
    /// solve into a closed, scorable stone with a movable
    /// tier in the target material, [`InferenceError::Model`] when the network rejects an
    /// observation, [`InferenceError::Unscorable`] when the report cannot be measured.
    pub fn retarget(
        &self,
        design: &Design,
        target_material: &ResolvedMaterial,
        weights: ObjectiveWeights,
    ) -> Result<RlRetargetProposal, InferenceError> {
        self.retarget_certified(design, target_material, weights)
            .map(|result| result.proposal)
    }

    /// Run a retarget and return its exact certified materialized geometry.
    pub fn retarget_certified(
        &self,
        design: &Design,
        target_material: &ResolvedMaterial,
        weights: ObjectiveWeights,
    ) -> Result<CertifiedRetargetResult, InferenceError> {
        let config = &self.config;
        let mut env = RetargetEnv::new(
            design.clone(),
            RetargetEnvConfig {
                max_steps: config.steps,
                max_step_deg: config.max_step_deg,
                weights,
                fidelity: config.rollout_fidelity,
                lighting: config.lighting,
                shape_target: config.shape_target,
                only_tiers: config.only_tiers.clone(),
                angle_bounds: config.angle_bounds.clone(),
                ..RetargetEnvConfig::new_with_target(target_material.clone())
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

        let gem = &target_material.gem;
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
        let mut target_score = before.score;
        let mut proposed_angles_deg = Vec::new();
        let mut chosen_design = design.clone();
        if end != design {
            let after = report_design_with_settings(
                end,
                gem,
                &weights,
                config.report_fidelity,
                config.lighting,
                config.shape_target,
            )?;
            if after.score < before.score {
                target_score = after.score;
                chosen_design = end.clone();
                proposed_angles_deg = changes
                    .iter()
                    .map(|change| (change.index, change.to_deg))
                    .collect();
            }
        }
        let solved = chosen_design
            .solve()
            .map_err(|error| InferenceError::Unscorable(error.to_string()))?;
        let solved_angles_deg: Vec<f64> = chosen_design
            .tiers
            .iter()
            .map(|tier| tier.angle_deg)
            .collect();
        let solved_masts: Vec<f64> = solved.iter().map(|tier| tier.mast).collect();
        let proposal = RlRetargetProposal {
            target_material_name: gem.name.clone(),
            base_score: before.score,
            target_score,
            steps_taken: run.steps,
            evaluations: run.evaluations,
            proposed_angles_deg,
            solved_angles_deg: solved_angles_deg.clone(),
            solved_masts: solved_masts.clone(),
        };
        let materialized = chosen_design;
        let preservation = env
            .certify_design(&materialized)
            .map_err(InferenceError::Preservation)?;
        Ok(CertifiedRetargetResult {
            proposal,
            angles_deg: solved_angles_deg,
            masts: solved_masts,
            design: materialized,
            preservation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        env::{ContinuousTierAction, DEFAULT_LINE_SEARCH, OBS_LAYOUT_VERSION},
        model::{ModelTier, WeightError},
    };
    use indicatrix_cut_core::{
        BuiltinMaterials, MaterialSelection, ObjectiveFidelity, ObjectivePreset,
    };
    use std::path::PathBuf;

    fn sapphire() -> ResolvedMaterial {
        MaterialSelection {
            name: Some("Sapphire".to_string()),
            ..MaterialSelection::none()
        }
        .resolve(&BuiltinMaterials)
    }

    fn model() -> RlRetargetModel {
        RlRetargetModel::new(
            ModelTier::Small.init_network(21),
            InferenceConfig::retarget()
                .with_steps(4)
                .with_report_fidelity(ObjectiveFidelity::Fast),
        )
        .expect("a freshly initialised network fits the layout")
    }

    #[test]
    fn only_tiers_that_moved_are_proposed() {
        let design = Design::concave_fixture();
        let proposal = model()
            .retarget(&design, &sapphire(), ObjectiveWeights::default())
            .expect("the fixture retargets");
        assert_eq!(proposal.target_material_name, sapphire().gem.name);
        assert!(proposal.base_score.is_finite());
        assert!(proposal.target_score <= proposal.base_score);
        assert!((1..=4).contains(&proposal.steps_taken));
        // Between one and the whole line search per step.
        assert!(
            (1..=proposal.steps_taken * DEFAULT_LINE_SEARCH.len()).contains(&proposal.evaluations),
            "{proposal:?}"
        );
        for &(index, angle) in &proposal.proposed_angles_deg {
            assert_ne!(angle.to_bits(), design.tiers[index].angle_deg.to_bits());
            assert_eq!(
                angle.is_sign_negative(),
                design.tiers[index].angle_deg.is_sign_negative(),
                "a tier never crosses the girdle"
            );
        }
        let original_masts: Vec<f64> = design
            .solve()
            .expect("fixture solves")
            .iter()
            .map(|tier| tier.mast)
            .collect();
        let has_materialized_motion = proposal
            .solved_angles_deg
            .iter()
            .zip(&design.tiers)
            .any(|(after, before)| after.to_bits() != before.angle_deg.to_bits())
            || proposal
                .solved_masts
                .iter()
                .zip(&original_masts)
                .any(|(after, before)| after.to_bits() != before.to_bits());
        assert_eq!(proposal.solved_angles_deg.len(), design.tiers.len());
        assert_eq!(proposal.solved_masts.len(), design.tiers.len());
        if !has_materialized_motion {
            assert_eq!(proposal.target_score, proposal.base_score);
        } else {
            assert!(proposal.target_score < proposal.base_score);
        }
    }

    /// A network that gives every tier the same sensitivity, whatever it is shown: the
    /// sensitivity head ignores its input (all weights zero) and its bias starts `[bias, 0, ...]`,
    /// so every tier has the windowing sensitivity `bias` and none to the other components.
    /// The composed mean is then `-MEAN_BOUND` on every tier for `bias > 0` and `+MEAN_BOUND`
    /// for `bias < 0` (the objective's windowing fraction is positive for the default
    /// weights), and zero for `bias == 0`. The projection then turns the tiers it lets move
    /// by `tanh(+-MEAN_BOUND) * max_step_deg`, 95 % of the step limit, in one direction.
    fn constant_network(bias: f32) -> GemstoneSetAttentionNetwork {
        let mut network = ModelTier::Small.init_network(21);
        network.sens_head.weights.fill(0.0);
        network.sens_head.biases.fill(0.0);
        network.sens_head.biases[0] = bias;
        network
    }

    /// A 2-degree step limit: the proposal is 1.9 degrees per tier and the line search tries
    /// 1.9, 0.95 and 0.475 degrees, so the turned-away stone (1.5 degrees off) comes back to
    /// the fixture's angles in a step or two whichever scale the greedy gate takes. The test is
    /// about the adoption and reporting path, not about the step size.
    fn constant_model(bias: f32) -> RlRetargetModel {
        RlRetargetModel::new(
            constant_network(bias),
            InferenceConfig::retarget()
                .with_steps(4)
                .with_max_step_deg(2.0)
                .with_report_fidelity(ObjectiveFidelity::Fast),
        )
        .expect("a constant-sensitivity network fits the layout")
    }

    /// Both directions of the turn: a positive bias turns every tier by a negative delta, a
    /// negative bias by a positive one. The size of the bias does not matter (the composed
    /// mean is scale-free), only its sign.
    const PROBE_BIASES: [f32; 2] = [1.0, -1.0];

    /// The fixture with every free tier turned `TURN_AWAY_DEG` away from its authored angle,
    /// re-anchored on its hinge exactly as an environment step does it. The fixture itself
    /// sits at a local optimum of the `Fast` score in sapphire (a uniform turn in either
    /// direction scores worse, measured 2026-10-12), so the adoption test starts from this
    /// turned-away stone, for which the uniform turn back (a negative delta, a positive bias)
    /// is an improving policy.
    const TURN_AWAY_DEG: f64 = 1.5;

    fn turned_away_fixture() -> Design {
        use crate::env::common::{MoveSpace, apply_action};
        let design = Design::concave_fixture();
        let space = MoveSpace::measure(&design, true).expect("the fixture has movable tiers");
        let action = ContinuousTierAction::new(vec![TURN_AWAY_DEG; design.tiers.len()]);
        match apply_action(&space, &design, &action, TURN_AWAY_DEG) {
            Ok(turned) => turned,
            // The turn goes through the meet-preserving projection like every candidate; name
            // the exact refusal so a gate change is visible from the failure alone.
            Err(rejection) => panic!(
                "the uniform {TURN_AWAY_DEG} degree turn of the fixture's free tiers under \
                 PreservationPolicy::OPTIMIZE is refused ({rejection:?}): {}",
                crate::env::moves::preservation_refusal(&space, &design, &action, TURN_AWAY_DEG)
                    .unwrap_or_else(|| "refused before the meet-preserving projection".to_owned())
            ),
        }
    }

    #[test]
    fn a_known_improving_policy_has_its_angles_adopted_and_reported_exactly() {
        let design = turned_away_fixture();
        let target = sapphire();
        let weights = ObjectiveWeights::default();
        let (bias, proposal) = PROBE_BIASES
            .iter()
            .find_map(|&bias| {
                let proposal = constant_model(bias)
                    .retarget(&design, &target, weights)
                    .expect("the fixture retargets");
                (!proposal.proposed_angles_deg.is_empty()).then_some((bias, proposal))
            })
            .expect("a uniform turn of the free tiers in one direction improves in sapphire");

        // Replay the same policy by hand and measure its end design directly.
        let config = constant_model(bias).config().clone();
        let mut env = RetargetEnv::new(
            design.clone(),
            RetargetEnvConfig {
                max_steps: config.steps,
                max_step_deg: config.max_step_deg,
                weights,
                fidelity: config.rollout_fidelity,
                ..RetargetEnvConfig::new_with_target(target.clone())
            },
        )
        .expect("the fixture retargets");
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
        let end = env.current_design();
        let before = report_design(&design, &target.gem, &weights, config.report_fidelity)
            .expect("the start is scorable");
        let after = report_design(end, &target.gem, &weights, config.report_fidelity)
            .expect("the end is scorable");
        assert!(
            after.score < before.score,
            "the probe found an improving policy"
        );

        assert_eq!(proposal.steps_taken, run.steps);
        assert_eq!(proposal.evaluations, run.evaluations);
        assert_eq!(proposal.base_score, before.score);
        assert_eq!(proposal.target_score, after.score);
        let expected: Vec<(usize, f64)> = angle_changes(&design, end)
            .iter()
            .map(|change| (change.index, change.to_deg))
            .collect();
        assert_eq!(proposal.proposed_angles_deg, expected);
    }

    #[test]
    fn a_policy_that_does_not_move_proposes_nothing() {
        // Zero sensitivities give a zero mean: every step is a zero turn, which the greedy
        // environment rejects at every scale of the line search.
        let proposal = constant_model(0.0)
            .retarget(
                &Design::concave_fixture(),
                &sapphire(),
                ObjectiveWeights::default(),
            )
            .expect("the fixture retargets");
        assert_eq!(proposal.proposed_angles_deg.len(), 0);
        assert_eq!(proposal.target_score, proposal.base_score);
        assert!((1..=4).contains(&proposal.steps_taken));
        assert!(proposal.evaluations <= proposal.steps_taken * DEFAULT_LINE_SEARCH.len());
    }

    #[test]
    fn a_stalled_run_ends_after_the_configured_number_of_steps() {
        let run_with = |stall_steps: usize| {
            RlRetargetModel::new(
                constant_network(0.0),
                InferenceConfig::retarget()
                    .with_steps(4)
                    .with_stall_steps(stall_steps)
                    .with_report_fidelity(ObjectiveFidelity::Fast),
            )
            .expect("a constant-sensitivity network fits the layout")
            .retarget(
                &Design::concave_fixture(),
                &sapphire(),
                ObjectiveWeights::default(),
            )
            .expect("the fixture retargets")
        };
        assert_eq!(run_with(1).steps_taken, 1);
        assert_eq!(run_with(2).steps_taken, 2);
        assert_eq!(run_with(0).steps_taken, 4);
    }

    #[test]
    fn a_single_scale_line_search_costs_at_most_one_evaluation_per_step() {
        let proposal = RlRetargetModel::new(
            constant_network(0.0),
            InferenceConfig::retarget()
                .with_steps(3)
                .with_line_search(&[1.0])
                .with_stall_steps(0)
                .with_report_fidelity(ObjectiveFidelity::Fast),
        )
        .expect("a constant-sensitivity network fits the layout")
        .retarget(
            &Design::concave_fixture(),
            &sapphire(),
            ObjectiveWeights::default(),
        )
        .expect("the fixture retargets");
        assert_eq!(proposal.steps_taken, 3);
        assert!(proposal.evaluations <= 3, "{proposal:?}");
    }

    #[test]
    fn tone_objectives_reach_the_scoring_environment() {
        // One step: the environment refuses a zero step count (see the optimizer's
        // `a_zero_step_run_is_an_error_not_an_empty_result`), and one is enough to score.
        let model = RlRetargetModel::new(
            ModelTier::Small.init_network(21),
            InferenceConfig::retarget()
                .with_steps(1)
                .with_report_fidelity(ObjectiveFidelity::Fast),
        )
        .expect("the network fits");
        let design = Design::concave_fixture();
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let result = model.retarget(&design, &sapphire(), preset.weights());
            assert!(result.is_ok(), "{preset:?}: {result:?}");
        }
    }

    #[test]
    fn the_supported_presets_all_retarget() {
        let design = Design::concave_fixture();
        let single_step = RlRetargetModel::new(
            ModelTier::Small.init_network(21),
            InferenceConfig::retarget()
                .with_steps(1)
                .with_report_fidelity(ObjectiveFidelity::Fast),
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
            let proposal = single_step
                .retarget(&design, &sapphire(), preset.weights())
                .unwrap_or_else(|error| panic!("{preset:?}: {error}"));
            assert!(proposal.base_score.is_finite(), "{preset:?}");
            assert!(proposal.target_score <= proposal.base_score, "{preset:?}");
        }
    }

    #[test]
    fn angle_of_reads_the_proposal() {
        let proposal = RlRetargetProposal {
            target_material_name: "Sapphire".to_string(),
            base_score: 40.0,
            target_score: 38.0,
            steps_taken: 3,
            evaluations: 7,
            proposed_angles_deg: vec![(2, -41.5), (5, 33.0)],
            solved_angles_deg: vec![-41.5, 33.0],
            solved_masts: vec![1.0, 1.0],
        };
        assert_eq!(proposal.angle_of(5), Some(33.0));
        assert_eq!(proposal.angle_of(3), None);
    }

    #[test]
    fn a_proposal_without_the_evaluation_count_still_parses() {
        let json = r#"{"target_material_name":"Sapphire","base_score":40.0,"target_score":38.0,
            "steps_taken":3,"proposed_angles_deg":[[2,-41.5]]}"#;
        let proposal: RlRetargetProposal = serde_json::from_str(json).expect("an older proposal");
        assert_eq!(proposal.evaluations, 0);
        assert_eq!(proposal.steps_taken, 3);
        assert_eq!(proposal.angle_of(2), Some(-41.5));
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
            "indicatrix-ai-{}-retarget-{name}.json",
            std::process::id()
        ));
        checkpoint_of(mode)
            .save(&path)
            .expect("the checkpoint is written");
        path
    }

    #[test]
    fn an_optimizer_checkpoint_is_not_a_retargeter() {
        assert!(matches!(
            RlRetargetModel::from_checkpoint(
                checkpoint_of(TrainingMode::Optimizer),
                InferenceConfig::retarget()
            ),
            Err(InferenceError::Weights(WeightError::ModeMismatch { .. }))
        ));
        assert!(
            RlRetargetModel::from_checkpoint(
                checkpoint_of(TrainingMode::Retarget),
                InferenceConfig::retarget()
            )
            .is_ok()
        );
    }

    #[test]
    fn loading_a_file_follows_the_same_mode_rule() {
        let optimizer = checkpoint_file(TrainingMode::Optimizer, "optimizer-file");
        let retarget = checkpoint_file(TrainingMode::Retarget, "retarget-file");
        let refused = RlRetargetModel::load(&optimizer, InferenceConfig::retarget());
        let accepted = RlRetargetModel::load(&retarget, InferenceConfig::retarget());
        let _ = std::fs::remove_file(&optimizer);
        let _ = std::fs::remove_file(&retarget);
        assert!(matches!(
            refused,
            Err(InferenceError::Weights(WeightError::ModeMismatch { .. }))
        ));
        assert!(accepted.is_ok());
    }
}
