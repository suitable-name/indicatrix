//! Flags of the command-line trainer: the help text, the parser, the cross-flag checks and the
//! [`RunConfig`] the flags ask for.
//!
//! A flag that is absent keeps the built-in value. Three settings resolve to the default of the
//! chosen algorithm instead of one common value (mini-batch size, epoch count and label period);
//! that resolution happens in [`build_config`], not in [`CliArgs::defaults`], so a flag that was
//! given can be told from one that was not.

use indicatrix_ai::{
    model::{ModelTier, TrainingMode},
    training::{ImitationConfig, LrSchedule},
};

use crate::{
    dataset::DEFAULT_MAX_TIERS_CAP,
    runner::{
        self, Algorithm, DeviceChoice, EvalFidelity, MAX_WORKERS, ObjectiveSampling, RunConfig,
        config::{default_workers, linear_schedule_for},
    },
};

/// The text printed by `--help` and after a usage error.
pub const HELP: &str = "\
indicatrix-trainer-cli: training of the Indicatrix cut optimizer / retarget policies

USAGE:
    indicatrix-trainer-cli [OPTIONS]

OPTIONS:
    --mode <optimizer|retarget>   Task to train (default: retarget)
    --tier <s|m|l|xl|xxl>         Model size tier (default: m)
    --device <cpu|gpu|auto>       Compute backend; 'gpu' fails when no adapter works, 'auto'
                                  falls back to the CPU with a warning (default: auto)
    --algo <imitation|ppo>        Training algorithm: 'imitation' labels the states of its own
                                  rollouts with finite differences and regresses the network onto
                                  them; 'ppo' is the reinforcement-learning baseline
                                  (default: imitation)
    --iterations <N>              Iterations to run, each one rollout and one update (default: 100)
    --workers <N>                 Parallel environments, 1-1024 (default: the core count)
    --batch-size <N>              Environment steps per iteration, 16-4096 (default: 256)
    --mini-batch <N>              Mini-batch size of both algorithms, 8-1024, not above the batch
                                  size (default: 256 for imitation, 64 for PPO)
    --epochs <N>                  Epochs per round (imitation, default: 2) or per iteration (PPO,
                                  default: 4), 1-16
    --validation-designs <N>      Held-out designs in the fixed validation subset, 1-64 (default: 16)
    --validation-every <N>        Validate every N iterations (and after the last), 1-1000
                                  (default: 10)
    --label-every <N>             Every N-th rollout sample is labelled with the measured per-tier
                                  sensitivities and the oracle step, 0-64; 0 = no labels, which
                                  imitation refuses (default: 2 for imitation; 0 for PPO, whose
                                  loss does not use them)
    --label-h <X>                 Probe size of the labels in degrees, above 0
                                  (default: the step limit)
    --label-capacity <N>          Labelled states the imitation aggregate keeps (the oldest are
                                  dropped first), 1-1000000 (default: 50000)
    --explore-std <X>             Exploration noise on the policy mean in imitation rollouts,
                                  pre-squash units, 0 or more (default: 0.3)
    --line-search <a,b,c>         Step scales tried in this order, each above 0; the first one
                                  that improves the score is kept (default: 1,0.5,0.25)
    --objectives <balanced|presets|mixed>
                                  How the objective weights of an episode are drawn: the default
                                  weights, one of the five non-tone presets, or presets 70 % and
                                  random weights 30 % (default: mixed)
    --same-material-frac <X>      Share of retarget episodes whose target is the stone's own
                                  material, 0 to 1 (default: 0.25)
    --imitation-lr <X>            Imitation peak learning rate, decayed linearly over the updates a
                                  full aggregate allows (default: 3e-4)
    --cosine-coeff <X>            Weight of the direction-cosine loss term, 0 or more
                                  (default: 0.5)
    --gain-coeff <X>              Weight of the gain-regression loss term, 0 or more
                                  (default: 0.1)
    --lr <X>                      PPO peak learning rate, decayed linearly to 0 over the run
                                  (default: the built-in PPO value, shown in the banner)
    --max-step-deg <X>            Step limit per tier and step, in degrees, above 0
                                  (default: the environment's own, shown in the banner)
    --entropy-coeff <X>           PPO entropy bonus coefficient, 0 or more
                                  (default: the built-in PPO value)
    --seed <N>                    Seed of sampling, shuffling and initialisation (default: 42)
    --max-tiers <N>               Skip designs with more tiers, 1-64 (default: 32)
    --no-corpus                   Train on the first train design only
    --no-spectrum                 Use one fixed target material instead of the whole spectrum
    --target-material <NAME>      Fixed target with --no-spectrum (default: Diamond)
    --eval-test                   Evaluate on the held-out test split at the end
    --eval-fidelity <fast|full>   Optics used for --eval-test; 'full' takes ~3 s per design and
                                  skips the oracle pass (default: fast)
    --fixture                     Train on the built-in concave fixture instead of the library
    --no-update-check             Do not ask GitHub for a newer release before starting (also
                                  off when INDICATRIX_NO_UPDATE_CHECK is set)
    --resume <PATH>               Continue from a checkpoint (mode and tier must match; the reward
                                  scale and the Adam moments are re-estimated)
    --save-stats <PATH>           Telemetry JSON output (default: training_stats.json)
    --save-weights <PATH>         Checkpoint output (default: indicatrix-<mode>-<tier>-iter<N>.json)
    -h, --help                    Print this help

The design library is found through the DIAGRAM_DB_PATH environment variable, or as
facet_diagrams.sqlite in the working directory or up to three directories above it.
";

/// PPO mini-batch size when `--mini-batch` is absent (the runner's own default).
const PPO_MINI_BATCH: usize = 64;
/// PPO epochs per iteration when `--epochs` is absent (the runner's own default).
const PPO_EPOCHS: usize = 4;

/// The parsed command line.
#[derive(Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the command line; corpus, spectrum, eval_test and fixture are independent switches"
)]
pub struct CliArgs {
    pub mode: TrainingMode,
    pub tier: ModelTier,
    pub device: DeviceChoice,
    pub algo: Algorithm,
    pub iterations: usize,
    pub workers: usize,
    pub batch_size: usize,
    /// `None` keeps the mini-batch size of each algorithm (see [`CliArgs::mini_batch_for`]).
    pub mini_batch: Option<usize>,
    /// `None` keeps the epoch count of each algorithm (see [`CliArgs::epochs_for`]).
    pub epochs: Option<usize>,
    pub validation_designs: usize,
    pub validation_every: usize,
    /// Label period; `None` = 2 for imitation, 0 for PPO.
    pub label_every: Option<usize>,
    /// Probe size of the labels in degrees; `None` = the step limit.
    pub label_h_deg: Option<f64>,
    /// Capacity of the imitation aggregate; `None` keeps the built-in value.
    pub label_capacity: Option<usize>,
    /// Exploration noise of the imitation rollouts; `None` keeps the built-in value.
    pub explore_std: Option<f32>,
    /// Line-search scales; `None` keeps the built-in list.
    pub line_search: Option<Vec<f64>>,
    pub objectives: ObjectiveSampling,
    /// Share of retarget episodes aimed at the stone's own material; `None` keeps the built-in
    /// value.
    pub same_material_frac: Option<f32>,
    /// Imitation peak learning rate; `None` keeps the built-in value.
    pub imitation_lr: Option<f32>,
    /// Weight of the direction-cosine loss term; `None` keeps the built-in value.
    pub cosine_coeff: Option<f32>,
    /// Weight of the gain-regression loss term; `None` keeps the built-in value.
    pub gain_coeff: Option<f32>,
    /// PPO peak learning rate; `None` keeps the built-in PPO value.
    pub learning_rate: Option<f32>,
    /// Step limit in degrees; `None` keeps each environment's own.
    pub max_step_deg: Option<f64>,
    /// PPO entropy bonus coefficient; `None` keeps the built-in PPO value.
    pub entropy_coeff: Option<f32>,
    pub seed: u64,
    pub max_tiers: usize,
    pub corpus: bool,
    pub spectrum: bool,
    pub target_material: String,
    pub eval_test: bool,
    pub eval_fidelity: EvalFidelity,
    pub fixture: bool,
    pub resume: Option<String>,
    pub stats_path: String,
    pub weights_path: Option<String>,
}

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    /// Print the help and stop.
    Help,
    /// Run the training.
    Run(Box<CliArgs>),
}

