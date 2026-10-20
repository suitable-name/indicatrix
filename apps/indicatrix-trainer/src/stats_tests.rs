use super::*;

#[test]
fn rfc3339_known_instants() {
    assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339_from_unix(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339_from_unix(1_700_000_000), "2023-11-14T22:13:20Z");
}

#[test]
fn major_version_is_compared() {
    assert_eq!(major("0.7.2"), "0");
    assert_eq!(major("1.0.0"), "1");
}

/// A metric point as the first two training runs wrote it (before the diagnostic fields).
const OLD_POINT: &str = r#"{"iteration":3,"episode":7,"total_steps":300,
    "steps_per_sec":12.5,"mean_return":-4.5,"best_score":2.0,"policy_loss":0.1,
    "value_loss":2.0,"entropy":1.0,"clip_fraction":0.2,"approx_kl":0.01,
    "solid_closed":true,"brilliance":50.0,"windowing":3.0,"extinction":4.0,
    "yield_pct":60.0}"#;

#[test]
fn a_metric_point_from_before_the_diagnostic_fields_still_loads() {
    let point: IterationMetricPoint = serde_json::from_str(OLD_POINT).unwrap();
    assert_eq!(point.iteration, 3);
    assert!(
        (point.mean_step_reward + 4.5).abs() < 1e-6,
        "alias mean_return"
    );
    assert!(point.explained_variance.is_none());
    assert!(point.mean_improvement.is_none());
    assert!(point.val_improvement.is_none() && point.val_improvement_sampled.is_none());
    assert_eq!(point.val_designs, 0);
    assert_eq!(point.rej_not_improved, 0);
    assert!(!point.early_stopped);
}

#[test]
fn the_imitation_and_utilisation_fields_default_in_old_files() {
    let point: IterationMetricPoint = serde_json::from_str(OLD_POINT).unwrap();
    assert_eq!(point.imitation_loss, 0.0);
    assert_eq!(point.sens_mse, 0.0);
    assert_eq!(point.gain_loss, 0.0);
    assert!(point.train_cosine.is_none());
    assert_eq!(point.labelled_samples, 0);
    assert_eq!(point.labels_this_round, 0);
    assert!(point.oracle_gain_mean.is_none());
    assert!(point.val_improvement_oracle.is_none());
    assert!(point.val_evaluations_per_step.is_none());
    assert_eq!(point.rollout_secs, 0.0);
    assert_eq!(point.label_secs, 0.0);
    assert_eq!(point.update_secs, 0.0);
    assert_eq!(point.evaluations_this_round, 0);
    assert_eq!(point.workers, 0);
}

#[test]
fn a_point_with_the_removed_teacher_fields_still_loads() {
    let mut value: serde_json::Value = serde_json::from_str(OLD_POINT).unwrap();
    let object = value.as_object_mut().unwrap();
    object.insert("teacher_cosine".to_owned(), 0.42.into());
    object.insert("teacher_loss".to_owned(), 1.5.into());
    let point: IterationMetricPoint = serde_json::from_value(value).unwrap();
    assert_eq!(point.iteration, 3);
    let text = serde_json::to_string(&point).unwrap();
    assert!(
        !text.contains("teacher_cosine") && !text.contains("teacher_loss"),
        "the removed fields are not written any more"
    );
}

#[test]
fn the_diagnostic_fields_round_trip() {
    let mut point: IterationMetricPoint = serde_json::from_str(OLD_POINT).unwrap();
    point.rej_manufacturability = 9;
    point.val_improvement_sampled = Some(1.25);
    point.early_stopped = true;
    let text = serde_json::to_string(&point).unwrap();
    let back: IterationMetricPoint = serde_json::from_str(&text).unwrap();
    assert_eq!(back.rej_manufacturability, 9);
    assert_eq!(back.val_improvement_sampled, Some(1.25));
    assert!(back.early_stopped);
}

