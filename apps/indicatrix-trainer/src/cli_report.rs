//! What the command-line trainer prints: the banner of the run, the one-line progress bar and the
//! end-of-run summary.
//!
//! The figures come from the run's own telemetry ([`IterationMetricPoint`]) and from the
//! validation report, and the phrases are built by small pure functions, so the algorithm-specific
//! parts (imitation shows loss, cosine and labels, PPO the critic's explained variance) can be
//! tested without a run.

use indicatrix_ai::{model::TrainingMode, training::LrSchedule};

use crate::{
    cli_args::CliArgs,
    cli_console::Console,
    dataset::MINERAL_SPECTRUM,
    runner::{Algorithm, ObjectiveSampling, RunConfig, TrainingRunner},
    stats::{IterationMetricPoint, describe_drops, drop_ratio},
};

/// Renders a sparkline of the last `max_chars` values in plain ASCII (block glyphs come out as
/// garbage on terminals without a Unicode font or charset, `PuTTY` among them).
fn render_sparkline(values: &[f32], max_chars: usize) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    let slice = &values[values.len().saturating_sub(max_chars)..];
    let min = slice.iter().copied().fold(f32::INFINITY, f32::min);
    let max = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(1e-4);
    let blocks = ['_', '.', ':', '-', '=', '+', '*', '#'];
    slice
        .iter()
        .map(|&v| {
            let norm = ((v - min) / range).clamp(0.0, 1.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "norm is clamped to 0..=1, so the rounded index is a small non-negative count"
            )]
            let idx = ((norm * (blocks.len() - 1) as f32).round() as usize).min(blocks.len() - 1);
            blocks[idx]
        })
        .collect()
}

/// The number of logical cores the process can use (at least 1).
fn core_count() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// The banner phrase of the parallel environments: `16 (cores 32)`.
fn describe_workers(workers: usize, cores: usize) -> String {
    format!("{workers} (cores {cores})")
}

/// The scales of a line search as one banner phrase: `1 / 0.5 / 0.25`.
fn describe_line_search(scales: &[f64]) -> String {
    scales
        .iter()
        .map(f64::to_string)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The training algorithm as one banner phrase.
const fn describe_algorithm(algo: Algorithm) -> &'static str {
    match algo {
        Algorithm::Imitation => {
            "imitation (label the rollout states with finite differences, regress onto them)"
        }
        Algorithm::Ppo => "ppo (clipped policy gradient on the step reward)",
    }
}

/// The objective sampling as one banner phrase.
const fn describe_objectives(sampling: ObjectiveSampling) -> &'static str {
    match sampling {
        ObjectiveSampling::Balanced => "balanced (the default weights in every episode)",
        ObjectiveSampling::Presets => "presets (uniform over the five non-tone presets)",
        ObjectiveSampling::Mixed => "mixed (presets 70 %, random weights 30 %)",
    }
}

/// The label rule as one banner phrase; `step_limit_deg` is the probe size when the run sets
/// none.
fn describe_labels(config: &RunConfig, step_limit_deg: f64) -> String {
    if config.label_every == 0 {
        return "off".to_owned();
    }
    let h = config.label_h_deg.unwrap_or(step_limit_deg);
    let rule = format!(
        "1 in {} rollout samples, probe h = {h:.2} deg",
        config.label_every
    );
    match config.algo {
        Algorithm::Imitation => {
            format!("{rule}, aggregate of the newest {}", config.label_capacity)
        }
        Algorithm::Ppo => format!("{rule} (telemetry only, the PPO loss ignores them)"),
    }
}

/// The learning-rate schedule as one banner phrase.
fn describe_schedule(schedule: LrSchedule) -> String {
    match schedule {
        LrSchedule::Constant => "constant".to_owned(),
        LrSchedule::Linear { total_updates } => {
            format!("linear decay to 0 over {total_updates} updates")
        }
    }
}

/// Prints the run configuration; `sizes` is the (train, val, test) design count.
pub fn print_banner(
    runner: &TrainingRunner,
    args: &CliArgs,
    sizes: (usize, usize, usize),
    max_tiers: usize,
) {
    let (train, val, test) = sizes;
    let tier = args.tier;
    println!("INDICATRIX RL TRAINER");
    println!(
        "  model        size {} (~{} parameters, {} layers, {} heads)",
        tier.code_str(),
        tier.param_count_approx(),
        tier.config().2,
        tier.config().1
    );
    println!("  backend      {}", runner.device_label());
    println!("  mode         {}", args.mode);
    println!(
        "  workers      {}",
        describe_workers(runner.config().workers, core_count())
    );
    print_tuning(runner, args);
    println!("  data set     {train} train / {val} val / {test} test, padded to {max_tiers} tiers");
    if val == 0 {
        println!("  warning      validation split is empty: no validation will run");
    }
    if args.mode == TrainingMode::Retarget {
        if args.spectrum {
            println!(
                "  targets      whole spectrum, n {:.2} to {:.2}",
                MINERAL_SPECTRUM.first().map_or(0.0, |m| m.refractive_index),
                MINERAL_SPECTRUM.last().map_or(0.0, |m| m.refractive_index)
            );
        } else {
            println!("  target       {}", args.target_material);
        }
    }
    println!("  scores       0-100, lower is better\n");
}

