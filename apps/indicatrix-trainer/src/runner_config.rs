//! The run configuration: what a run trains, how, and the limits every value must respect.

use indicatrix_ai::{
    env::DEFAULT_LINE_SEARCH,
    model::{ModelTier, TrainingMode},
    training::{ImitationConfig, LrSchedule, PpoConfig},
};

use super::{Algorithm, ObjectiveSampling, RunnerError};

/// Largest worker count. A worker is one environment (a design and two score evaluations), so
/// hundreds are cheap; the threads that run them follow the core count, not this number.
pub const MAX_WORKERS: usize = 1024;

/// Default of [`RunConfig::label_every`]: every second rollout sample gets a label.
pub const DEFAULT_LABEL_EVERY: usize = 2;
/// Largest value of [`RunConfig::label_every`].
pub const MAX_LABEL_EVERY: usize = 64;
/// Default of [`RunConfig::label_capacity`].
pub const DEFAULT_LABEL_CAPACITY: usize = 50_000;
/// Default of [`RunConfig::explore_std`], in pre-squash units.
pub const DEFAULT_EXPLORE_STD: f32 = 0.3;
/// Default of [`RunConfig::same_material_frac`].
pub const DEFAULT_SAME_MATERIAL_FRAC: f32 = 0.25;

/// Default of [`RunConfig::validation_designs`]: enough designs that the pass-to-pass noise of the
/// mean is small against the generalization-gap threshold. A pass costs about a second since the
/// designs are played in parallel.
pub const DEFAULT_VALIDATION_DESIGNS: usize = 16;
/// Default of [`RunConfig::validation_every`]: often enough to see a trend within a run of a few
/// hundred iterations (every 15 left only 13 points in 200 iterations).
pub const DEFAULT_VALIDATION_EVERY: usize = 10;
/// Fewest validated designs for which a generalization gap is reported.
pub const MIN_GAP_DESIGNS: usize = 8;

/// Which compute backend to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceChoice {
    /// CPU SIMD.
    Cpu,
    /// GPU; failing to initialise it is an error.
    Gpu,
    /// GPU when it works, otherwise CPU with a warning.
    Auto,
}

/// The editable run configuration.
#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Task being trained.
    pub mode: TrainingMode,
    /// Model size tier.
    pub tier: ModelTier,
    /// Backend choice.
    pub device: DeviceChoice,
    /// Parallel environments (1..=[`MAX_WORKERS`]). The threads that step them follow the core
    /// count, so more workers than cores only means more episodes in flight.
    pub workers: usize,
    /// Environment steps per update.
    pub rollout_size: usize,
    /// PPO hyperparameters (epochs and mini-batch size included); read in PPO mode.
    pub ppo: PpoConfig,
    /// Cycle the train split across workers (vs one focus stone).
    pub corpus_mode: bool,
    /// Sample the retarget target from the whole spectrum (vs `fixed_target`).
    pub universal_spectrum: bool,
    /// Target material name used when `universal_spectrum` is off.
    pub fixed_target: String,
    /// Seed of sampling, shuffling and weight initialisation (and of the fixed validation targets).
    pub seed: u64,
    /// Held-out designs per validation pass: always the first this many of the validation split,
    /// so consecutive passes measure the same stones (GUI and CLI share this value).
    pub validation_designs: usize,
    /// A validation pass runs every this many iterations (GUI and CLI share this value).
    pub validation_every: usize,
    /// Step limit override in degrees per step and tier (`None` = each environment's default);
    /// applies to both environment kinds.
    pub max_step_deg: Option<f64>,
    /// The training algorithm.
    pub algo: Algorithm,
    /// Imitation hyperparameters (epochs per round, mini-batch size, loss weights); read in
    /// imitation mode.
    pub imitation: ImitationConfig,
    /// Label period: one in every `label_every` rollout samples gets a finite-difference label
    /// (0 = none; at most [`MAX_LABEL_EVERY`]). A labelled sample costs one or two candidate
    /// evaluations per free tier plus the oracle step. In imitation mode the labels are the
    /// training data (0 is then refused); in PPO mode they only feed the telemetry.
    pub label_every: usize,
    /// Probe size of a label in degrees (`None` = the step limit).
    pub label_h_deg: Option<f64>,
    /// Labelled states the imitation aggregate keeps; past it the oldest leave.
    pub label_capacity: usize,
    /// Standard deviation (pre-squash units) of the exploration noise of an imitation rollout;
    /// 0 rolls the deterministic policy out.
    pub explore_std: f32,
    /// The line-search scales of every step, tried in this order (the first improving candidate
    /// is accepted). Validation and the oracle use the same list.
    pub line_search: Vec<f64>,
    /// How the objective weights of an episode are drawn.
    pub objectives: ObjectiveSampling,
    /// Share of the retarget episodes that are judged in the design's own material (the
    /// optimizer task), so one model learns both tasks.
    pub same_material_frac: f32,
}

