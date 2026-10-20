//! Training run telemetry and its JSON persistence, written by the CLI and the GUI worker and
//! read back by the GUI for analysis.

use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    time::{SystemTime, UNIX_EPOCH},
};

/// Version written into every stats file; a file whose major version differs is refused.
pub const STATS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One fixed-cohort validation or test evaluation saved alongside run telemetry.
#[derive(Clone, Debug, Serialize)]
#[allow(
    dead_code,
    reason = "only the CLI binary records evaluations; this file is shared with the GUI binary"
)]
pub struct EvaluationSnapshot {
    pub iteration: usize,
    pub kind: String,
    pub fidelity: String,
    pub cases: Vec<crate::runner::EvalCase>,
}

/// Atomically writes per-design outcomes collected so far.
#[allow(
    dead_code,
    reason = "only the CLI binary records evaluations; this file is shared with the GUI binary"
)]
pub fn save_evaluations(
    path: &std::path::Path,
    evaluations: &[EvaluationSnapshot],
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, evaluations)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// A single recorded metrics point across training iterations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IterationMetricPoint {
    /// Iteration sequence number.
    pub iteration: usize,
    /// Completed episode counter.
    pub episode: usize,
    /// Cumulative transition steps collected.
    pub total_steps: usize,
    /// Rollout throughput in environment steps per second.
    pub steps_per_sec: f32,
    /// Rolling mean reward per step.
    #[serde(alias = "mean_return")]
    pub mean_step_reward: f32,
    /// Lowest final episode score so far (lower is better); `None` before the first episode
    /// finished. Files written before this field was optional hold `0.0` for "none" and read
    /// back as `Some(0.0)`.
    #[serde(default)]
    pub best_score: Option<f32>,
    /// Actor policy loss.
    pub policy_loss: f32,
    /// Critic value loss.
    pub value_loss: f32,
    /// Action entropy.
    pub entropy: f32,
    /// PPO clipping fraction.
    pub clip_fraction: f32,
    /// Approximate KL divergence of the update.
    #[serde(default)]
    pub approx_kl: f32,
    /// Whether the geometry solved as a valid closed solid manifold.
    pub solid_closed: bool,
    /// Tilt brilliance percentage (0-100).
    pub brilliance: f32,
    /// Windowing light leakage percentage (0-100).
    pub windowing: f32,
    /// Extinction darkness percentage (0-100).
    pub extinction: f32,
    /// Volumetric yield percentage (0-100).
    pub yield_pct: f32,
    /// Latest validation score (deterministic policy, lower is better); `None` until the first
    /// validation pass.
    #[serde(default)]
    pub val_score: Option<f32>,
    /// Latest validation brilliance; `None` until the first validation pass.
    #[serde(default)]
    pub val_brilliance: Option<f32>,
    /// Latest validation windowing; `None` until the first validation pass.
    #[serde(default)]
    pub val_windowing: Option<f32>,
    /// Mean raw reward per step of THIS iteration's rollout (not the smoothed
    /// `mean_step_reward`); higher is better. 0 in files that predate the field.
    #[serde(default)]
    pub mean_step_reward_raw: f32,
    /// Factor the rewards were multiplied by before GAE (`1 / std` of the discounted return);
    /// it keeps the critic's targets O(1). 0 in files that predate the field.
    #[serde(default)]
    pub reward_scale: f32,
    /// Critic quality `1 - Var(return - value) / Var(return)` of this rollout: 1 is perfect, 0
    /// is no better than the mean, higher is better; `None` when undefined.
    #[serde(default)]
    pub explained_variance: Option<f32>,
    /// Global gradient L2 norm before clipping, mean over the iteration's updates.
    #[serde(default)]
    pub grad_norm: f32,
    /// Mean of the three block `log_std` values (crown, pavilion, girdle) in pre-squash units;
    /// falling means the policy narrows, pinned at its lower bound means collapse.
    #[serde(default)]
    pub mean_log_std: f32,
    /// Whether the KL stop ended this iteration's epochs early.
    #[serde(default)]
    pub early_stopped: bool,
    /// Fraction of this rollout's steps that changed the design (0-1, higher is better).
    #[serde(default)]
    pub accepted_frac: f32,
    /// Episodes that finished during this rollout.
    #[serde(default)]
    pub episodes_finished: usize,
    /// Fraction of the finished episodes whose final score beat their base score (0-1, higher
    /// is better); 0 when none finished.
    #[serde(default)]
    pub episodes_improved_frac: f32,
    /// Rolling mean score drop (base minus final, points, higher is better) over the most
    /// recent finished episodes; `None` before the first episode finished.
    #[serde(default)]
    pub mean_improvement: Option<f32>,
    /// Steps rejected as unsafe (angle outside its window, no usable mast, non-finite or
    /// wrongly sized action) in this rollout.
    #[serde(default)]
    pub rej_unsafe: usize,
    /// Steps rejected because the candidate or its relations did not solve in this rollout.
    #[serde(default)]
    pub rej_unsolvable: usize,
    /// Steps rejected because the candidate was not a closed solid or scored non-finite.
    #[serde(default)]
    pub rej_not_closed: usize,
    /// Steps rejected by the girdle gate in this rollout.
    #[serde(default)]
    pub rej_girdle: usize,
    /// Steps rejected by the manufacturability gate in this rollout.
    #[serde(default)]
    pub rej_manufacturability: usize,
    /// Steps whose valid candidate did not beat the current score in this rollout.
    #[serde(default)]
    pub rej_not_improved: usize,
    /// Latest validation score drop of the model (deterministic direction plus line search; base
    /// minus final, points, higher is better); `None` until the first validation pass.
    #[serde(default)]
    pub val_improvement: Option<f32>,
    /// Latest validation score drop of one sampled (stochastic) episode per design, same
    /// stones and targets as `val_improvement`; `None` until the first validation pass.
    #[serde(default)]
    pub val_improvement_sampled: Option<f32>,
    /// Designs the latest validation pass evaluated (0 until the first pass).
    #[serde(default)]
    pub val_designs: usize,
    /// Mean total imitation loss of this iteration's update (0 in PPO runs and in files that
    /// predate the field).
    #[serde(default)]
    pub imitation_loss: f32,
    /// Mean squared error of the predicted component sensitivities over the labelled entries
    /// of this iteration's update, lower is better (0 in PPO runs and in older files).
    #[serde(default)]
    pub sens_mse: f32,
    /// Mean cosine between the composed policy direction and the direction of the measured
    /// score slopes over this iteration's update (-1 to 1, higher is better); `None` when no
    /// imitation update ran.
    #[serde(default)]
    pub train_cosine: Option<f32>,
    /// Mean Huber loss of the predicted oracle gain in this iteration's update (0 in PPO runs
    /// and in older files).
    #[serde(default)]
    pub gain_loss: f32,
    /// Labelled states in the aggregate the update trained on (0 in PPO runs and in older
    /// files).
    #[serde(default)]
    pub labelled_samples: usize,
    /// Finite training labels selected for the seven components in stable environment order.
    #[serde(default)]
    pub component_support: [usize; indicatrix_ai::env::N_COMPONENTS],
    /// Samples labelled by the finite-difference teacher in this iteration's rollout.
    #[serde(default)]
    pub labels_this_round: usize,
    /// Mean score drop of the oracle step over this iteration's labels (points, higher is
    /// better); `None` when the rollout produced no oracle figure.
    #[serde(default)]
    pub oracle_gain_mean: Option<f32>,
    /// Latest validation score drop of the finite-difference oracle (the same direction rule on
    /// the measured slopes plus line search; a reference direction, not a guaranteed upper bound),
    /// same stones and targets as `val_improvement`; `None` until the first validation pass.
    /// Compare it with `val_improvement`: the ratio `model / oracle` is the figure to watch.
    #[serde(default)]
    pub val_improvement_oracle: Option<f32>,
    /// Candidate evaluations per environment step of the latest validation's model pass (1 to
    /// the number of line-search scales); `None` until the first validation pass.
    #[serde(default)]
    pub val_evaluations_per_step: Option<f32>,
    /// Wall seconds this iteration spent collecting the rollout (labels included).
    #[serde(default)]
    pub rollout_secs: f32,
    /// Wall seconds this iteration spent measuring labels (the probe phase of the rollout).
    #[serde(default)]
    pub label_secs: f32,
    /// Wall seconds this iteration spent updating the network.
    #[serde(default)]
    pub update_secs: f32,
    /// Cumulative host milliseconds spent packing training batches.
    #[serde(default)]
    pub packing_ms: f32,
    /// Cumulative host milliseconds in training update bodies.
    #[serde(default)]
    pub training_host_ms: f32,
    /// Host milliseconds spent exporting weights to the inference twin.
    #[serde(default)]
    pub weight_export_ms: f32,
    /// Host milliseconds in batched rollout forwards (not device utilization).
    #[serde(default)]
    pub rollout_forward_ms: f32,
    /// Wall seconds the main thread waited for the update thread (0 in files that predate the
    /// field; see [`Self::main_thread_secs`]).
    #[serde(default)]
    pub update_wait_secs: f32,
    /// The iteration whose rollout the update of this point trained on (the previous one in
    /// imitation runs); `None` when no update was installed during this iteration (the first
    /// one of a run) and in files that predate the field.
    #[serde(default)]
    pub update_of_iteration: Option<usize>,
    /// Candidate evaluations of this iteration: the steps' candidates, the label probes and the
    /// oracle candidates.
    #[serde(default)]
    pub evaluations_this_round: usize,
    /// Parallel environments (workers) of the run (0 in files that predate the field).
    #[serde(default)]
    pub workers: usize,
    /// Active rollout slots, capped by rollout size (0 in files that predate this field).
    #[serde(default)]
    pub effective_slots: usize,
    /// Persistent CPU pool width (1 when pool initialization falls back to serial).
    #[serde(default)]
    pub cpu_pool_threads: usize,
}

