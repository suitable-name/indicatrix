//! The error the inference wrappers return instead of a plausible-looking wrong answer.

use crate::{
    env::EnvError,
    model::{ModelError, WeightError},
};
use std::fmt;

/// Why an inference call could not produce a result.
#[derive(Debug)]
pub enum InferenceError {
    /// The environment could not be built from the design and the configuration (configuration
    /// out of range, design does not solve, is not closed, or has no tier the policy can move).
    Env(EnvError),
    /// The network rejected an observation (its tokens do not match the network's widths).
    Model(ModelError),
    /// The network or checkpoint is unusable: wrong shape for the current observation layout,
    /// wrong task, wrong layout version, or a damaged file.
    Weights(WeightError),
    /// The final design (or the starting design) could not be scored for the report.
    Unscorable(String),
    /// The baseline plan the retarget starts from could not be built.
    Baseline(String),
    /// The objective weighs something the models do not cover: a face-up tone term (the presets
    /// Lighten Dark Rough and Intensify Pale Rough). The text names the term and the presets.
    /// Returned before any environment is built.
    UnsupportedObjective(String),
}

impl fmt::Display for InferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(error) => write!(f, "cannot start the episode: {error}"),
            Self::Model(error) => write!(f, "the network rejected the observation: {error}"),
            Self::Weights(error) => write!(f, "unusable network: {error}"),
            Self::Unscorable(why) => write!(f, "cannot score the design for the report: {why}"),
            Self::Baseline(why) => write!(f, "cannot build the retarget baseline: {why}"),
            Self::UnsupportedObjective(why) => write!(f, "unsupported objective: {why}"),
        }
    }
}

impl std::error::Error for InferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Env(error) => Some(error),
            Self::Model(error) => Some(error),
            Self::Weights(error) => Some(error),
            Self::Unscorable(_) | Self::Baseline(_) | Self::UnsupportedObjective(_) => None,
        }
    }
}

impl From<EnvError> for InferenceError {
    fn from(error: EnvError) -> Self {
        Self::Env(error)
    }
}

impl From<ModelError> for InferenceError {
    fn from(error: ModelError) -> Self {
        Self::Model(error)
    }
}

impl From<WeightError> for InferenceError {
    fn from(error: WeightError) -> Self {
        Self::Weights(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn an_unsupported_objective_displays_its_reason_and_has_no_source() {
        let error = InferenceError::UnsupportedObjective("the tone term".to_owned());
        assert_eq!(error.to_string(), "unsupported objective: the tone term");
        assert!(error.source().is_none());
    }

    #[test]
    fn a_wrapped_environment_error_keeps_its_cause_visible() {
        let error = InferenceError::from(EnvError::NoMovableTiers);
        assert!(matches!(
            error,
            InferenceError::Env(EnvError::NoMovableTiers)
        ));
        assert!(error.to_string().contains("no tier the policy can move"));
    }
}
