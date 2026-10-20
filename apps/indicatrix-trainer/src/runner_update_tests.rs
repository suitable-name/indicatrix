//! Tests of the overlap of rollout and update (`runner_update.rs`): which weights a rollout acts
//! with, that the timing of an install changes nothing, cancellation, the sync points and the
//! equality of the thread's PPO update with a direct call. They run on the built-in fixture with
//! the CPU trainer.

use super::{tests::fixture_runner_with, update::UpdateJob, *};

/// A small imitation runner: one worker, 8 steps a rollout, every state labelled.
fn small_imitation_runner() -> TrainingRunner {
    fixture_runner_with(|c| {
        c.rollout_size = 8;
        c.label_every = 1;
        c.imitation.mini_batch_size = 8;
    })
}

/// Asserts that the rollout copy of the model of `runner` holds exactly the weights of `net`:
/// it gives the very numbers a reference model built from `net` on the same device gives, and
/// that is what a rollout acts with.
fn assert_rollout_acts_with(runner: &TrainingRunner, net: &GemstoneSetAttentionNetwork) {
    let (mut reference, _, _) = build_trainer(
        &runner.cfg.ppo,
        runner.cfg.tier,
        runner.split.max_tiers,
        DeviceChoice::Cpu,
        runner.cfg.seed,
    )
    .expect("a CPU trainer builds");
    reference.load_weights(net).expect("the shapes agree");
    let reference = reference.rollout_model();
    let states: Vec<Vec<f32>> = runner
        .slots
        .iter()
        .map(|slot| slot.obs.try_flatten(runner.split.max_tiers).unwrap())
        .collect();
    let n_tiers: Vec<usize> = runner.slots.iter().map(|s| s.obs.tiers.len()).collect();
    let mine = runner
        .rollout_model
        .forward_host(&states, &n_tiers)
        .unwrap();
    let theirs = reference.forward_host(&states, &n_tiers).unwrap();
    assert_eq!(mine.means, theirs.means);
    assert_eq!(mine.log_stds, theirs.log_stds);
    assert_eq!(mine.values, theirs.values);
}

fn iterate_once(runner: &mut TrainingRunner) -> IterationReport {
    runner
        .iterate(&mut || true)
        .expect("the round runs")
        .expect("it was not interrupted")
}

#[test]
fn a_rollout_acts_with_the_weights_of_the_update_before_the_last() {
    let mut runner = small_imitation_runner();
    let initial = runner.net.clone();
    assert_rollout_acts_with(&runner, &initial);

    // Iteration 0: update 0 is in flight, nothing is installed, rollout 1 still has the
    // initial weights.
    let first = iterate_once(&mut runner);
    assert_eq!(first.update_of_iteration, None);
    assert_eq!(runner.updater.in_flight(), Some(0));
    assert_eq!(runner.net, initial);
    assert_rollout_acts_with(&runner, &initial);

    // Iteration 1: update 0 is installed at the end of rollout 1 (which acted with the initial
    // weights), update 1 starts, rollout 2 will act with the weights of update 0.
    let second = iterate_once(&mut runner);
    assert_eq!(second.update_of_iteration, Some(0));
    assert_eq!(runner.updater.in_flight(), Some(1));
    let after_first = runner.net.clone();
    assert_ne!(after_first, initial, "update 0 moved the weights");
    assert_rollout_acts_with(&runner, &after_first);

    // Somebody syncs (a validation, a checkpoint): the network moves on to update 1, the rollout
    // copy does not, so the sequence of rollouts does not depend on when anybody looks.
    runner.sync_update().expect("the update finishes");
    assert!(runner.updater.in_flight().is_none());
    let after_second = runner.net.clone();
    assert_ne!(after_second, after_first, "update 1 moved the weights");
    assert_rollout_acts_with(&runner, &after_first);

    // The next wait point points the rollout copy at the installed weights, and the figures of
    // the update that was installed by the sync reach the report.
    let third = iterate_once(&mut runner);
    assert_eq!(third.update_of_iteration, Some(1));
    assert_rollout_acts_with(&runner, &after_second);
}

#[test]
fn the_weights_do_not_depend_on_when_the_update_is_installed() {
    // One run lets the updates finish on their own schedule; the other waits for every update
    // right after its iteration. Same rollouts, same updates, same weights.
    let mut free = small_imitation_runner();
    let mut waiting = small_imitation_runner();
    for _ in 0..3 {
        iterate_once(&mut free);
        iterate_once(&mut waiting);
        waiting.sync_update().expect("the update finishes");
        assert!(waiting.updater.in_flight().is_none());
    }
    free.sync_update().expect("the update finishes");
    assert_eq!(free.net, waiting.net);
    assert_eq!(free.stats().total_steps, waiting.stats().total_steps);
    assert_eq!(free.aggregate.len(), waiting.aggregate.len());
}

#[test]
fn cancelling_a_rollout_installs_the_update_in_flight() {
    let mut runner = small_imitation_runner();
    let initial = runner.net.clone();
    iterate_once(&mut runner);
    assert_eq!(runner.updater.in_flight(), Some(0));

    assert!(runner.iterate(&mut || false).unwrap().is_none());
    assert!(runner.updater.in_flight().is_none());
    assert_ne!(runner.net, initial, "update 0 was installed, not abandoned");
    let installed = runner.net.clone();
    assert_rollout_acts_with(&runner, &installed);
    assert_eq!(
        runner.stats().iteration,
        1,
        "the cancelled rollout counts for nothing"
    );

    // Its figures arrive with the next report.
    let next = iterate_once(&mut runner);
    assert_eq!(next.update_of_iteration, Some(0));
    assert!(next.imitation.is_some());
}