/// The default worker count: one environment per core (at least 1, at most [`MAX_WORKERS`]).
#[must_use]
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .clamp(1, MAX_WORKERS)
}

impl RunConfig {
    /// Defaults for `mode` and `tier`: imitation, one worker per core ([`default_workers`]), 128
    /// steps per update, a label on every second sample, exploration noise of 0.3, the default
    /// line search, mixed objective weights, a quarter of the retarget episodes in the design's
    /// own material, the environments' own step limit and a constant learning rate (the CLI
    /// selects a linear decay with [`linear_schedule_for`]; a GUI run has no fixed length). The
    /// PPO settings are 4 epochs of 64.
    #[must_use]
    pub fn new(mode: TrainingMode, tier: ModelTier) -> Self {
        Self {
            mode,
            tier,
            device: DeviceChoice::Auto,
            workers: default_workers(),
            rollout_size: 128,
            ppo: PpoConfig {
                epochs_per_batch: 4,
                mini_batch_size: 64,
                lr_schedule: LrSchedule::Constant,
                ..PpoConfig::default()
            },
            corpus_mode: true,
            universal_spectrum: true,
            fixed_target: "Diamond".to_owned(),
            seed: 42,
            validation_designs: DEFAULT_VALIDATION_DESIGNS,
            validation_every: DEFAULT_VALIDATION_EVERY,
            max_step_deg: None,
            algo: Algorithm::Imitation,
            imitation: ImitationConfig::default(),
            label_every: DEFAULT_LABEL_EVERY,
            label_h_deg: None,
            label_capacity: DEFAULT_LABEL_CAPACITY,
            explore_std: DEFAULT_EXPLORE_STD,
            line_search: DEFAULT_LINE_SEARCH.to_vec(),
            objectives: ObjectiveSampling::Mixed,
            same_material_frac: DEFAULT_SAME_MATERIAL_FRAC,
        }
    }

    /// Checks every value against its limits.
    ///
    /// # Errors
    ///
    /// [`RunnerError::Config`] naming the first offending value, or the trainer's own complaint
    /// about the PPO or imitation hyperparameters.
    pub(super) fn validate(&self) -> Result<(), RunnerError> {
        if !(1..=MAX_WORKERS).contains(&self.workers) {
            return Err(RunnerError::Config(format!(
                "workers must be 1..={MAX_WORKERS} (got {})",
                self.workers
            )));
        }
        if self.rollout_size == 0 {
            return Err(RunnerError::Config(
                "rollout size must be at least 1".into(),
            ));
        }
        if self.algo == Algorithm::Ppo && self.ppo.mini_batch_size > self.rollout_size {
            return Err(RunnerError::Config(format!(
                "mini-batch size ({}) must not exceed the rollout size ({})",
                self.ppo.mini_batch_size, self.rollout_size
            )));
        }
        if self.validation_designs == 0 {
            return Err(RunnerError::Config(
                "validation designs must be at least 1".into(),
            ));
        }
        if self.validation_every == 0 {
            return Err(RunnerError::Config(
                "validation interval must be at least 1 iteration".into(),
            ));
        }
        if let Some(h) = self.max_step_deg
            && !(h.is_finite() && h > 0.0)
        {
            return Err(RunnerError::Config(format!(
                "step limit must be a finite number of degrees above 0 (got {h})"
            )));
        }
        self.validate_labelling()?;
        self.ppo.validate()?;
        self.imitation.validate()?;
        Ok(())
    }

