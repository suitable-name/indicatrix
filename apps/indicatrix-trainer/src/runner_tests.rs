//! Unit tests of the training runner: configuration, the learning-rate schedule, the rejection
//! and label counters, the target and weight draws, and the runner on the built-in fixture (an
//! imitation round, a PPO round, the three-way validation, the threaded evaluation). The
//! labelling schedule is tested in `runner_teacher.rs`, the labelled aggregate in
//! `runner_imitation.rs`, the objective draws in `runner_objective.rs`, the thread pools in
//! `runner_pool.rs` and the evaluation reduction in `runner_eval.rs`.

use indicatrix_ai::{
    env::{
        DEFAULT_LINE_SEARCH, StepInfo, StepRejection, TeacherLabel, TierSensitivity,
        objective_coefficients, weight_fractions,
    },
    training::{ImitationConfig, LrSchedule, TrainingMetrics},
};
use indicatrix_cut_core::{Design, ObjectivePreset};

use super::{
    config::{
        DEFAULT_VALIDATION_DESIGNS, DEFAULT_VALIDATION_EVERY, MAX_LABEL_EVERY, default_workers,
        linear_schedule_for,
    },
    objective::TRAINABLE_PRESETS,
    types::{Bucket, bucket},
    *,
};
use crate::dataset::MINERAL_SPECTRUM;

#[test]
fn the_linear_schedule_counts_one_update_per_mini_batch_and_epoch() {
    // 200 iterations x 4 epochs x (1024 / 128 = 8 mini-batches).
    assert_eq!(
        linear_schedule_for(200, 1024, 128, 4),
        LrSchedule::Linear {
            total_updates: 6400
        }
    );
    // 300 steps in mini-batches of 128 are 3 batches (the last one short): 100 x 2 x 3.
    assert_eq!(
        linear_schedule_for(100, 300, 128, 2),
        LrSchedule::Linear { total_updates: 600 }
    );
    // An empty run still has a valid (non-zero) schedule.
    assert_eq!(
        linear_schedule_for(0, 128, 64, 4),
        LrSchedule::Linear { total_updates: 1 }
    );
}

#[test]
fn a_resumed_linear_schedule_ends_later_by_the_updates_already_done() {
    assert_eq!(
        resumed_schedule(LrSchedule::Linear { total_updates: 100 }, 50),
        LrSchedule::Linear { total_updates: 150 }
    );
    assert_eq!(
        resumed_schedule(LrSchedule::Constant, 50),
        LrSchedule::Constant
    );
    assert_eq!(
        resumed_schedule(LrSchedule::Linear { total_updates: 10 }, u64::MAX),
        LrSchedule::Linear {
            total_updates: u64::MAX
        }
    );
}

/// A default retarget configuration with `edit` applied.
fn config_with(edit: impl FnOnce(&mut RunConfig)) -> RunConfig {
    let mut cfg = RunConfig::new(TrainingMode::Retarget, ModelTier::Small);
    edit(&mut cfg);
    cfg
}

/// Whether `edit` makes the default configuration invalid.
fn refused(edit: impl FnOnce(&mut RunConfig)) -> bool {
    config_with(edit).validate().is_err()
}

#[test]
fn the_config_defaults() {
    let cfg = config_with(|_| {});
    assert_eq!(cfg.workers, default_workers());
    assert!((1..=MAX_WORKERS).contains(&cfg.workers));
    assert_eq!(cfg.validation_every, DEFAULT_VALIDATION_EVERY);
    assert_eq!(cfg.validation_every, 10);
    assert_eq!(cfg.validation_designs, DEFAULT_VALIDATION_DESIGNS);
    assert_eq!(cfg.max_step_deg, None);
    assert_eq!(cfg.ppo.lr_schedule, LrSchedule::Constant);
    assert_eq!(cfg.algo, Algorithm::Imitation);
    assert_eq!(cfg.imitation, ImitationConfig::default());
    assert_eq!(cfg.label_every, 2);
    assert_eq!(cfg.label_h_deg, None);
    assert_eq!(cfg.label_capacity, 50_000);
    assert_eq!(cfg.explore_std, 0.3);
    assert_eq!(cfg.line_search, DEFAULT_LINE_SEARCH);
    assert_eq!(cfg.objectives, ObjectiveSampling::Mixed);
    assert_eq!(cfg.same_material_frac, 0.25);
    assert!(cfg.validate().is_ok());
    assert_eq!(MAX_WORKERS, 1024);
}

