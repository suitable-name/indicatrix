//! The plain data the runner reports: episode summaries, rollout statistics, the per-iteration
//! report, the cumulative run statistics and the evaluation report.
//!
//! Conventions: scores are `ObjectiveWeights::score` figures (lower is better); an episode's
//! "improvement" is `base score - final score` (never negative: the environments are greedy).

use serde::Serialize;
use std::collections::VecDeque;

use indicatrix_ai::{
    env::{GlobalFeature, StepRejection, TeacherLabel},
    training::{ImitationMetrics, TrainingMetrics},
};

use super::imitation::label_is_informative;

/// Optical readings of one stone state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Optics {
    /// Tilt brilliance percentage (higher is better).
    pub brilliance: f32,
    /// Windowing percentage.
    pub windowing: f32,
    /// Extinction percentage.
    pub extinction: f32,
    /// Volumetric yield percentage.
    pub yield_pct: f32,
}

impl Optics {
    /// The readings in an observation's global features.
    pub(super) const fn from_global(g: &GlobalFeature) -> Self {
        Self {
            brilliance: g.tilt_brilliance_pct,
            windowing: g.windowing_pct,
            extinction: g.extinction_pct,
            yield_pct: g.yield_pct,
        }
    }
}

/// A material as shown to the user.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialLabel {
    /// Species name, or `Design (I-line)` for a design's own refractive index.
    pub name: String,
    /// Refractive index at the sodium D line.
    pub n_d: f64,
}

/// One finished episode.
#[derive(Clone, Debug)]
pub struct EpisodeSummary {
    /// Index into the data set's `entries`.
    pub entry: usize,
    /// Score of the design the episode started from.
    pub base_score: f32,
    /// Score of the state the episode ended in.
    pub final_score: f32,
    /// Optics of the final state.
    pub optics: Optics,
}

impl EpisodeSummary {
    /// Score drop achieved by the episode.
    #[must_use]
    pub const fn improvement(&self) -> f32 {
        self.base_score - self.final_score
    }
}

/// Where the outcome of one environment step falls in the rejection statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Bucket {
    /// The proposal changed the design.
    Accepted,
    /// An unsafe angle, no usable mast, invalid action, or preserved meet/constraint violation.
    Unsafe,
    /// The candidate or its tier relations did not solve.
    Unsolvable,
    /// The candidate was not a closed solid, or its score was not a number.
    NotClosed,
    /// The girdle gate refused the candidate.
    Girdle,
    /// The manufacturability gate refused the candidate.
    Manufacturability,
    /// A valid stone that did not score lower than the current one.
    NotImproved,
}

/// The statistics bucket of a step's [`StepRejection`] (`None` = accepted). Exhaustive on
/// purpose: a new rejection reason must be assigned a bucket here.
pub(super) const fn bucket(rejection: Option<StepRejection>) -> Bucket {
    match rejection {
        None => Bucket::Accepted,
        Some(
            StepRejection::UnsafeAngle
            | StepRejection::NoUsableMast
            | StepRejection::NonFiniteDelta
            | StepRejection::PreservationViolation
            | StepRejection::WrongActionLength { .. },
        ) => Bucket::Unsafe,
        Some(StepRejection::Unsolvable | StepRejection::RelationsUnsolvable) => Bucket::Unsolvable,
        Some(StepRejection::NotClosed | StepRejection::NonFiniteScore) => Bucket::NotClosed,
        Some(StepRejection::GirdleViolation) => Bucket::Girdle,
        Some(StepRejection::ManufacturabilityRegression) => Bucket::Manufacturability,
        Some(StepRejection::NotImproved) => Bucket::NotImproved,
    }
}

/// Result of a rollout.
#[derive(Debug, Default)]
pub struct RolloutStats {
    /// Environment steps collected.
    pub steps: usize,
    /// Sum of the raw step rewards.
    pub reward_sum: f32,
    /// Episodes that finished during the rollout.
    pub finished: Vec<EpisodeSummary>,
    /// Steps that changed the design.
    pub accepted: usize,
    /// Steps rejected as unsafe (angle window, mast, invalid action, or meet/constraint preservation).
    pub rej_unsafe: usize,
    /// Steps rejected because the candidate or its relations did not solve.
    pub rej_unsolvable: usize,
    /// Steps rejected because the candidate was not a closed solid or scored non-finite.
    pub rej_not_closed: usize,
    /// Steps rejected by the girdle gate.
    pub rej_girdle: usize,
    /// Steps rejected by the manufacturability gate.
    pub rej_manufacturability: usize,
    /// Steps whose valid candidate did not beat the current score.
    pub rej_not_improved: usize,
    /// Finished episodes whose final score beat their base score.
    pub episodes_improved: usize,
    /// Samples whose finite-difference label taught something (see
    /// `imitation::label_is_informative`).
    pub labels: usize,
    /// Sum of the oracle gains (points) of the labels that had a finite one.
    pub oracle_gain_sum: f32,
    /// Labels that had a finite oracle gain.
    pub oracle_gain_n: usize,
    /// Candidates the steps scored (the sum of `StepInfo::evaluations`).
    pub evaluations: usize,
    /// Candidates the labels scored: the probes and the oracle step.
    pub label_evaluations: usize,
    /// Wall-clock seconds the probe phases of the passes took.
    pub label_secs: f32,
}