    /// The checks of the labelling, exploration, line-search and objective settings.
    fn validate_labelling(&self) -> Result<(), RunnerError> {
        if self.label_every > MAX_LABEL_EVERY {
            return Err(RunnerError::Config(format!(
                "label period must be 0..={MAX_LABEL_EVERY} (got {})",
                self.label_every
            )));
        }
        if self.algo == Algorithm::Imitation && self.label_every == 0 {
            return Err(RunnerError::Config(
                "imitation trains on labels: the label period must be at least 1".into(),
            ));
        }
        if let Some(h) = self.label_h_deg
            && !(h.is_finite() && h > 0.0)
        {
            return Err(RunnerError::Config(format!(
                "label probe size must be a finite number of degrees above 0 (got {h})"
            )));
        }
        if self.label_capacity == 0 {
            return Err(RunnerError::Config(
                "label capacity must be at least 1".into(),
            ));
        }
        if !(self.explore_std.is_finite() && self.explore_std >= 0.0) {
            return Err(RunnerError::Config(format!(
                "exploration noise must be finite and at least 0 (got {})",
                self.explore_std
            )));
        }
        if self.line_search.is_empty() {
            return Err(RunnerError::Config(
                "the line search needs at least one scale".into(),
            ));
        }
        if let Some(scale) = self
            .line_search
            .iter()
            .find(|scale| !(scale.is_finite() && **scale > 0.0))
        {
            return Err(RunnerError::Config(format!(
                "every line-search scale must be finite and above 0 (got {scale})"
            )));
        }
        if !(0.0..=1.0).contains(&self.same_material_frac) {
            return Err(RunnerError::Config(format!(
                "the same-material fraction must lie in 0..=1 (got {})",
                self.same_material_frac
            )));
        }
        Ok(())
    }
}

/// The linear learning-rate decay for a run of `iterations` PPO iterations: one optimizer step
/// per mini-batch, `ceil(rollout_size / mini_batch)` mini-batches per epoch and `epochs` epochs
/// per iteration (review TQ-20). When the KL stop ends epochs early fewer steps happen and the
/// rate ends above zero, which is the intent.
#[must_use]
pub fn linear_schedule_for(
    iterations: usize,
    rollout_size: usize,
    mini_batch: usize,
    epochs: usize,
) -> LrSchedule {
    let mini_batches = rollout_size.div_ceil(mini_batch.max(1));
    let total_updates = (iterations as u64)
        .saturating_mul(epochs as u64)
        .saturating_mul(mini_batches as u64);
    LrSchedule::Linear {
        total_updates: total_updates.max(1),
    }
}

/// `schedule` as it must read after `done` optimizer steps were already taken by the run a
/// checkpoint resumes: a linear schedule describes the steps still to run, so its end moves out
/// by the steps done (otherwise a resumed run would start at a rate of zero). Constant stays.
#[must_use]
pub const fn resumed_schedule(schedule: LrSchedule, done: u64) -> LrSchedule {
    match schedule {
        LrSchedule::Constant => LrSchedule::Constant,
        LrSchedule::Linear { total_updates } => LrSchedule::Linear {
            total_updates: total_updates.saturating_add(done),
        },
    }
}

/// The hyperparameters that may change while a run is live.
///
/// In imitation mode `learning_rate`, `mini_batch` and `ppo_epochs` are the imitation settings
/// (`imitation.learning_rate`, `imitation.mini_batch_size`, `imitation.epochs_per_round`); in PPO
/// mode they are the PPO ones. `clip_epsilon` and `gae_lambda` are PPO settings and are ignored
/// by an imitation run.
#[derive(Clone, Debug, PartialEq)]
pub struct Hyperparams {
    /// Parallel environments.
    pub workers: usize,
    /// Environment steps per update.
    pub rollout_size: usize,
    /// Mini-batch size.
    pub mini_batch: usize,
    /// Epochs per update (PPO epochs, or imitation epochs per round).
    pub ppo_epochs: usize,
    /// Adam learning rate.
    pub learning_rate: f32,
    /// PPO clipping threshold.
    pub clip_epsilon: f32,
    /// GAE lambda.
    pub gae_lambda: f32,
}
