//! Tests of the status line the worker logs.

use indicatrix_ai::training::TrainingMetrics;

use super::*;

/// A report with the update figures given and everything else blank.
fn blank_report(update_of_iteration: Option<usize>) -> IterationReport {
    IterationReport {
        train: TrainingMetrics::default(),
        mean_step_reward: 0.0,
        steps: 0,
        new_best: None,
        new_best_improvement: None,
        explained_variance: None,
        accepted_frac: 0.0,
        imitation: None,
        labels_this_round: 0,
        oracle_gain_mean: None,
        rollout_secs: 0.0,
        label_secs: 0.0,
        update_secs: 4.26,
        packing_ms: 0.0,
        training_host_ms: 0.0,
        weight_export_ms: 0.0,
        rollout_forward_ms: 0.0,
        update_wait_secs: 0.04,
        update_of_iteration,
        evaluations_this_round: 0,
    }
}

#[test]
fn the_update_note_names_the_iteration_the_update_belongs_to_and_the_wait() {
    assert_eq!(
        update_note(&blank_report(Some(39))),
        ", update of iteration #39 took 4.3s (waited 0.0s)"
    );
    // The first iteration of a run has installed no update yet.
    assert_eq!(update_note(&blank_report(None)), "");
}

/// A metric point with the required fields only; everything else takes its serde default.
fn blank_point() -> IterationMetricPoint {
    serde_json::from_str(
        r#"{"iteration":3,"episode":7,"total_steps":300,"steps_per_sec":150.0,
        "mean_step_reward":-4.5,"policy_loss":0.0,"value_loss":0.0,"entropy":0.0,
        "clip_fraction":0.0,"solid_closed":true,"brilliance":50.0,"windowing":3.0,
        "extinction":4.0,"yield_pct":60.0}"#,
    )
    .unwrap()
}

#[test]
fn the_imitation_status_line_names_loss_cosine_and_labels() {
    let mut point = blank_point();
    point.imitation_loss = 0.123;
    point.update_of_iteration = Some(39);
    point.train_cosine = Some(0.71);
    point.labels_this_round = 1024;
    point.labelled_samples = 20_480;
    point.component_support = [3, 2, 3, 3, 0, 0, 0];
    point.accepted_frac = 0.62;
    point.approx_kl = 0.5;
    let line = status_line(40, 1.5, 120.4, &point, true);
    assert!(
        line.starts_with("Iter #40: step reward +1.500, 120 steps/s"),
        "{line}"
    );
    assert!(line.contains("loss 0.123"), "{line}");
    assert!(line.contains("cosine +0.71"), "{line}");
    assert!(line.contains("1024 labels this round"), "{line}");
    assert!(
        line.contains("20480 in the aggregate; support 3/2/3/3/0/0/0"),
        "{line}"
    );
    assert!(line.contains("accepted 62%"), "{line}");
    assert!(
        !line.contains("KL") && !line.contains("explained"),
        "{line}"
    );

    point.train_cosine = None;
    assert!(status_line(40, 1.5, 120.4, &point, true).contains("cosine -,"));
}

#[test]
fn the_first_imitation_iteration_shows_no_loss_yet() {
    let mut point = blank_point();
    point.labelled_samples = 512;
    let line = status_line(0, 0.0, 10.0, &point, true);
    assert!(line.contains("loss -,"), "{line}");
    // No update yet: the aggregate is counted, the per-component support is still zero.
    assert!(
        line.contains("512 in the aggregate; support 0/0/0/0/0/0/0"),
        "{line}"
    );
    point.update_secs = 2.0;
    assert!(status_line(1, 0.0, 10.0, &point, true).contains("loss 0.000,"));
}

#[test]
fn the_database_status_names_updates_per_round_or_the_epochs() {
    use commands::algorithm_status;

    let sizes = [16, 2048, 256, 2];
    let bounded = algorithm_status(Algorithm::Imitation, sizes, 64, "GPU");
    assert!(
        bounded.contains("64 updates per round") && !bounded.contains("epochs"),
        "{bounded}"
    );
    assert!(bounded.contains("mini-batch 256"), "{bounded}");
    let epochs = algorithm_status(Algorithm::Imitation, sizes, 0, "GPU");
    assert!(
        epochs.contains("2 epochs") && !epochs.contains("updates per round"),
        "{epochs}"
    );
    // PPO has no updates per round: its epochs always show.
    let ppo = algorithm_status(Algorithm::Ppo, [16, 2048, 64, 4], 64, "GPU");
    assert!(
        ppo.contains("4 epochs") && !ppo.contains("updates per round"),
        "{ppo}"
    );
    assert!(ppo.contains("pool: 16 parallel envs"), "{ppo}");
}

#[test]
fn the_ppo_status_line_keeps_kl_and_explained_variance() {
    let mut point = blank_point();
    point.approx_kl = 0.0123;
    point.explained_variance = Some(0.97);
    point.accepted_frac = 0.5;
    let line = status_line(20, -0.25, 80.0, &point, false);
    assert!(line.contains("KL 0.0123,"), "{line}");
    assert!(line.contains("accepted 50%"), "{line}");
    assert!(line.contains("explained variance +0.97"), "{line}");
    assert!(
        !line.contains("cosine") && !line.contains("labels"),
        "{line}"
    );

    point.early_stopped = true;
    point.explained_variance = None;
    let line = status_line(20, -0.25, 80.0, &point, false);
    assert!(line.contains("KL 0.0123 (stopped early)"), "{line}");
    assert!(line.ends_with("explained variance -"), "{line}");
}

#[test]
fn the_status_line_shows_the_evaluation_rate_once_it_is_known() {
    let mut point = blank_point();
    assert!(!status_line(20, 0.0, 80.0, &point, true).contains("evals/s"));
    point.evaluations_this_round = 9000;
    point.rollout_secs = 8.0;
    point.update_secs = 1.0;
    assert!(status_line(20, 0.0, 80.0, &point, true).contains(", 1000 evals/s"));
    assert!(status_line(20, 0.0, 80.0, &point, false).contains(", 1000 evals/s"));
}
