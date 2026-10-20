use super::*;

fn optics(brilliance: f32) -> Optics {
    Optics {
        brilliance,
        ..Optics::default()
    }
}

/// Scores of a finished design.
struct Scores {
    base: f32,
    fin: f32,
    sampled: Option<f32>,
    oracle: Option<f32>,
}

/// A finished design: its scores, the brilliance before (it gains 10 points in the model's
/// episode) and the model's (evaluations, steps).
#[expect(
    clippy::unnecessary_wraps,
    reason = "builds the Ok entries of the DesignOutcome list that reduce consumes"
)]
fn done(scores: &Scores, brilliance: f32, model: (usize, usize)) -> DesignOutcome {
    Ok(DesignResult::Figures(DesignFigures {
        case: EvalCase::default(),
        base_score: scores.base,
        final_score: scores.fin,
        base: optics(brilliance),
        fin: optics(brilliance + 10.0),
        sampled_final_score: scores.sampled,
        oracle_final_score: scores.oracle,
        model_evaluations: model.0,
        model_steps: model.1,
    }))
}

fn scores(base: f32, fin: f32, sampled: Option<f32>, oracle: Option<f32>) -> Scores {
    Scores {
        base,
        fin,
        sampled,
        oracle,
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "builds the Ok entries of the DesignOutcome list that reduce consumes"
)]
fn failed(text: &str) -> DesignOutcome {
    Ok(DesignResult::Failed(EvalCase {
        failure: Some(text.to_owned()),
        ..EvalCase::default()
    }))
}

const ALL_PASSES: EvalPasses = EvalPasses {
    sampled: true,
    oracle: true,
};

#[test]
fn the_reduction_averages_the_designs_and_keeps_the_first_failure_in_entry_order() {
    let results = vec![
        failed("'a': no light"),
        done(&scores(10.0, 6.0, Some(8.0), Some(2.0)), 40.0, (90, 60)),
        failed("'c': unsolvable"),
        done(&scores(20.0, 12.0, Some(18.0), Some(8.0)), 60.0, (60, 60)),
    ];
    let report = reduce(EvalFidelity::Fast, ALL_PASSES, results).unwrap();
    assert_eq!((report.designs, report.failed), (2, 2));
    assert_eq!(report.first_failure.as_deref(), Some("'a': no light"));
    // Scores: base (10 + 20) / 2 = 15, final (6 + 12) / 2 = 9, drop 6.
    assert!((report.mean_base_score - 15.0).abs() < 1e-6);
    assert!((report.mean_final_score - 9.0).abs() < 1e-6);
    assert!((report.mean_improvement - 6.0).abs() < 1e-6);
    // Sampled drops 10 - 8 = 2 and 20 - 18 = 2.
    let sampled = report.mean_improvement_sampled.unwrap();
    assert!((sampled - 2.0).abs() < 1e-6);
    // Oracle drops 10 - 2 = 8 and 20 - 8 = 12.
    let oracle = report.mean_improvement_oracle.unwrap();
    assert!((oracle - 10.0).abs() < 1e-6);
    // Brilliance before (40 + 60) / 2 = 50, after 10 more.
    assert!((report.base_optics.brilliance - 50.0).abs() < 1e-4);
    assert!((report.final_optics.brilliance - 60.0).abs() < 1e-4);
    assert!(!report.full_fidelity);
    // The oracle is never marked as cached by the reduction: that is the runner's call.
    assert!(!report.oracle_cached);
}

#[test]
fn without_the_oracle_figures_there_is_no_oracle_mean() {
    let results = vec![
        done(&scores(10.0, 6.0, Some(8.0), None), 40.0, (60, 60)),
        done(&scores(20.0, 12.0, Some(18.0), None), 60.0, (60, 60)),
    ];
    let report = reduce(
        EvalFidelity::Fast,
        EvalPasses {
            sampled: true,
            oracle: false,
        },
        results,
    )
    .unwrap();
    assert!(report.mean_improvement_oracle.is_none());
    assert!(report.mean_improvement_sampled.is_some());
    assert_eq!(report.designs, 2);
}

