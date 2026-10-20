//! Representative authored-state reproduction checks; run by the owner, never by this task.
use super::fixtures::{imported_rbc, rbc_445};
use crate::{
    Design, OptimizeOptions,
    design::tier_hinge_points,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    optimize::project_preserved_design,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// The capture of a design must accept that same design: capture and validation share one
/// definition of a junction (`network_topology::arrangement_junctions`), so an unchanged
/// design reproduces every original meet with nothing added and nothing moved. A
/// junction with a second candidate point carrying its whole support (a many-plane meet
/// clustered twice) used to refuse it as `DetachedJunction`.
fn assert_unchanged_design_validates(name: &str, design: &Design) {
    let baseline = MeetPreservationBaseline::capture(design)
        .unwrap_or_else(|error| panic!("{name} must capture: {error}"));
    for policy in [PreservationPolicy::OPTIMIZE, PreservationPolicy::RETARGET] {
        let facts = baseline.validate(design, policy).unwrap_or_else(|error| {
            panic!("{name} must validate against its own capture: {error}")
        });
        assert!(facts.original_meets > 0, "{name}");
        assert_eq!(facts.added_meets, 0, "{name}: no meet is added");
        assert!(facts.shape_distance <= 1e-9, "{name}: {facts:?}");
        assert!(facts.max_normalized_residual <= 2e-6, "{name}: {facts:?}");
    }
}

#[test]
fn an_unchanged_imported_pinned_design_validates_against_its_own_capture() {
    assert_unchanged_design_validates("imported RBC-445", &imported_rbc());
}

#[test]
fn an_unchanged_meet_derived_design_validates_against_its_own_capture() {
    assert_unchanged_design_validates("meet-derived RBC-445", &rbc_445());
}

fn check_representable_motion(original: &Design) {
    let baseline =
        MeetPreservationBaseline::capture(original).expect("trustworthy original fixture");
    baseline
        .validate(original, PreservationPolicy::OPTIMIZE)
        .expect("capture and validation must agree on the unchanged source topology");
    let solved = original.solve().expect("ordinary authoring solve");
    let options = OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: tier_hinge_points(original, &solved),
        ..Default::default()
    };
    let candidate = project_preserved_design(
        original,
        &crate::optimize::families::axial_preference(original, 1.015).unwrap(),
        &options,
        None,
    )
    .expect("useful representable feasible family");
    baseline
        .validate(&candidate, PreservationPolicy::OPTIMIZE)
        .expect("complete protected network");
    assert!(
        candidate
            .tiers
            .iter()
            .zip(&original.tiers)
            .any(|(a, b)| (a.angle_deg - b.angle_deg).abs() > 1e-3),
        "a guard that only returns the original is insufficient"
    );
    for (before, after) in original.tiers.iter().zip(&candidate.tiers) {
        if !matches!(before.constraint, MeetConstraint::ScaleReference(_)) {
            assert_eq!(
                before.constraint, after.constraint,
                "derived authoring instructions stay intact"
            );
        }
    }
    let serialized =
        crate::native::design_to_string(&candidate, None, &crate::native::DesignExtras::default())
            .unwrap();
    let reopened = crate::native::design_from_str(&serialized).unwrap().design;
    baseline
        .validate(&reopened, PreservationPolicy::OPTIMIZE)
        .expect("native authored-state reproduction");
    assert_eq!(candidate.planes().unwrap(), reopened.planes().unwrap());
}

#[test]
fn imported_pinned_fixture_has_useful_persistable_feasible_motion() {
    check_representable_motion(&imported_rbc());
}

#[test]
fn meet_derived_fixture_has_useful_persistable_feasible_motion() {
    check_representable_motion(&rbc_445());
}
