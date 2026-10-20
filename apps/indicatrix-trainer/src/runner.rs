//! The training loop shared by the GUI worker thread and the command-line trainer.
//!
//! [`TrainingRunner`] owns the vectorized environments, the pure-Rust set-attention network that
//! validation and the checkpoints use, the Burn trainer that updates the model (and its rollout
//! copy), the labelled aggregate of an imitation run and the run statistics. Callers drive it
//! with three operations: [`TrainingRunner::iterate`] (collect a rollout, update the model),
//! [`TrainingRunner::evaluate`] / [`TrainingRunner::validate`] (episodes on held-out designs) and
//! the checkpoint methods. Everything else is configuration.
//!
//! # Overlap of rollout and update
//!
//! The Burn trainer lives on its own update thread (`runner_update.rs`). In imitation mode the
//! update of iteration `i` trains there while the main thread collects the rollout of iteration
//! `i + 1`, which acts with the weights of update `i - 1`; PPO waits for its update at once.
//! The weights each rollout sees are fixed by the iteration index alone, never by timing (the
//! module docs of `runner_update.rs` give the schedule). Anything that reads the network or the
//! trainer for another purpose goes through [`TrainingRunner::sync_update`] first.
//!
//! # Algorithms
//!
//! [`Algorithm::Imitation`] (the default) rolls the policy out with a little exploration noise,
//! asks the simulator for the finite-difference sensitivities of every `label_every`-th state
//! and trains the model on the growing aggregate of those labelled states (`DAgger`).
//! [`Algorithm::Ppo`] is the policy-gradient alternative on the same model. Both step the
//! environment with the run's line search, and both draw the objective weights of every episode
//! ([`ObjectiveSampling`]); the observation carries those weights as task context.
//!
//! # Devices
//!
//! The action of every rollout step comes from one batched forward of the rollout copy of the
//! Burn model on the training device, so rollout numbers are the device's. Validation, the
//! checkpoints and anything that calls [`TrainingRunner::evaluate`] use the pure-Rust twin,
//! which is exported after every update.
//!
//! Conventions: scores are `ObjectiveWeights::score` figures (lower is better); an episode's
//! "improvement" is `base score - final score` (never negative: the environments are greedy).
//!
//! The rest of the runner lives in child modules, which see this module's private items: the
//! configuration (`runner_config.rs`), the data types (`runner_types.rs`), the environment
//! wrappers (`runner_env.rs`), the rollout (`runner_rollout.rs`), the update thread
//! (`runner_update.rs`), the labelled
//! aggregate (`runner_imitation.rs`), the labelling schedule (`runner_teacher.rs`), the thread
//! pools (`runner_pool.rs`), the objective sampling (`runner_objective.rs`), the evaluation
//! machinery (`runner_eval.rs`), the changes to a live run (`runner_setters.rs`), the telemetry
//! and checkpoints (`runner_export.rs`) and the unit tests (`runner_tests.rs`).

// The GUI and the CLI binary both include this file; each uses a different subset of the setters.
#![allow(dead_code, reason = "shared by two binaries that each use a subset")]

use std::fmt;

use indicatrix_ai::{
    env::{EnvError, GLOBAL_DIM, ObservationError, TIER_DIM},
    model::{
        GemstoneSetAttentionNetwork, InferenceScratch, ModelError, ModelTier, Prng, TrainingMode,
        WeightError,
    },
    training::{
        BurnSetAttention, BurnTrainer, ModelShape, PpoConfig, RolloutBuffer, TrainError,
        buffer::ReturnNormalizer,
    },
};
use indicatrix_cut_core::optimize::ObjectiveFidelity;

use crate::dataset::{DatasetError, DatasetSplit};
use envs::{Slot, build_env, pick_target, sampled_source_material};
use eval::{OracleCache, OracleKey};
use imitation::LabelledStates;
use update::{BuildArgs, InstalledUpdate, Updater};