/// Banner lines of the algorithm and the settings in effect that decide how the run learns:
/// rollout, step limit, label rule, line search, objectives, exploration, learning rate and
/// schedule, and the validation cadence.
fn print_tuning(runner: &TrainingRunner, args: &CliArgs) {
    let config = runner.config();
    println!("  algorithm    {}", describe_algorithm(config.algo));
    let (mini_batch, epochs, unit) = match config.algo {
        Algorithm::Imitation => (
            config.imitation.mini_batch_size,
            config.imitation.epochs_per_round,
            "per round",
        ),
        Algorithm::Ppo => (
            config.ppo.mini_batch_size,
            config.ppo.epochs_per_batch,
            "per iteration",
        ),
    };
    println!(
        "  rollout      {} steps, mini-batch {mini_batch}, {epochs} epochs {unit}",
        config.rollout_size
    );
    println!(
        "  step limit   {:.2} degrees per tier and step ({})",
        runner.max_step_deg(),
        if args.max_step_deg.is_some() {
            "--max-step-deg"
        } else {
            "environment default"
        }
    );
    println!(
        "  labels       {}",
        describe_labels(config, runner.max_step_deg())
    );
    println!(
        "  line search  {}",
        describe_line_search(&config.line_search)
    );
    println!("  objectives   {}", describe_objectives(config.objectives));
    if config.algo == Algorithm::Imitation {
        println!(
            "  exploration  std {} on the policy mean",
            config.explore_std
        );
    }
    if config.mode == TrainingMode::Retarget {
        println!(
            "  same material {:.0} % of the episodes are aimed at the stone's own material",
            config.same_material_frac * 100.0
        );
    }
    print_learning(config);
    println!(
        "  validation   every {} iterations on {} held-out designs (model, oracle and sampled \
         pass on each design; the oracle is computed once)",
        config.validation_every, config.validation_designs
    );
}

/// Banner line of the learning rate, its schedule and the loss weights of the algorithm.
fn print_learning(config: &RunConfig) {
    match config.algo {
        Algorithm::Imitation => println!(
            "  learning     rate {:e}, {}; cosine coeff {}, gain coeff {}",
            config.imitation.learning_rate,
            describe_schedule(config.imitation.lr_schedule),
            config.imitation.cosine_coeff,
            config.imitation.gain_coeff
        ),
        Algorithm::Ppo => println!(
            "  learning     rate {:e}, {}; entropy bonus {}",
            config.ppo.learning_rate,
            describe_schedule(config.ppo.lr_schedule),
            config.ppo.entropy_coeff
        ),
    }
}

/// The figures that tell how the update is going, as one progress-line phrase: loss, direction
/// cosine and aggregate size for imitation, the critic's explained variance for PPO.
fn learning_figures(point: &IterationMetricPoint, imitation: bool) -> String {
    if imitation {
        format!(
            "loss {:.3} | cos {} | labels {}",
            point.imitation_loss,
            point
                .train_cosine
                .map_or_else(|| "-".to_owned(), |c| format!("{c:+.2}")),
            point.labelled_samples
        )
    } else {
        format!(
            "ev {}",
            point
                .explained_variance
                .map_or_else(|| "-".to_owned(), |ev| format!("{ev:+.2}"))
        )
    }
}

/// Width of the progress bar in characters.
const BAR_WIDTH: usize = 20;

/// The parts of the progress line after iteration `iter` of `total`, most important first, so a
/// narrow console can drop the tail. `rewards` is the recent per-step reward history for the
/// sparkline. Plain ASCII only.
fn progress_segments(
    point: &IterationMetricPoint,
    imitation: bool,
    iter: usize,
    total: usize,
    rewards: &[f32],
) -> Vec<String> {
    let fraction = (iter as f32 / total.max(1) as f32).clamp(0.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "fraction is clamped to 0..=1, so the rounded bar fill is a small non-negative count"
    )]
    let filled = (fraction * BAR_WIDTH as f32).round() as usize;
    let bar = "#".repeat(filled) + &"-".repeat(BAR_WIDTH - filled);
    let evaluations = point
        .evaluations_per_sec()
        .map_or_else(|| "-".to_owned(), |rate| format!("{rate:.0}"));
    let mut segments = vec![
        format!("[{bar}] {:>5.1}%", fraction * 100.0),
        format!("iter {iter:>4}/{total}"),
        format!(
            "best {}",
            point
                .best_score
                .map_or_else(|| "-".to_owned(), |b| format!("{b:.2}"))
        ),
        format!(
            "drop {}",
            point
                .mean_improvement
                .map_or_else(|| "-".to_owned(), |d| format!("{d:.2}"))
        ),
        learning_figures(point, imitation),
    ];
    if let Some(ratio) = point
        .val_improvement
        .and_then(|model| drop_ratio(model, point.val_improvement_oracle))
    {
        segments.push(format!("model/oracle {ratio:.2}"));
    }
    segments.extend([
        format!("acc {:.0}%", point.accepted_frac * 100.0),
        format!("{:>6.0} steps/s", point.steps_per_sec),
        format!("{evaluations:>6} evals/s"),
        format!("reward [{}]", render_sparkline(rewards, 16)),
    ]);
    segments
}

