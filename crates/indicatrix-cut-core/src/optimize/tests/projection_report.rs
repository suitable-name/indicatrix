//! `project_preserved_design_report` shares the projection of the plain entry points
//! (follow-up plan 2026-10-19, section 8.1). Run by the owner.
use super::fixtures::{imported_rbc, rbc_445};
use crate::{
    Design, OptimizeOptions,
    design::tier_hinge_points,
    optimize::{
        families::axial_preference, project_preserved_design_controlled,
        project_preserved_design_report,
    },
};
use indicatrix::geometry::meet_solver::SolveControl;

fn options(original: &Design) -> OptimizeOptions {
    let solved = original.solve().expect("ordinary authoring solve");
    OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: tier_hinge_points(original, &solved),
        ..Default::default()
    }
}

/// The plain projection and the report of the same inputs agree bit for bit.
fn assert_same(name: &str, original: &Design, factor: f64) {
    let proposed = axial_preference(original, factor).expect("a preference");
    let options = options(original);
    let control = SolveControl::default();
    let plain = project_preserved_design_controlled(original, &proposed, &options, &control);
    let report = project_preserved_design_report(original, &proposed, &options, &control);
    match (&plain, &report.result) {
        (Ok(a), Ok(b)) => {
            for (x, y) in a.tiers.iter().zip(&b.tiers) {
                assert_eq!(x.angle_deg.to_bits(), y.angle_deg.to_bits(), "{name}");
                assert_eq!(x.constraint, y.constraint, "{name}");
            }
            assert_eq!(a, b, "{name}");
        }
        (Err(a), Err(b)) => assert_eq!(a, b, "{name}"),
        _ => panic!("{name}: the report disagrees with the plain projection"),
    }
}

#[test]
fn the_report_and_the_plain_projection_return_the_same_design_bit_for_bit() {
    assert_same("imported RBC-445", &imported_rbc(), 1.015);
    assert_same("meet-derived RBC-445", &rbc_445(), 1.015);
    assert_same("imported RBC-445, large stretch", &imported_rbc(), 1.5);
}

#[test]
fn a_refused_projection_reports_its_last_probe() {
    let original = imported_rbc();
    let proposed = axial_preference(&original, 3.0).expect("a preference");
    let options = options(&original);
    let report =
        project_preserved_design_report(&original, &proposed, &options, &SolveControl::default());
    assert!(
        report.result.is_err(),
        "premise: a threefold axial stretch cannot keep every meet"
    );
    let probe = report.last_probe.expect("the refused probe is reported");
    assert_eq!(probe.tiers.len(), original.tiers.len());
    assert!(report.iterations <= 12);
}

#[test]
fn an_accepted_projection_reports_no_refused_probe() {
    let original = imported_rbc();
    let proposed = original.clone();
    let report = project_preserved_design_report(
        &original,
        &proposed,
        &options(&original),
        &SolveControl::default(),
    );
    assert!(report.result.is_ok());
    assert!(report.last_probe.is_none());
    assert_eq!(report.iterations, 0);
}