fn parse_number<T: std::str::FromStr>(flag: &str, text: &str) -> Result<T, String> {
    text.parse::<T>()
        .map_err(|_| format!("{flag} expects a whole number, got '{text}'"))
}

fn parse_range(flag: &str, text: &str, lo: usize, hi: usize) -> Result<usize, String> {
    let value: usize = parse_number(flag, text)?;
    if (lo..=hi).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{flag} must be between {lo} and {hi}, got {value}"))
    }
}

/// Parses a finite decimal number above 0 (or 0 or more with `allow_zero`).
fn parse_decimal(flag: &str, text: &str, allow_zero: bool) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("{flag} expects a number, got '{text}'"))?;
    let in_range = value.is_finite()
        && if allow_zero {
            value >= 0.0
        } else {
            value > 0.0
        };
    if in_range {
        Ok(value)
    } else {
        let bound = if allow_zero { "0 or more" } else { "above 0" };
        Err(format!(
            "{flag} must be a finite number {bound}, got '{text}'"
        ))
    }
}

/// [`parse_decimal`] narrowed to `f32`; a value that underflows to 0 or overflows is refused.
fn parse_decimal_f32(flag: &str, text: &str, allow_zero: bool) -> Result<f32, String> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "narrowing is the point; the result is checked for overflow and underflow right below"
    )]
    let value = parse_decimal(flag, text, allow_zero)? as f32;
    if value.is_finite() && (allow_zero || value > 0.0) {
        Ok(value)
    } else {
        Err(format!(
            "{flag} is out of range for a 32-bit number, got '{text}'"
        ))
    }
}

