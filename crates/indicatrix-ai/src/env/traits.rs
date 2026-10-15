//! Fundamental traits for Reinforcement Learning Markov Decision Processes (MDPs) in gemstone design.
//!
//! # Conventions shared by both environments
//!
//! - Angles are degrees. `Design` tier angles are SIGNED: crown positive, pavilion negative.
//! - Scores are the `ObjectiveWeights::score` figure, `0..=100`, LOWER is better.
//! - The MDP is greedy: a candidate design replaces the current one only when it is valid,
//!   passes the girdle and manufacturability gates and scores strictly lower. The state
//!   therefore never gets worse, and a rejected proposal leaves it untouched.
//! - A step may try its action at several scales (a line search, see
//!   [`GemstoneRlEnv::step_scaled`]); the first improving candidate is accepted and ONE step
//!   is counted. [`GemstoneRlEnv::step`] is the single-scale case.
//! - The score is the optical `ObjectiveWeights::score`, plus the yield term when the weights
//!   weigh the yield and the yield is measurable. A face-up tone weight is refused at
//!   construction.

use serde::{Deserialize, Serialize};

/// Discrete or continuous step transition result in an RL environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step<O> {
    /// Observation resulting from the action.
    pub observation: O,
    /// Reward scalar emitted by the transition.
    pub reward: f32,
    /// `true` if episode has reached a terminal state (the score fell below the configured
    /// terminal score).
    pub terminated: bool,
    /// `true` if truncated due to the step limit rather than natural termination.
    pub truncated: bool,
    /// Diagnostic telemetry and breakdown of metrics for logging and reward tracking.
    pub info: StepInfo,
}

/// Why a proposed action did not change the design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepRejection {
    /// The action does not carry exactly one delta per tier of the design.
    WrongActionLength {
        /// `design.tiers.len()`.
        expected: usize,
        /// The length of the action's delta vector.
        got: usize,
    },
    /// A delta for a movable tier is NaN or infinite.
    NonFiniteDelta,
    /// A movable tier would leave its safe angle range or change side of the girdle.
    UnsafeAngle,
    /// A hinge-anchored tier has no positive mast through its hinge at the proposed angle.
    NoUsableMast,
    /// The tier relations cannot be evaluated, or give a follower an unusable angle.
    RelationsUnsolvable,
    /// The candidate design does not solve.
    Unsolvable,
    /// The candidate's planes do not bound a closed solid.
    NotClosed,
    /// The candidate's optical score is not a finite number.
    NonFiniteScore,
    /// The candidate thins the girdle band below the configured fraction, or loses it.
    GirdleViolation,
    /// The candidate has more vanishing or undersized facets than the base design.
    ManufacturabilityRegression,
    /// The candidate is a valid stone that does not score lower than the current one.
    NotImproved,
}

/// Telemetry information returned alongside a transition step.
///
/// The optical fields describe the CURRENT (accepted) state after the step, not the rejected
/// proposal, so they only change when a proposal was accepted. `rejection`, `candidate_score`
/// and `scale` describe the candidate the step decided on: the accepted one or, when none was
/// accepted, the first (full-scale) one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StepInfo {
    /// Current optical score (lower is better in indicatrix convention).
    pub current_score: f32,
    /// Score improvement this step produced in the current state (`previous - current`, so
    /// positive indicates improvement and `0.0` means the proposal was rejected).
    pub score_delta: f32,
    /// Windowing percentage of the current state.
    pub windowing_pct: f32,
    /// Extinction percentage of the current state.
    pub extinction_pct: f32,
    /// Tilt brilliance percentage of the current state (higher is better).
    pub tilt_brilliance_pct: f32,
    /// Volumetric yield of the current state, in percent of the preform; `0.0` when the
    /// preform volume cannot be measured.
    pub yield_pct: f32,
    /// Thinnest girdle band of the current state relative to the base design's (`1.0` when
    /// the design has no measurable girdle band).
    pub girdle_ratio: f32,
    /// Whether the proposal solved into a closed polyhedron.
    pub is_closed: bool,
    /// Whether the proposal passed the manufacturability gate (no more vanishing or
    /// undersized facets than the base design).
    pub manufacturability_ok: bool,
    /// Step index within current episode.
    pub episode_step: usize,
    /// Why the proposal did not change the design; `None` when it was accepted.
    pub rejection: Option<StepRejection>,
    /// The evaluated candidate's own score, whenever it solved, closed and scored finitely,
    /// WHATEVER the gates and the acceptance decided: a proposal that failed the girdle or
    /// manufacturability gate, or did not improve, still reports the score it would have had
    /// (the reward does not credit a gate-failed score, this field does not hide it). `None`
    /// for a proposal rejected before it was scored (malformed action, unsafe angle,
    /// unsolvable, open or non-finite candidate). Lets a runner log the proposal quality
    /// without recomputing it from the reward.
    pub candidate_score: Option<f32>,
    /// The line-search scale of the accepted candidate (`Some(1.0)` = the action as
    /// proposed, `Some(0.5)` = half of it); `None` when no candidate was accepted.
    pub scale: Option<f32>,
    /// Candidates scored in this step: those that reached the scorer, whatever the gates and
    /// the acceptance decided (at most the number of scales). `0` when the action was refused
    /// before any scoring (wrong length, non-finite delta, unsafe angle at every scale); a
    /// candidate refused before scoring costs nothing and is not counted.
    #[serde(default)]
    pub evaluations: u8,
}

/// Reinforcement learning environment trait for gemstone design search.
pub trait GemstoneRlEnv {
    /// Observation type representing the stone's geometric & optical state.
    type Observation;
    /// Action type representing policy decisions (e.g. angle modifications).
    type Action;

    /// Reset environment to the initial design state and return starting observation.
    ///
    /// The environments are deterministic: they contain no stochastic element, so `seed` is
    /// accepted for trait uniformity and does not change the result. Exploration noise
    /// belongs to the policy that samples actions.
    fn reset(&mut self, seed: u64) -> Self::Observation;

    /// Apply an action, advance the state by solving geometry and scoring optics,
    /// and return the transition step.
    ///
    /// Exactly [`Self::step_scaled`] with `scales = [1.0]`, bit for bit.
    fn step(&mut self, action: &Self::Action) -> Step<Self::Observation>;

    /// Apply an action along a line search and return the transition step.
    ///
    /// Each scale in `scales`, in order, multiplies every delta of the action; the first
    /// candidate that is a valid stone, passes both gates and scores strictly lower is
    /// accepted and the search stops. ONE step is counted whatever the number of scales. The
    /// reward and the [`StepInfo`] describe the accepted candidate or, when none was
    /// accepted, the first one tried; [`StepInfo::scale`] names the accepted scale and
    /// [`StepInfo::evaluations`] counts the candidates scored. A scale that is not finite and
    /// positive is skipped, and an empty list (or one with no usable scale) behaves like
    /// `[1.0]`.
    fn step_scaled(&mut self, action: &Self::Action, scales: &[f64]) -> Step<Self::Observation>;
}
