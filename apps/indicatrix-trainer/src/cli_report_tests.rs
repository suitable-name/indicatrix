//! Tests of the phrases and lines the command-line trainer prints.

use indicatrix_ai::env::DEFAULT_LINE_SEARCH;

use super::*;
use crate::cli_args::{Parsed, build_config, parse_args};

/// The run configuration the flags build.
fn config_for(flags: &[&str]) -> RunConfig {
    let args: Vec<String> = flags.iter().map(|s| (*s).to_owned()).collect();
    let Ok(Parsed::Run(parsed)) = parse_args(&args) else {
        panic!("{flags:?} must parse");
    };
    build_config(&parsed)
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
fn the_schedule_is_described_for_the_banner() {
    assert_eq!(describe_schedule(LrSchedule::Constant), "constant");
    let text = describe_schedule(LrSchedule::Linear {
        total_updates: 6400,
    });
    assert!(text.contains("linear") && text.contains("6400"), "{text}");
}

#[test]
fn the_workers_are_described_with_the_core_count() {
    assert_eq!(describe_workers(16, 32), "16 (cores 32)");
    assert_eq!(describe_workers(1, 1), "1 (cores 1)");
    assert!(core_count() >= 1);
}

#[test]
fn the_line_search_is_described_for_the_banner() {
    assert_eq!(describe_line_search(&[1.0, 0.5, 0.25]), "1 / 0.5 / 0.25");
    assert_eq!(describe_line_search(&DEFAULT_LINE_SEARCH), "1 / 0.5 / 0.25");
    assert_eq!(describe_line_search(&[1.0]), "1");
    assert_eq!(describe_line_search(&[]), "");
}

#[test]
fn the_label_rule_is_described_for_the_banner() {
    let config = config_for(&[]);
    assert_eq!(
        describe_labels(&config, 0.5),
        "1 in 2 rollout samples, probe h = 0.50 deg, aggregate of the newest 50000"
    );
    let config = config_for(&["--label-every", "4", "--label-h", "1"]);
    assert_eq!(
        describe_labels(&config, 0.5),
        "1 in 4 rollout samples, probe h = 1.00 deg, aggregate of the newest 50000"
    );
    let config = config_for(&["--algo", "ppo", "--label-every", "3"]);
    assert_eq!(
        describe_labels(&config, 0.25),
        "1 in 3 rollout samples, probe h = 0.25 deg (telemetry only, the PPO loss ignores them)"
    );
    let config = config_for(&["--algo", "ppo"]);
    assert_eq!(describe_labels(&config, 0.5), "off");
}

#[test]
fn the_algorithm_and_the_objectives_are_described_for_the_banner() {
    assert!(describe_algorithm(Algorithm::Imitation).starts_with("imitation"));
    assert!(describe_algorithm(Algorithm::Ppo).starts_with("ppo"));
    assert!(describe_objectives(ObjectiveSampling::Balanced).starts_with("balanced"));
    assert!(describe_objectives(ObjectiveSampling::Presets).starts_with("presets"));
    assert!(describe_objectives(ObjectiveSampling::Mixed).starts_with("mixed"));
}

#[test]
fn the_imitation_progress_line_shows_loss_cosine_and_labels_instead_of_the_critic() {
    let mut point = blank_point();
    point.imitation_loss = 0.123;
    point.train_cosine = Some(0.71);
    point.labelled_samples = 4096;
    point.explained_variance = Some(0.97);
    let line = progress_line(&point, true, 5, 10, &[0.5, 1.0]);
    assert!(line.contains("loss 0.123"), "{line}");
    assert!(line.contains("cos +0.71"), "{line}");
    assert!(line.contains("labels 4096"), "{line}");
    assert!(!line.contains("ev "), "{line}");
    assert!(line.contains(" 50.0%"), "{line}");
    assert!(line.contains("iter    5/10"), "{line}");
    assert!(line.contains("150 steps/s"), "{line}");
    assert!(line.is_ascii(), "{line}");
    assert!(line.starts_with("[##########----------]  50.0%"), "{line}");

    point.train_cosine = None;
    assert!(progress_line(&point, true, 5, 10, &[]).contains("cos -"));
}

#[test]
fn the_ppo_progress_line_keeps_the_explained_variance() {
    let mut point = blank_point();
    point.imitation_loss = 0.123;
    point.explained_variance = Some(0.97);
    let line = progress_line(&point, false, 10, 10, &[]);
    assert!(line.contains("ev +0.97"), "{line}");
    assert!(!line.contains("loss") && !line.contains("labels"), "{line}");
    assert!(line.contains("100.0%"), "{line}");
    point.explained_variance = None;
    assert!(progress_line(&point, false, 10, 10, &[]).contains("ev -"));
}

#[test]
fn the_progress_line_shows_the_evaluation_rate_and_the_model_oracle_ratio() {
    let mut point = blank_point();
    let line = progress_line(&point, true, 1, 10, &[]);
    assert!(line.contains("- evals/s"), "{line}");
    assert!(!line.contains("model/oracle"), "{line}");

    point.evaluations_this_round = 9000;
    point.rollout_secs = 8.0;
    point.update_secs = 1.0;
    point.val_improvement = Some(3.0);
    point.val_improvement_oracle = Some(6.0);
    let line = progress_line(&point, true, 1, 10, &[]);
    assert!(line.contains("1000 evals/s"), "{line}");
    assert!(line.contains("model/oracle 0.50"), "{line}");

    // Without an oracle drop there is nothing to divide by.
    point.val_improvement_oracle = None;
    assert!(!progress_line(&point, true, 1, 10, &[]).contains("model/oracle"));
    // A PPO line carries the ratio too once both drops exist.
    point.val_improvement_oracle = Some(6.0);
    assert!(progress_line(&point, false, 1, 10, &[]).contains("model/oracle 0.50"));
}

#[test]
fn the_phase_seconds_are_averaged_over_the_iterations() {
    assert_eq!(mean_phase_seconds(&[]), None);
    let mut a = blank_point();
    a.rollout_secs = 10.0;
    a.label_secs = 6.0;
    a.update_secs = 1.0;
    let mut b = blank_point();
    b.rollout_secs = 14.0;
    b.label_secs = 8.0;
    b.update_secs = 3.0;
    assert_eq!(mean_phase_seconds(&[a, b]), Some((12.0, 7.0, 2.0)));
}

#[test]
fn the_evaluation_rate_of_a_run_sums_the_rounds() {
    assert_eq!(overall_evaluations_per_sec(&[]), None);
    let mut a = blank_point();
    a.evaluations_this_round = 6000;
    a.rollout_secs = 5.0;
    a.update_secs = 1.0;
    let mut b = blank_point();
    b.evaluations_this_round = 3000;
    b.rollout_secs = 2.0;
    b.update_secs = 1.0;
    let rate = overall_evaluations_per_sec(&[a, b]).unwrap();
    assert!((rate - 1000.0).abs() < 1e-3, "{rate}");
    // Points without recorded time or evaluations (old files) give no rate.
    assert_eq!(overall_evaluations_per_sec(&[blank_point()]), None);
}

#[test]
fn sparkline_scales_to_the_window() {
    assert_eq!(render_sparkline(&[], 4), "-");
    let line = render_sparkline(&[0.0, 1.0], 4);
    assert_eq!(line.chars().count(), 2);
    assert_eq!(line, "_#");
    // Plain ASCII only: block glyphs are garbage on terminals without a Unicode charset (PuTTY).
    assert!(render_sparkline(&[3.0, 1.0, 2.0, 5.0], 8).is_ascii());
    assert_eq!(
        render_sparkline(&[1.0, 2.0, 3.0, 4.0, 5.0], 3)
            .chars()
            .count(),
        3
    );
}