#[test]
fn without_a_sampled_pass_there_is_no_sampled_mean() {
    let results = vec![done(&scores(10.0, 6.0, None, None), 40.0, (60, 60))];
    let report = reduce(EvalFidelity::Full, EvalPasses::default(), results).unwrap();
    assert!(report.mean_improvement_sampled.is_none());
    assert!(report.mean_improvement_oracle.is_none());
    assert!(report.full_fidelity);
    assert!((report.mean_improvement - 4.0).abs() < 1e-6);
}

#[test]
fn the_evaluations_per_step_are_the_totals_over_the_designs() {
    // 120 evaluations over 70 steps, not the mean of the per-design ratios (1.5 and 3.0
    // would give 2.25).
    let results = vec![
        done(&scores(10.0, 6.0, None, None), 40.0, (90, 60)),
        done(&scores(20.0, 12.0, None, None), 60.0, (30, 10)),
    ];
    let report = reduce(EvalFidelity::Fast, EvalPasses::default(), results).unwrap();
    assert!((report.mean_evaluations_per_step - 120.0 / 70.0).abs() < 1e-6);
    // No steps, no ratio.
    let none = vec![done(&scores(1.0, 1.0, None, None), 0.0, (0, 0))];
    let report = reduce(EvalFidelity::Fast, EvalPasses::default(), none).unwrap();
    assert_eq!(report.mean_evaluations_per_step, 0.0);
}

#[test]
fn nothing_evaluated_leaves_the_means_empty() {
    let report = reduce(EvalFidelity::Fast, ALL_PASSES, vec![failed("'x': dead")]).unwrap();
    assert_eq!((report.designs, report.failed), (0, 1));
    assert!(report.mean_improvement_sampled.is_none());
    assert!(report.mean_improvement_oracle.is_none());
    assert!(
        reduce(EvalFidelity::Fast, ALL_PASSES, Vec::new())
            .unwrap()
            .first_failure
            .is_none()
    );
}

#[test]
fn a_hard_error_is_the_first_one_in_entry_order() {
    let results = vec![
        done(&scores(10.0, 6.0, None, None), 40.0, (60, 60)),
        Err(RunnerError::Data("first".into())),
        Err(RunnerError::Data("second".into())),
    ];
    let error = reduce(EvalFidelity::Fast, EvalPasses::default(), results).unwrap_err();
    assert_eq!(error.to_string(), "first");
}

#[test]
fn the_oracle_key_changes_with_everything_the_oracle_depends_on() {
    let cfg = RunConfig::new(
        TrainingMode::Retarget,
        indicatrix_ai::model::ModelTier::Small,
    );
    let key = OracleKey::new(&cfg, &[0, 1], 7, 0.5);
    assert_eq!(key, OracleKey::new(&cfg, &[0, 1], 7, 0.5));
    assert_ne!(key, OracleKey::new(&cfg, &[0], 7, 0.5), "entries");
    assert_ne!(key, OracleKey::new(&cfg, &[0, 1], 8, 0.5), "seed");
    assert_ne!(key, OracleKey::new(&cfg, &[0, 1], 7, 0.25), "step limit");
    let changed = |edit: &dyn Fn(&mut RunConfig)| {
        let mut other = cfg.clone();
        edit(&mut other);
        OracleKey::new(&other, &[0, 1], 7, 0.5)
    };
    assert_ne!(key, changed(&|c| c.mode = TrainingMode::Optimizer), "mode");
    assert_ne!(key, changed(&|c| c.universal_spectrum = false), "spectrum");
    assert_ne!(key, changed(&|c| c.fixed_target = "Ruby".into()), "target");
    assert_ne!(key, changed(&|c| c.line_search = vec![1.0]), "line search");
    assert_ne!(key, changed(&|c| c.label_h_deg = Some(1.0)), "probe size");
    assert_ne!(
        key,
        changed(&|c| c.objectives = ObjectiveSampling::Balanced),
        "objectives"
    );
    assert_ne!(
        key,
        changed(&|c| c.same_material_frac = 0.0),
        "same material"
    );
    // Settings the oracle does not depend on leave the key alone.
    assert_eq!(key, changed(&|c| c.workers += 1));
    assert_eq!(key, changed(&|c| c.explore_std += 0.1));
    assert_eq!(key, changed(&|c| c.corpus_mode = !c.corpus_mode));
}
