//! Shared test fixtures for [`super`]'s topic modules: a tiny [`ConstraintTier`]
//! builder and a fresh, tier-less [`Design`] to build test schedules on top of.

use crate::{
    design::{ConstraintTier, Design},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

pub(super) fn tier(
    name: &str,
    angle_deg: f64,
    constraint: MeetConstraint,
    indices: &[f64],
) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

pub(super) fn fresh_design() -> Design {
    Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62)
}