#[test]
fn the_worker_count_is_limited_to_the_documented_range() {
    assert!(refused(|c| c.workers = 0));
    assert!(refused(|c| c.workers = MAX_WORKERS + 1));
    assert!(!refused(|c| c.workers = MAX_WORKERS));
    assert!(!refused(|c| c.workers = 1));
    assert!(refused(|c| c.rollout_size = 0));
    assert!(refused(|c| c.validation_every = 0));
    assert!(refused(|c| c.validation_designs = 0));
}

#[test]
fn the_step_limit_must_be_a_positive_number_of_degrees() {
    for bad in [0.0, -0.5, f64::NAN, f64::INFINITY] {
        assert!(
            refused(|c| c.max_step_deg = Some(bad)),
            "{bad} must be refused"
        );
    }
    assert!(!refused(|c| c.max_step_deg = Some(0.25)));
}

#[test]
fn the_ppo_mini_batch_must_fit_the_rollout_only_in_ppo_mode() {
    let too_big = |c: &mut RunConfig| {
        c.rollout_size = 32;
        c.ppo.mini_batch_size = 64;
    };
    assert!(
        !refused(too_big),
        "imitation does not read the PPO mini-batch"
    );
    assert!(refused(|c| {
        too_big(c);
        c.algo = Algorithm::Ppo;
    }));
    // The imitation mini-batch is capped by the data, not by the rollout.
    assert!(!refused(|c| {
        c.rollout_size = 8;
        c.imitation.mini_batch_size = 256;
    }));
    assert!(refused(|c| c.imitation.mini_batch_size = 0));
    assert!(refused(|c| c.imitation.learning_rate = f32::NAN));
}

#[test]
fn the_label_period_is_limited_and_imitation_needs_labels() {
    assert!(!refused(|c| c.label_every = MAX_LABEL_EVERY));
    assert!(refused(|c| c.label_every = MAX_LABEL_EVERY + 1));
    assert_eq!(MAX_LABEL_EVERY, 64);
    // Without labels an imitation run has nothing to learn from; PPO ignores them.
    assert!(refused(|c| c.label_every = 0));
    assert!(!refused(|c| {
        c.label_every = 0;
        c.algo = Algorithm::Ppo;
    }));
    assert!(refused(|c| c.label_capacity = 0));
    assert!(!refused(|c| c.label_capacity = 1));
}

#[test]
fn the_probe_size_must_be_a_positive_number_of_degrees_when_set() {
    for bad in [0.0, -0.25, f64::NAN, f64::INFINITY] {
        assert!(
            refused(|c| c.label_h_deg = Some(bad)),
            "{bad} must be refused"
        );
    }
    assert!(!refused(|c| c.label_h_deg = Some(0.25)));
    assert!(!refused(|c| c.label_h_deg = None));
}

#[test]
fn the_exploration_noise_must_be_finite_and_not_negative() {
    for bad in [-0.1, f32::NAN, f32::INFINITY] {
        assert!(refused(|c| c.explore_std = bad), "{bad} must be refused");
    }
    assert!(!refused(|c| c.explore_std = 0.0));
    assert!(!refused(|c| c.explore_std = 1.5));
}

#[test]
fn the_line_search_needs_at_least_one_positive_finite_scale() {
    assert!(refused(|c| c.line_search = Vec::new()));
    for bad in [0.0, -0.5, f64::NAN, f64::INFINITY] {
        assert!(
            refused(|c| c.line_search = vec![1.0, bad]),
            "{bad} must be refused"
        );
    }
    assert!(!refused(|c| c.line_search = vec![1.0]));
    assert!(!refused(|c| c.line_search = vec![2.0, 0.5, 0.1, 0.01]));
}