/// Oracle drops at or below this many points are treated as "nothing to gain": a ratio against
/// them would only amplify the quantisation of the score (about 0.16 points per ray).
const MIN_ORACLE_DROP: f32 = 1e-3;

impl IterationMetricPoint {
    /// Whether an update was installed during the iteration of this point. `false` in the first
    /// iteration of an imitation run (the first update trains during the second rollout): the
    /// loss, cosine and update figures of the point are then defaults, not measurements, and the
    /// displays show a dash. Files that predate `update_of_iteration` count by `update_secs`.
    #[must_use]
    pub fn has_update(&self) -> bool {
        self.update_of_iteration.is_some() || self.update_secs > 0.0
    }

    /// Wall seconds the main thread spent on the iteration: the rollout plus the time it stood
    /// still for the update. An imitation update trains on its own thread during the next
    /// rollout, so only the wait counts; a PPO update is waited for at once, so its wait already
    /// holds the whole update and the same formula is right for both algorithms. A point from a
    /// file that predates `update_wait_secs` (an update, but no `update_of_iteration`) falls back
    /// to the old rollout-plus-update sum.
    #[must_use]
    pub fn main_thread_secs(&self) -> f32 {
        let legacy = self.update_of_iteration.is_none() && self.update_secs > 0.0;
        let wait = if legacy {
            self.update_secs
        } else {
            self.update_wait_secs
        };
        self.rollout_secs + wait
    }

