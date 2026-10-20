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
//! The network predicts per-tier slopes for seven components: windowing, extinction, brilliance
//! loss, yield loss, lighter-tone loss, deeper-tone loss and shape distance. [`model::compose_mean`]
//! weights them with observed objective coefficients and composes a direction; the environment's
//! line search determines the accepted length. A value head supports PPO and a gain head predicts
//! the oracle score drop.
//!
//! # Conventions
//!
//! - Angles are degrees and signed: crown tiers positive, pavilion tiers negative; no tier ever
//!   changes sign.
//! - Scores use the shared cut-core optical/tone objective, add yield loss when measurable and
//!   weighted, and add configured shape penalty. Lower is better. Yield loss follows core's
//!   `100 - yield_percent` convention without an AI-side clamp.
//! - Everything is deterministic: identical network, design and settings give identical results.
//!   The environments contain no randomness; exploration noise belongs to the training sampler
//!   ([`model::Prng`], `SplitMix64` with explicit seeds).
//!
//! # Known limits
//!
//! - Training defaults to `Fast`; either environment accepts `Fast` or `Full`. Inference uses its
//!   configured rollout fidelity and independently scores endpoints at report fidelity.
//! - Tone is measured and both tone goals are supported. A score readout can be requested directly
//!   with `inference::score_design_at_fidelity` without capturing a new environment.
//! - Checkpoints are task-specific: optimizer and retarget wrappers accept only their matching task
//!   and reject wrong schema/layout stamps, even for an in-memory checkpoint.
//! - Retargeting preserves the exact certified angles and masts in `CertifiedRetargetResult`.
//!   The legacy editor adapter verifies its angle-plan reconstruction and returns an error if it
//!   cannot reproduce that certified geometry.
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