#[test]
fn the_same_material_share_is_a_fraction() {
    for bad in [-0.1, 1.1, f32::NAN] {
        assert!(
            refused(|c| c.same_material_frac = bad),
            "{bad} must be refused"
        );
    }
    assert!(!refused(|c| c.same_material_frac = 0.0));
    assert!(!refused(|c| c.same_material_frac = 1.0));
}

#[test]
fn every_rejection_reason_has_its_bucket() {
    let wrong_length = StepRejection::WrongActionLength {
        expected: 3,
        got: 2,
    };
    for (rejection, want) in [
        (None, Bucket::Accepted),
        (Some(StepRejection::UnsafeAngle), Bucket::Unsafe),
        (Some(StepRejection::NoUsableMast), Bucket::Unsafe),
        (Some(StepRejection::NonFiniteDelta), Bucket::Unsafe),
        (Some(StepRejection::PreservationViolation), Bucket::Unsafe),
        (Some(wrong_length), Bucket::Unsafe),
        (Some(StepRejection::Unsolvable), Bucket::Unsolvable),
        (Some(StepRejection::RelationsUnsolvable), Bucket::Unsolvable),
        (Some(StepRejection::NotClosed), Bucket::NotClosed),
        (Some(StepRejection::NonFiniteScore), Bucket::NotClosed),
        (Some(StepRejection::GirdleViolation), Bucket::Girdle),
        (
            Some(StepRejection::ManufacturabilityRegression),
            Bucket::Manufacturability,
        ),
        (Some(StepRejection::NotImproved), Bucket::NotImproved),
    ] {
        assert_eq!(bucket(rejection), want, "{rejection:?}");
    }
}

#[test]
fn a_rollout_counts_each_step_in_one_bucket() {
    let info = |rejection| StepInfo {
        rejection,
        ..StepInfo::default()
    };
    let mut rollout = RolloutStats::default();
    for step in [
        info(None),
        info(Some(StepRejection::NonFiniteScore)),
        info(Some(StepRejection::GirdleViolation)),
        info(Some(StepRejection::NotImproved)),
        info(Some(StepRejection::NotImproved)),
        info(Some(StepRejection::UnsafeAngle)),
    ] {
        rollout.count_rejection(step.rejection);
    }
    assert_eq!(rollout.accepted, 1);
    assert_eq!(rollout.rej_not_closed, 1);
    assert_eq!(rollout.rej_girdle, 1);
    assert_eq!(rollout.rej_not_improved, 2);
    assert_eq!(rollout.rej_unsafe, 1);
    assert_eq!(rollout.rej_unsolvable + rollout.rej_manufacturability, 0);
    // Every step is in exactly one bucket.
    let counted = rollout.accepted
        + rollout.rej_unsafe
        + rollout.rej_unsolvable
        + rollout.rej_not_closed
        + rollout.rej_girdle
        + rollout.rej_manufacturability
        + rollout.rej_not_improved;
    assert_eq!(counted, 6);
}

/// A label of `evaluations` candidates with one tier of the given score slope.
fn label(score: f32, gain: Option<f32>, evaluations: usize) -> TeacherLabel {
    let tier = TierSensitivity {
        components: [score; 7],
        score,
        forward_probe: true,
    };
    TeacherLabel {
        tiers: vec![Some(tier)],
        oracle_gain: gain,
        oracle_scale: gain.map(|_| 1.0),
        evaluations,
    }
}