    /// Candidate evaluations per second of the iteration: its evaluations over the wall time
    /// the main thread spent ([`Self::main_thread_secs`]). `None` when no evaluation was
    /// counted or no time was recorded (files that predate the fields).
    #[must_use]
    pub fn evaluations_per_sec(&self) -> Option<f32> {
        let secs = self.main_thread_secs();
        (self.evaluations_this_round > 0 && secs > 0.0)
            .then_some(self.evaluations_this_round as f32 / secs)
    }
}

/// The model's share of the oracle's score drop (`model / oracle`; 1 means the model matches the
/// finite-difference oracle). `None` when the oracle drop is missing, not finite or too small
/// for a ratio to mean anything.
#[must_use]
pub fn drop_ratio(model: f32, oracle: Option<f32>) -> Option<f32> {
    oracle
        .filter(|oracle_drop| oracle_drop.is_finite() && *oracle_drop > MIN_ORACLE_DROP)
        .map(|oracle_drop| model / oracle_drop)
}

/// The three validation drops as one phrase, `drop model 4.20 / oracle 6.10 / sampled 1.30
/// (model/oracle 0.69)`; a missing figure reads `-` and the ratio is left out when it is
/// undefined (see [`drop_ratio`]).
#[must_use]
pub fn describe_drops(model: f32, oracle: Option<f32>, sampled: Option<f32>) -> String {
    let cell = |value: Option<f32>| value.map_or_else(|| "-".to_owned(), |d| format!("{d:.2}"));
    let ratio = drop_ratio(model, oracle)
        .map_or_else(String::new, |ratio| format!(" (model/oracle {ratio:.2})"));
    format!(
        "drop model {model:.2} / oracle {} / sampled {}{ratio}",
        cell(oracle),
        cell(sampled)
    )
}

