//! Imported designs whose meets are opened by the file's own quantisation.
//!
//! `GemCad` 4.x writes masts with five decimals, so a meet of four facets with different masts
//! misses by up to half a quantum (`5e-6`), a few millionths of the stone: inside the band the
//! former near-incidence rule refused (54 of the first 100 library designs, census
//! 2026-10-20). The meets are identified at `5e-5` of the stone instead; the design's own miss
//! is recorded as the junction's slack, which a candidate need not close.
//!
//! The standard brilliant template is no miniature of such a file (its masts are two-decimal
//! constants and its meets are not concurrent to begin with), so the miniature is the
//! octahedron with its four crown masts written the way a five-decimal file rounds them.

use super::*;
use indicatrix::geometry::meet_solver::SolveControl;

/// The reproduction bar, as a fraction of the stone.
const BAR: f64 = 2e-6;

/// The octahedron with its four crown facets' masts off by quantisation-sized amounts (at most
/// half a five-decimal quantum): the top apex and the four equator apexes are four-plane meets
/// that now miss by a few millionths of the stone.
fn quantised_octahedron() -> Design {
    let mut design = octahedron();
    for (name, miss) in [("C1", 4e-6), ("C2", -3e-6), ("C3", 2e-6)] {
        let tier = design
            .tiers
            .iter_mut()
            .find(|tier| tier.name == name)
            .expect("the crown tier exists");
        let MeetConstraint::ScaleReference(mast) = &mut tier.constraint else {
            unreachable!("the octahedron is pinned")
        };
        *mast += miss;
    }
    design
}

#[test]
fn a_quantised_import_captures_and_validates_unchanged() {
    let exact = MeetPreservationBaseline::capture(&octahedron()).expect("the exact octahedron");
    let design = quantised_octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("a quantised import captures");
    // The meets are identified, not split into micro-junctions joined by micro-edges.
    assert_eq!(baseline.structure(), exact.structure());
    // The quantisation is real (beyond the reproduction bar) and recorded, within the radius.
    assert!(baseline.max_slack() > BAR, "{}", baseline.max_slack());
    assert!(baseline.max_slack() <= baseline.identify_radius());
    let facts = baseline
        .validate(&design, PreservationPolicy::RETARGET)
        .expect("an unchanged import keeps its own meets");
    assert_eq!(facts.added_meets, 0);
    // The repair aims at the design's own miss, not at a closure it never had.
    let residuals = baseline
        .residuals(&design, &SolveControl::default())
        .expect("the residuals of the original");
    assert!(residuals.iter().all(|r| r.abs() <= BAR), "{residuals:?}");
}

#[test]
fn a_damaged_near_meet_is_still_refused() {
    let design = quantised_octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("a quantised import captures");
    let shift = 10.0 * baseline.max_slack() * baseline.scale();
    let mut damaged = design;
    let tier = damaged
        .tiers
        .iter_mut()
        .find(|tier| tier.name == "C1")
        .expect("the crown tier exists");
    let MeetConstraint::ScaleReference(mast) = &mut tier.constraint else {
        unreachable!("the octahedron is pinned")
    };
    *mast += shift;
    // Far below any shape bound: only the meet itself can refuse it.
    assert!(shift / baseline.scale() < 1e-3);
    let error = baseline
        .validate(&damaged, PreservationPolicy::RETARGET)
        .expect_err("a meet opened ten times its own slack is damaged");
    assert!(
        ["detached_junction", "lost_facet", "collapsed_edge"].contains(&error.kind()),
        "{error}"
    );
}

#[test]
fn exact_designs_keep_their_residual_bar() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("the exact octahedron");
    assert!(baseline.max_slack() <= BAR, "{}", baseline.max_slack());
    let facts = baseline
        .validate(&design, PreservationPolicy::RETARGET)
        .expect("an unchanged design keeps its meets");
    assert!(facts.max_normalized_residual <= BAR);
}
