use super::*;
use crate::{ConstraintTier, PreformSpec, ScheduleMeta};
use indicatrix::geometry::meet_solver::{MeetConstraint, SolveControl};

#[test]
fn residuals_detect_an_original_junction_clipped_by_another_plane() {
    let original = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let baseline = MeetPreservationBaseline::capture(&original).unwrap();
    baseline
        .validate(&original, PreservationPolicy::RETARGET)
        .expect("the captured source must validate against its own meet network");
    let control = SolveControl::default();
    let initial = baseline.residuals(&original, &control).unwrap();
    assert!(initial.iter().all(|r| r.abs() < 2e-6));

    let mut clipped = original;
    let culet = clipped
        .tiers
        .iter_mut()
        .find(|tier| tier.name == "Culet")
        .unwrap();
    culet.constraint = MeetConstraint::ScaleReference(1.2);
    let residual = baseline.residuals(&clipped, &control).unwrap();
    // The old support triples still intersect algebraically, but outside the solid.
    // Basis equalities alone cannot give the correction a direction to restore them.
    assert_eq!(
        residual.len(),
        initial.len(),
        "Jacobian row layout stays stable"
    );
    assert!(residual.iter().any(|r| r.abs() > 1e-3));
    assert!(
        baseline
            .validate(&clipped, PreservationPolicy::RETARGET)
            .is_err()
    );
}

#[test]
fn failed_projection_retains_the_actual_geometry_reason() {
    let original = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let baseline = MeetPreservationBaseline::capture(&original).unwrap();
    // A valid geometry with an intentionally impossible policy cannot be repaired by
    // an equality correction. Its final refusal must identify the actual shape gate.
    let options = crate::OptimizeOptions {
        preservation_baseline: Some(std::sync::Arc::new(baseline)),
        preservation_policy: PreservationPolicy {
            max_junction_displacement: -1.0,
            ..PreservationPolicy::RETARGET
        },
        ..Default::default()
    };
    let error = crate::optimize::project_preserved_design(&original, &original, &options, None)
        .expect_err("invalid policy must not be silently accepted");
    assert!(
        matches!(&error, PreservationViolation::ProjectionFailed { reason, .. }
        if reason.contains("permitted change")),
        "{error}"
    );
}

/// The junction capture must not depend on how large the rough is around the stone: the
/// display mesh welds in units of the rough, the preservation tolerance is the stone's.
#[test]
fn the_brilliant_network_is_captured_and_clipping_detected_whatever_the_rough_size() {
    for (width, height, depth) in [(2.0, 1.0, 4.0), (16.0, 8.0, 32.0), (128.0, 64.0, 256.0)] {
        let rough = format!("rough {width} x {height} x {depth}");
        let original = Design::new(
            PreformSpec::block(width, height, depth),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let baseline = MeetPreservationBaseline::capture(&original)
            .unwrap_or_else(|error| panic!("{rough}: {error}"));
        baseline
            .validate(&original, PreservationPolicy::RETARGET)
            .unwrap_or_else(|error| panic!("{rough}: {error}"));
        let mut clipped = original.clone();
        let culet = clipped
            .tiers
            .iter_mut()
            .find(|tier| tier.name == "Culet")
            .unwrap();
        culet.constraint = MeetConstraint::ScaleReference(1.2);
        assert!(
            baseline
                .validate(&clipped, PreservationPolicy::RETARGET)
                .is_err(),
            "{rough}"
        );
    }
}
