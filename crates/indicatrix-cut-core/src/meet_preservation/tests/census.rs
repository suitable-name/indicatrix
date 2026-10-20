//! The census measurements (follow-ups plan 2026-10-19, sections 4.3 and 8.1): raw shape
//! deltas under `PreservationPolicy::CENSUS`, the baseline's structure counts and the stable
//! violation kinds. They measure; no verdict of `OPTIMIZE` or `RETARGET` depends on them.

use super::*;
use crate::{
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy, PreservationViolation},
    templates::TEMPLATES,
};

/// Every built-in template at its own schedule and preform, and the concave fixture.
fn census_designs() -> Vec<(String, Design)> {
    let mut designs: Vec<_> = TEMPLATES
        .iter()
        .map(|spec| {
            (
                spec.name.to_owned(),
                Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers()),
            )
        })
        .collect();
    designs.push(("octahedron".to_owned(), octahedron()));
    designs.push(("concave fixture".to_owned(), Design::concave_fixture()));
    designs
}

#[test]
fn measure_deltas_of_the_unchanged_design_is_zero_everywhere() {
    let mut measured = Vec::new();
    for (name, design) in census_designs() {
        let Ok(baseline) = MeetPreservationBaseline::capture(&design) else {
            continue;
        };
        let deltas = baseline
            .measure_deltas(&design)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(deltas.d_aspect.abs() < 1e-12, "{name}: {deltas:?}");
        for delta in [
            deltas.d_table_pts,
            deltas.d_crown,
            deltas.d_pavilion,
            deltas.d_girdle,
        ]
        .into_iter()
        .flatten()
        {
            assert!(delta.abs() < 1e-12, "{name}: {deltas:?}");
        }
        assert!(deltas.junction_disp.abs() < 1e-12, "{name}: {deltas:?}");
        assert_eq!(deltas.travel_width.to_bits(), 0.0_f64.to_bits(), "{name}");
        assert_eq!(deltas.travel_scale.to_bits(), 0.0_f64.to_bits(), "{name}");
        measured.push(name);
    }
    // A template that does not capture is a census row, not a failure here; these three must.
    for required in ["Standard Round Brilliant", "octahedron", "concave fixture"] {
        assert!(
            measured.iter().any(|name| name == required),
            "{required} did not capture: {measured:?}"
        );
    }
}

/// A ten-degree turn about the girdle outline moves the top apex by more than the constant
/// Retarget bound. The constant policy alone refuses it; the census policy measures it, and the
/// excess over the travel the gate would grant is the repair left after the turn (about zero
/// for a pure hinge turn).
#[test]
fn the_census_policy_accepts_what_the_constant_retarget_refuses_on_a_ten_degree_turn() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).unwrap();
    let turned = crown_turned(&design, 10.0);
    let constant = PreservationPolicy {
        bounds_follow_turn: false,
        ..PreservationPolicy::RETARGET
    };
    assert!(matches!(
        baseline.validate(&turned, constant),
        Err(PreservationViolation::ShapeLimit)
    ));
    baseline
        .validate(&turned, PreservationPolicy::CENSUS)
        .expect("the census limits admit the turned stone");
    let deltas = baseline.measure_deltas(&turned).unwrap();
    assert!(
        deltas.junction_disp > PreservationPolicy::RETARGET.max_junction_displacement,
        "{deltas:?}"
    );
    assert!((deltas.junction_disp - 0.214).abs() < 0.005, "{deltas:?}");
    assert!((deltas.travel_scale - 0.214).abs() < 0.005, "{deltas:?}");
    assert!(deltas.excess_junction().abs() < 0.01, "{deltas:?}");
    // The depth is measured in width units and the travel in the same units.
    if let Some(excess) = deltas.excess_depth() {
        assert!(excess.is_finite(), "{deltas:?}");
    }
}

#[test]
fn measure_deltas_refuses_a_split_apex_with_the_gates_violation() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).unwrap();
    let mut split = design;
    split.tiers[0].angle_deg += 1.0;
    let error = baseline.measure_deltas(&split).unwrap_err();
    assert!(
        ["detached_junction", "lost_facet", "collapsed_edge"].contains(&error.kind()),
        "{error}"
    );
}

#[test]
fn the_octahedron_baseline_reports_its_structure() {
    let baseline = MeetPreservationBaseline::capture(&octahedron()).unwrap();
    let structure = baseline.structure();
    assert_eq!(structure.junctions, 6);
    assert_eq!(structure.live_facets, 8);
    assert_eq!(structure.edges, 12);
    assert_eq!(structure.faces, 8);
    assert!(structure.reach_fallbacks_possible <= 8);
    // An exact design misses its own meets by rounding only.
    assert!(baseline.max_slack() <= 2e-6, "{}", baseline.max_slack());
    assert!((baseline.identify_radius() - 5e-5).abs() <= 1e-18);
    assert!(baseline.width() > 0.0);
    assert!(baseline.shape().aspect.is_finite());
}

#[test]
fn kinds_are_stable_strings() {
    let every = [
        (PreservationViolation::Cancelled, "cancelled"),
        (
            PreservationViolation::AtSurfaces {
                detail: "x".into(),
                surfaces: Vec::new(),
            },
            "at_surfaces",
        ),
        (
            PreservationViolation::InvalidBaseline(String::new()),
            "invalid_baseline",
        ),
        (PreservationViolation::Relation(String::new()), "relation"),
        (
            PreservationViolation::CandidateTopology(String::new()),
            "candidate_topology",
        ),
        (PreservationViolation::StaleCandidate, "stale_candidate"),
        (
            PreservationViolation::DetachedJunction(0),
            "detached_junction",
        ),
        (PreservationViolation::LostFacet(0), "lost_facet"),
        (PreservationViolation::CollapsedEdge(0), "collapsed_edge"),
        (PreservationViolation::ShapeLimit, "shape_limit"),
        (
            PreservationViolation::GirdleGauge {
                reason: String::new(),
            },
            "girdle_gauge",
        ),
        (PreservationViolation::Unrepresentable, "unrepresentable"),
        (
            PreservationViolation::ProjectionFailed {
                stage: String::new(),
                reason: String::new(),
            },
            "projection_failed",
        ),
    ];
    let mut seen = Vec::new();
    for (violation, expected) in &every {
        assert_eq!(violation.kind(), *expected);
        assert!(!seen.contains(expected), "{expected} repeats");
        seen.push(*expected);
    }
    // The surface-annotated forms keep the kind of the failure they annotate.
    for (detail, expected) in [
        ("Original junction #1 would separate.", "detached_junction"),
        ("Original facet #1 would disappear.", "lost_facet"),
        (
            "Original edge #1 would collapse or disconnect.",
            "collapsed_edge",
        ),
    ] {
        let annotated = PreservationViolation::AtSurfaces {
            detail: detail.into(),
            surfaces: Vec::new(),
        };
        assert_eq!(annotated.kind(), expected);
    }
}
