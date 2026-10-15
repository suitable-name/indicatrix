//! Training run telemetry and its JSON persistence, written by the CLI and the GUI worker and
//! read back by the GUI for analysis.

use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufReader, BufWriter},
    time::{SystemTime, UNIX_EPOCH},
};

/// Version written into every stats file; a file whose major version differs is refused.
pub const STATS_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// Samples labelled by the finite-difference teacher in this iteration's rollout.
    #[serde(default)]
    pub labels_this_round: usize,
    /// Mean score drop of the oracle step over this iteration's labels (points, higher is
    /// better); `None` when the rollout produced no oracle figure.
    #[serde(default)]
    pub oracle_gain_mean: Option<f32>,
    /// Latest validation score drop of the finite-difference oracle (the same direction rule on
    /// the measured slopes plus line search; the upper bound of what the policy class can do),
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
    /// Candidate evaluations of this iteration: the steps' candidates, the label probes and the
    /// oracle candidates.
    #[serde(default)]
    pub evaluations_this_round: usize,
    /// Parallel environments (workers) of the run (0 in files that predate the field).
    #[serde(default)]
    pub workers: usize,
}

/// Oracle drops at or below this many points are treated as "nothing to gain": a ratio against
/// them would only amplify the quantisation of the score (about 0.16 points per ray).
const MIN_ORACLE_DROP: f32 = 1e-3;

impl IterationMetricPoint {
    /// Candidate evaluations per second of the iteration: its evaluations over the rollout and
    /// update wall time. `None` when no evaluation was counted or no time was recorded (files
    /// that predate the fields).
    #[must_use]
    pub fn evaluations_per_sec(&self) -> Option<f32> {
        let secs = self.rollout_secs + self.update_secs;
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
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, self)?;
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
mod tests {
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
}
