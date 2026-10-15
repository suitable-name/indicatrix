//! Inference pipelines for running trained optimizer and retarget networks.
//!
//! Both models wrap a [`GemstoneSetAttentionNetwork`](crate::model::GemstoneSetAttentionNetwork)
//! (the network the trainer produces and a [`Checkpoint`](crate::model::Checkpoint) carries),
//! run it greedily in the matching environment and report a result in the shape the rest of the
//! workspace already consumes:
//!
//! - [`RlOptimizerModel::optimize`] returns cut-core's `OptimizeResult`.
//! - [`RlRetargetModel::retarget`] returns an [`RlRetargetProposal`] of changed tier angles.
//!
//! # How a run goes
//!
//! Each step the network predicts, per tier, how much each objective component changes per
//! degree; the policy mean is composed from those sensitivities and the objective's weight
//! fractions (so one model serves every weighting), and the deterministic action
//! `tanh(mean) * max_step_deg` is projected onto the feasible angles. The environment then tries
//! the action along a line search ([`InferenceConfig::line_search`], by default the full action,
//! then half and a quarter of it) and keeps the first candidate that improves the stone, so the
//! model supplies a direction and the environment finds the length. A run ends after
//! [`InferenceConfig::steps`] steps, when the environment ends the episode, or after
//! [`InferenceConfig::stall_steps`] consecutive steps without an accepted candidate. The result
//! reports the candidate evaluations the rollout spent.
//!
//! # What is supported
//!
//! One trained model serves both tasks: the optimizer is the retarget task with the target equal
//! to the source material, so [`RlOptimizerModel`] accepts a retarget checkpoint as well as an
//! optimizer one (the reverse is refused). The objective may be any weighting of windowing,
//! extinction, tilt brilliance and yield; a face-up tone term (the presets Lighten Dark Rough
//! and Intensify Pale Rough) is not covered and is refused with
//! [`InferenceError::UnsupportedObjective`] before any environment is built.
//!
//! Every fallible step returns an [`InferenceError`]; nothing panics on a bad network file or
//! an unsolvable design, and nothing falls back to a made-up answer. Settings live in
//! [`InferenceConfig`]. Runs are deterministic: the same network, design and settings give
//! identical results.

mod config;
mod error;
pub mod optimizer_model;
mod policy;
mod report;
pub mod retarget_model;

pub use config::{
    DEFAULT_OPTIMIZER_STEP_DEG, DEFAULT_RETARGET_STEP_DEG, DEFAULT_STALL_STEPS, DEFAULT_STEPS,
    InferenceConfig,
};
pub use error::InferenceError;
pub use optimizer_model::RlOptimizerModel;
pub(crate) use policy::ensure_supported_objective;
pub use retarget_model::{RlRetargetModel, RlRetargetProposal};
