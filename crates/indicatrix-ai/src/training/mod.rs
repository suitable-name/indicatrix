//! Training pipelines: the rollout buffer, the Burn twin of the network, the Burn trainer with its
//! PPO and imitation losses, and the shared hyperparameter types.
//!
//! - [`burn_model`]: [`BurnSetAttention`], the model on Burn tensors, convertible to and from the
//!   pure-Rust network.
//! - [`burn_model_host`]: the batched rollout forward of that model
//!   ([`BurnSetAttention::forward_host`], [`HostPolicyBatch`]) and the state packing the trainers
//!   share.
//! - [`burn_ppo`]: [`BurnTrainer`] and the PPO update (the secondary algorithm), and
//!   [`BurnTrainer::rollout_model`], the inference copy a rollout runs on.
//! - [`imitation`]: the supervised loss on finite-difference labels (the primary algorithm).

pub mod buffer;
pub mod burn_model;
pub mod burn_model_host;
pub mod burn_ppo;
pub mod error;
pub mod imitation;
pub mod ppo;

pub use buffer::{EpisodeEnd, RolloutBuffer, TrajectoryStep};
pub use burn_model::{BurnForward, BurnSetAttention, HostPolicyBatch};
pub use burn_ppo::{BurnTrainer, MAX_TIERS_LIMIT, ModelShape, TrainBatch};
pub use error::TrainError;
pub use imitation::{ImitationBatch, ImitationConfig, ImitationMetrics};
pub use ppo::{LrSchedule, PpoConfig, TrainingMetrics};