impl RolloutStats {
    /// Counts one step's outcome in its rejection bucket.
    pub(super) const fn count_rejection(&mut self, rejection: Option<StepRejection>) {
        let counter = match bucket(rejection) {
            Bucket::Accepted => &mut self.accepted,
            Bucket::Unsafe => &mut self.rej_unsafe,
            Bucket::Unsolvable => &mut self.rej_unsolvable,
            Bucket::NotClosed => &mut self.rej_not_closed,
            Bucket::Girdle => &mut self.rej_girdle,
            Bucket::Manufacturability => &mut self.rej_manufacturability,
            Bucket::NotImproved => &mut self.rej_not_improved,
        };
        *counter += 1;
    }

    /// Folds one finished label into the totals: the candidates it cost always, the sample and
    /// its oracle gain when it carries anything to learn. Returns whether it does.
    pub(super) fn record_label(&mut self, label: &TeacherLabel) -> bool {
        self.label_evaluations += label.evaluations;
        if let Some(gain) = label.oracle_gain.filter(|gain| gain.is_finite()) {
            self.oracle_gain_sum += gain;
            self.oracle_gain_n += 1;
        }
        let informative = label_is_informative(label);
        self.labels += usize::from(informative);
        informative
    }

    /// Mean oracle gain (points) of the rollout's labels; `None` when no label had one.
    #[must_use]
    pub fn mean_oracle_gain(&self) -> Option<f32> {
        (self.oracle_gain_n > 0).then(|| self.oracle_gain_sum / self.oracle_gain_n as f32)
    }

    /// Candidates the rollout scored: those of the steps plus those of the labels.
    #[must_use]
    pub const fn total_evaluations(&self) -> usize {
        self.evaluations + self.label_evaluations
    }
}

/// Result of one [`super::TrainingRunner::iterate`].
#[derive(Debug)]
pub struct IterationReport {
    /// PPO update metrics (all zero in imitation mode and when no update was installed during
    /// this iteration).
    pub train: TrainingMetrics,
    /// Mean reward per step of the rollout.
    pub mean_step_reward: f32,
    /// Environment steps of the rollout.
    pub steps: usize,
    /// A new lowest final score, with the episode that reached it. Dominated by which design
    /// happened to have the lowest base score (review TQ-19); the logs report
    /// [`Self::new_best_improvement`] instead.
    pub new_best: Option<EpisodeSummary>,
    /// A new largest score drop (base minus final), with the episode that reached it.
    pub new_best_improvement: Option<EpisodeSummary>,
    /// Critic explained variance over this rollout (1 = perfect, 0 = the mean); `None` when
    /// undefined or in imitation mode.
    pub explained_variance: Option<f32>,
    /// Fraction of this rollout's steps that changed the design (0-1).
    pub accepted_frac: f32,
    /// Imitation update metrics of the update installed during this iteration (see
    /// [`Self::update_of_iteration`]); `None` in PPO mode and when no update was installed.
    pub imitation: Option<ImitationMetrics>,
    /// Samples of this rollout that got a usable finite-difference label.
    pub labels_this_round: usize,
    /// Mean oracle gain (points) of this rollout's labels; `None` when there was none.
    pub oracle_gain_mean: Option<f32>,
    /// Wall-clock seconds of the rollout (every pass: labels and steps).
    pub rollout_secs: f32,
    /// Wall-clock seconds of the rollout spent probing for labels (phase 1 of the passes).
    pub label_secs: f32,
    /// The update's own wall-clock seconds, measured on the update thread (0 when no update was
    /// installed during this iteration). In imitation mode it overlaps the rollout.
    pub update_secs: f32,
    /// Cumulative host milliseconds spent by imitation/PPO packing.
    pub packing_ms: f32,
    /// Cumulative host milliseconds spent in imitation/PPO update bodies.
    pub training_host_ms: f32,
    /// Host milliseconds spent exporting trained weights to the rollout network.
    pub weight_export_ms: f32,
    /// Host milliseconds in the batched rollout forward (not device utilization).
    pub rollout_forward_ms: f32,
    /// Seconds the main thread waited for the update thread at the wait point (0 when the
    /// update had already finished). Near 0: the rollout is the bottleneck; large: the update.
    pub update_wait_secs: f32,
    /// The iteration whose rollout the reported update trained on: this one in PPO mode, the
    /// previous one in imitation mode, `None` when no update was installed (iteration 0, or the
    /// first one after a restart).
    pub update_of_iteration: Option<usize>,
    /// Candidate evaluations of this iteration: the steps', the probes' and the oracle's.
    pub evaluations_this_round: usize,
}