/// Parses a share between 0 and 1 (both included).
fn parse_fraction(flag: &str, text: &str) -> Result<f32, String> {
    let value = parse_decimal_f32(flag, text, true)?;
    if value <= 1.0 {
        Ok(value)
    } else {
        Err(format!("{flag} must be between 0 and 1, got '{text}'"))
    }
}

/// Parses a comma-separated list of step scales, each a finite number above 0 (`1,0.5,0.25`).
/// An empty list or an empty item (`1,`) is refused.
fn parse_scales(flag: &str, text: &str) -> Result<Vec<f64>, String> {
    text.split(',')
        .map(|item| parse_decimal(flag, item.trim(), false))
        .collect()
}

fn parse_mode(text: &str) -> Result<TrainingMode, String> {
    match text {
        "optimizer" => Ok(TrainingMode::Optimizer),
        "retarget" => Ok(TrainingMode::Retarget),
        other => Err(format!(
            "--mode expects optimizer or retarget, got '{other}'"
        )),
    }
}

fn parse_device(text: &str) -> Result<DeviceChoice, String> {
    match text {
        "cpu" => Ok(DeviceChoice::Cpu),
        "gpu" => Ok(DeviceChoice::Gpu),
        "auto" => Ok(DeviceChoice::Auto),
        other => Err(format!("--device expects cpu, gpu or auto, got '{other}'")),
    }
}

fn parse_fidelity(text: &str) -> Result<EvalFidelity, String> {
    match text {
        "fast" => Ok(EvalFidelity::Fast),
        "full" => Ok(EvalFidelity::Full),
        other => Err(format!(
            "--eval-fidelity expects fast or full, got '{other}'"
        )),
    }
}

/// The value following the flag at `*i`, advancing `*i` past it.
fn take<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} expects a value"))
}

