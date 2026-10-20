//! The error an environment constructor returns instead of training on a broken design.

use indicatrix_cut_core::DesignSolveError;
use std::fmt;

/// Why an environment could not be built from a design and a configuration.
///
/// Constructors reject unsolvable, open, unscorable or frozen designs and designs whose
/// immutable preservation baseline cannot be captured. A no-light base design is valid and is
/// scored normally.
#[derive(Debug)]
pub enum EnvError {
    /// The base design does not solve into masts.
    Unsolvable(DesignSolveError),
    /// The base design solves but its planes do not bound a closed solid.
    NotClosed,
    /// The base design is a closed solid but its optical score is not a finite number.
    NonFiniteScore,
    /// The immutable preservation baseline could not be captured from the base design.
    InvalidBaseline(String),
    /// No tier of the base design can move: every tier is horizontal, vertical, driven by a
    /// relation, or pinned without a hinge to turn about (see
    /// `OptimizerEnvConfig::vary_anchored`).
    NoMovableTiers,
    /// Legacy compatibility variant. Current constructors do not reject no-light designs.
    NoLightReturn,
    /// A configuration value is out of range; the text names it.
    InvalidConfig(&'static str),
}

impl fmt::Display for EnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsolvable(error) => write!(f, "the base design does not solve: {error}"),
            Self::NotClosed => f.write_str("the base design does not bound a closed solid"),
            Self::NonFiniteScore => {
                f.write_str("the base design's optical score is not a finite number")
            }
            Self::InvalidBaseline(reason) => {
                write!(f, "the base design has no valid preservation baseline: {reason}")
            }
            Self::NoMovableTiers => f.write_str("the base design has no tier the policy can move"),
            Self::NoLightReturn => f.write_str(
                "the base design returns no light face-up (tilt brilliance 0 %): nothing to optimise",
            ),
            Self::InvalidConfig(what) => write!(f, "invalid environment configuration: {what}"),
        }
    }
}

impl std::error::Error for EnvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unsolvable(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dead_stone_error_names_its_cause() {
        assert_eq!(
            EnvError::NoLightReturn.to_string(),
            "the base design returns no light face-up (tilt brilliance 0 %): nothing to optimise"
        );
    }

    #[test]
    fn invalid_baseline_error_preserves_the_capture_diagnostic() {
        let error = EnvError::InvalidBaseline("fixture capture failure".to_owned());
        assert_eq!(
            error.to_string(),
            "the base design has no valid preservation baseline: fixture capture failure"
        );
    }
}