/// The full progress line (every segment), as a log file without a console width gets it.
#[cfg(test)]
fn progress_line(
    point: &IterationMetricPoint,
    imitation: bool,
    iter: usize,
    total: usize,
    rewards: &[f32],
) -> String {
    progress_segments(point, imitation, iter, total, rewards).join(" | ")
}

/// Shows the progress after iteration `iter` of `total` on the console's live line.
pub fn print_progress(
    console: &mut Console,
    runner: &TrainingRunner,
    point: &IterationMetricPoint,
    iter: usize,
    total: usize,
) {
    let rewards: Vec<f32> = runner.stats().reward_history.iter().copied().collect();
    let imitation = runner.config().algo == Algorithm::Imitation;
    console.progress(&progress_segments(point, imitation, iter, total, &rewards));
}

/// Mean rollout, label and update seconds per iteration over `history`; `None` for an empty one.
fn mean_phase_seconds(history: &[IterationMetricPoint]) -> Option<(f32, f32, f32)> {
    if history.is_empty() {
        return None;
    }
    let n = history.len() as f32;
    let mean = |seconds: fn(&IterationMetricPoint) -> f32| -> f32 {
        history.iter().map(seconds).sum::<f32>() / n
    };
    Some((
        mean(|p| p.rollout_secs),
        mean(|p| p.label_secs),
        mean(|p| p.update_secs),
    ))
}

/// Candidate evaluations per second over the whole run (rollout and update time); `None` when no
/// evaluation or no time was recorded.
fn overall_evaluations_per_sec(history: &[IterationMetricPoint]) -> Option<f32> {
    let evaluations: usize = history.iter().map(|p| p.evaluations_this_round).sum();
    let seconds: f32 = history.iter().map(|p| p.rollout_secs + p.update_secs).sum();
    (evaluations > 0 && seconds > 0.0).then_some(evaluations as f32 / seconds)
}

/// Summary lines of where the time went: the workers, the mean seconds per phase and iteration,
/// and the evaluation rate.
fn print_utilisation(workers: usize, history: &[IterationMetricPoint]) {
    println!(
        "  workers                {}",
        describe_workers(workers, core_count())
    );
    if let Some((rollout, label, update)) =
        mean_phase_seconds(history).filter(|&(r, l, u)| r + l + u > 0.0)
    {
        println!(
            "  seconds per iteration  rollout {rollout:.2} / label {label:.2} / update {update:.2}"
        );
    }
    if let Some(rate) = overall_evaluations_per_sec(history) {
        println!("  evaluations per second {rate:.0}");
    }
}

/// Prints the end-of-run totals; `elapsed_secs` is the wall time of the training loop and
/// `skipped` the number of designs whose environment could not be built.
pub fn print_summary(
    runner: &TrainingRunner,
    args: &CliArgs,
    elapsed_secs: f32,
    skipped: usize,
    history: &[IterationMetricPoint],
) {
    let s = runner.stats();
    println!(
        "\nTraining complete: {} iterations, {} steps, {} episodes in {elapsed_secs:.1}s",
        args.iterations, s.total_steps, s.episodes,
    );
    if let Some(best) = &s.best_improvement {
        println!(
            "  largest score drop     {:.2} ({:.2} -> {:.2})",
            best.improvement(),
            best.base_score,
            best.final_score
        );
    }
    if let Some(best) = &s.best {
        println!("  lowest episode score   {:.2}", best.final_score);
    }
    if let Some(d) = s.mean_improvement() {
        println!("  recent mean score drop {d:.2}");
    }
    if let Some(v) = s.last_validation.as_ref().filter(|v| v.designs > 0) {
        println!(
            "  validation             score {:.2}, {}, {:.2} evals/step, brilliance {:.1}%, \
             windowing {:.1}%",
            v.mean_final_score,
            describe_drops(
                v.mean_improvement,
                v.mean_improvement_oracle,
                v.mean_improvement_sampled
            ),
            v.mean_evaluations_per_step,
            v.final_optics.brilliance,
            v.final_optics.windowing
        );
    }
    print_utilisation(runner.config().workers, history);
    if skipped > 0 {
        println!("  skipped designs        {skipped} (their environment could not be built)");
    }
}

#[cfg(test)]
#[path = "cli_report_tests.rs"]
mod tests;