#[path = "runner_config.rs"]
pub mod config;
#[cfg(test)]
#[path = "runner_device_tests.rs"]
mod device_tests;
#[path = "runner_env.rs"]
mod envs;
#[path = "runner_eval.rs"]
mod eval;
#[path = "runner_export.rs"]
mod export;
#[path = "runner_imitation.rs"]
mod imitation;
#[path = "runner_objective.rs"]
pub mod objective;
#[path = "runner_pool.rs"]
mod pool;
#[path = "runner_rollout.rs"]
mod rollout;
#[path = "runner_setters.rs"]
mod setters;
#[path = "runner_teacher.rs"]
mod teacher;
#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
#[path = "runner_types.rs"]
mod types;
#[path = "runner_update.rs"]
mod update;
#[cfg(test)]
#[path = "runner_update_tests.rs"]
mod update_tests;

// Only what both binaries use is re-exported; the CLI and the tests take the rest from
// `config` directly (an unused re-export is a warning in a binary crate).
pub use config::{
    DeviceChoice, Hyperparams, MAX_WORKERS, MIN_GAP_DESIGNS, RunConfig, resumed_schedule,
};
pub use objective::{Algorithm, ObjectiveSampling, draw_weights, draws_same_material};
pub use types::{
    EpisodeSummary, EvalCase, EvalFidelity, EvalPasses, EvalReport, IterationReport, MaterialLabel,
    Optics, RolloutStats, RunnerStats,
};

/// The episode of `entry` (at `pos` of the train split): its environment with the target
/// material and the objective weights drawn from `seed`, reset to its first observation.
///
/// Pure in the runner's state, so the rollout builds the next episodes of several slots in
/// parallel. The outer error is a hard one (an unknown fixed target material); the inner one
/// says the design's environment does not build, which corpus mode skips.
fn build_slot(
    cfg: &RunConfig,
    split: &DatasetSplit,
    pos: usize,
    entry: usize,
    seed: u64,
) -> Result<Result<Slot, EnvError>, RunnerError> {
    let design = &split.entries[entry].design;
    let target = pick_target(cfg, seed, design)?;
    let weights = draw_weights(cfg.objectives, seed);
    let mut env = match build_env(
        cfg,
        design,
        target.as_ref(),
        weights,
        Some(cfg.train_fidelity),
        seed,
    ) {
        Ok(env) => env,
        Err(error) => return Ok(Err(error)),
    };
    let obs = env.reset(seed);
    let (source_name, _, _) = sampled_source_material(seed)?;
    let src = MaterialLabel {
        name: source_name,
        n_d: env.source_material().n_d,
    };
    let tgt = target.as_ref().map_or_else(
        || src.clone(),
        |t| MaterialLabel {
            name: t.name.clone(),
            n_d: t.material.n_d,
        },
    );
    Ok(Ok(Slot {
        env,
        pos,
        entry,
        src,
        tgt,
        obs,
    }))
}

/// Backend label of the GPU trainer.
pub const GPU_LABEL: &str = "GPU (WGPU)";
/// Backend label of native CUDA training.
pub const CUDA_LABEL: &str = "GPU (CUDA)";
/// Backend label of the CPU trainer.
pub const CPU_LABEL: &str = "CPU (Flex SIMD)";

/// Reward history length kept for the bar chart.
const REWARD_HISTORY_LEN: usize = 60;
/// Finished episodes the rolling improvement is averaged over.
const IMPROVEMENT_WINDOW: usize = 64;
/// Smoothing of the per-step reward average.
const REWARD_EMA: f32 = 0.05;
/// Prefix of the warning raised for a design whose environment cannot be built; the CLI uses it
/// to cap the number of skip lines it prints.
pub const SKIPPED_PREFIX: &str = "skipping design ";

/// Anything that can stop the run.
#[derive(Debug)]
pub enum RunnerError {
    /// A configuration value is out of range.
    Config(String),
    /// The data does not allow the requested operation.
    Data(String),
    /// Data set or material error.
    Dataset(DatasetError),
    /// An environment could not be built.
    Env(EnvError),
    /// The network rejected an input.
    Model(ModelError),
    /// The trainer failed.
    Train(TrainError),
    /// A checkpoint could not be written, read or validated.
    Weights(WeightError),
    /// A design has more tiers than the observation layout holds.
    Observation(ObservationError),
}

impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) | Self::Data(m) => f.write_str(m),
            Self::Dataset(e) => write!(f, "{e}"),
            Self::Env(e) => write!(f, "{e}"),
            Self::Model(e) => write!(f, "{e}"),
            Self::Train(e) => write!(f, "{e}"),
            Self::Weights(e) => write!(f, "{e}"),
            Self::Observation(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RunnerError {}

macro_rules! runner_from {
    ($($from:ty => $variant:ident),* $(,)?) => {
        $(impl From<$from> for RunnerError {
            fn from(e: $from) -> Self { Self::$variant(e) }
        })*
    };
}
runner_from!(
    DatasetError => Dataset,
    EnvError => Env,
    ModelError => Model,
    TrainError => Train,
    WeightError => Weights,
    ObservationError => Observation,
);

/// The file name a checkpoint gets when the caller names none.
#[must_use]
pub fn checkpoint_file_name(mode: TrainingMode, tier: ModelTier, iteration: usize) -> String {
    format!(
        "indicatrix-{mode}-{}-iter{iteration}.json",
        tier.code_str().to_ascii_lowercase()
    )
}

/// Threads [`TrainingRunner::evaluate`] spreads `designs` designs over (0 for no designs): the
/// core count, but never more than the designs.
#[must_use]
pub fn evaluation_threads(designs: usize) -> usize {
    pool::pass_threads(designs)
}

/// Persistent environment worker pool width (serial fallback reports one).
#[must_use]
pub fn cpu_pool_threads() -> usize {
    pool::pool_threads()
}

fn build_trainer(
    ppo: &PpoConfig,
    tier: ModelTier,
    max_tiers: usize,
    device: DeviceChoice,
    seed: u64,
) -> Result<(BurnTrainer, &'static str, Option<String>), RunnerError> {
    let (d_model, num_heads, num_layers) = tier.config();
    let shape = ModelShape {
        d_tier: TIER_DIM,
        d_global: GLOBAL_DIM,
        d_model,
        num_heads,
        num_layers,
        max_tiers,
    };
    let (trainer, label, warning) = match device {
        DeviceChoice::Cpu => (BurnTrainer::new_cpu(shape, ppo.clone())?, CPU_LABEL, None),
        DeviceChoice::Gpu => (BurnTrainer::new_gpu(shape, ppo.clone())?, GPU_LABEL, None),
        DeviceChoice::Cuda => (BurnTrainer::new_cuda(shape, ppo.clone())?, CUDA_LABEL, None),
        DeviceChoice::Auto => {
            let (trainer, skipped) = BurnTrainer::new_auto(shape, ppo.clone())?;
            match skipped {
                None => (trainer, GPU_LABEL, None),
                Some(reason) => (
                    trainer,
                    CPU_LABEL,
                    Some(format!("GPU unavailable, training on the CPU: {reason}")),
                ),
            }
        }
    };
    Ok((trainer.with_seed(seed), label, warning))
}

/// The explicit device choice that reproduces the backend `label` names (what an `Auto` choice
/// resolved to), so the rollout trainer lands on the same device as the update thread's.
fn device_of_label(label: &str) -> DeviceChoice {
    match label {
        GPU_LABEL => DeviceChoice::Gpu,
        CUDA_LABEL => DeviceChoice::Cuda,
        _ => DeviceChoice::Cpu,
    }
}

/// The training run: vectorized environments, a model and its trainer, statistics.
pub struct TrainingRunner {
    cfg: RunConfig,
    split: DatasetSplit,
    /// The update thread, which owns the Burn trainer that updates the model.
    updater: Updater,
    /// A second, never-trained trainer on the same device, kept only to turn `net` into the
    /// rollout copy (`rollout_model`) on the main thread, so no Burn object crosses threads.
    rollout_trainer: BurnTrainer,
    /// The pure-Rust twin of the model as of the last installed update: validation, `evaluate`
    /// and the checkpoints read it (call [`Self::sync_update`] first to include the update in
    /// flight).
    net: GemstoneSetAttentionNetwork,
    /// The inference copy of the Burn model the rollout acts with (one batched forward per
    /// pass on the training device). Refreshed at the wait point of every iteration and after
    /// every weight change, never by a bare [`Self::sync_update`].
    rollout_model: BurnSetAttention,
    /// The update installed since the last report (its figures go into the next one).
    installed_update: Option<InstalledUpdate>,
    /// Softmax buffers reused by the pure network's forward passes (allocated on first use).
    scratch: InferenceScratch,
    slots: Vec<Slot>,
    /// PPO only: the rollout's samples.
    buffer: RolloutBuffer,
    /// PPO only: per sample of the rollout, the real tier count.
    n_tiers: Vec<usize>,
    /// PPO only: per sample of the rollout, which real tiers the environment will move.
    free_masks: Vec<Vec<bool>>,
    /// Imitation only: the labelled states of every round so far. Not part of a checkpoint.
    aggregate: LabelledStates,
    /// Rollout passes finished so far; rotates which slots a pass labels.
    label_phase: usize,
    /// Scale of the discounted return the rewards are divided by (review TQ-15). It persists
    /// across iterations and restarts with the environments; a resumed run re-estimates it.
    normalizer: ReturnNormalizer,
    rng: Prng,
    focus_entry: usize,
    warnings: Vec<String>,
    stats: RunnerStats,
    /// The oracle's validation drop, kept while nothing it depends on changes.
    oracle_cache: Option<OracleCache>,
    device_label: &'static str,
}

impl TrainingRunner {
    /// Builds the run: trainer on the requested device, seeded network, one environment per
    /// worker. Warnings (for example a GPU fallback) are available from
    /// [`Self::take_warnings`].
    ///
    /// # Errors
    ///
    /// [`RunnerError`] for an invalid configuration, an unavailable explicit device, or when no
    /// training design builds an environment.
    pub fn new(cfg: RunConfig, split: DatasetSplit) -> Result<Self, RunnerError> {
        cfg.validate()?;
        if split.train.is_empty() {
            return Err(RunnerError::Data("the train split is empty".into()));
        }
        let net = cfg.tier.init_network(cfg.seed);
        let (updater, built) = Updater::spawn(BuildArgs {
            ppo: cfg.ppo.clone(),
            tier: cfg.tier,
            max_tiers: split.max_tiers,
            device: cfg.device,
            seed: cfg.seed,
            net: net.clone(),
        })?;
        let (device_label, warning) = (built.label, built.warning);
        let (mut rollout_trainer, _, _) = build_trainer(
            &cfg.ppo,
            cfg.tier,
            split.max_tiers,
            device_of_label(device_label),
            cfg.seed,
        )?;
        rollout_trainer.load_weights(&net)?;
        let focus_entry = split.train[0];
        let rollout_model = rollout_trainer.rollout_model();
        let mut runner = Self {
            rng: Prng::from_stream(cfg.seed, 1),
            aggregate: LabelledStates::new(cfg.label_capacity),
            cfg,
            split,
            updater,
            rollout_trainer,
            net,
            rollout_model,
            installed_update: None,
            scratch: InferenceScratch::default(),
            slots: Vec::new(),
            buffer: RolloutBuffer::default(),
            n_tiers: Vec::new(),
            free_masks: Vec::new(),
            label_phase: 0,
            normalizer: ReturnNormalizer::new(),
            focus_entry,
            warnings: warning.into_iter().collect(),
            stats: RunnerStats::new(),
            oracle_cache: None,
            device_label,
        };
        runner.rebuild_slots()?;
        Ok(runner)
    }

    /// The configuration in effect.
    #[must_use]
    pub const fn config(&self) -> &RunConfig {
        &self.cfg
    }

    /// The data set.
    #[must_use]
    pub const fn split(&self) -> &DatasetSplit {
        &self.split
    }

    /// Cumulative statistics.
    #[must_use]
    pub const fn stats(&self) -> &RunnerStats {
        &self.stats
    }

    /// Backend label of the trainer in use.
    #[must_use]
    pub const fn device_label(&self) -> &'static str {
        self.device_label
    }

    /// The step limit in effect, degrees per step and tier: the configured override, or the
    /// environments' own default (read from the displayed environment).
    #[must_use]
    pub fn max_step_deg(&self) -> f64 {
        self.slots
            .first()
            .map(|s| s.env.max_step_deg())
            .or(self.cfg.max_step_deg)
            .unwrap_or(0.0)
    }

    /// Source and target material of the displayed stone (worker 0).
    #[must_use]
    pub fn display_materials(&self) -> Option<(&MaterialLabel, &MaterialLabel)> {
        self.slots.first().map(|s| (&s.src, &s.tgt))
    }

    /// Data-set entry the displayed stone (worker 0) trains on.
    #[must_use]
    pub fn display_entry(&self) -> Option<usize> {
        self.slots.first().map(|s| s.entry)
    }

    /// The objective weights the displayed stone (worker 0) is scored with.
    #[must_use]
    pub fn display_weights(&self) -> Option<&indicatrix_cut_core::ObjectiveWeights> {
        self.slots.first().map(|s| s.env.weights())
    }

    /// Warnings raised since the last call (skipped designs, device fallback).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// The editable hyperparameters in effect (the imitation settings in imitation mode).
    #[must_use]
    pub const fn hyperparams(&self) -> Hyperparams {
        let (mini_batch, ppo_epochs, learning_rate) = match self.cfg.algo {
            Algorithm::Ppo => (
                self.cfg.ppo.mini_batch_size,
                self.cfg.ppo.epochs_per_batch,
                self.cfg.ppo.learning_rate,
            ),
            Algorithm::Imitation => (
                self.cfg.imitation.mini_batch_size,
                self.cfg.imitation.epochs_per_round,
                self.cfg.imitation.learning_rate,
            ),
        };
        Hyperparams {
            workers: self.cfg.workers,
            rollout_size: self.cfg.rollout_size,
            mini_batch,
            ppo_epochs,
            learning_rate,
            clip_epsilon: self.cfg.ppo.clip_epsilon,
            gae_lambda: self.cfg.ppo.gae_lambda,
        }
    }

    // ---- environments -------------------------------------------------------------------

    /// First usable (position, entry) at or after `start` in the train split, or the focus
    /// stone in single-stone mode.
    fn choose(&self, start: usize) -> Result<(usize, usize), RunnerError> {
        if !self.cfg.corpus_mode {
            return Ok((0, self.focus_entry));
        }
        let n = self.split.train.len();
        if n == 0 {
            return Err(RunnerError::Data("the train split is empty".to_owned()));
        }
        let pos = start % n;
        Ok((pos, self.split.train[pos]))
    }

    /// Starts an episode: a design from the train split at or after `start`, its target
    /// material and its objective weights, all drawn from `seed`. A design whose environment
    /// does not build is skipped ([`Self::skip_entry`]) and the next one is tried.
    fn spawn_slot(&mut self, start: usize, seed: u64) -> Result<Slot, RunnerError> {
        let mut start = start;
        let mut last_error = None;
        for _ in 0..self.split.train.len() {
            let (pos, entry) = self.choose(start)?;
            match build_slot(&self.cfg, &self.split, pos, entry, seed)? {
                Ok(slot) => return Ok(slot),
                Err(error) => {
                    last_error = Some(error.to_string());
                    start = self.skip_entry(pos, entry, &error)?;
                }
            }
        }
        Err(RunnerError::Data(format!(
            "no sampled training design builds an environment for this target/objective: {}",
            last_error.unwrap_or_else(|| "empty train split".to_owned())
        )))
    }

    /// Records that the environment of `entry` (at `pos` of the train split) failed to build
    /// with `error` and returns the position the search for a design continues from. In
    /// single-stone mode there is nothing to continue with: the error is hard.
    fn skip_entry(
        &mut self,
        pos: usize,
        entry: usize,
        error: &dyn fmt::Display,
    ) -> Result<usize, RunnerError> {
        let title = self.split.entries[entry].title.clone();
        if !self.cfg.corpus_mode {
            return Err(RunnerError::Data(format!(
                "the selected design '{title}' cannot be trained: {error}"
            )));
        }
        self.warnings
            .push(format!("{SKIPPED_PREFIX}'{title}': {error}"));
        Ok(pos + 1)
    }

    fn install_slot(&mut self, w: usize, slot: Slot) {
        if w == 0 {
            self.stats.baseline = Some(Optics::from_global(&slot.obs.global));
            self.stats.current = self.stats.baseline;
        }
        if w < self.slots.len() {
            self.slots[w] = slot;
        } else {
            self.slots.push(slot);
        }
    }

    /// Builds a fresh environment per worker. The new slots replace the old ones only when every
    /// one built, so a failure leaves the run exactly as it was.
    fn rebuild_slots(&mut self) -> Result<(), RunnerError> {
        let n = self.cfg.effective_workers();
        let starts = (0..n)
            .map(|w| {
                let seed = self.rng.next_u64();
                let (pos, entry) = self.choose(w)?;
                Ok((pos, entry, seed))
            })
            .collect::<Result<Vec<_>, RunnerError>>()?;
        let cfg = &self.cfg;
        let split = &self.split;
        let first_attempts = pool::run_indexed(n, pool::pass_threads(n), |i| {
            let (pos, entry, seed) = starts[i];
            (pos, entry, seed, build_slot(cfg, split, pos, entry, seed))
        });
        let mut fresh = Vec::with_capacity(n);
        for (pos, entry, seed, attempt) in first_attempts {
            match attempt? {
                Ok(slot) => fresh.push(slot),
                Err(error) => {
                    let next = self.skip_entry(pos, entry, &error)?;
                    fresh.push(self.spawn_slot(next, seed)?);
                }
            }
        }
        self.slots = fresh;
        self.clear_rollout();
        // The in-flight episodes are gone and the next rewards come from other environments (new
        // task, targets, worker set, model): the scale estimated on them no longer applies.
        self.normalizer.reset();
        if let Some(first) = self.slots.first() {
            self.stats.baseline = Some(Optics::from_global(&first.obs.global));
            self.stats.current = self.stats.baseline;
        }
        Ok(())
    }

    /// Empties the PPO rollout buffers (the imitation aggregate is a different thing: it
    /// outlives rollouts).
    fn clear_rollout(&mut self) {
        self.buffer.clear();
        self.n_tiers.clear();
        self.free_masks.clear();
    }

    /// Points the rollout copy of the model at the weights of `net` (the installed update).
    fn refresh_rollout_model(&mut self) -> Result<(), RunnerError> {
        self.rollout_trainer.load_weights(&self.net)?;
        self.rollout_model = self.rollout_trainer.rollout_model();
        Ok(())
    }

    /// Rebuilds the environments, then starts the statistics over and empties the labelled
    /// aggregate. The update in flight finishes first and is installed (the weights are kept);
    /// its figures belong to the statistics that start over and are dropped. When no
    /// environment can be built nothing changes (statistics and aggregate included).
    fn restart_run(&mut self) -> Result<(), RunnerError> {
        self.sync_update()?;
        self.refresh_rollout_model()?;
        let previous = std::mem::replace(&mut self.stats, RunnerStats::new());
        self.rebuild_slots().inspect_err(|_| {
            self.stats = previous;
        })?;
        self.aggregate.clear();
        self.installed_update = None;
        Ok(())
    }

    // ---- evaluation -------------------------------------------------------------------------

    /// The fixed validation subset: the first `count` entries of the validation split (all of it
    /// when smaller). The same stones every pass, so consecutive scores are comparable.
    #[must_use]
    pub fn validation_entries(&self, count: usize) -> &[usize] {
        &self.split.val[..count.min(self.split.val.len())]
    }

    /// Plays one model episode per entry (the policy mean, projected, stepped with the run's
    /// line search) and averages the results; `passes` adds one stochastic episode per design
    /// ([`EvalReport::mean_improvement_sampled`]) and/or one finite-difference oracle episode
    /// ([`EvalReport::mean_improvement_oracle`]). Retarget targets and objective weights are
    /// drawn from `seed` and the entry, so every pass scores the same stone against the same
    /// target under the same weights.
    ///
    /// The designs are handed one at a time to a pool of up to `available_parallelism` scoped
    /// threads, each with its own softmax scratch. Every outcome lands in the slot of its entry
    /// and the reduction (sums, failures, the first failure) runs serially in entry order
    /// afterwards, so the result does not depend on the thread count. The GPU plays no part:
    /// more than 99 % of the time is the solver, mesh and tracer inside the environment step,
    /// which is CPU code, and the network forward of an M-tier model is well under a
    /// millisecond per step.
    ///
    /// It plays the network as of the last installed update: [`Self::validate`] and
    /// [`Self::sync_update`] install the update in flight first, a bare `evaluate` (which only
    /// borrows the runner) does not.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] for a material or network failure (the first one in entry order); a
    /// design whose environment cannot be built is counted in [`EvalReport::failed`] instead.
    pub fn evaluate(
        &self,
        entries: &[usize],
        fidelity: EvalFidelity,
        seed: u64,
        passes: EvalPasses,
    ) -> Result<EvalReport, RunnerError> {
        eval::evaluate_designs(
            eval::EvalJob {
                net: &self.net,
                split: &self.split,
                cfg: &self.cfg,
                fidelity,
                seed,
                passes,
            },
            entries,
        )
    }

    /// Mean score drop over the recent training episodes minus the validation drop; `None` until
    /// both exist, fidelities match and validation covered at least [`MIN_GAP_DESIGNS`] designs.
    ///
    /// Like with like: the training drop comes from episodes that draw their actions, so it is
    /// compared with the drop of the sampled validation episodes
    /// ([`EvalReport::mean_improvement_sampled`]), which carry the same exploration noise. A
    /// report without a sampled pass falls back to the deterministic drop, and the gap then also
    /// contains the exploration-noise effect. Different training and validation fidelities are
    /// not comparable and produce no gap.
    #[must_use]
    pub fn generalization_gap(&self) -> Option<f32> {
        if !matches!(
            (self.cfg.train_fidelity, self.cfg.validation_fidelity),
            (ObjectiveFidelity::Fast, EvalFidelity::Fast)
                | (ObjectiveFidelity::Full, EvalFidelity::Full)
        ) {
            return None;
        }
        let train = self.stats.mean_improvement()?;
        let val = self.stats.last_validation.as_ref()?;
        let val_drop = val.mean_improvement_sampled.unwrap_or(val.mean_improvement);
        (val.designs >= MIN_GAP_DESIGNS).then_some(train - val_drop)
    }

    /// Validates on the fixed subset of `count` held-out designs at configured validation fidelity
    /// (callers pass [`RunConfig::validation_designs`]) and remembers the result as
    /// [`RunnerStats::last_validation`]. Every design is played three times, in parallel: by the
    /// model (policy mean), by the sampled exploration policy and by the finite-difference
    /// oracle, and the report carries the three drops. The retarget target and the objective
    /// weights of each design are fixed by the run seed, so passes differ only by what the
    /// model learned.
    ///
    /// The oracle does not depend on the model, so it is played once: its drop is cached until
    /// something it depends on changes (the validation designs, the seed, the step limit, the
    /// task, the target settings, the line search, the objective sampling, the probe size), and
    /// a later report carries the cached figure with [`EvalReport::oracle_cached`] set.
    /// `None` when the validation split is empty.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate`].
    pub fn validate(&mut self, count: usize) -> Result<Option<EvalReport>, RunnerError> {
        // The update in flight is part of the model being judged.
        self.sync_update()?;
        let entries = self.validation_entries(count).to_vec();
        if entries.is_empty() {
            return Ok(None);
        }
        let key = OracleKey::new(&self.cfg, &entries, self.cfg.seed, self.max_step_deg());
        let cached = self
            .oracle_cache
            .as_ref()
            .filter(|cache| cache.key == key)
            .map(|cache| cache.drop);
        let passes = EvalPasses {
            sampled: true,
            oracle: cached.is_none(),
        };
        let mut report = self.evaluate(
            &entries,
            self.cfg.validation_fidelity,
            self.cfg.seed,
            passes,
        )?;
        if report.designs > 0 {
            if let Some(drop) = cached {
                report.mean_improvement_oracle = Some(drop);
                report.oracle_cached = true;
            } else if let Some(drop) = report.mean_improvement_oracle {
                self.oracle_cache = Some(OracleCache { key, drop });
            }
        }
        self.stats.val_oracle = report.mean_improvement_oracle;
        self.stats.last_validation = Some(report.clone());
        Ok(Some(report))
    }
}
