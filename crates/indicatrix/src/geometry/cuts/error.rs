//! [`CutError`]: everything that can go wrong reconstructing a validated B-Rep
//! from an `.asc` file's cutting instructions.

use std::fmt;

use crate::geometry::brep::BrepError;

/// Everything that can go wrong in [`super::StandardGemCuts::reconstruct_validated_brep_from_asc`].
#[derive(Debug, Clone, PartialEq)]
pub enum CutError {
    /// [`crate::geometry::GemPolyhedron::from_planes`] itself failed to reconstruct a valid, closed,
    /// finite solid from the schedule's planes.
    Brep(BrepError),
    /// The planes reconstructed into a valid solid, but one or more of them
    /// contributed no facet -- the schedule is over-constrained.
    OverConstrained {
        /// Number of planes that contributed no facet.
        untouched_count: usize,
        /// Total number of planes reconstructed from the schedule.
        plane_count: usize,
        /// Indices of the untouched planes.
        untouched: Vec<usize>,
    },
}

impl fmt::Display for CutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Verbatim pass-through: this matches the original `?`-propagated text
            // exactly, since `BrepError::Display` reproduces `from_planes`'s own
            // former `String` error message.
            Self::Brep(e) => write!(f, "{e}"),
            Self::OverConstrained {
                untouched_count,
                plane_count,
                untouched,
            } => write!(
                f,
                "{untouched_count} of {plane_count} planes from the .asc schedule contribute no facet (untouched indices: \
                 {untouched:?}); the schedule is over-constrained -- most often a near-duplicate tier revision \
                 left in the source file (e.g. the same facet listed twice at slightly different masts), \
                 occasionally a crown/pavilion sign misclassification on a zero-angle tier"
            ),
        }
    }
}

impl std::error::Error for CutError {}

impl From<BrepError> for CutError {
    fn from(e: BrepError) -> Self {
        Self::Brep(e)
    }
}