#[test]
fn validation_waits_for_the_update_in_flight() {
    let mut runner = small_imitation_runner();
    let initial = runner.net.clone();
    iterate_once(&mut runner);
    assert_eq!(runner.updater.in_flight(), Some(0));
    // No designs to play: the call only has to install the update.
    assert!(runner.validate(0).unwrap().is_none());
    assert!(runner.updater.in_flight().is_none());
    assert_ne!(runner.net, initial);
}

#[test]
fn a_restart_keeps_the_weights_of_the_update_in_flight() {
    let mut runner = small_imitation_runner();
    let initial = runner.net.clone();
    iterate_once(&mut runner);
    runner.reset().expect("the environments rebuild");
    assert!(runner.updater.in_flight().is_none());
    assert_ne!(runner.net, initial);
    let installed = runner.net.clone();
    assert_rollout_acts_with(&runner, &installed);
    // The figures belong to the statistics that were reset.
    assert!(runner.installed_update.is_none());
    assert_eq!(runner.stats().iteration, 0);
    let after = iterate_once(&mut runner);
    assert_eq!(after.update_of_iteration, None);
}

#[test]
fn a_ppo_update_on_the_thread_is_the_update_a_direct_call_makes() {
    let mut runner = fixture_runner_with(|c| {
        c.algo = Algorithm::Ppo;
        c.workers = 2;
        c.rollout_size = 8;
        c.label_every = 0;
        c.ppo.mini_batch_size = 8;
        c.ppo.epochs_per_batch = 1;
    });
    let initial = runner.net.clone();
    runner
        .collect_rollout(&mut || true)
        .expect("the rollout runs")
        .expect("it was not interrupted");
    let UpdateJob::Ppo(batch) = runner.ppo_job().expect("the batch builds") else {
        panic!("a PPO run builds a PPO job");
    };

    // The serial path: a fresh trainer like the thread's, the same batch, a direct call.
    let (mut direct, _, _) = build_trainer(
        &runner.cfg.ppo,
        runner.cfg.tier,
        runner.split.max_tiers,
        DeviceChoice::Cpu,
        runner.cfg.seed,
    )
    .expect("a CPU trainer builds");
    direct.load_weights(&initial).expect("the shapes agree");
    let mut expected = direct
        .train_rollout_epochs(&batch.as_batch())
        .expect("the direct update runs");
    let expected_net = direct.model.export_to_set_attention().unwrap();

    runner.updater.start(0, UpdateJob::Ppo(batch)).unwrap();
    runner.sync_update().unwrap();
    assert_eq!(runner.net, expected_net);
    assert_ne!(runner.net, initial);
    let installed = runner
        .installed_update
        .take()
        .expect("the update was installed");
    assert!(installed.train.packing_ms.is_finite() && installed.train.packing_ms >= 0.0);
    assert!(installed.train.update_ms.is_finite() && installed.train.update_ms >= 0.0);
    expected.packing_ms = 0.0;
    expected.update_ms = 0.0;
    let mut installed_metrics = installed.train;
    installed_metrics.packing_ms = 0.0;
    installed_metrics.update_ms = 0.0;
    assert_eq!(installed_metrics, expected);
}

#[test]
fn synchronized_metrics_show_pending_update_then_iteration_reports_it_once() {
    let mut runner = small_imitation_runner();
    let _ = iterate_once(&mut runner);
    runner.stats.last_train = indicatrix_ai::training::TrainingMetrics {
        policy_loss: -1.0,
        ..Default::default()
    };
    runner.stats.last_imitation = Some(indicatrix_ai::training::ImitationMetrics {
        loss: -2.0,
        ..Default::default()
    });
    runner.sync_update().unwrap();
    let installed = runner
        .installed_update
        .as_ref()
        .expect("sync installed the completed update");
    let update_id = installed.iteration;
    let train = installed.train.clone();
    let imitation = installed
        .imitation
        .expect("the imitation update has metrics");

    let point = runner.metric_point(0.0);
    assert_eq!(point.update_of_iteration, Some(update_id));
    assert_eq!(point.policy_loss, train.policy_loss);
    assert_eq!(point.imitation_loss, imitation.loss);
    assert_eq!(point.sens_mse, imitation.sens_mse);
    assert_eq!(point.component_support, imitation.component_support);
    assert_eq!(point.update_secs, installed.secs);
    assert_eq!(point.weight_export_ms, installed.export_secs * 1_000.0);
    assert_eq!(point.packing_ms, imitation.packing_ms);
    assert_eq!(point.training_host_ms, imitation.update_ms);

    let report = iterate_once(&mut runner);
    assert_eq!(report.update_of_iteration, Some(update_id));
    assert_eq!(report.train, train);
    assert_eq!(report.imitation, Some(imitation));
    assert_eq!(
        runner.metric_point(0.0).update_of_iteration,
        Some(update_id)
    );
}
