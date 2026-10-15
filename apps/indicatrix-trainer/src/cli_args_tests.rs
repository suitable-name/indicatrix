//! Tests of the flag parser and of the configuration the flags build.

use indicatrix_ai::{env::DEFAULT_LINE_SEARCH, training::PpoConfig};

use super::*;

fn parse(args: &[&str]) -> Result<Parsed, String> {
    parse_args(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
}

fn run_args(flags: &[&str]) -> Box<CliArgs> {
    let Ok(Parsed::Run(args)) = parse(flags) else {
        panic!("{flags:?} must parse");
    };
    args
}

#[test]
fn defaults_and_help() {
    assert_eq!(parse(&["--help"]), Ok(Parsed::Help));
    assert_eq!(parse(&["-h"]), Ok(Parsed::Help));
    let Ok(Parsed::Run(args)) = parse(&[]) else {
        panic!("defaults must parse");
    };
    assert_eq!(args.iterations, 100);
    assert_eq!(args.mode, TrainingMode::Retarget);
    assert_eq!(args.algo, Algorithm::Imitation);
}

#[test]
fn the_help_documents_every_flag_and_not_the_removed_one() {
    for flag in [
        "--mode",
        "--tier",
        "--device",
        "--algo",
        "--iterations",
        "--workers",
        "--batch-size",
        "--mini-batch",
        "--epochs",
        "--validation-designs",
        "--validation-every",
        "--label-every",
        "--label-h",
        "--label-capacity",
        "--explore-std",
        "--line-search",
        "--objectives",
        "--same-material-frac",
        "--imitation-lr",
        "--cosine-coeff",
        "--gain-coeff",
        "--lr",
        "--max-step-deg",
        "--entropy-coeff",
        "--seed",
        "--max-tiers",
        "--no-corpus",
        "--no-spectrum",
        "--target-material",
        "--eval-test",
        "--eval-fidelity",
        "--fixture",
        "--resume",
        "--save-stats",
        "--save-weights",
    ] {
        assert!(
            HELP.contains(&format!("\n    {flag} ")),
            "{flag} is not documented"
        );
    }
    assert!(!HELP.contains("--teacher-every"));
    assert!(HELP.contains("the core count"));
    assert!(HELP.contains("Epochs per round (imitation"));
}

#[test]
fn the_help_names_the_update_check_flag_and_variable() {
    // The flag itself is removed from the arguments before the parser (see `cli_main.rs`).
    assert!(HELP.contains(&format!(
        "\n    {} ",
        indicatrix_update::NO_UPDATE_CHECK_FLAG
    )));
    assert!(HELP.contains(indicatrix_update::NO_UPDATE_CHECK_ENV));
}

#[test]
fn unknown_flags_are_rejected() {
    assert!(
        parse(&["--bogus"])
            .unwrap_err()
            .contains("unknown argument")
    );
    assert!(parse(&["--steps", "5"]).is_err());
}

#[test]
fn the_removed_teacher_flag_is_an_unknown_argument() {
    for flags in [&["--teacher-every", "4"][..], &["--teacher-every", "0"]] {
        let message = parse(flags).unwrap_err();
        assert!(message.contains("unknown argument"), "{message}");
        assert!(message.contains("--teacher-every"), "{message}");
    }
}

#[test]
fn bad_numbers_and_missing_values_are_rejected() {
    assert!(parse(&["--iterations", "abc"]).is_err());
    assert!(parse(&["--iterations", "0"]).is_err());
    assert!(parse(&["--workers", "0"]).is_err());
    assert!(
        parse(&["--iterations"])
            .unwrap_err()
            .contains("expects a value")
    );
    assert!(parse(&["--mini-batch", "512", "--batch-size", "256"]).is_err());
}

#[test]
fn the_workers_default_to_the_core_count_and_are_range_checked() {
    assert_eq!(run_args(&[]).workers, default_workers());
    assert!((1..=MAX_WORKERS).contains(&run_args(&[]).workers));
    assert_eq!(run_args(&["--workers", "1"]).workers, 1);
    assert_eq!(run_args(&["--workers", "256"]).workers, 256);
    let largest = MAX_WORKERS.to_string();
    let too_many = (MAX_WORKERS + 1).to_string();
    assert_eq!(
        run_args(&["--workers", largest.as_str()]).workers,
        MAX_WORKERS
    );
    assert!(parse(&["--workers", "0"]).is_err());
    assert!(parse(&["--workers", too_many.as_str()]).is_err());
    assert!(parse(&["--workers", "-3"]).is_err());
    assert!(parse(&["--workers", "many"]).is_err());
    assert_eq!(build_config(&run_args(&["--workers", "12"])).workers, 12);
    assert_eq!(
        build_config(&run_args(&[])).workers,
        default_workers(),
        "the configuration carries the default too"
    );
}

#[test]
fn validation_designs_flag_is_range_checked() {
    assert!(parse(&["--validation-designs", "0"]).is_err());
    assert!(parse(&["--validation-designs", "65"]).is_err());
    let Ok(Parsed::Run(args)) = parse(&["--validation-designs", "12"]) else {
        panic!("a valid count must parse");
    };
    assert_eq!(args.validation_designs, 12);
}

#[test]
fn enums_match_exactly() {
    assert!(parse(&["--tier", "xyz"]).is_err());
    assert!(parse(&["--mode", "opt"]).is_err());
    assert!(parse(&["--device", "gpu0"]).is_err());
    let Ok(Parsed::Run(args)) = parse(&["--mode", "optimizer", "--device", "cpu", "--tier", "s"])
    else {
        panic!("valid flags must parse");
    };
    assert_eq!(args.mode, TrainingMode::Optimizer);
    assert_eq!(args.device, DeviceChoice::Cpu);
    assert_eq!(args.tier, ModelTier::Small);
}

#[test]
fn the_training_flags_default_to_the_built_in_values() {
    let args = run_args(&[]);
    assert_eq!(args.validation_every, 10);
    assert_eq!(args.validation_designs, 16);
    assert!(args.learning_rate.is_none());
    assert!(args.max_step_deg.is_none());
    assert!(args.entropy_coeff.is_none());
    assert!(args.mini_batch.is_none() && args.epochs.is_none());
}

#[test]
fn the_training_flags_parse() {
    let args = run_args(&[
        "--validation-every",
        "5",
        "--lr",
        "1e-4",
        "--max-step-deg",
        "0.5",
        "--entropy-coeff",
        "0.002",
    ]);
    assert_eq!(args.validation_every, 5);
    assert!((args.learning_rate.unwrap() - 1e-4).abs() < 1e-9);
    assert!((args.max_step_deg.unwrap() - 0.5).abs() < 1e-12);
    assert!((args.entropy_coeff.unwrap() - 0.002).abs() < 1e-9);
    // The edges of the ranges are valid: an entropy bonus of zero, 1000 passes.
    assert_eq!(
        run_args(&["--validation-every", "1000"]).validation_every,
        1000
    );
    assert_eq!(run_args(&["--entropy-coeff", "0"]).entropy_coeff, Some(0.0));
}

#[test]
fn the_training_flags_are_range_checked() {
    for flags in [
        &["--validation-every", "0"][..],
        &["--validation-every", "1001"],
        &["--validation-every", "x"],
        &["--lr", "0"],
        &["--lr", "-1e-4"],
        &["--lr", "nan"],
        &["--lr", "inf"],
        &["--lr", "1e-60"],
        &["--lr", "fast"],
        &["--max-step-deg", "0"],
        &["--max-step-deg", "-0.5"],
        &["--max-step-deg", "inf"],
        &["--entropy-coeff", "-0.1"],
        &["--entropy-coeff", "nan"],
    ] {
        assert!(parse(flags).is_err(), "{flags:?} must be refused");
    }
    let message = parse(&["--lr", "0"]).unwrap_err();
    assert!(
        message.contains("--lr") && message.contains("above 0"),
        "{message}"
    );
    assert!(
        parse(&["--max-step-deg"])
            .unwrap_err()
            .contains("expects a value")
    );
}

#[test]
fn the_algorithm_flags_default_to_the_documented_values() {
    let args = run_args(&[]);
    assert_eq!(args.algo, Algorithm::Imitation);
    assert_eq!(args.objectives, ObjectiveSampling::Mixed);
    assert!(args.label_every.is_none() && args.label_h_deg.is_none());
    assert!(args.label_capacity.is_none() && args.explore_std.is_none());
    assert!(args.line_search.is_none() && args.same_material_frac.is_none());
    let config = build_config(&args);
    assert_eq!(config.algo, Algorithm::Imitation);
    assert_eq!(config.label_every, 2);
    assert_eq!(config.label_h_deg, None);
    assert_eq!(config.label_capacity, 50_000);
    assert!((config.explore_std - 0.3).abs() < 1e-9);
    assert_eq!(config.line_search, DEFAULT_LINE_SEARCH.to_vec());
    assert_eq!(config.objectives, ObjectiveSampling::Mixed);
    assert!((config.same_material_frac - 0.25).abs() < 1e-9);
    let imitation = ImitationConfig::default();
    assert!((config.imitation.learning_rate - imitation.learning_rate).abs() < 1e-12);
    assert!((config.imitation.cosine_coeff - imitation.cosine_coeff).abs() < 1e-12);
    assert!((config.imitation.gain_coeff - imitation.gain_coeff).abs() < 1e-12);
}

#[test]
fn the_algorithm_flags_parse() {
    let args = run_args(&[
        "--algo",
        "ppo",
        "--label-every",
        "4",
        "--label-h",
        "0.75",
        "--label-capacity",
        "2048",
        "--explore-std",
        "0.15",
        "--line-search",
        "1,0.5",
        "--objectives",
        "presets",
        "--same-material-frac",
        "0.5",
        "--imitation-lr",
        "1e-3",
        "--cosine-coeff",
        "0.25",
        "--gain-coeff",
        "0",
    ]);
    assert_eq!(args.algo, Algorithm::Ppo);
    assert_eq!(args.label_every, Some(4));
    assert_eq!(args.label_h_deg, Some(0.75));
    assert_eq!(args.label_capacity, Some(2048));
    assert!((args.explore_std.unwrap() - 0.15).abs() < 1e-9);
    assert_eq!(args.line_search, Some(vec![1.0, 0.5]));
    assert_eq!(args.objectives, ObjectiveSampling::Presets);
    assert_eq!(args.same_material_frac, Some(0.5));
    assert!((args.imitation_lr.unwrap() - 1e-3).abs() < 1e-9);
    assert_eq!(args.cosine_coeff, Some(0.25));
    assert_eq!(args.gain_coeff, Some(0.0));
    assert_eq!(
        run_args(&["--algo", "imitation"]).algo,
        Algorithm::Imitation
    );
    assert_eq!(
        run_args(&["--objectives", "balanced"]).objectives,
        ObjectiveSampling::Balanced
    );
    assert_eq!(
        run_args(&["--objectives", "mixed"]).objectives,
        ObjectiveSampling::Mixed
    );
}

#[test]
fn the_algorithm_flags_reach_the_run_configuration() {
    let config = build_config(&run_args(&[
        "--label-every",
        "4",
        "--label-h",
        "0.75",
        "--label-capacity",
        "2048",
        "--explore-std",
        "0.15",
        "--line-search",
        "1,0.5",
        "--objectives",
        "presets",
        "--same-material-frac",
        "0.5",
        "--imitation-lr",
        "1e-3",
        "--cosine-coeff",
        "0.25",
        "--gain-coeff",
        "0.05",
    ]));
    assert_eq!(config.label_every, 4);
    assert_eq!(config.label_h_deg, Some(0.75));
    assert_eq!(config.label_capacity, 2048);
    assert!((config.explore_std - 0.15).abs() < 1e-9);
    assert_eq!(config.line_search, vec![1.0, 0.5]);
    assert_eq!(config.objectives, ObjectiveSampling::Presets);
    assert!((config.same_material_frac - 0.5).abs() < 1e-9);
    assert!((config.imitation.learning_rate - 1e-3).abs() < 1e-9);
    assert!((config.imitation.cosine_coeff - 0.25).abs() < 1e-9);
    assert!((config.imitation.gain_coeff - 0.05).abs() < 1e-9);
}

#[test]
fn the_algorithm_flags_are_range_checked() {
    for flags in [
        &["--algo", "sac"][..],
        &["--algo", ""],
        &["--algo"],
        &["--label-every", "65"],
        &["--label-every", "-1"],
        &["--label-every", "x"],
        &["--label-h", "0"],
        &["--label-h", "-0.5"],
        &["--label-h", "inf"],
        &["--label-h", "half"],
        &["--label-capacity", "0"],
        &["--label-capacity", "1000001"],
        &["--label-capacity", "many"],
        &["--explore-std", "-0.1"],
        &["--explore-std", "nan"],
        &["--explore-std", "inf"],
        &["--objectives", "random"],
        &["--objectives", ""],
        &["--same-material-frac", "1.5"],
        &["--same-material-frac", "-0.1"],
        &["--same-material-frac", "nan"],
        &["--imitation-lr", "0"],
        &["--imitation-lr", "-3e-4"],
        &["--imitation-lr", "1e-60"],
        &["--cosine-coeff", "-1"],
        &["--cosine-coeff", "nan"],
        &["--gain-coeff", "-0.5"],
        &["--gain-coeff", "inf"],
    ] {
        assert!(parse(flags).is_err(), "{flags:?} must be refused");
    }
    assert_eq!(
        parse(&["--algo", "sac"]).unwrap_err(),
        "--algo expects imitation or ppo, got 'sac'"
    );
    assert!(
        parse(&["--objectives", "random"])
            .unwrap_err()
            .contains("balanced, presets or mixed")
    );
}

#[test]
fn the_edges_of_the_algorithm_ranges_are_valid() {
    assert_eq!(run_args(&["--label-every", "1"]).label_every, Some(1));
    assert_eq!(run_args(&["--label-every", "64"]).label_every, Some(64));
    assert_eq!(run_args(&["--label-capacity", "1"]).label_capacity, Some(1));
    assert_eq!(
        run_args(&["--label-capacity", "1000000"]).label_capacity,
        Some(1_000_000)
    );
    assert_eq!(run_args(&["--explore-std", "0"]).explore_std, Some(0.0));
    assert_eq!(
        run_args(&["--same-material-frac", "0"]).same_material_frac,
        Some(0.0)
    );
    assert_eq!(
        run_args(&["--same-material-frac", "1"]).same_material_frac,
        Some(1.0)
    );
    assert_eq!(run_args(&["--cosine-coeff", "0"]).cosine_coeff, Some(0.0));
    assert_eq!(run_args(&["--gain-coeff", "0"]).gain_coeff, Some(0.0));
}

#[test]
fn a_line_search_is_a_comma_separated_list_of_positive_scales() {
    let scales = |text: &str| {
        parse(&["--line-search", text]).map(|parsed| match parsed {
            Parsed::Run(args) => args.line_search,
            Parsed::Help => None,
        })
    };
    assert_eq!(scales("1,0.5,0.25"), Ok(Some(vec![1.0, 0.5, 0.25])));
    assert_eq!(scales("1"), Ok(Some(vec![1.0])));
    assert_eq!(scales("1, 0.5"), Ok(Some(vec![1.0, 0.5])));
    assert_eq!(scales("0.5,1"), Ok(Some(vec![0.5, 1.0])));
    assert_eq!(scales("2,1"), Ok(Some(vec![2.0, 1.0])));
    for bad in [
        "1,0", "a", "", "1,", ",1", "1,,0.5", "-1", "1,-0.5", "inf", "1;0.5",
    ] {
        assert!(scales(bad).is_err(), "'{bad}' must be refused");
    }
    let message = scales("1,0").unwrap_err();
    assert!(
        message.contains("--line-search") && message.contains("above 0"),
        "{message}"
    );
}

#[test]
fn the_label_period_defaults_follow_the_algorithm() {
    assert_eq!(build_config(&run_args(&[])).label_every, 2);
    assert_eq!(
        build_config(&run_args(&["--algo", "imitation"])).label_every,
        2
    );
    // PPO ignores the labels in its loss, so by default it pays for none.
    assert_eq!(build_config(&run_args(&["--algo", "ppo"])).label_every, 0);
    assert_eq!(
        build_config(&run_args(&["--algo", "ppo", "--label-every", "3"])).label_every,
        3
    );
    assert_eq!(
        build_config(&run_args(&["--algo", "ppo", "--label-every", "0"])).label_every,
        0
    );
}

#[test]
fn imitation_refuses_a_run_without_labels() {
    for flags in [
        &["--label-every", "0"][..],
        &["--algo", "imitation", "--label-every", "0"],
        &["--label-every", "0", "--algo", "imitation"],
    ] {
        let message = parse(flags).unwrap_err();
        assert!(message.contains("--label-every 0"), "{message}");
    }
    assert!(parse(&["--algo", "ppo", "--label-every", "0"]).is_ok());
    assert!(parse(&["--label-every", "1"]).is_ok());
}

#[test]
fn the_mini_batch_and_epoch_defaults_follow_the_algorithm() {
    // The built-in PPO values are the runner's own.
    let runner_default = RunConfig::new(TrainingMode::Retarget, ModelTier::Small);
    assert_eq!(runner_default.ppo.mini_batch_size, PPO_MINI_BATCH);
    assert_eq!(runner_default.ppo.epochs_per_batch, PPO_EPOCHS);

    let imitation = build_config(&run_args(&[]));
    assert_eq!(imitation.imitation.mini_batch_size, 256);
    assert_eq!(imitation.imitation.epochs_per_round, 2);
    assert_eq!(imitation.ppo.mini_batch_size, 64);
    assert_eq!(imitation.ppo.epochs_per_batch, 4);

    let ppo = build_config(&run_args(&["--algo", "ppo"]));
    assert_eq!(ppo.ppo.mini_batch_size, 64);
    assert_eq!(ppo.ppo.epochs_per_batch, 4);
    assert_eq!(ppo.imitation.mini_batch_size, 256);
    assert_eq!(ppo.imitation.epochs_per_round, 2);

    // The flags apply to both algorithms.
    for algo in ["imitation", "ppo"] {
        let config = build_config(&run_args(&[
            "--algo",
            algo,
            "--mini-batch",
            "128",
            "--epochs",
            "3",
        ]));
        assert_eq!(config.imitation.mini_batch_size, 128, "{algo}");
        assert_eq!(config.ppo.mini_batch_size, 128, "{algo}");
        assert_eq!(config.imitation.epochs_per_round, 3, "{algo}");
        assert_eq!(config.ppo.epochs_per_batch, 3, "{algo}");
    }
}

#[test]
fn the_mini_batch_must_fit_the_batch_size() {
    // An explicit flag is checked for both algorithms.
    for algo in ["imitation", "ppo"] {
        let message =
            parse(&["--algo", algo, "--mini-batch", "512", "--batch-size", "256"]).unwrap_err();
        assert!(message.contains("--mini-batch (512)"), "{message}");
        assert!(parse(&["--algo", algo, "--mini-batch", "256"]).is_ok());
    }
    // The PPO default of 64 is checked as well ...
    assert!(parse(&["--algo", "ppo", "--batch-size", "32"]).is_err());
    assert!(parse(&["--algo", "ppo", "--batch-size", "64"]).is_ok());
    // ... the imitation default draws from the aggregate and is not, and the unused PPO
    // default cannot fail the runner's own check on the small rollout.
    let config = build_config(&run_args(&["--batch-size", "32"]));
    assert_eq!(config.imitation.mini_batch_size, 256);
    assert_eq!(config.ppo.mini_batch_size, 32);
    assert_eq!(config.rollout_size, 32);
}

#[test]
fn the_imitation_schedule_is_bounded_by_the_aggregate_capacity() {
    // 300 rounds x 2 epochs x ceil(50000 / 256 = 195.3) = 196 mini-batches.
    let config = build_config(&run_args(&[
        "--iterations",
        "300",
        "--epochs",
        "2",
        "--mini-batch",
        "256",
        "--label-capacity",
        "50000",
    ]));
    assert_eq!(
        config.imitation.lr_schedule,
        LrSchedule::Linear {
            total_updates: 300 * 2 * 196
        }
    );
    // The defaults: 100 rounds, 2 epochs, 256 per mini-batch, 50000 labelled states.
    assert_eq!(
        build_config(&run_args(&[])).imitation.lr_schedule,
        LrSchedule::Linear {
            total_updates: 100 * 2 * 196
        }
    );
    // A smaller aggregate, a smaller mini-batch and more epochs move the bound accordingly.
    let small = build_config(&run_args(&[
        "--iterations",
        "10",
        "--epochs",
        "5",
        "--mini-batch",
        "100",
        "--batch-size",
        "100",
        "--label-capacity",
        "1000",
    ]));
    assert_eq!(
        small.imitation.lr_schedule,
        LrSchedule::Linear {
            total_updates: 10 * 5 * 10
        }
    );
    assert_eq!(
        imitation_schedule_for(7, 1, 256, 2),
        LrSchedule::Linear { total_updates: 14 },
        "one labelled state still fills one mini-batch"
    );
}

#[test]
fn the_flags_reach_the_run_configuration() {
    let args = run_args(&[
        "--algo",
        "ppo",
        "--iterations",
        "200",
        "--batch-size",
        "1024",
        "--mini-batch",
        "128",
        "--epochs",
        "4",
        "--lr",
        "2e-4",
        "--entropy-coeff",
        "0.005",
        "--label-every",
        "4",
        "--max-step-deg",
        "0.25",
        "--validation-every",
        "7",
    ]);
    let config = build_config(&args);
    assert_eq!(config.algo, Algorithm::Ppo);
    assert_eq!(config.validation_every, 7);
    assert_eq!(config.label_every, 4);
    assert_eq!(config.max_step_deg, Some(0.25));
    assert!((config.ppo.learning_rate - 2e-4).abs() < 1e-9);
    assert!((config.ppo.entropy_coeff - 0.005).abs() < 1e-9);
    // 200 iterations x 4 epochs x (1024 / 128 = 8 mini-batches).
    assert_eq!(
        config.ppo.lr_schedule,
        LrSchedule::Linear {
            total_updates: 6400
        }
    );

    // Without the flags the PPO defaults stay.
    let defaults = build_config(&run_args(&["--algo", "ppo"]));
    let ppo = PpoConfig::default();
    assert!((defaults.ppo.learning_rate - ppo.learning_rate).abs() < 1e-12);
    assert!((defaults.ppo.entropy_coeff - ppo.entropy_coeff).abs() < 1e-12);
    assert_eq!(defaults.max_step_deg, None);
    assert_eq!(defaults.label_every, 0);
}

#[test]
fn flags_of_the_other_algorithm_are_reported_as_ignored() {
    assert_eq!(run_args(&[]).ignored_flag_notes(), Vec::<String>::new());
    assert_eq!(
        run_args(&["--algo", "ppo", "--lr", "1e-4"]).ignored_flag_notes(),
        Vec::<String>::new()
    );
    let notes = run_args(&["--lr", "1e-4", "--entropy-coeff", "0.01"]).ignored_flag_notes();
    assert_eq!(
        notes,
        vec![
            "--lr applies to --algo ppo only and is ignored".to_owned(),
            "--entropy-coeff applies to --algo ppo only and is ignored".to_owned(),
        ]
    );
    let notes = run_args(&[
        "--algo",
        "ppo",
        "--imitation-lr",
        "1e-3",
        "--cosine-coeff",
        "0.5",
        "--gain-coeff",
        "0.1",
    ])
    .ignored_flag_notes();
    assert_eq!(notes.len(), 3);
    assert!(notes.iter().all(|n| n.contains("--algo imitation only")));
}