#[test]
fn the_imitation_and_utilisation_fields_round_trip() {
    let mut point: IterationMetricPoint = serde_json::from_str(OLD_POINT).unwrap();
    point.imitation_loss = 0.75;
    point.sens_mse = 0.5;
    point.train_cosine = Some(0.625);
    point.gain_loss = 0.25;
    point.labelled_samples = 4096;
    point.labels_this_round = 512;
    point.oracle_gain_mean = Some(1.5);
    point.val_improvement_oracle = Some(6.5);
    point.val_evaluations_per_step = Some(1.75);
    point.rollout_secs = 12.5;
    point.label_secs = 8.25;
    point.update_secs = 1.5;
    point.evaluations_this_round = 9000;
    point.workers = 16;
    let text = serde_json::to_string(&point).unwrap();
    let back: IterationMetricPoint = serde_json::from_str(&text).unwrap();
    assert_eq!(back.imitation_loss, 0.75);
    assert_eq!(back.sens_mse, 0.5);
    assert_eq!(back.train_cosine, Some(0.625));
    assert_eq!(back.gain_loss, 0.25);
    assert_eq!(back.labelled_samples, 4096);
    assert_eq!(back.labels_this_round, 512);
    assert_eq!(back.oracle_gain_mean, Some(1.5));
    assert_eq!(back.val_improvement_oracle, Some(6.5));
    assert_eq!(back.val_evaluations_per_step, Some(1.75));
    assert_eq!(back.rollout_secs, 12.5);
    assert_eq!(back.label_secs, 8.25);
    assert_eq!(back.update_secs, 1.5);
    assert_eq!(back.evaluations_this_round, 9000);
    assert_eq!(back.workers, 16);
}

#[test]
fn evaluations_per_second_use_the_rollout_and_update_time() {
    let mut point: IterationMetricPoint = serde_json::from_str(OLD_POINT).unwrap();
    assert!(point.evaluations_per_sec().is_none(), "nothing recorded");
    point.evaluations_this_round = 9000;
    assert!(point.evaluations_per_sec().is_none(), "no time recorded");
    point.rollout_secs = 8.0;
    point.update_secs = 1.0;
    point.label_secs = 5.0;
    let rate = point.evaluations_per_sec().unwrap();
    assert!((rate - 1000.0).abs() < 1e-3, "{rate}");
    point.evaluations_this_round = 0;
    assert!(
        point.evaluations_per_sec().is_none(),
        "no evaluation counted"
    );
}

#[test]
fn the_update_wait_fields_default_in_old_files_and_round_trip() {
    let mut point: IterationMetricPoint = serde_json::from_str(OLD_POINT).unwrap();
    assert_eq!(point.update_wait_secs, 0.0);
    assert!(point.update_of_iteration.is_none());
    point.update_wait_secs = 0.25;
    point.update_of_iteration = Some(6);
    let text = serde_json::to_string(&point).unwrap();
    let back: IterationMetricPoint = serde_json::from_str(&text).unwrap();
    assert_eq!(back.update_wait_secs, 0.25);
    assert_eq!(back.update_of_iteration, Some(6));
}

#[test]
fn throughput_uses_the_wait_not_the_update_when_the_update_overlaps() {
    let mut point: IterationMetricPoint = serde_json::from_str(OLD_POINT).unwrap();
    point.evaluations_this_round = 9000;
    point.rollout_secs = 8.0;
    // The update ran 6 s on its own thread during the rollout and the main thread waited 1 s.
    point.update_secs = 6.0;
    point.update_wait_secs = 1.0;
    point.update_of_iteration = Some(2);
    assert!((point.main_thread_secs() - 9.0).abs() < 1e-6);
    assert!(point.has_update());
    let rate = point.evaluations_per_sec().unwrap();
    assert!((rate - 1000.0).abs() < 1e-3, "{rate}");
    // Iteration 0: no update installed, the rollout alone counts.
    point.update_secs = 0.0;
    point.update_wait_secs = 0.0;
    point.update_of_iteration = None;
    assert!((point.main_thread_secs() - 8.0).abs() < 1e-6);
    assert!(!point.has_update());
}

#[test]
fn the_drop_ratio_is_the_model_over_the_oracle() {
    assert_eq!(drop_ratio(3.0, Some(6.0)), Some(0.5));
    assert_eq!(drop_ratio(6.0, Some(6.0)), Some(1.0));
    assert_eq!(drop_ratio(-0.5, Some(2.0)), Some(-0.25));
    assert_eq!(drop_ratio(3.0, None), None);
    assert_eq!(drop_ratio(3.0, Some(0.0)), None, "nothing to gain");
    assert_eq!(drop_ratio(3.0, Some(MIN_ORACLE_DROP)), None);
    assert_eq!(drop_ratio(3.0, Some(f32::NAN)), None);
    assert_eq!(drop_ratio(3.0, Some(f32::INFINITY)), None);
}