#[test]
fn the_oracle_gain_of_a_rollout_is_the_mean_over_its_labels() {
    let mut rollout = RolloutStats::default();
    assert!(rollout.mean_oracle_gain().is_none());
    assert_eq!(rollout.total_evaluations(), 0);

    assert!(rollout.record_label(&label(1.0, Some(2.0), 7)));
    assert!(rollout.record_label(&label(-1.0, Some(4.0), 5)));
    // A label with a slope but no oracle step still teaches, and costs its candidates.
    assert!(rollout.record_label(&label(0.5, None, 3)));
    assert_eq!(rollout.labels, 3);
    assert_eq!(rollout.oracle_gain_n, 2);
    assert!((rollout.mean_oracle_gain().unwrap() - 3.0).abs() < 1e-6);
    assert_eq!(rollout.label_evaluations, 15);
    rollout.evaluations = 10;
    assert_eq!(rollout.total_evaluations(), 25);
}

#[test]
fn a_label_without_anything_to_learn_costs_evaluations_but_is_not_counted() {
    let mut rollout = RolloutStats::default();
    let empty = TeacherLabel {
        tiers: vec![None, None],
        oracle_gain: None,
        oracle_scale: None,
        evaluations: 4,
    };
    assert!(!rollout.record_label(&empty));
    // A gain that is not a number is no gain.
    assert!(!rollout.record_label(&TeacherLabel {
        oracle_gain: Some(f32::NAN),
        ..empty.clone()
    }));
    assert_eq!(rollout.labels, 0);
    assert_eq!(rollout.oracle_gain_n, 0);
    assert!(rollout.mean_oracle_gain().is_none());
    assert_eq!(rollout.label_evaluations, 8);
}

#[test]
fn the_evaluation_thread_count_is_the_core_count_capped_by_the_designs() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    assert_eq!(evaluation_threads(0), 0);
    assert_eq!(evaluation_threads(1), 1);
    for designs in [2, 5, 100, 5000] {
        assert_eq!(evaluation_threads(designs), cores.min(designs));
    }
}

#[test]
fn a_retarget_episode_can_be_judged_in_its_sampled_source_material() {
    let design = Design::concave_fixture();
    let mut cfg = config_with(|c| c.same_material_frac = 1.0);
    for seed in 0..8 {
        let target = pick_target(&cfg, seed, &design)
            .unwrap()
            .expect("a retarget episode has a target");
        let (source_name, source_material, augmentation) = sampled_source_material(seed).unwrap();
        assert_eq!(target.name, source_name);
        assert_eq!(target.material, source_material);
        assert_eq!(target.augmentation, augmentation);
    }

    // Without the share the target is drawn from the spectrum, reproducibly.
    cfg.same_material_frac = 0.0;
    let first = pick_target(&cfg, 3, &design).unwrap().unwrap();
    let again = pick_target(&cfg, 3, &design).unwrap().unwrap();
    assert_eq!(first.name, again.name);
    assert!(
        MINERAL_SPECTRUM
            .iter()
            .any(|entry| entry.name == first.name)
    );

    // With the spectrum off it is the fixed material.
    cfg.universal_spectrum = false;
    cfg.fixed_target = "Topaz".to_owned();
    assert_eq!(
        pick_target(&cfg, 3, &design).unwrap().unwrap().name,
        "Topaz"
    );
    cfg.fixed_target = "Unobtainium".to_owned();
    assert!(matches!(
        pick_target(&cfg, 3, &design),
        Err(RunnerError::Dataset(_))
    ));

    // The optimizer task has no target at all.
    cfg.mode = TrainingMode::Optimizer;
    assert!(pick_target(&cfg, 3, &design).unwrap().is_none());
}

#[test]
fn generalization_gap_requires_matching_training_and_validation_fidelity() {
    let mut runner = fixture_runner_with(|cfg| {
        cfg.train_fidelity = indicatrix_cut_core::ObjectiveFidelity::Fast;
        cfg.validation_fidelity = EvalFidelity::Fast;
    });
    runner.stats.recent_improvements.push_back(0.4);
    runner.stats.last_validation = Some(EvalReport {
        designs: MIN_GAP_DESIGNS,
        mean_improvement: 0.2,
        mean_improvement_sampled: Some(0.1),
        ..EvalReport::default()
    });
    assert!((runner.generalization_gap().unwrap() - 0.3).abs() < 1e-6);

    runner.cfg.validation_fidelity = EvalFidelity::Full;
    assert_eq!(runner.generalization_gap(), None);
}

