//! Basic sanity tests: a fresh design is already closed, a single
//! scale-referenced tier extends the preform, `PreformShape` round-trips, and
//! a missing scale anchor fails closed with a named error.

use crate::{
    design::{ConstraintTier, Design, DesignSolveError},
    preform::{PreformShape, PreformSpec},
};
use indicatrix::geometry::meet_solver::{Block, MeetConstraint};

/// A fresh design (preform, no tiers) must already be a closed, positive-
/// volume solid -- the whole point of modeling the preform as a real plane
/// set instead of leaving the viewport empty until the first facet exists.
/// With no tiers at all, no block is even present, so there is nothing for
/// `solve` to complain about missing an anchor for.
#[test]
fn fresh_design_alone_is_closed() {
    let design = Design::fresh(PreformSpec::cylinder(96, 1.0, 1.0, 0.8), 96, 8, 1.54);
    assert_eq!(design.tiers.len(), 0);
    assert!(
        design
            .solve()
            .expect("no blocks, nothing to anchor")
            .is_empty()
    );
    assert!(design.is_closed());
    let metrics = design
        .measure()
        .expect("fresh design must measure")
        .expect("fresh design must be closed");
    assert!(metrics.volume > 0.0);
    // With no facets at all, the design's own planes are exactly the
    // preform's.
    assert_eq!(design.planes().unwrap(), design.preform.planes());
}

/// Adding a single tier with an explicit scale-reference constraint (a
/// real authored dimension, not a meet reference) extends -- never
/// replaces -- the preform's own planes, and the resulting arrangement
/// still closes.
#[test]
fn schedule_planes_extend_the_preform() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.32),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    let planes = design.planes().expect("a single anchored tier must solve");
    assert_eq!(planes.len(), design.preform.planes().len() + 1);
    assert!(
        planes[..design.preform.planes().len()]
            .iter()
            .zip(&design.preform.planes())
            .all(|(a, b)| a == b)
    );
    // A lone table facet, cutting into (not through) an oversized block
    // preform, still leaves a closed solid.
    assert!(design.is_closed());
}

/// `PreformShape` is re-exported for callers building a `PreformSpec`
/// directly (exercised here just to confirm the re-export compiles and
/// matches).
#[test]
fn preform_shape_round_trips_through_design() {
    let design = Design::fresh(PreformSpec::block(0.5, 1.0, 0.4), 48, 2, 1.5);
    assert_eq!(design.preform.shape, PreformShape::Block);
}

/// A schedule with a meet-derived tier and no stated scale reference at
/// all must fail closed (a named `MissingAnchor`), not silently fall back
/// to the solver's own internal default scale.
#[test]
fn solve_reports_the_missing_anchor_block_rather_than_a_silent_default() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 30.0,
        name: "C1".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    let err = design.solve().expect_err("crown has no scale reference");
    let DesignSolveError::MissingAnchor(missing) = err else {
        panic!("expected MissingAnchor, got {err:?}");
    };
    assert_eq!(missing.blocks, vec![Block::Crown]);
    assert!(
        !design.is_closed(),
        "planes()/status() must fail closed too"
    );
}
