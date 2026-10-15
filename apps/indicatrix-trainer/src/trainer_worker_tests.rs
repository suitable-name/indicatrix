//! Tests of the status line the worker logs.

use super::*;

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
    point.train_cosine = Some(0.71);
    point.labels_this_round = 1024;
    point.labelled_samples = 20_480;
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
    assert!(line.contains("20480 in the aggregate"), "{line}");
    assert!(line.contains("accepted 62%"), "{line}");
    assert!(
        !line.contains("KL") && !line.contains("explained"),
        "{line}"
    );

    point.train_cosine = None;
    assert!(status_line(40, 1.5, 120.4, &point, true).contains("cosine -,"));
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