#[test]
fn the_same_material_share_of_the_draws_follows_the_fraction() {
    let design = Design::concave_fixture();
    let cfg = config_with(|c| c.same_material_frac = 0.25);
    let draws: u64 = 2000;
    let same = (0..draws)
        .filter(|&seed| {
            let source_name = sampled_source_material(seed).unwrap().0;
            pick_target(&cfg, seed, &design)
                .unwrap()
                .is_some_and(|target| target.name == source_name)
        })
        .count();
    let share = same as f64 / draws as f64;
    // The spectrum draw can pick the design's own species by chance, so the share is a little
    // above the fraction.
    assert!((0.2..0.36).contains(&share), "{share}");
}

/// A CPU runner on the built-in fixture, retargeted to sapphire, with `edit` applied to its
/// configuration (one worker unless the edit says otherwise).
pub(super) fn fixture_runner_with(edit: impl FnOnce(&mut RunConfig)) -> TrainingRunner {
    let mut cfg = config_with(|c| {
        c.device = DeviceChoice::Cpu;
        c.workers = 1;
        c.universal_spectrum = false;
        c.fixed_target = "Sapphire".to_owned();
    });
    edit(&mut cfg);
    TrainingRunner::new(cfg, DatasetSplit::fixture()).expect("the fixture builds a CPU runner")
}

#[test]
fn every_episode_draws_its_own_objective_weights_into_its_environment() {
    let runner = fixture_runner_with(|c| {
        c.workers = 4;
        c.objectives = ObjectiveSampling::Presets;
    });
    let presets: Vec<_> = TRAINABLE_PRESETS.iter().map(|p| p.weights()).collect();
    for slot in &runner.slots {
        let weights = *slot.env.weights();
        assert!(presets.contains(&weights), "{weights:?}");
        // The observation carries the same weights as coefficients: the first six are the
        // fractions of the optical, yield and tone weights (the fixture's block preform makes
        // the yield measurable), the seventh is the episode's sampled shape weight (0 or 1).
        let global = &slot.obs.global;
        assert_eq!(global.yield_available, 1.0);
        assert!(
            [0.0, 1.0].contains(&global.shape_weight),
            "{}",
            global.shape_weight
        );
        assert_eq!(
            global.weight_fractions,
            objective_coefficients(&weights, true, global.shape_weight)
        );
        assert_eq!(
            global.weight_fractions[..6],
            weight_fractions(&weights)[..6]
        );
        assert_eq!(global.weight_fractions[6], global.shape_weight);
    }
    assert!(runner.display_weights().is_some());

    let balanced = fixture_runner_with(|c| c.objectives = ObjectiveSampling::Balanced);
    assert_eq!(
        balanced.display_weights().copied(),
        Some(ObjectivePreset::Balanced.weights())
    );
}

#[test]
fn hyperparameters_are_the_imitation_ones_in_imitation_mode() {
    let mut runner = fixture_runner_with(|_| {});
    let before = runner.hyperparams();
    assert_eq!(before.mini_batch, runner.config().imitation.mini_batch_size);
    assert_eq!(
        before.ppo_epochs,
        runner.config().imitation.epochs_per_round
    );
    assert_eq!(
        before.learning_rate,
        runner.config().imitation.learning_rate
    );

    let ppo_before = runner.config().ppo.clone();
    let edited = Hyperparams {
        mini_batch: 32,
        ppo_epochs: 3,
        learning_rate: 1e-3,
        rollout_size: 64,
        clip_epsilon: 0.1,
        ..before
    };
    runner.set_hyperparams(&edited).expect("valid values");
    let cfg = runner.config();
    assert_eq!(cfg.imitation.mini_batch_size, 32);
    assert_eq!(cfg.imitation.epochs_per_round, 3);
    assert_eq!(cfg.imitation.learning_rate, 1e-3);
    assert_eq!(cfg.rollout_size, 64);
    // The PPO settings the GUI has no field for in this mode stay; the two PPO-only fields move.
    assert_eq!(cfg.ppo.mini_batch_size, ppo_before.mini_batch_size);
    assert_eq!(cfg.ppo.epochs_per_batch, ppo_before.epochs_per_batch);
    assert_eq!(cfg.ppo.learning_rate, ppo_before.learning_rate);
    assert_eq!(cfg.ppo.clip_epsilon, 0.1);
    assert_eq!(runner.hyperparams(), edited);

    // A value the imitation trainer refuses changes nothing.
    let bad = Hyperparams {
        mini_batch: 0,
        ..edited.clone()
    };
    assert!(runner.set_hyperparams(&bad).is_err());
    assert_eq!(runner.hyperparams(), edited);
}