#[test]
fn the_three_drops_are_described_with_their_ratio() {
    assert_eq!(
        describe_drops(4.2, Some(6.1), Some(1.3)),
        "drop model 4.20 / oracle 6.10 / sampled 1.30 (model/oracle 0.69)"
    );
    // Missing figures read as a dash; the ratio needs the oracle.
    assert_eq!(
        describe_drops(4.2, None, Some(1.3)),
        "drop model 4.20 / oracle - / sampled 1.30"
    );
    assert_eq!(
        describe_drops(4.2, Some(6.1), None),
        "drop model 4.20 / oracle 6.10 / sampled - (model/oracle 0.69)"
    );
    assert_eq!(
        describe_drops(0.0, Some(0.0), Some(0.0)),
        "drop model 0.00 / oracle 0.00 / sampled 0.00"
    );
}

/// A run record with every field set, to exercise the (de)serialisation of the whole file.
fn sample_run() -> TrainingRunStats {
    TrainingRunStats {
        version: STATS_VERSION.to_owned(),
        timestamp_rfc3339: "2026-10-13T08:30:00Z".to_owned(),
        mode: "Retarget".to_owned(),
        model_tier: "M".to_owned(),
        device: "CPU (Flex SIMD)".to_owned(),
        active_design_title: "Test".to_owned(),
        active_design_filename: "test.asc".to_owned(),
        target_material: "Diamond".to_owned(),
        total_iterations: 3,
        total_episodes: 7,
        total_steps: 300,
        best_score: Some(2.0),
        best_improvement: Some(5.0),
        final_return: -1.0,
        baseline_brilliance: 50.0,
        baseline_windowing: 3.0,
        baseline_extinction: 4.0,
        baseline_yield_pct: 60.0,
        final_brilliance: 55.0,
        final_windowing: 2.0,
        final_extinction: 3.5,
        final_yield_pct: 59.0,
        val_score: Some(4.0),
        generalization_gap: None,
        train_designs_count: 10,
        val_designs_count: 2,
        test_designs_count: 1,
        split_seed: 7,
        algo: "imitation".to_owned(),
        label_every: 2,
        objectives: "mixed".to_owned(),
        train_fidelity: "fast".to_owned(),
        validation_fidelity: "fast".to_owned(),
        shape_target_sampling: "episode-seeded".to_owned(),
        lighting_sampling: "episode-seeded".to_owned(),
        objective_sweep: false,
        classical_budget: None,
        explore_std: 0.3,
        max_step_deg: 0.5,
        recent_rewards: vec![0.5, 0.25],
        metrics_history: vec![serde_json::from_str(OLD_POINT).unwrap()],
        event_logs: vec![("t".to_owned(), "INFO".to_owned(), "m".to_owned())],
    }
}

#[test]
fn the_run_record_keeps_the_algorithm_fields() {
    let text = serde_json::to_string(&sample_run()).unwrap();
    let back: TrainingRunStats = serde_json::from_str(&text).unwrap();
    assert_eq!(back.algo, "imitation");
    assert_eq!(back.label_every, 2);
    assert_eq!(back.objectives, "mixed");
    assert_eq!(back.explore_std, 0.3);
    assert!(
        !text.contains("teacher_every"),
        "the removed field is not written any more"
    );
}

#[test]
fn a_run_record_from_the_teacher_era_still_loads() {
    let mut value = serde_json::to_value(sample_run()).unwrap();
    let object = value.as_object_mut().unwrap();
    for key in ["algo", "label_every", "objectives", "explore_std"] {
        assert!(
            object.remove(key).is_some(),
            "{key} is a field of the record"
        );
    }
    object.insert("teacher_every".to_owned(), 4.into());
    let back: TrainingRunStats = serde_json::from_value(value).unwrap();
    assert_eq!(back.algo, "");
    assert_eq!(back.label_every, 0);
    assert_eq!(back.objectives, "");
    assert_eq!(back.explore_std, 0.0);
    assert_eq!(back.total_iterations, 3);
}