/// Serialized training run telemetry record.
///
/// Exported by the CLI trainer on a remote headless machine or by the GUI worker, and loaded
/// into the Slint GUI for visual analysis. Scores are `ObjectiveWeights::score` figures
/// (`0..=100`, LOWER is better).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrainingRunStats {
    /// Schema format version (the writer's crate version).
    pub version: String,
    /// RFC 3339 UTC timestamp of the export.
    pub timestamp_rfc3339: String,
    /// Training mode: "Optimizer" or "Retarget".
    pub mode: String,
    /// Model tier architecture: "S", "M", "L", "XL", "XXL".
    pub model_tier: String,
    /// Execution backend device label.
    pub device: String,
    /// Human-readable title of the active gemstone design.
    pub active_design_title: String,
    /// Filename of the active gemstone design.
    pub active_design_filename: String,
    /// Target material name, or "Universal Spectrum".
    pub target_material: String,
    /// Total completed iterations.
    pub total_iterations: usize,
    /// Total completed episodes.
    pub total_episodes: usize,
    /// Total transition steps taken.
    pub total_steps: usize,
    /// Lowest final episode score on the train split; `None` before the first episode finished.
    /// (Older files hold `0.0` for "none" and read back as `Some(0.0)`.)
    #[serde(default)]
    pub best_score: Option<f32>,
    /// Largest score drop (base minus final, points, higher is better) any finished training
    /// episode achieved; `None` before the first episode finished. Unlike `best_score` it does
    /// not depend on which design happened to have the lowest base score.
    #[serde(default)]
    pub best_improvement: Option<f32>,
    /// Final rolling mean reward per step (the field keeps its historical name).
    pub final_return: f32,
    /// Starting baseline brilliance.
    pub baseline_brilliance: f32,
    /// Starting baseline windowing.
    pub baseline_windowing: f32,
    /// Starting baseline extinction.
    pub baseline_extinction: f32,
    /// Starting baseline volumetric yield.
    pub baseline_yield_pct: f32,
    /// Final achieved brilliance.
    pub final_brilliance: f32,
    /// Final achieved windowing.
    pub final_windowing: f32,
    /// Final achieved extinction.
    pub final_extinction: f32,
    /// Final achieved volumetric yield.
    pub final_yield_pct: f32,
    /// Latest validation score on the unseen validation split; `None` when no validation ran.
    /// (Older files hold `0.0` for "none" and read back as `Some(0.0)`.)
    #[serde(default)]
    pub val_score: Option<f32>,
    /// Mean train-episode score drop (sampled actions) minus the mean validation score drop of
    /// the sampled validation episodes, so both sides carry the same exploration noise; files
    /// written before the sampled validation pass compared against the deterministic drop
    /// instead. `None` until enough designs were validated. (Older files hold `0.0` for "none"
    /// and read back as `Some(0.0)`.)
    #[serde(default)]
    pub generalization_gap: Option<f32>,
    /// Count of designs in the training split.
    pub train_designs_count: usize,
    /// Count of designs in the validation split.
    pub val_designs_count: usize,
    /// Count of designs in the test split.
    pub test_designs_count: usize,
    /// Salt of the split hash, so the split can be reproduced.
    #[serde(default)]
    pub split_seed: u64,
    /// Training algorithm of the run, `"imitation"` or `"ppo"` (empty in files that predate the
    /// field, which were all PPO runs).
    #[serde(default)]
    pub algo: String,
    /// Label period of the run: every N-th rollout sample was labelled by the finite-difference
    /// teacher; 0 = no labels (0 in files that predate the field).
    #[serde(default)]
    pub label_every: usize,
    /// How the objective weights of the episodes were drawn, `"balanced"`, `"presets"` or
    /// `"mixed"` (empty in files that predate the field).
    #[serde(default)]
    pub objectives: String,
    /// Training scoring fidelity (`fast` or `full`).
    #[serde(default)]
    pub train_fidelity: String,
    /// Endpoint fidelity used for held-out validation.
    #[serde(default)]
    pub validation_fidelity: String,
    /// Episode-seeded shape target policy used by trainer environments.
    #[serde(default)]
    pub shape_target_sampling: String,
    /// Episode-seeded lighting policy used by trainer environments.
    #[serde(default)]
    pub lighting_sampling: String,
    /// Whether fixed held-out objectives were swept across all named presets.
    #[serde(default)]
    pub objective_sweep: bool,
    /// Optional candidate budget used by the classical comparison.
    #[serde(default)]
    pub classical_budget: Option<usize>,
    /// Standard deviation of the exploration noise added to the policy mean in imitation
    /// rollouts, pre-squash units (0.0 in files that predate the field and in PPO runs).
    #[serde(default)]
    pub explore_std: f32,
    /// Step limit in effect, degrees per step and tier (0 in files that predate the field).
    #[serde(default)]
    pub max_step_deg: f64,
    /// Recent per-iteration mean step rewards.
    pub recent_rewards: Vec<f32>,
    /// Sequential time series of iteration metric points.
    pub metrics_history: Vec<IterationMetricPoint>,
    /// Chronological log events (timestamp, level, message).
    pub event_logs: Vec<(String, String, String)>,
}