#[test]
fn hyperparameters_are_the_ppo_ones_in_ppo_mode() {
    let mut runner = fixture_runner_with(|c| {
        c.algo = Algorithm::Ppo;
        c.rollout_size = 64;
    });
    let before = runner.hyperparams();
    assert_eq!(before.mini_batch, runner.config().ppo.mini_batch_size);
    assert_eq!(before.ppo_epochs, runner.config().ppo.epochs_per_batch);
    assert_eq!(before.learning_rate, runner.config().ppo.learning_rate);

    let imitation_before = runner.config().imitation.clone();
    let edited = Hyperparams {
        mini_batch: 16,
        ppo_epochs: 2,
        learning_rate: 5e-5,
        ..before
    };
    runner.set_hyperparams(&edited).expect("valid values");
    assert_eq!(runner.config().ppo.mini_batch_size, 16);
    assert_eq!(runner.config().ppo.epochs_per_batch, 2);
    assert_eq!(runner.config().ppo.learning_rate, 5e-5);
    assert_eq!(runner.config().imitation, imitation_before);
    assert_eq!(runner.cfg.ppo.mini_batch_size, 16);

    // The PPO mini-batch still has to fit the rollout.
    let too_big = Hyperparams {
        mini_batch: 65,
        ..edited.clone()
    };
    assert!(runner.set_hyperparams(&too_big).is_err());
    assert_eq!(runner.hyperparams(), edited);
}

#[test]
fn an_interrupted_rollout_trains_nothing() {
    let mut runner = fixture_runner_with(|_| {});
    assert!(runner.iterate(&mut || false).unwrap().is_none());
    assert_eq!(runner.stats().iteration, 0);
    assert!(runner.aggregate.is_empty());
}

