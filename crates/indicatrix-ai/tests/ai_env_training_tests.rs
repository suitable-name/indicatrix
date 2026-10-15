//! Environment-reward, action, line-search, teacher and rollout-buffer tests (split out of
//! `ai_pipeline_tests.rs`).
//!
//! When `compute_step_reward`, `TrajectoryStep` or `compute_gae_per_worker` change shape, adapt
//! the tests here (the `reward_scale` argument of `compute_gae_per_worker` is `1.0` = raw rewards).

use indicatrix_ai::{
    env::{
        COMPONENT_BRILLIANCE_LOSS, COMPONENT_EXTINCTION, COMPONENT_WINDOWING, COMPONENT_YIELD_LOSS,
        ContinuousTierAction, DEFAULT_LINE_SEARCH, EnvError, GLOBAL_DIM, GemstoneRlEnv, MEAN_BOUND,
        N_COMPONENTS, OBS_LAYOUT_VERSION, OptimizerEnv, OptimizerEnvConfig, RetargetEnv,
        RetargetEnvConfig, RewardConfig, RewardEvaluator, SLOPE_EPS, StepFlags,
        WEIGHT_FRACTION_INDEX, action_from_direction, direction_from_slopes, weight_fractions,
    },
    training::{EpisodeEnd, PpoConfig, RolloutBuffer, TrajectoryStep},
};
use indicatrix_cut_core::{
    BuiltinMaterials, Design, MaterialSelection, ObjectivePreset, ObjectiveWeights,
    ResolvedMaterial,
};

/// Sapphire as the retarget environment judges the fixture.
fn sapphire() -> ResolvedMaterial {
    MaterialSelection {
        name: Some("Sapphire".to_string()),
        ..MaterialSelection::none()
    }
    .resolve(&BuiltinMaterials)
}

/// The concave fixture retargeted to Sapphire under `weights`.
fn retarget_env(weights: ObjectiveWeights) -> Result<RetargetEnv, EnvError> {
    RetargetEnv::new(
        Design::concave_fixture(),
        RetargetEnvConfig {
            weights,
            ..RetargetEnvConfig::new_with_target(sapphire())
        },
    )
}

#[test]
fn test_continuous_action_clamp() {
    let mut action = ContinuousTierAction::new(vec![-2.0, -0.5, 0.0, 0.8, 3.5]);
    action.clamp(1.0);
    assert_eq!(action.deltas_deg, vec![-1.0, -0.5, 0.0, 0.8, 1.0]);
}

#[test]
fn test_reward_evaluator_penalties_and_bonuses() {
    let config = RewardConfig::default();
    let evaluator = RewardEvaluator::new(config.clone());

    // Infeasible geometry penalty
    let ok = StepFlags::ALL_OK;
    let invalid = StepFlags {
        is_valid_solid: false,
        ..ok
    };
    let r_invalid = evaluator.compute_step_reward(50.0, 45.0, invalid, 0.0);
    assert_eq!(r_invalid, config.invalid_geometry_penalty);

    // Safety violation penalty
    let unsafe_flags = StepFlags {
        is_safe_bounds: false,
        ..ok
    };
    let r_unsafe = evaluator.compute_step_reward(50.0, 45.0, unsafe_flags, 0.0);
    assert_eq!(r_unsafe, config.safety_bound_penalty);

    // Valid improvement
    let r_improve = evaluator.compute_step_reward(50.0, 45.0, ok, 0.0);
    // score_delta = +5.0, step_cost = -0.02 => 4.98
    assert!(r_improve > 4.9);

    // The terminal bonus is off by default (0.0) and only counts when configured.
    let best = StepFlags {
        is_new_best: true,
        ..ok
    };
    let r_best_default = evaluator.compute_step_reward(50.0, 45.0, best, 0.0);
    assert_eq!(r_best_default, r_improve);
    let with_bonus = RewardEvaluator::new(RewardConfig {
        new_best_bonus: 5.0,
        ..RewardConfig::default()
    });
    let r_best = with_bonus.compute_step_reward(50.0, 45.0, best, 0.0);
    assert!((r_best - (r_improve + 5.0)).abs() < 1e-6);
}

#[test]
fn test_rollout_buffer_gae() {
    let mut buffer = RolloutBuffer::with_capacity(10);
    for i in 0..5 {
        let end = if i == 4 {
            EpisodeEnd::Terminated
        } else {
            EpisodeEnd::Continue
        };
        buffer.push_worker_step(
            0,
            TrajectoryStep {
                observation: vec![i as f32; 4],
                action: vec![0.1],
                reward: 1.0,
                value: 0.5,
                log_prob: -0.2,
            },
            end,
        );
    }

    assert_eq!(buffer.steps.len(), 5);
    buffer
        .compute_gae_per_worker(&[0.0], 0.99, 0.95, 1.0)
        .expect("gae");

    assert_eq!(buffer.advantages.len(), 5);
    assert_eq!(buffer.returns.len(), 5);

    // Standardized advantages should have mean approx 0
    let mean: f32 = buffer.advantages.iter().sum::<f32>() / 5.0;
    assert!(mean.abs() < 1e-5);
}

#[test]
fn test_ppo_config_defaults() {
    let cfg = PpoConfig::default();
    assert_eq!(cfg.clip_epsilon, 0.2);
    assert_eq!(cfg.gamma, 0.99);
    assert_eq!(cfg.gae_lambda, 0.95);
}

