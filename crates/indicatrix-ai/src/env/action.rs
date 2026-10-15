//! Action space for gemstone cut optimization and retargeting agents.

use serde::{Deserialize, Serialize};

/// Continuous action representing simultaneous angle deltas for every tier of the design.
///
/// The vector is per TIER, in design order, and its length must equal the design's tier
/// count: entry `k` is the delta (degrees) of tier `k`. Entries of tiers the environment does
/// not move (horizontal, vertical, pinned without a hinge, driven by a relation) are ignored.
/// An action of any other length is rejected by the environment (reward = invalid-geometry
/// penalty, `StepRejection::WrongActionLength`), never reinterpreted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContinuousTierAction {
    /// Proposed angle changes in degrees, indexed by tier position.
    pub deltas_deg: Vec<f64>,
}

impl ContinuousTierAction {
    /// Create a new continuous action from a slice of angle deltas.
    #[must_use]
    pub const fn new(deltas_deg: Vec<f64>) -> Self {
        Self { deltas_deg }
    }

    /// Clamp all deltas within symmetric bounds `[-max_step_deg, max_step_deg]`.
    pub fn clamp(&mut self, max_step_deg: f64) {
        for delta in &mut self.deltas_deg {
            *delta = delta.clamp(-max_step_deg, max_step_deg);
        }
    }
}