#[test]
fn one_imitation_round_labels_states_and_trains_on_them() {
    let mut runner = fixture_runner_with(|c| {
        c.workers = 2;
        c.rollout_size = 8;
        c.label_every = 1;
        c.label_capacity = 5;
        c.imitation.mini_batch_size = 8;
    });
    let first = runner
        .iterate(&mut || true)
        .expect("the round runs")
        .expect("it was not interrupted");
    assert_eq!(first.steps, 8);
    assert!(first.labels_this_round > 0, "{first:?}");
    assert!(first.evaluations_this_round > 0);
    assert!(
        first
            .oracle_gain_mean
            .is_none_or(|gain| gain.is_finite() && gain >= 0.0)
    );
    assert!(first.rollout_secs > 0.0);
    assert!(first.label_secs <= first.rollout_secs);
    // The update of iteration 0 trains during rollout 1: nothing to report yet.
    assert_eq!(first.update_of_iteration, None);
    assert!(first.imitation.is_none() && first.update_secs == 0.0);
    assert!(runner.stats().last_imitation.is_none());

    // The second iteration installs the update of the first.
    let report = runner
        .iterate(&mut || true)
        .expect("the round runs")
        .expect("it was not interrupted");
    assert_eq!(report.update_of_iteration, Some(0));
    assert!(report.update_secs > 0.0 && report.update_wait_secs >= 0.0);

    // The update was the imitation one: its metrics are there and finite, the PPO ones zero.
    let metrics = report
        .imitation
        .expect("imitation mode reports its metrics");
    for (name, value) in [
        ("loss", metrics.loss),
        ("sens_mse", metrics.sens_mse),
        ("cosine", metrics.cosine),
        ("gain_loss", metrics.gain_loss),
        ("grad_norm", metrics.grad_norm),
    ] {
        assert!(value.is_finite(), "{name} = {value}");
    }
    assert!(
        metrics.updates > 0 && metrics.labelled_tiers > 0,
        "{metrics:?}"
    );
    assert_eq!(report.train, TrainingMetrics::default());

    // The aggregate holds the labelled states, newest 5 at most.
    let stats = runner.stats();
    assert_eq!(stats.labelled_samples, runner.aggregate.len());
    assert_eq!(
        stats.labelled_samples,
        (first.labels_this_round + report.labels_this_round).min(5)
    );
    assert!(stats.last_imitation.is_some());
    assert_eq!(stats.iteration, 2);
    assert!(
        runner.buffer.is_empty(),
        "imitation does not fill the PPO buffer"
    );

    // The telemetry point carries the round.
    let point = runner.metric_point(10.0);
    assert_eq!(point.labelled_samples, stats.labelled_samples);
    assert_eq!(point.labels_this_round, report.labels_this_round);
    assert_eq!(point.train_cosine, Some(metrics.cosine));
    assert_eq!(point.imitation_loss, metrics.loss);
    assert_eq!(point.evaluations_this_round, report.evaluations_this_round);
    assert_eq!(point.workers, 2);
    assert!(point.val_improvement_oracle.is_none() && point.val_evaluations_per_step.is_none());

    // A restart empties the aggregate along with the statistics.
    runner.reset().expect("the environments rebuild");
    assert!(runner.aggregate.is_empty());
    assert_eq!(runner.stats().labelled_samples, 0);
}

#[test]
fn one_ppo_round_trains_the_policy_and_leaves_the_aggregate_alone() {
    let mut runner = fixture_runner_with(|c| {
        c.algo = Algorithm::Ppo;
        c.workers = 2;
        c.rollout_size = 8;
        c.label_every = 0;
        c.ppo.mini_batch_size = 8;
        c.ppo.epochs_per_batch = 1;
    });
    let report = runner
        .iterate(&mut || true)
        .expect("the round runs")
        .expect("it was not interrupted");
    assert_eq!(report.steps, 8);
    // PPO is on-policy: its update runs on this rollout and is installed before iterate returns.
    assert_eq!(report.update_of_iteration, Some(0));
    assert!(runner.updater.in_flight().is_none());
    assert!(report.imitation.is_none());
    assert_eq!(report.labels_this_round, 0);
    assert!(report.oracle_gain_mean.is_none());
    assert!(report.train.policy_loss.is_finite() && report.train.value_loss.is_finite());
    assert!(runner.aggregate.is_empty());
    assert!(runner.stats().last_imitation.is_none());
    // The telemetry of a PPO round has no imitation figures.
    let point = runner.metric_point(1.0);
    assert!(point.train_cosine.is_none());
    assert_eq!(point.labelled_samples, 0);
}

