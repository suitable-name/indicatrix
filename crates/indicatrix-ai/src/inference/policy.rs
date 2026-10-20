//! The deterministic policy rollout and the network and objective checks both inference models
//! share.

#![allow(
    clippy::redundant_pub_crate,
    reason = "private module: `pub(crate)` documents the intended reach of each item"
)]

use super::error::InferenceError;
use crate::{
    env::{
        ContinuousTierAction, GLOBAL_DIM, GemstoneObservation, GemstoneRlEnv, OBS_LAYOUT_VERSION,
        TIER_DIM,
    },
    model::{
        CHECKPOINT_SCHEMA_VERSION, Checkpoint, GemstoneSetAttentionNetwork, TrainingMode,
        WeightError,
    },
};
use std::path::Path;

/// Rejects a network that cannot read the current observation layout or whose parameters are
/// inconsistent (a hand-edited or truncated file would otherwise fail inside `forward`).
///
/// The layout is checked first, so a network of another token width is named as such
/// ([`WeightError::DimensionMismatch`] with the layout version) rather than by whichever
/// structural rule a narrower token happens to break.
pub(crate) fn check_network(network: &GemstoneSetAttentionNetwork) -> Result<(), InferenceError> {
    for (what, expected, got) in [
        ("d_tier", TIER_DIM, network.d_tier),
        ("d_global", GLOBAL_DIM, network.d_global),
    ] {
        if expected != got {
            return Err(WeightError::DimensionMismatch {
                what: format!("{what} (observation layout version {OBS_LAYOUT_VERSION})"),
                expected,
                got,
            }
            .into());
        }
    }
    network.validate()?;
    Ok(())
}

/// Whether a network trained as `trained_for` can run `task`.
///
/// A checkpoint only serves the task it was explicitly trained for.
const fn serves(trained_for: TrainingMode, task: TrainingMode) -> bool {
    matches!(
        (trained_for, task),
        (TrainingMode::Optimizer, TrainingMode::Optimizer)
            | (TrainingMode::Retarget, TrainingMode::Retarget)
    )
}

/// The network of `checkpoint`, after checking that it can run `task` (see [`serves`]) and fits
/// the current observation layout.
pub(crate) fn network_of(
    checkpoint: Checkpoint,
    task: TrainingMode,
) -> Result<GemstoneSetAttentionNetwork, InferenceError> {
    if checkpoint.schema_version != CHECKPOINT_SCHEMA_VERSION {
        return Err(WeightError::UnsupportedSchema {
            expected: CHECKPOINT_SCHEMA_VERSION,
            got: checkpoint.schema_version,
        }
        .into());
    }
    if checkpoint.obs_layout_version != OBS_LAYOUT_VERSION {
        return Err(WeightError::ObsLayoutMismatch {
            expected: OBS_LAYOUT_VERSION,
            got: checkpoint.obs_layout_version,
        }
        .into());
    }
    if !serves(checkpoint.mode, task) {
        return Err(WeightError::ModeMismatch {
            expected: task,
            got: checkpoint.mode,
        }
        .into());
    }
    check_network(&checkpoint.network)?;
    Ok(checkpoint.network)
}

/// Loads the checkpoint at `path` (rejecting another schema, another observation layout or a
/// damaged network) and returns its network for `task` (see [`network_of`]).
pub(crate) fn network_from_file(
    path: &Path,
    task: TrainingMode,
) -> Result<GemstoneSetAttentionNetwork, InferenceError> {
    network_of(Checkpoint::load(path, OBS_LAYOUT_VERSION)?, task)
}

/// What a policy rollout did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolicyRun {
    /// Policy steps taken. Every step counts, whether or not its candidate was accepted.
    pub(crate) steps: usize,
    /// Candidate evaluations the steps spent: the sum of the steps' evaluation counts, between
    /// one and the number of line-search scales per step for a step that reached the scorer,
    /// none for a step whose candidates were all refused before scoring.
    pub(crate) evaluations: usize,
}