impl CliArgs {
    /// Handles the training-tuning flags; `Ok(false)` when `flag` is not one of them.
    fn parse_tuning_flag(
        &mut self,
        flag: &str,
        args: &[String],
        i: &mut usize,
    ) -> Result<bool, String> {
        match flag {
            "--validation-every" => {
                self.validation_every = parse_range(flag, take(args, i, flag)?, 1, 1000)?;
            }
            "--lr" => {
                self.learning_rate = Some(parse_decimal_f32(flag, take(args, i, flag)?, false)?);
            }
            "--max-step-deg" => {
                self.max_step_deg = Some(parse_decimal(flag, take(args, i, flag)?, false)?);
            }
            "--entropy-coeff" => {
                self.entropy_coeff = Some(parse_decimal_f32(flag, take(args, i, flag)?, true)?);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Handles the algorithm flags (algorithm, labels, exploration, objectives, imitation loss);
    /// `Ok(false)` when `flag` is not one of them.
    fn parse_algorithm_flag(
        &mut self,
        flag: &str,
        args: &[String],
        i: &mut usize,
    ) -> Result<bool, String> {
        match flag {
            "--algo" => {
                let text = take(args, i, flag)?;
                self.algo = Algorithm::parse(text)
                    .ok_or_else(|| format!("--algo expects imitation or ppo, got '{text}'"))?;
            }
            "--label-every" => {
                self.label_every = Some(parse_range(flag, take(args, i, flag)?, 0, 64)?);
            }
            "--label-h" => {
                self.label_h_deg = Some(parse_decimal(flag, take(args, i, flag)?, false)?);
            }
            "--label-capacity" => {
                self.label_capacity = Some(parse_range(flag, take(args, i, flag)?, 1, 1_000_000)?);
            }
            "--explore-std" => {
                self.explore_std = Some(parse_decimal_f32(flag, take(args, i, flag)?, true)?);
            }
            "--line-search" => {
                self.line_search = Some(parse_scales(flag, take(args, i, flag)?)?);
            }
            "--objectives" => {
                let text = take(args, i, flag)?;
                self.objectives = ObjectiveSampling::parse(text).ok_or_else(|| {
                    format!("--objectives expects balanced, presets or mixed, got '{text}'")
                })?;
            }
            "--same-material-frac" => {
                self.same_material_frac = Some(parse_fraction(flag, take(args, i, flag)?)?);
            }
            "--imitation-lr" => {
                self.imitation_lr = Some(parse_decimal_f32(flag, take(args, i, flag)?, false)?);
            }
            "--cosine-coeff" => {
                self.cosine_coeff = Some(parse_decimal_f32(flag, take(args, i, flag)?, true)?);
            }
            "--gain-coeff" => {
                self.gain_coeff = Some(parse_decimal_f32(flag, take(args, i, flag)?, true)?);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The documented defaults of every flag.
    fn defaults() -> Self {
        Self {
            mode: TrainingMode::Retarget,
            tier: ModelTier::Medium,
            device: DeviceChoice::Auto,
            algo: Algorithm::default(),
            iterations: 100,
            workers: default_workers(),
            batch_size: 256,
            mini_batch: None,
            epochs: None,
            validation_designs: runner::config::DEFAULT_VALIDATION_DESIGNS,
            validation_every: runner::config::DEFAULT_VALIDATION_EVERY,
            label_every: None,
            label_h_deg: None,
            label_capacity: None,
            explore_std: None,
            line_search: None,
            objectives: ObjectiveSampling::default(),
            same_material_frac: None,
            imitation_lr: None,
            cosine_coeff: None,
            gain_coeff: None,
            learning_rate: None,
            max_step_deg: None,
            entropy_coeff: None,
            seed: 42,
            max_tiers: DEFAULT_MAX_TIERS_CAP,
            corpus: true,
            spectrum: true,
            target_material: "Diamond".to_owned(),
            eval_test: false,
            eval_fidelity: EvalFidelity::Fast,
            fixture: false,
            resume: None,
            stats_path: "training_stats.json".to_owned(),
            weights_path: None,
        }
    }

    /// The mini-batch size `algo` trains with: the flag, which applies to both algorithms, or
    /// the algorithm's own default (256 for imitation, 64 for PPO).
    fn mini_batch_for(&self, algo: Algorithm) -> usize {
        self.mini_batch.unwrap_or_else(|| match algo {
            Algorithm::Imitation => ImitationConfig::default().mini_batch_size,
            Algorithm::Ppo => PPO_MINI_BATCH,
        })
    }

    /// The epoch count `algo` trains with: the flag, which applies to both algorithms (per round
    /// for imitation, per iteration for PPO), or the algorithm's own default (2 and 4).
    fn epochs_for(&self, algo: Algorithm) -> usize {
        self.epochs.unwrap_or_else(|| match algo {
            Algorithm::Imitation => ImitationConfig::default().epochs_per_round,
            Algorithm::Ppo => PPO_EPOCHS,
        })
    }

    /// Checks that span several flags, after every flag was read.
    fn validate(&self) -> Result<(), String> {
        // A PPO mini-batch always has to fit the rollout. The imitation mini-batch draws from
        // the aggregate, which outgrows the rollout, so only an explicit flag is checked there.
        let mini_batch = self.mini_batch_for(self.algo);
        if (self.mini_batch.is_some() || self.algo == Algorithm::Ppo)
            && mini_batch > self.batch_size
        {
            return Err(format!(
                "--mini-batch ({mini_batch}) must not exceed --batch-size ({})",
                self.batch_size
            ));
        }
        if self.algo == Algorithm::Imitation && self.label_every == Some(0) {
            return Err(
                "--label-every 0 leaves --algo imitation without labels to learn from; use 1 to \
                 64, or --algo ppo"
                    .to_owned(),
            );
        }
        Ok(())
    }

    /// Notes on flags that were given but belong to the other algorithm and so have no effect.
    #[must_use]
    pub fn ignored_flag_notes(&self) -> Vec<String> {
        let (applies_to, flags): (&str, Vec<(&str, bool)>) = match self.algo {
            Algorithm::Imitation => (
                "ppo",
                vec![
                    ("--lr", self.learning_rate.is_some()),
                    ("--entropy-coeff", self.entropy_coeff.is_some()),
                ],
            ),
            Algorithm::Ppo => (
                "imitation",
                vec![
                    ("--imitation-lr", self.imitation_lr.is_some()),
                    ("--cosine-coeff", self.cosine_coeff.is_some()),
                    ("--gain-coeff", self.gain_coeff.is_some()),
                ],
            ),
        };
        flags
            .into_iter()
            .filter(|&(_, given)| given)
            .map(|(flag, _)| format!("{flag} applies to --algo {applies_to} only and is ignored"))
            .collect()
    }
}

/// Parses the command line (without the program name).
///
/// # Errors
///
/// A message naming the offending flag for an unknown argument, a missing or malformed value, a
/// value outside its range, or a combination of flags that cannot work together.
pub fn parse_args(args: &[String]) -> Result<Parsed, String> {
    let mut parsed = CliArgs::defaults();

    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--mode" => parsed.mode = parse_mode(take(args, &mut i, flag)?)?,
            "--tier" => {
                let text = take(args, &mut i, flag)?;
                parsed.tier = ModelTier::from_str_loose(text)
                    .ok_or_else(|| format!("--tier expects s, m, l, xl or xxl, got '{text}'"))?;
            }
            "--device" => parsed.device = parse_device(take(args, &mut i, flag)?)?,
            "--iterations" => {
                parsed.iterations = parse_range(flag, take(args, &mut i, flag)?, 1, usize::MAX)?;
            }
            "--workers" => {
                parsed.workers = parse_range(flag, take(args, &mut i, flag)?, 1, MAX_WORKERS)?;
            }
            "--batch-size" => {
                parsed.batch_size = parse_range(flag, take(args, &mut i, flag)?, 16, 4096)?;
            }
            "--mini-batch" => {
                parsed.mini_batch = Some(parse_range(flag, take(args, &mut i, flag)?, 8, 1024)?);
            }
            "--epochs" => {
                parsed.epochs = Some(parse_range(flag, take(args, &mut i, flag)?, 1, 16)?);
            }
            "--validation-designs" => {
                parsed.validation_designs = parse_range(flag, take(args, &mut i, flag)?, 1, 64)?;
            }
            "--seed" => parsed.seed = parse_number(flag, take(args, &mut i, flag)?)?,
            "--max-tiers" => {
                parsed.max_tiers = parse_range(flag, take(args, &mut i, flag)?, 1, 64)?;
            }
            "--no-corpus" => parsed.corpus = false,
            "--no-spectrum" => parsed.spectrum = false,
            "--eval-test" => parsed.eval_test = true,
            "--fixture" => parsed.fixture = true,
            "--eval-fidelity" => parsed.eval_fidelity = parse_fidelity(take(args, &mut i, flag)?)?,
            "--target-material" => {
                take(args, &mut i, flag)?.clone_into(&mut parsed.target_material);
            }
            "--resume" => parsed.resume = Some(take(args, &mut i, flag)?.to_owned()),
            "--save-stats" => take(args, &mut i, flag)?.clone_into(&mut parsed.stats_path),
            "--save-weights" => parsed.weights_path = Some(take(args, &mut i, flag)?.to_owned()),
            other => {
                if !parsed.parse_tuning_flag(other, args, &mut i)?
                    && !parsed.parse_algorithm_flag(other, args, &mut i)?
                {
                    return Err(format!("unknown argument '{other}'"));
                }
            }
        }
        i += 1;
    }
    parsed.validate()?;
    Ok(Parsed::Run(Box::new(parsed)))
}

/// The linear learning-rate decay of an imitation run of `iterations` rounds: one optimizer step
/// per mini-batch, `epochs` passes per round over the aggregate. The aggregate holds at most
/// `label_capacity` states, so `iterations * epochs * ceil(label_capacity / mini_batch)` is an
/// upper bound of the updates a run can take. While the aggregate is smaller than its capacity
/// (short runs, a sparse label rule) fewer steps happen and the rate ends above zero.
fn imitation_schedule_for(
    iterations: usize,
    label_capacity: usize,
    mini_batch: usize,
    epochs: usize,
) -> LrSchedule {
    linear_schedule_for(iterations, label_capacity, mini_batch, epochs)
}

/// Applies the PPO flags. The PPO mini-batch is capped at the rollout size so that the unused
/// default of an imitation run on a small `--batch-size` cannot fail the PPO validation.
fn apply_ppo(config: &mut RunConfig, args: &CliArgs) {
    config.ppo.mini_batch_size = args.mini_batch_for(Algorithm::Ppo).min(args.batch_size);
    config.ppo.epochs_per_batch = args.epochs_for(Algorithm::Ppo);
    if let Some(lr) = args.learning_rate {
        config.ppo.learning_rate = lr;
    }
    if let Some(coeff) = args.entropy_coeff {
        config.ppo.entropy_coeff = coeff;
    }
    config.ppo.lr_schedule = linear_schedule_for(
        args.iterations,
        args.batch_size,
        config.ppo.mini_batch_size,
        config.ppo.epochs_per_batch,
    );
}

/// Applies the imitation flags and the learning-rate decay bound of [`imitation_schedule_for`].
fn apply_imitation(config: &mut RunConfig, args: &CliArgs) {
    let imitation = &mut config.imitation;
    imitation.mini_batch_size = args.mini_batch_for(Algorithm::Imitation);
    imitation.epochs_per_round = args.epochs_for(Algorithm::Imitation);
    if let Some(lr) = args.imitation_lr {
        imitation.learning_rate = lr;
    }
    if let Some(coeff) = args.cosine_coeff {
        imitation.cosine_coeff = coeff;
    }
    if let Some(coeff) = args.gain_coeff {
        imitation.gain_coeff = coeff;
    }
    imitation.lr_schedule = imitation_schedule_for(
        args.iterations,
        config.label_capacity,
        imitation.mini_batch_size,
        imitation.epochs_per_round,
    );
}

/// The run configuration the flags ask for. The learning rate decays linearly to zero over the
/// requested iterations (review TQ-20); a flag that is not given keeps the built-in value, and
/// the mini-batch size, the epoch count and the label period resolve to the default of the
/// chosen algorithm.
#[must_use]
pub fn build_config(args: &CliArgs) -> RunConfig {
    let mut config = RunConfig::new(args.mode, args.tier);
    config.device = args.device;
    config.workers = args.workers;
    config.rollout_size = args.batch_size;
    config.corpus_mode = args.corpus;
    config.universal_spectrum = args.spectrum;
    config.fixed_target.clone_from(&args.target_material);
    config.seed = args.seed;
    config.validation_designs = args.validation_designs;
    config.validation_every = args.validation_every;
    config.max_step_deg = args.max_step_deg;
    config.algo = args.algo;
    config.objectives = args.objectives;
    // PPO ignores the labels in its loss, so it only pays for them when asked to.
    config.label_every = args.label_every.unwrap_or(match args.algo {
        Algorithm::Imitation => config.label_every,
        Algorithm::Ppo => 0,
    });
    config.label_h_deg = args.label_h_deg;
    if let Some(capacity) = args.label_capacity {
        config.label_capacity = capacity;
    }
    if let Some(std) = args.explore_std {
        config.explore_std = std;
    }
    if let Some(scales) = &args.line_search {
        config.line_search.clone_from(scales);
    }
    if let Some(frac) = args.same_material_frac {
        config.same_material_frac = frac;
    }
    apply_ppo(&mut config, args);
    apply_imitation(&mut config, args);
    config
}

#[cfg(test)]
#[path = "cli_args_tests.rs"]
mod tests;