/// Cumulative run statistics.
#[derive(Clone, Debug)]
pub struct RunnerStats {
    /// Updates done (continues from a resumed checkpoint).
    pub iteration: usize,
    /// Episodes finished.
    pub episodes: usize,
    /// Environment steps collected.
    pub total_steps: usize,
    /// Moving average of the mean reward per step.
    pub mean_step_reward: f32,
    /// Recent per-iteration mean step rewards.
    pub reward_history: VecDeque<f32>,
    /// The finished episode with the lowest final score.
    pub best: Option<EpisodeSummary>,
    /// The finished episode with the largest score drop (base minus final); unlike
    /// [`Self::best`] it does not depend on which design had the lowest base score.
    pub best_improvement: Option<EpisodeSummary>,
    /// Score drops of the most recent finished episodes.
    pub recent_improvements: VecDeque<f32>,
    /// Optics of the displayed stone at the start of its episode.
    pub baseline: Option<Optics>,
    /// Optics of the displayed stone's current state.
    pub current: Option<Optics>,
    /// Whether the displayed stone's current state is a closed solid.
    pub solid_closed: bool,
    /// The last validation result.
    pub last_validation: Option<EvalReport>,
    /// The last PPO training metrics (all zero in imitation mode).
    pub last_train: TrainingMetrics,
    /// Mean raw reward per step of the last rollout (not the smoothed average).
    pub mean_step_reward_raw: f32,
    /// Factor the last update multiplied the rewards by (1 until the normaliser has two steps).
    pub reward_scale: f32,
    /// Critic explained variance over the last rollout; `None` when undefined.
    pub explained_variance: Option<f32>,
    /// Fraction of the last rollout's steps that changed the design (0-1).
    pub accepted_frac: f32,
    /// Episodes that finished in the last rollout.
    pub episodes_finished: usize,
    /// Fraction of those episodes that improved on their base score (0 when none finished).
    pub episodes_improved_frac: f32,
    /// Unsafe-action rejections in the last rollout.
    pub rej_unsafe: usize,
    /// Unsolvable-candidate rejections in the last rollout.
    pub rej_unsolvable: usize,
    /// Not-closed or non-finite-score rejections in the last rollout.
    pub rej_not_closed: usize,
    /// Girdle-gate rejections in the last rollout.
    pub rej_girdle: usize,
    /// Manufacturability-gate rejections in the last rollout.
    pub rej_manufacturability: usize,
    /// Valid-but-not-better rejections in the last rollout.
    pub rej_not_improved: usize,
    /// The last installed imitation update's metrics (one iteration behind the rollout); `None`
    /// in PPO mode and before the first update was installed.
    pub last_imitation: Option<ImitationMetrics>,
    /// Labelled states in the aggregate after the last update.
    pub labelled_samples: usize,
    /// Samples of the last rollout that got a usable label.
    pub labels_this_round: usize,
    /// Mean oracle gain (points) of the last rollout's labels; `None` when there was none.
    pub oracle_gain_mean: Option<f32>,
    /// The oracle's mean score drop on the validation designs (points), as of the last
    /// validation; `None` before the first one.
    pub val_oracle: Option<f32>,
    /// Wall-clock seconds of the last rollout.
    pub rollout_secs: f32,
    /// Wall-clock seconds of the last rollout spent probing for labels.
    pub label_secs: f32,
    /// Wall-clock seconds of the last installed update (its own duration on the update thread).
    pub update_secs: f32,
    pub weight_export_secs: f32,
    /// Host milliseconds spent in batched rollout forwards.
    pub rollout_forward_ms: f32,
    /// Seconds the last iteration waited for the update thread.
    pub update_wait_secs: f32,
    /// The iteration whose rollout the update installed during the last iteration trained on;
    /// `None` when none was installed (see `IterationReport::update_of_iteration`).
    pub update_of_iteration: Option<usize>,
    /// Candidate evaluations of the last iteration.
    pub evaluations_this_round: usize,
}