/// Three 60-step episodes of the fixture at `Fast` fidelity per design and pass (model, sampled,
/// oracle): about 0.26 s per candidate evaluation with the geometry crates optimised in the test
/// profile, so the test takes a few minutes. It is the only check that the passes, the line
/// search and the oracle cache work together on a real episode.
#[test]
fn validation_reports_the_model_the_sampled_and_the_oracle_drop_and_caches_the_oracle() {
    let mut runner = fixture_runner_with(|_| {});
    let first = runner
        .validate(1)
        .expect("the validation runs")
        .expect("the fixture has a validation design");
    assert_eq!((first.designs, first.failed), (1, 0));
    assert!(!first.oracle_cached, "the first pass plays the oracle");
    let oracle = first
        .mean_improvement_oracle
        .expect("the oracle was played");
    let sampled = first
        .mean_improvement_sampled
        .expect("the sampled pass was played");
    for (name, drop) in [
        ("model", first.mean_improvement),
        ("sampled", sampled),
        ("oracle", oracle),
    ] {
        assert!(drop.is_finite() && drop >= 0.0, "{name} drop {drop}");
    }
    assert!(
        first.mean_evaluations_per_step > 0.0 && first.mean_evaluations_per_step <= 3.0,
        "{}",
        first.mean_evaluations_per_step
    );
    assert_eq!(runner.stats().val_oracle, Some(oracle));

    // The second pass takes the oracle from the cache: the very same figure.
    let second = runner.validate(1).unwrap().unwrap();
    assert!(second.oracle_cached);
    assert_eq!(second.mean_improvement_oracle, Some(oracle));
    assert!(second.mean_improvement_sampled.is_some());
    // The model has not changed, so its figures have not either.
    assert_eq!(second.mean_improvement, first.mean_improvement);
    assert_eq!(second.mean_base_score, first.mean_base_score);

    // The telemetry carries the cached oracle drop and the evaluations per step.
    let point = runner.metric_point(1.0);
    assert_eq!(point.val_improvement_oracle, Some(oracle));
    assert_eq!(
        point.val_evaluations_per_step,
        Some(second.mean_evaluations_per_step)
    );

    // The runner kept the oracle drop under the key of what it depends on (the key itself is
    // tested in `runner_eval.rs`).
    let cache = runner
        .oracle_cache
        .as_ref()
        .expect("the oracle drop is cached");
    assert_eq!(cache.drop, oracle);
}

/// Five 60-step episodes of the fixture at `Fast` fidelity: about 0.26 s per step with the
/// geometry crates optimised in the test profile (measured 2026-10-12: one episode 16 s, four
/// copies on four threads 23 s; the runner itself builds in a quarter of a second), so the test
/// takes 75-90 s per binary. It is the only check that the threaded evaluation reports the
/// figures of the serial one. The line search has the single unit scale, as the runs of that
/// measurement had.
#[test]
fn evaluation_gives_the_same_figures_for_one_and_for_four_copies_of_a_design() {
    let runner = fixture_runner_with(|c| c.line_search = vec![1.0]);
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4 * (1.0 + a.abs());
    let sampled = EvalPasses {
        sampled: true,
        oracle: false,
    };

    let one = runner
        .evaluate(&[0], EvalFidelity::Fast, 7, sampled)
        .expect("one design");
    let four = runner
        .evaluate(&[0, 0, 0, 0], EvalFidelity::Fast, 7, sampled)
        .expect("four copies");
    assert_eq!((one.designs, four.designs), (1, 4));
    assert!(near(one.mean_base_score, four.mean_base_score));
    assert!(near(one.mean_final_score, four.mean_final_score));
    assert!(near(one.mean_improvement, four.mean_improvement));
    assert!(near(
        one.mean_improvement_sampled.unwrap(),
        four.mean_improvement_sampled.unwrap()
    ));
    assert!(near(
        one.final_optics.brilliance,
        four.final_optics.brilliance
    ));
    assert!(near(
        one.mean_evaluations_per_step,
        four.mean_evaluations_per_step
    ));

    // The sampled pass does not disturb the deterministic one, and is absent when not asked for.
    let plain = runner
        .evaluate(&[0], EvalFidelity::Fast, 7, EvalPasses::default())
        .expect("without the sampled pass");
    assert!(plain.mean_improvement_sampled.is_none());
    assert!(plain.mean_improvement_oracle.is_none());
    assert!(near(plain.mean_final_score, one.mean_final_score));
}

#[test]
fn evaluating_no_designs_builds_nothing_and_reports_nothing() {
    let runner = fixture_runner_with(|_| {});
    let all = EvalPasses {
        sampled: true,
        oracle: true,
    };
    let report = runner.evaluate(&[], EvalFidelity::Fast, 7, all).unwrap();
    assert_eq!((report.designs, report.failed), (0, 0));
    assert!(report.mean_improvement_oracle.is_none());
    assert_eq!(runner.validation_entries(0).len(), 0);
}