#[test]
fn test_the_teacher_constants_are_the_contract() {
    assert_eq!(N_COMPONENTS, 4);
    assert_eq!(
        [
            COMPONENT_WINDOWING,
            COMPONENT_EXTINCTION,
            COMPONENT_BRILLIANCE_LOSS,
            COMPONENT_YIELD_LOSS
        ],
        [0, 1, 2, 3]
    );
    assert_eq!(DEFAULT_LINE_SEARCH, [1.0, 0.5, 0.25]);
    assert_eq!(SLOPE_EPS, 1e-6);
    // The steepest tier of a composed direction moves 95 % of the step.
    assert!((MEAN_BOUND.tanh() - 0.95).abs() < 1e-6);
}

#[test]
fn test_the_observation_layout_ends_with_the_weight_fractions() {
    assert_eq!(OBS_LAYOUT_VERSION, 4);
    assert_eq!(GLOBAL_DIM, 19);
    assert_eq!(WEIGHT_FRACTION_INDEX, 15);
    assert_eq!(WEIGHT_FRACTION_INDEX + N_COMPONENTS, GLOBAL_DIM);

    // Fractions are the weights over their sum.
    let brilliance = weight_fractions(&ObjectivePreset::Brilliance.weights());
    for (got, want) in brilliance
        .iter()
        .zip([1.0 / 6.0, 1.0 / 6.0, 4.0 / 6.0, 0.0])
    {
        assert!((got - want).abs() < 1e-6, "{brilliance:?}");
    }
    // A weighting without any positive weight falls back to the three optical terms equally.
    let silent = weight_fractions(&ObjectiveWeights {
        windowing: 0.0,
        extinction: 0.0,
        tilt_brilliance: 0.0,
        yield_weight: 0.0,
        ..ObjectiveWeights::default()
    });
    for (got, want) in silent.iter().zip([1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0]) {
        assert!((got - want).abs() < 1e-6, "{silent:?}");
    }

    // The environment puts them in the last four global entries.
    let env = retarget_env(ObjectivePreset::KeepWeight.weights()).expect("the fixture retargets");
    let token = env.build_observation().global_token();
    assert_eq!(token.len(), GLOBAL_DIM);
    assert_eq!(
        token[WEIGHT_FRACTION_INDEX..],
        weight_fractions(env.weights())
    );
}

#[test]
fn test_the_environments_refuse_a_face_up_tone_weight() {
    for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
        assert!(
            matches!(
                retarget_env(preset.weights()),
                Err(EnvError::InvalidConfig(_))
            ),
            "{preset:?} (retarget)"
        );
        assert!(
            matches!(
                OptimizerEnv::new(
                    Design::concave_fixture(),
                    OptimizerEnvConfig {
                        weights: preset.weights(),
                        ..OptimizerEnvConfig::default()
                    },
                ),
                Err(EnvError::InvalidConfig(_))
            ),
            "{preset:?} (optimizer)"
        );
    }
    // Every other preset builds.
    for preset in [
        ObjectivePreset::Balanced,
        ObjectivePreset::Brilliance,
        ObjectivePreset::LowWindowing,
        ObjectivePreset::LowExtinction,
        ObjectivePreset::KeepWeight,
    ] {
        assert!(retarget_env(preset.weights()).is_ok(), "{preset:?}");
    }
}

#[test]
fn test_a_line_search_step_counts_once_and_reports_its_evaluations() {
    let mut env = retarget_env(ObjectiveWeights::default()).expect("the fixture retargets");
    let base = env.base_score();
    let tiers = env.base_design().tiers.len();
    let step = env.step_scaled(
        &ContinuousTierAction::new(vec![-0.4; tiers]),
        &DEFAULT_LINE_SEARCH,
    );
    // One step whatever the number of scales; the candidates scored are one to three.
    assert_eq!(env.step_count(), 1);
    assert_eq!(step.info.episode_step, 1);
    assert!((1..=3).contains(&step.info.evaluations), "{:?}", step.info);
    // The accepted scale is reported exactly when a candidate was accepted.
    assert_eq!(step.info.scale.is_some(), step.info.rejection.is_none());
    if let Some(scale) = step.info.scale {
        assert!(
            DEFAULT_LINE_SEARCH
                .iter()
                .any(|&known| known as f32 == scale)
        );
    }
    // The greedy environment never gets worse.
    assert!(env.current_score() <= base);
}

#[test]
fn test_the_teacher_label_measures_the_free_tiers_and_changes_nothing() {
    let env = retarget_env(ObjectiveWeights::default()).expect("the fixture retargets");
    let tiers = env.base_design().tiers.len();
    let base = env.base_score();

    let label = env.teacher_label(0.25, &DEFAULT_LINE_SEARCH);
    assert_eq!(label.tiers.len(), tiers);
    // One measurement per free tier, none for the tiers the environment does not move.
    assert_eq!(
        label.tiers.iter().flatten().count(),
        env.movable_tier_indices().len()
    );
    for sensitivity in label.tiers.iter().flatten() {
        assert!(sensitivity.score.is_finite());
        assert!(sensitivity.components.iter().all(|slope| slope.is_finite()));
    }
    assert!(label.evaluations >= env.movable_tier_indices().len());
    let gain = label.oracle_gain.expect("the fixture has slopes");
    assert!(gain.is_finite() && gain >= 0.0, "{gain}");

    // Pure: no step counted, no state change.
    assert_eq!(env.step_count(), 0);
    assert_eq!(env.current_score(), base);

    // The oracle's direction is bounded like the model's composed mean.
    let slopes: Vec<Option<f32>> = label
        .tiers
        .iter()
        .map(|tier| tier.map(|sensitivity| sensitivity.score))
        .collect();
    let direction = direction_from_slopes(&slopes);
    assert_eq!(direction.len(), tiers);
    assert!(direction.iter().all(|d| d.abs() <= MEAN_BOUND));
    let action = action_from_direction(&direction, 0.5);
    assert_eq!(action.deltas_deg.len(), tiers);
    assert!(action.deltas_deg.iter().all(|d| d.abs() <= 0.475 + 1e-6));
}
