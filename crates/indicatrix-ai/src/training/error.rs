//! Error type for the training pipeline (PPO updates, rollout buffers, device set-up, weight export).

use std::fmt;

/// Error raised by the training pipeline.
///
/// Every failure that used to be a panic, a silent zero padding or an empty `Vec` is reported through
/// this type so the caller (GUI worker or CLI) can log it and decide whether to continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainError {
    /// A hyperparameter or constructor argument is out of range.
    InvalidConfig(String),
    /// A batch row, action row, tier count or weight array has the wrong length.
    ShapeMismatch(String),
    /// A loss, reward or value was NaN or infinite; the optimizer step was skipped.
    NonFinite(String),
    /// Reading a tensor back to the host failed.
    Tensor(String),
    /// The requested compute device could not be initialised.
    DeviceUnavailable(String),
}

impl fmt::Display for TrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(m) => write!(f, "invalid training configuration: {m}"),
            Self::ShapeMismatch(m) => write!(f, "shape mismatch: {m}"),
            Self::NonFinite(m) => write!(f, "non-finite value: {m}"),
            Self::Tensor(m) => write!(f, "tensor read-back failed: {m}"),
            Self::DeviceUnavailable(m) => write!(f, "compute device unavailable: {m}"),
        }
    }
}

impl std::error::Error for TrainError {}
