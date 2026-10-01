//! [`SolveError`]: why a solve-role job produced no answer.
//!
//! The page's solve clients (`host::SolveClient`, wasm32 only) resolve a job's future with
//! a [`SolveResponse`](crate::solve::SolveResponse) or one of these. A caller that treats
//! "the page gave up on the job" differently from "the job failed" (a superseded metrics
//! request is sent again, a cancelled search says so, a failure is shown as an error)
//! matches on the variant instead of comparing message text across crates.

use std::fmt;

/// Why a solve-role job ended without a `SolveResponse`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolveError {
    /// A newer request on the same client replaced the job before it finished; its
    /// answer is not wanted.
    Superseded,
    /// The page cancelled the job and its Worker was terminated (a hard cancel, or a
    /// graceful one the Worker did not answer in time), so no partial result exists.
    Cancelled,
    /// Anything else: a timeout, a crashed or missing Worker, an unreadable message.
    Failed(String),
}

impl SolveError {
    /// `true` when the job ended because the page asked it to ([`Self::Superseded`] or
    /// [`Self::Cancelled`]) rather than because something went wrong.
    #[must_use]
    pub const fn is_requested(&self) -> bool {
        matches!(self, Self::Superseded | Self::Cancelled)
    }
}

impl fmt::Display for SolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Superseded => f.write_str("superseded by a newer solve request"),
            Self::Cancelled => f.write_str("solve cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SolveError {}

impl From<String> for SolveError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_endings_are_told_apart_from_failures() {
        assert!(SolveError::Superseded.is_requested());
        assert!(SolveError::Cancelled.is_requested());
        assert!(!SolveError::Failed("boom".to_string()).is_requested());
    }

    #[test]
    fn each_variant_reads_as_a_sentence_and_a_failure_keeps_its_text() {
        assert_eq!(
            SolveError::Superseded.to_string(),
            "superseded by a newer solve request"
        );
        assert_eq!(SolveError::Cancelled.to_string(), "solve cancelled");
        assert_eq!(
            SolveError::from("the solve worker crashed: oops".to_string()).to_string(),
            "the solve worker crashed: oops"
        );
    }
}