/// Runs the network greedily in `env` and returns what it took.
///
/// Each step feeds the observation's tokens to the network
/// ([`GemstoneSetAttentionNetwork::forward`]: the policy mean is composed from the predicted
/// per-tier sensitivities and the objective's weight fractions), takes the deterministic action
/// `tanh(mean) * max_step_deg` and passes it through the kinematic projection
/// ([`GemstoneSetAttentionNetwork::project_actions`]: pinned and flat tiers stay put, movable
/// angles remain sign-preserving within the shared safe interval, nothing crosses the girdle;
/// critical-angle margin is contextual, not an autonomous correction). The environment then tries
/// the action at each of the `line_search` scales in turn and keeps the first improving one
/// ([`GemstoneRlEnv::step_scaled`]); the step counts once whatever the scale count.
/// `observation` is the environment's current observation.
///
/// The run ends at the first of three things:
///
/// 1. `steps` steps have been taken;
/// 2. the environment terminates or truncates the episode;
/// 3. `stall_steps` consecutive steps ended without an accepted candidate
///    ([`StepInfo::rejection`](crate::env::StepInfo::rejection) is `Some`). The policy is
///    deterministic and the design only changes on an accepted candidate, so refused steps
///    repeat themselves. `0` switches the rule off.
///
/// # Errors
///
/// [`InferenceError::Model`] when the network rejects an observation.
pub(crate) fn run_policy<E>(
    env: &mut E,
    mut observation: GemstoneObservation,
    network: &GemstoneSetAttentionNetwork,
    steps: usize,
    max_step_deg: f64,
    line_search: &[f64],
    stall_steps: usize,
) -> Result<PolicyRun, InferenceError>
where
    E: GemstoneRlEnv<Observation = GemstoneObservation, Action = ContinuousTierAction>,
{
    let mut run = PolicyRun {
        steps: 0,
        evaluations: 0,
    };
    let mut stalled = 0;
    for _ in 0..steps {
        let (policy, _value) =
            network.forward(&observation.tier_tokens(), &observation.global_token())?;
        let mut action = policy.sample(0, true, max_step_deg);
        GemstoneSetAttentionNetwork::project_actions(
            &mut action,
            &observation.tiers,
            observation.global.target_critical_angle_deg,
            max_step_deg,
        );
        let step = env.step_scaled(&action, line_search);
        run.steps += 1;
        run.evaluations += usize::from(step.info.evaluations);
        stalled = if step.info.rejection.is_some() {
            stalled + 1
        } else {
            0
        };
        observation = step.observation;
        if step.terminated || step.truncated || (stall_steps > 0 && stalled >= stall_steps) {
            break;
        }
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        env::{DEFAULT_LINE_SEARCH, RetargetEnv, RetargetEnvConfig, Step, StepInfo, StepRejection},
        model::ModelTier,
    };
    use indicatrix_cut_core::{BuiltinMaterials, Design, MaterialSelection};

    #[test]
    fn a_network_of_the_current_layout_passes() {
        let network = ModelTier::Small.init_network(1);
        assert!(check_network(&network).is_ok());
    }

    #[test]
    fn in_memory_checkpoints_must_match_schema_and_observation_stamp() {
        let tier = ModelTier::Small;
        let network = tier.init_network(41);
        let mut checkpoint = Checkpoint::new(
            TrainingMode::Optimizer,
            tier,
            OBS_LAYOUT_VERSION,
            16,
            0,
            0,
            network,
        );
        checkpoint.schema_version += 1;
        assert!(matches!(
            network_of(checkpoint, TrainingMode::Optimizer),
            Err(InferenceError::Weights(
                WeightError::UnsupportedSchema { .. }
            ))
        ));

        let mut checkpoint = checkpoint_of(TrainingMode::Optimizer);
        checkpoint.obs_layout_version += 1;
        assert!(matches!(
            network_of(checkpoint, TrainingMode::Optimizer),
            Err(InferenceError::Weights(
                WeightError::ObsLayoutMismatch { .. }
            ))
        ));
    }

    #[test]
    fn a_network_with_another_token_width_is_rejected() {
        let network = GemstoneSetAttentionNetwork::new(TIER_DIM + 1, GLOBAL_DIM, 16, 2, 1, 3)
            .expect("valid dimensions");
        match check_network(&network) {
            Err(InferenceError::Weights(WeightError::DimensionMismatch {
                expected, got, ..
            })) => {
                assert_eq!(expected, TIER_DIM);
                assert_eq!(got, TIER_DIM + 1);
            }
            other => panic!("expected a dimension mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_truncated_network_is_rejected_instead_of_panicking() {
        let mut network = ModelTier::Small.init_network(2);
        network.tier_proj.weights.pop();
        assert!(matches!(
            check_network(&network),
            Err(InferenceError::Weights(_))
        ));
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
            tier.init_network(4),
        )
    }

    #[test]
    fn a_checkpoint_serves_its_own_task() {
        for mode in [TrainingMode::Optimizer, TrainingMode::Retarget] {
            assert!(network_of(checkpoint_of(mode), mode).is_ok(), "{mode}");
        }
    }

    #[test]
    fn a_retarget_checkpoint_does_not_serve_the_optimizer() {
        assert!(matches!(
            network_of(
                checkpoint_of(TrainingMode::Retarget),
                TrainingMode::Optimizer
            ),
            Err(InferenceError::Weights(WeightError::ModeMismatch {
                expected: TrainingMode::Optimizer,
                got: TrainingMode::Retarget,
            }))
        ));
    }

    #[test]
    fn an_optimizer_checkpoint_does_not_serve_the_retargeter() {
        match network_of(
            checkpoint_of(TrainingMode::Optimizer),
            TrainingMode::Retarget,
        ) {
            Err(InferenceError::Weights(WeightError::ModeMismatch { expected, got })) => {
                assert_eq!(expected, TrainingMode::Retarget);
                assert_eq!(got, TrainingMode::Optimizer);
            }
            other => panic!("expected a mode mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_checkpoint_of_a_narrow_network_is_rejected_whatever_its_mode() {
        let tier = ModelTier::Small;
        let (d_model, num_heads, num_layers) = tier.config();
        let narrow = GemstoneSetAttentionNetwork::new(
            TIER_DIM,
            GLOBAL_DIM - 1,
            d_model,
            num_heads,
            num_layers,
            4,
        )
        .expect("valid dimensions");
        let checkpoint = Checkpoint::new(
            TrainingMode::Retarget,
            tier,
            OBS_LAYOUT_VERSION,
            16,
            0,
            0,
            narrow,
        );
        assert!(matches!(
            network_of(checkpoint, TrainingMode::Retarget),
            Err(InferenceError::Weights(
                WeightError::DimensionMismatch { .. }
            ))
        ));
    }

    // --- the rollout against the simulator ----------------------------------------------------

    fn sapphire_env(max_steps: usize) -> RetargetEnv {
        let target = MaterialSelection {
            name: Some("Sapphire".to_string()),
            ..MaterialSelection::none()
        }
        .resolve(&BuiltinMaterials);
        RetargetEnv::new(
            Design::concave_fixture(),
            RetargetEnvConfig {
                max_steps,
                ..RetargetEnvConfig::new_with_target(target)
            },
        )
        .expect("the fixture is a closed, movable stone")
    }

    /// A network whose sensitivities are all zero: the composed mean is zero, so every step
    /// proposes a zero turn, which the greedy environment refuses as `NotImproved`.
    fn zero_sensitivity_network() -> GemstoneSetAttentionNetwork {
        let mut network = ModelTier::Small.init_network(3);
        network.sens_head.weights.fill(0.0);
        network.sens_head.biases.fill(0.0);
        network
    }

    const STEPS: usize = 4;

    #[test]
    fn a_fresh_network_stays_inside_its_step_and_evaluation_budget() {
        let mut env = sapphire_env(STEPS);
        let observation = env.build_observation();
        let base = env.base_score();
        let run = run_policy(
            &mut env,
            observation,
            &ModelTier::Small.init_network(7),
            STEPS,
            0.5,
            &DEFAULT_LINE_SEARCH,
            8,
        )
        .expect("the policy runs");
        assert!((1..=STEPS).contains(&run.steps), "{run:?}");
        assert!(
            run.evaluations <= run.steps * DEFAULT_LINE_SEARCH.len(),
            "{run:?}"
        );
        // The greedy environment never lets the stone get worse.
        assert!(env.current_score() <= base);
        assert_eq!(env.step_count(), run.steps);
    }

    #[test]
    fn one_stalled_step_ends_the_run_when_the_limit_is_one() {
        let mut env = sapphire_env(5);
        let observation = env.build_observation();
        let base = env.base_score();
        let run = run_policy(
            &mut env,
            observation,
            &zero_sensitivity_network(),
            5,
            0.5,
            &DEFAULT_LINE_SEARCH,
            1,
        )
        .expect("the policy runs");
        assert_eq!(run.steps, 1);
        assert!(run.evaluations <= DEFAULT_LINE_SEARCH.len(), "{run:?}");
        assert_eq!(env.step_count(), 1);
        assert_eq!(env.current_score(), base);
    }

    #[test]
    fn the_stall_limit_counts_consecutive_steps() {
        let mut env = sapphire_env(6);
        let observation = env.build_observation();
        let run = run_policy(
            &mut env,
            observation,
            &zero_sensitivity_network(),
            6,
            0.5,
            &DEFAULT_LINE_SEARCH,
            3,
        )
        .expect("the policy runs");
        assert_eq!(run.steps, 3);
    }

    #[test]
    fn a_stall_limit_of_zero_runs_every_step() {
        let mut env = sapphire_env(5);
        let observation = env.build_observation();
        let run = run_policy(
            &mut env,
            observation,
            &zero_sensitivity_network(),
            5,
            0.5,
            &DEFAULT_LINE_SEARCH,
            0,
        )
        .expect("the policy runs");
        // The episode truncates at its fifth step.
        assert_eq!(run.steps, 5);
        assert!(run.evaluations <= 5 * DEFAULT_LINE_SEARCH.len(), "{run:?}");
        assert_eq!(env.step_count(), 5);
    }

    #[test]
    fn zero_steps_do_nothing() {
        let mut env = sapphire_env(5);
        let observation = env.build_observation();
        let run = run_policy(
            &mut env,
            observation,
            &ModelTier::Small.init_network(7),
            0,
            0.5,
            &DEFAULT_LINE_SEARCH,
            8,
        )
        .expect("the policy runs");
        assert_eq!(
            run,
            PolicyRun {
                steps: 0,
                evaluations: 0
            }
        );
        assert_eq!(env.step_count(), 0);
    }

    #[test]
    fn a_network_that_rejects_the_observation_is_an_error() {
        let mut env = sapphire_env(5);
        let observation = env.build_observation();
        let wrong = GemstoneSetAttentionNetwork::new(TIER_DIM + 1, GLOBAL_DIM, 16, 2, 1, 3)
            .expect("valid dimensions");
        let result = run_policy(&mut env, observation, &wrong, 5, 0.5, &[1.0], 8);
        assert!(matches!(result, Err(InferenceError::Model(_))));
        assert_eq!(env.step_count(), 0);
    }

    // --- the rollout loop against a script ----------------------------------------------------

    /// What one scripted step reports.
    #[derive(Clone, Copy)]
    struct Outcome {
        rejection: Option<StepRejection>,
        evaluations: u8,
        terminated: bool,
        truncated: bool,
    }

    /// Refused after the full line search: three evaluations, nothing accepted.
    const REFUSED: Outcome = Outcome {
        rejection: Some(StepRejection::NotImproved),
        evaluations: 3,
        terminated: false,
        truncated: false,
    };
    /// Accepted at the first scale: one evaluation.
    const ACCEPTED: Outcome = Outcome {
        rejection: None,
        evaluations: 1,
        terminated: false,
        truncated: false,
    };

    /// An environment that replays a script of step outcomes and records the scales it was
    /// given, so the loop around the simulator is tested without the simulator.
    struct ScriptedEnv {
        observation: GemstoneObservation,
        script: std::vec::IntoIter<Outcome>,
        scales_seen: Vec<Vec<f64>>,
    }

    impl ScriptedEnv {
        fn new(script: Vec<Outcome>) -> Self {
            Self {
                observation: sapphire_env(5).build_observation(),
                script: script.into_iter(),
                scales_seen: Vec::new(),
            }
        }
    }

    impl GemstoneRlEnv for ScriptedEnv {
        type Observation = GemstoneObservation;
        type Action = ContinuousTierAction;

        fn reset(&mut self, _seed: u64) -> GemstoneObservation {
            self.observation.clone()
        }

        fn step(&mut self, action: &ContinuousTierAction) -> Step<GemstoneObservation> {
            self.step_scaled(action, &[1.0])
        }

        fn step_scaled(
            &mut self,
            _action: &ContinuousTierAction,
            scales: &[f64],
        ) -> Step<GemstoneObservation> {
            self.scales_seen.push(scales.to_vec());
            let outcome = self.script.next().expect("the script is long enough");
            Step {
                observation: self.observation.clone(),
                reward: 0.0,
                terminated: outcome.terminated,
                truncated: outcome.truncated,
                info: StepInfo {
                    rejection: outcome.rejection,
                    evaluations: outcome.evaluations,
                    ..StepInfo::default()
                },
            }
        }
    }

    fn run_script(
        script: Vec<Outcome>,
        steps: usize,
        stall_steps: usize,
    ) -> (PolicyRun, ScriptedEnv) {
        let mut env = ScriptedEnv::new(script);
        let observation = env.observation.clone();
        let run = run_policy(
            &mut env,
            observation,
            &ModelTier::Small.init_network(5),
            steps,
            0.5,
            &DEFAULT_LINE_SEARCH,
            stall_steps,
        )
        .expect("the policy runs");
        (run, env)
    }

    #[test]
    fn consecutive_refusals_end_the_run_at_the_stall_limit() {
        let (run, env) = run_script(vec![REFUSED; 10], 10, 3);
        assert_eq!(
            run,
            PolicyRun {
                steps: 3,
                evaluations: 9
            }
        );
        assert_eq!(env.scales_seen.len(), 3);
    }

    #[test]
    fn an_accepted_step_resets_the_stall_count() {
        let script = vec![
            REFUSED, REFUSED, ACCEPTED, REFUSED, REFUSED, ACCEPTED, REFUSED, REFUSED, ACCEPTED,
            REFUSED,
        ];
        // Never three refusals in a row: the whole script runs.
        let (run, _) = run_script(script.clone(), 10, 3);
        assert_eq!(run.steps, 10);
        assert_eq!(run.evaluations, 7 * 3 + 3);
        // Two in a row is reached at the second step.
        let (run, _) = run_script(script, 10, 2);
        assert_eq!(run.steps, 2);
        assert_eq!(run.evaluations, 6);
    }

    #[test]
    fn a_stall_limit_of_zero_never_stops_early() {
        let (run, _) = run_script(vec![REFUSED; 10], 10, 0);
        assert_eq!(run.steps, 10);
        assert_eq!(run.evaluations, 30);
    }

    #[test]
    fn the_step_budget_ends_the_run() {
        let (run, env) = run_script(vec![ACCEPTED; 10], 4, 8);
        assert_eq!(run.steps, 4);
        assert_eq!(run.evaluations, 4);
        assert_eq!(env.scales_seen.len(), 4);
    }

    #[test]
    fn a_terminated_or_truncated_episode_ends_the_run() {
        let terminated = Outcome {
            terminated: true,
            ..ACCEPTED
        };
        let truncated = Outcome {
            truncated: true,
            ..REFUSED
        };
        let (run, _) = run_script(vec![ACCEPTED, ACCEPTED, terminated, ACCEPTED], 10, 8);
        assert_eq!(run.steps, 3);
        assert_eq!(run.evaluations, 3);
        let (run, _) = run_script(vec![REFUSED, truncated, ACCEPTED], 10, 0);
        assert_eq!(run.steps, 2);
        assert_eq!(run.evaluations, 6);
    }

    #[test]
    fn the_evaluations_are_the_sum_of_the_steps_counts() {
        let free_refusal = Outcome {
            evaluations: 0,
            ..REFUSED
        };
        let two = Outcome {
            evaluations: 2,
            ..ACCEPTED
        };
        let (run, _) = run_script(vec![free_refusal, two, REFUSED, ACCEPTED], 4, 0);
        assert_eq!(run.steps, 4);
        assert_eq!(run.evaluations, 6);
    }

    #[test]
    fn every_step_receives_the_line_search() {
        let mut env = ScriptedEnv::new(vec![ACCEPTED; 3]);
        let observation = env.observation.clone();
        let scales = [1.0, 0.8, 0.2];
        let run = run_policy(
            &mut env,
            observation,
            &ModelTier::Small.init_network(5),
            3,
            0.5,
            &scales,
            8,
        )
        .expect("the policy runs");
        assert_eq!(run.steps, 3);
        assert_eq!(env.scales_seen, vec![scales.to_vec(); 3]);
    }
}
