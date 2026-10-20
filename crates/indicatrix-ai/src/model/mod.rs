//! Neural network architectures, policy heads, value estimators, and weight persistence.
//!
//! - [`actor_critic`]: the set-attention network with its sensitivity, value and gain heads.
//! - [`policy`]: the Gaussian policy distribution and the composed policy mean.
//! - [`network`]: the layer primitives, [`weights`]: validation and bare-network JSON,
//!   [`checkpoint`]: the schema-versioned training checkpoint, [`rng`]: the deterministic PRNG.

pub mod actor_critic;
pub mod checkpoint;
pub mod network;
pub mod policy;
pub mod rng;
pub mod weights;

pub use actor_critic::{ForwardOutput, GemstoneSetAttentionNetwork, ModelTier};
pub use checkpoint::{CHECKPOINT_SCHEMA_VERSION, Checkpoint, TrainingMode};
pub use network::{
    InferenceScratch, LayerNorm, Linear, MlpBlock, ModelError, MultiHeadAttention, gelu,
};
pub use policy::{
    GaussianPolicy, LOG_STD_INIT, LOG_STD_MAX, LOG_STD_MIN, LOG_STD_SLOTS, block_slot,
    bounded_log_std, compose_mean, compose_mean_masked, log_std_raw_init,
};
pub use rng::Prng;
pub use weights::WeightError;
