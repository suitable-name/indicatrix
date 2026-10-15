//! Command-line trainer for the Indicatrix RL policies.
//!
//! Headless counterpart of the GUI: the same `TrainingRunner` (supervised imitation of the
//! finite-difference teacher by default, PPO on request; vectorized environments, held-out
//! validation with a model, an oracle and a sampled pass, schema-versioned checkpoints), driven by
//! flags and reporting on stdout. Exports the telemetry (`TrainingRunStats`) the GUI can import.
//!
//! The flags live in `cli_args.rs` and everything the run prints in `cli_report.rs`.
//!
//! Exit status: `0` on success, `1` when the run fails, `2` for a usage error.

mod cli_args;
mod cli_console;
mod cli_report;
mod dataset;
#[path = "runner.rs"]
mod runner;
mod stats;

use std::{
    error::Error,
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

use cli_args::{CliArgs, HELP, Parsed, build_config, parse_args};
use cli_console::Console;
use cli_report::{print_banner, print_progress, print_summary};
use dataset::{DEFAULT_SPLIT_SEED, DatasetSplit, load_db_designs};
use indicatrix_update::{NO_UPDATE_CHECK_ENV, cli_notice, take_update_check};
use runner::{
    EvalFidelity, EvalPasses, SKIPPED_PREFIX, TrainingRunner, checkpoint_file_name,
    evaluation_threads,
};
use stats::{IterationMetricPoint, describe_drops, rfc3339_utc_now};

/// Skip warnings printed in full before the console switches to one summary line per
/// [`SKIP_SUMMARY_EVERY`] further skips (a library holds hundreds of dead stones).
const SKIP_LINES_FULL: usize = 10;
/// Further skips summarised by one console line.
const SKIP_SUMMARY_EVERY: usize = 50;
fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // The new-version check is the very first output: one `info:` line when a newer release
    // exists, nothing when up to date, switched off or on any failure.
    let skip = std::env::var_os(NO_UPDATE_CHECK_ENV);
    if take_update_check(&mut args, skip.as_deref())
        && let Some(line) = cli_notice("indicatrix-trainer-cli", env!("CARGO_PKG_VERSION"))
    {
        println!("info: {line}");
    }
    match parse_args(&args) {
        Ok(Parsed::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Parsed::Run(args)) => match run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("\nerror: {e}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("error: {message}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

/// Evaluates the held-out test split and prints the before/after figures. The oracle pass costs
/// a probe per free tier and step, so it runs at fast fidelity only.
fn evaluate_test_split(runner: &TrainingRunner, args: &CliArgs) -> Result<(), Box<dyn Error>> {
    let entries = &runner.split().test;
    let fast = args.eval_fidelity == EvalFidelity::Fast;
    println!(
        "\nTest split: {} designs, {} optics{}",
        entries.len(),
        if fast { "fast" } else { "full-sweep" },
        if fast {
            ""
        } else {
            " (model and sampled pass; no oracle pass)"
        }
    );
    // The designs are played in parallel on the CPU (the solver, mesh and tracer behind every
    // environment step are CPU code, the network forward is negligible), one pass per design.
    let started = Instant::now();
    let passes = EvalPasses {
        sampled: true,
        oracle: fast,
    };
    let report = runner.evaluate(entries, args.eval_fidelity, args.seed, passes)?;
    println!(
        "  evaluated in {:.1}s on {} threads",
        started.elapsed().as_secs_f32(),
        evaluation_threads(entries.len())
    );
    if report.designs == 0 {
        println!(
            "  no design could be evaluated ({})",
            report.first_failure.unwrap_or_default()
        );
    } else {
        println!(
            "  score before {:.2} -> after {:.2}, {}, {:.2} evals/step",
            report.mean_base_score,
            report.mean_final_score,
            describe_drops(
                report.mean_improvement,
                report.mean_improvement_oracle,
                report.mean_improvement_sampled
            ),
            report.mean_evaluations_per_step
        );
        println!(
            "  brilliance {:.1}% -> {:.1}%, windowing {:.1}% -> {:.1}%, extinction {:.1}% -> {:.1}%",
            report.base_optics.brilliance,
            report.final_optics.brilliance,
            report.base_optics.windowing,
            report.final_optics.windowing,
            report.base_optics.extinction,
            report.final_optics.extinction
        );
        if report.failed > 0 {
            println!("  {} designs could not be evaluated", report.failed);
        }
    }
    Ok(())
}

/// Caps the console noise of skipped designs: the first [`SKIP_LINES_FULL`] skips are shown in
/// full, every [`SKIP_SUMMARY_EVERY`] further ones as one summary line naming the latest. The
/// total is kept for the end-of-run summary.
#[derive(Debug, Default)]
struct SkipLog {
    /// Skip warnings seen so far.
    total: usize,
    /// Skips since the last summary line.
    pending: usize,
}

impl SkipLog {
    /// The line to show for the skip warning with `detail` (`'<title>': <reason>`), if any.
    fn line(&mut self, detail: &str) -> Option<String> {
        self.total += 1;
        if self.total <= SKIP_LINES_FULL {
            return Some(format!("{SKIPPED_PREFIX}{detail}"));
        }
        self.pending += 1;
        if self.pending < SKIP_SUMMARY_EVERY {
            return None;
        }
        self.pending = 0;
        Some(format!(
            "… {SKIP_SUMMARY_EVERY} more designs skipped (last: {detail})"
        ))
    }
}

/// Drains the runner's warnings and returns the lines to show: device fallbacks and the like
/// as they are, skipped designs through the cap of `skips`.
fn warning_lines(runner: &mut TrainingRunner, skips: &mut SkipLog) -> Vec<String> {
    runner
        .take_warnings()
        .into_iter()
        .filter_map(|warning| {
            if let Some(detail) = warning.strip_prefix(SKIPPED_PREFIX) {
                return skips.line(detail);
            }
            Some(warning)
        })
        .collect()
}

/// The data set the run trains on; designs skipped at load time are listed on stderr.
fn load_split(args: &CliArgs) -> Result<DatasetSplit, Box<dyn Error>> {
    let split = if args.fixture {
        DatasetSplit::fixture()
    } else {
        let records = load_db_designs()?;
        DatasetSplit::from_records(&records, DEFAULT_SPLIT_SEED, args.max_tiers)?
    };
    if !split.skipped.is_empty() {
        eprintln!(
            "warning: {} designs skipped at load time",
            split.skipped.len()
        );
        for s in split.skipped.iter().take(5) {
            eprintln!("  '{}': {}", s.title, s.reason);
        }
    }
    Ok(split)
}

/// Runs the validation pass and returns its report line; `None` when nothing could be
/// validated (the split is empty or no design evaluated).
fn run_validation(runner: &mut TrainingRunner) -> Result<Option<String>, Box<dyn Error>> {
    let designs = runner.config().validation_designs;
    let Some(v) = runner.validate(designs)?.filter(|v| v.designs > 0) else {
        return Ok(None);
    };
    let gap = runner.generalization_gap().map_or_else(
        || format!("n/a (needs {} designs)", runner::MIN_GAP_DESIGNS),
        |g| format!("{g:+.2}"),
    );
    let failed = if v.failed > 0 {
        format!(", {} designs failed", v.failed)
    } else {
        String::new()
    };
    Ok(Some(format!(
        "validation on {} unseen designs: score {:.2}, {}, {:.2} evals/step, generalization gap \
         {gap}, brilliance {:.1}%, windowing {:.1}%{failed}",
        v.designs,
        v.mean_final_score,
        describe_drops(
            v.mean_improvement,
            v.mean_improvement_oracle,
            v.mean_improvement_sampled
        ),
        v.mean_evaluations_per_step,
        v.final_optics.brilliance,
        v.final_optics.windowing
    )))
}

fn run(args: &CliArgs) -> Result<(), Box<dyn Error>> {
    for note in args.ignored_flag_notes() {
        eprintln!("note: {note}");
    }
    let split = load_split(args)?;
    let (train, val, test) = (split.train.len(), split.val.len(), split.test.len());
    let max_tiers = split.max_tiers;
    let mut runner = TrainingRunner::new(build_config(args), split)?;
    let mut skips = SkipLog::default();
    for line in warning_lines(&mut runner, &mut skips) {
        eprintln!("warning: {line}");
    }
    if let Some(path) = &args.resume {
        runner.load_checkpoint(Path::new(path))?;
        println!(
            "Resumed from {path} at iteration {}",
            runner.stats().iteration
        );
    }

    print_banner(&runner, args, (train, val, test), max_tiers);

    let mut console = Console::new();
    let started = Instant::now();
    let mut window_start = Instant::now();
    let mut window_steps = 0usize;
    let mut steps_per_sec = 0.0f32;
    let mut history: Vec<IterationMetricPoint> = Vec::new();
    let mut logs: Vec<(String, String, String)> = Vec::new();
    let mut note = |level: &str, message: String| {
        logs.push((rfc3339_utc_now(), level.to_owned(), message));
    };

    for iter in 1..=args.iterations {
        let report = runner
            .iterate(&mut || true)?
            .ok_or("the rollout ended early")?;
        for line in warning_lines(&mut runner, &mut skips) {
            console.warning(&format!("warning: {line}"));
            note("WARN", line);
        }

        window_steps += report.steps;
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_millis(500) || iter == 1 {
            steps_per_sec = window_steps as f32 / elapsed.as_secs_f32().max(1e-3);
            window_steps = 0;
            window_start = Instant::now();
        }

        // The record that matters is the largest score drop, not the lowest final score (which
        // mostly names the design that started lowest).
        if let Some(best) = &report.new_best_improvement {
            note(
                "BEST",
                format!(
                    "iteration {iter}: score drop {:.2} (from {:.2} to {:.2})",
                    best.improvement(),
                    best.base_score,
                    best.final_score
                ),
            );
        }
        if (iter.is_multiple_of(runner.config().validation_every) || iter == args.iterations)
            && let Some(line) = run_validation(&mut runner)?
        {
            console.message(&format!("  {line}"));
            note("VAL", line);
        }
        let point = runner.metric_point(steps_per_sec);
        print_progress(&mut console, &runner, &point, iter, args.iterations);
        history.push(point);
    }
    console.finish();

    print_summary(
        &runner,
        args,
        started.elapsed().as_secs_f32(),
        skips.total,
        &history,
    );
    finish_run(&runner, args, &history, &logs)
}

/// Evaluates the test split, then writes the telemetry and the checkpoint.
fn finish_run(
    runner: &TrainingRunner,
    args: &CliArgs,
    history: &[IterationMetricPoint],
    logs: &[(String, String, String)],
) -> Result<(), Box<dyn Error>> {
    if args.eval_test {
        evaluate_test_split(runner, args)?;
    }

    runner
        .export_stats(history, logs)
        .save_to_file(&args.stats_path)?;
    println!("\nTelemetry written to {}", args.stats_path);
    let weights_path = args
        .weights_path
        .clone()
        .unwrap_or_else(|| checkpoint_file_name(args.mode, args.tier, runner.stats().iteration));
    runner.save_checkpoint(Path::new(&weights_path))?;
    println!("Checkpoint written to {weights_path}");
    println!("Import the telemetry in the GUI with 'Import Run'.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_warnings_are_capped_to_ten_then_one_line_per_fifty() {
        let mut skips = SkipLog::default();
        for n in 1..=10 {
            let line = skips.line(&format!("'d{n}': dead stone")).unwrap();
            assert_eq!(line, format!("{SKIPPED_PREFIX}'d{n}': dead stone"));
        }
        // Skips 11 to 59 are silent; the 60th closes the first block of fifty.
        for n in 11..=59 {
            assert!(
                skips.line(&format!("'d{n}': dead stone")).is_none(),
                "skip {n}"
            );
        }
        assert_eq!(
            skips.line("'d60': dead stone").as_deref(),
            Some("… 50 more designs skipped (last: 'd60': dead stone)")
        );
        assert_eq!(skips.total, 60);
        for _ in 61..=109 {
            assert!(skips.line("'x': dead stone").is_none());
        }
        assert!(skips.line("'d110': dead stone").is_some());
        assert_eq!(skips.total, 110);
    }
}