impl RunnerStats {
    /// Statistics of a run that has not started.
    pub(super) fn new() -> Self {
        Self {
            iteration: 0,
            episodes: 0,
            total_steps: 0,
            mean_step_reward: 0.0,
            reward_history: VecDeque::new(),
            best: None,
            best_improvement: None,
            recent_improvements: VecDeque::new(),
            baseline: None,
            current: None,
            solid_closed: true,
            last_validation: None,
            last_train: TrainingMetrics::default(),
            mean_step_reward_raw: 0.0,
            reward_scale: 1.0,
            explained_variance: None,
            accepted_frac: 0.0,
            episodes_finished: 0,
            episodes_improved_frac: 0.0,
            rej_unsafe: 0,
            rej_unsolvable: 0,
            rej_not_closed: 0,
            rej_girdle: 0,
            rej_manufacturability: 0,
            rej_not_improved: 0,
            last_imitation: None,
            labelled_samples: 0,
            labels_this_round: 0,
            oracle_gain_mean: None,
            val_oracle: None,
            rollout_secs: 0.0,
            label_secs: 0.0,
            update_secs: 0.0,
            weight_export_secs: 0.0,
            rollout_forward_ms: 0.0,
            update_wait_secs: 0.0,
            update_of_iteration: None,
            evaluations_this_round: 0,
        }
    }

    /// Mean score drop over the recent finished episodes.
    #[must_use]
    pub fn mean_improvement(&self) -> Option<f32> {
        (!self.recent_improvements.is_empty()).then(|| {
            self.recent_improvements.iter().sum::<f32>() / self.recent_improvements.len() as f32
        })
    }
}

/// Evaluation fidelity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalFidelity {
    /// The single canonical pose used by fast scoring.
    Fast,
    /// The full tilt sweep.
    Full,
}

/// Which passes besides the model's own an evaluation plays on each design. The model pass
/// (the policy mean, projected, stepped with the run's line search) is always played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvalPasses {
    /// Also play one stochastic episode per design: the exploration policy of the run (the
    /// lower bound a trained model must beat).
    pub sampled: bool,
    /// Also play the finite-difference reference direction on measured sensitivities with the
    /// same line search. It is not a guaranteed upper bound.
    pub oracle: bool,
}

/// Mean result of deterministic episodes (and optionally a sampled and an oracle episode) on a
/// set of designs.
#[derive(Clone, Debug, Default)]
pub struct EvalReport {
    /// Designs evaluated.
    pub designs: usize,
    /// Designs that could not be evaluated, and why for the first one.
    pub failed: usize,
    /// Reason of the first failure (the first failing design in entry order).
    pub first_failure: Option<String>,
    /// Whether the optics and scores are the full-sweep figures.
    pub full_fidelity: bool,
    /// Mean score before the episode.
    pub mean_base_score: f32,
    /// Mean score after the deterministic (policy mean) episode.
    pub mean_final_score: f32,
    /// Mean score drop of the deterministic episode.
    pub mean_improvement: f32,
    /// Mean score drop of one stochastic episode per design (actions drawn and projected as in
    /// the rollout), same stones, targets, weights and fidelity; `None` when the sampled pass
    /// was not requested. Training episodes draw their actions too, so this is the figure the
    /// training drop is compared with.
    pub mean_improvement_sampled: Option<f32>,
    /// Mean score drop of the finite-difference oracle episode per design, same stones,
    /// targets, weights and fidelity; `None` when the oracle pass was neither played nor taken
    /// from the cache. It does not depend on the model, which is why a run plays it once.
    pub mean_improvement_oracle: Option<f32>,
    /// Whether [`Self::mean_improvement_oracle`] was taken from the cache of an earlier
    /// validation instead of being played now.
    pub oracle_cached: bool,
    /// Candidate evaluations per environment step of the model pass, over the designs (1 when
    /// the first line-search scale is always accepted, up to the number of scales).
    pub mean_evaluations_per_step: f32,
    /// Mean optics before.
    pub base_optics: Optics,
    /// Mean optics after.
    pub final_optics: Optics,
    /// Deterministically ordered per-design outcomes, including objective and material context.
    pub cases: Vec<EvalCase>,
}

/// One held-out design outcome and the exact sampled task context.
#[derive(Clone, Debug, Default, Serialize)]
pub struct EvalCase {
    pub design_id: i64,
    pub design_title: String,
    pub objective: String,
    pub target_material: String,
    pub source_material: String,
    pub seed: u64,
    pub material_augmentation: String,
    pub target_augmentation: String,
    pub lighting: String,
    pub shape_target: String,
    pub base_score: Option<f32>,
    pub final_score: Option<f32>,
    pub score_drop: Option<f32>,
    pub accepted: Option<bool>,
    pub evaluations: usize,
    pub steps: usize,
    pub elapsed_seconds: Option<f32>,
    pub model_seconds: Option<f32>,
    pub failure: Option<String>,
    pub classical_base_score: Option<f32>,
    pub classical_final_score: Option<f32>,
    pub classical_evaluations: usize,
    pub classical_seconds: Option<f32>,
    pub classical_failure: Option<String>,
}