/// Errors reading a stats file.
#[derive(Debug)]
pub enum StatsError {
    /// The file could not be opened or read.
    Io(std::io::Error),
    /// The file is not valid stats JSON.
    Json(serde_json::Error),
    /// The file was written by an incompatible major version.
    Version {
        /// Version in the file.
        file: String,
        /// Version of this build.
        current: &'static str,
    },
}

impl std::fmt::Display for StatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "not a valid stats file: {e}"),
            Self::Version { file, current } => write!(
                f,
                "stats file version {file} is not compatible with this build ({current})"
            ),
        }
    }
}

impl std::error::Error for StatsError {}

fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

impl TrainingRunStats {
    /// Save run statistics to a JSON file on disk.
    ///
    /// # Errors
    ///
    /// IO or serialization failures.
    #[allow(
        dead_code,
        reason = "the CLI's path-string entry point; the GUI worker uses save_to_path"
    )]
    pub fn save_to_file(&self, path: &str) -> std::io::Result<()> {
        self.save_to_path(std::path::Path::new(path))
    }

    /// Writes the run statistics to `path` as pretty JSON (an existing file is replaced). The
    /// GUI worker takes a path from the file dialog, which need not be valid UTF-8.
    ///
    /// # Errors
    ///
    /// The I/O or serialization error, as [`std::io::Error`].
    pub fn save_to_path(&self, path: &std::path::Path) -> std::io::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut writer = BufWriter::new(temp.as_file_mut());
            serde_json::to_writer_pretty(&mut writer, self)?;
            writer.flush()?;
        }
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Load run statistics from a JSON file on disk.
    ///
    /// # Errors
    ///
    /// [`StatsError`] for IO failures, invalid JSON, or a major-version mismatch.
    #[allow(
        dead_code,
        reason = "used by the GUI binary only; the CLI binary shares this module"
    )]
    pub fn load_from_file(path: &str) -> Result<Self, StatsError> {
        let file = File::open(path).map_err(StatsError::Io)?;
        let stats: Self =
            serde_json::from_reader(BufReader::new(file)).map_err(StatsError::Json)?;
        if major(&stats.version) != major(STATS_VERSION) {
            return Err(StatsError::Version {
                file: stats.version,
                current: STATS_VERSION,
            });
        }
        Ok(stats)
    }
}

/// The current time as an RFC 3339 UTC string (`2026-10-12T08:30:00Z`).
#[must_use]
pub fn rfc3339_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    rfc3339_from_unix(secs)
}

/// Formats Unix seconds as RFC 3339 UTC (proleptic Gregorian, civil-from-days algorithm).
#[must_use]
pub fn rfc3339_from_unix(secs: u64) -> String {
    let days = (secs / 86_400).cast_signed();
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem / 60) % 60,
        rem % 60
    )
}

#[cfg(test)]
#[path = "stats_tests.rs"]
mod tests;
