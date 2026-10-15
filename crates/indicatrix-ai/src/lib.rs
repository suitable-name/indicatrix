//! `indicatrix-ai`: reinforcement-learning environments, a set-attention policy network, imitation
//! and PPO training and inference wrappers for gemstone cut optimization and material retargeting.
//!
//! # Pipeline
//!
//! - [`env`]: two deterministic, greedy environments that share one episode core.
//!   [`env::OptimizerEnv`] improves a design in its own material, [`env::RetargetEnv`] adapts it
//!   to a target material. An action is one angle delta per tier ([`env::ContinuousTierAction`]);
//!   hinge-anchored tiers (every tier of an imported `.asc`) turn about their hinge. A step tries
//!   its action along a line search ([`env::DEFAULT_LINE_SEARCH`]: the action, then half and a
//!   quarter of it) and keeps the first candidate that improves the stone. [`env::teacher`] holds
//!   what the simulator can say about the direction to move: finite-difference labels of the
//!   per-tier component sensitivities and the steepest-descent oracle step. The observation layout
//!   is versioned by [`env::OBS_LAYOUT_VERSION`]; its global token carries the objective's weight
//!   fractions.
//! - [`model`]: the pure-Rust [`model::GemstoneSetAttentionNetwork`] (token embeddings, pre-norm
//!   multi-head attention blocks, a final `LayerNorm`, a value head, a sensitivity head, a gain
//!   head and a state-independent, smoothly bounded `log_std` per block), the kinematic action
//!   projection, and the schema-versioned [`model::Checkpoint`] file.
//! - [`training`]: a Burn twin of the network ([`training::BurnSetAttention`]) and its trainer
//!   ([`training::BurnTrainer`]). The primary algorithm is supervised imitation of the
//!   finite-difference labels on the policy's own state distribution
//!   ([`training::ImitationConfig`]); PPO stays available on the same model, with a per-worker
//!   [`training::RolloutBuffer`] for GAE. Gradients are clipped by their global norm. Training is
//!   independent of the desktop workspace; the `indicatrix-trainer` app drives it.
//! - [`inference`]: [`inference::RlOptimizerModel`] and [`inference::RlRetargetModel`] run a
//!   trained network greedily in the matching environment and report cut-core's
//!   `OptimizeResult` or a list of changed tier angles, with the candidate evaluations spent.
//! - [`bridge`]: [`bridge::EditorRetargetAdapter`] merges a retarget result into the editor's
//!   `RetargetPlan` on top of the critical-angle shift baseline.
//!
//! # The composed policy
//!
//! The network does not predict an action. For every tier it predicts how much each of four
//! objective components (windowing, extinction, brilliance loss, yield loss) changes per degree of
//! that tier (the sensitivity heads). [`model::compose_mean`] weights those sensitivities with the
//! objective's weight fractions, which are part of the observation, and scales the result so the
//! steepest tier sits at [`env::MEAN_BOUND`] in pre-squash units; the squash then moves it 95 % of
//! the step limit. One model therefore serves every objective preset and any custom weighting
//! without having seen it. The model supplies the direction only: the environment's line search
//! finds the length, in training and in inference alike. A value head (the PPO baseline) and a
//! gain head (the predicted score drop of the oracle step, telemetry) read the same network.
//!
//! # Conventions
//!
//! - Angles are degrees and signed: crown tiers positive, pavilion tiers negative; no tier ever
//!   changes sign.
//! - Scores are `0..=100`, lower is better: the optical `ObjectiveWeights::score` figure, plus the
//!   yield loss (`ObjectiveWeights::score_with_yield`) when the weights weigh the yield and the
//!   design's preform makes it measurable.
//! - Everything is deterministic: identical network, design and settings give identical results.
//!   The environments contain no randomness; exploration noise belongs to the training sampler
//!   ([`model::Prng`], `SplitMix64` with explicit seeds).
//!
//! # Known limits
//!
//! - Rollouts score candidates at `Fast` fidelity (one table-up pose): the policy sees no tilt
//!   information while it acts, and the `Fast` score is quantised (about 0.16 points: a move that
//!   changes no ray count scores the same as no move, which the greedy gate rejects). Inference
//!   reports its before/after figures at `Full` fidelity and only proposes a change that also
//!   measures better there.
//! - The face-up tone term is not supported: the environments do not trace the body color and
//!   the observation carries none. The environments refuse a tone weight
//!   ([`env::EnvError::InvalidConfig`]) and the inference wrappers return
//!   [`inference::InferenceError::UnsupportedObjective`], so the presets Lighten Dark Rough and
//!   Intensify Pale Rough are not covered; every other weighting is.
//! - One model serves both tasks and every supported objective (the optimizer is the retarget
//!   task with the target equal to the source material), so there is no model per task or per
//!   objective; the optimizer wrapper accepts a retarget checkpoint, the retarget wrapper does not
//!   accept an optimizer one.
//! - An inference run stops early after a number of consecutive refused steps
//!   ([`inference::InferenceConfig::stall_steps`]); a rejected step spends the whole line search,
//!   so a run costs at most `steps * line_search.len()` evaluations.
//! - The network handles any tier count; the batched training pass pads to
//!   `ModelShape::max_tiers` (at most [`training::MAX_TIERS_LIMIT`]) and masks the padding.

#![warn(missing_docs)]

pub mod bridge;
pub mod env;
pub mod inference;
pub mod model;
pub mod training;
