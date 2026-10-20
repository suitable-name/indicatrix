//! The gate grants a retarget the travel its REQUESTED facet turn implies
//! (`PreservationPolicy::bounds_follow_turn`), and nothing more.
//!
//! Owner run 3 (2026-10-19) left the diamond-to-sapphire retarget of the standard brilliant
//! red with `ShapeLimit`: its anchored seed kept every original meet, the girdle gauge and
//! the corner band, and `RETARGET` bounded the pavilion depth and the culet's travel by a
//! constant 0.08 while a ten-degree turn about the outline moves them by 0.17 of the stone.
//! The owner's tightened rule (same day): the travel is the largest per-tier product of a
//! turned tier's inward reach from the outline and its tangent change; the tangent change is
//! read from the requested angles (the projection's seed), so a repair cannot enlarge its
//! own allowance; only tiers the request turned count; and a tier steeper than 75 degrees
//! has its tangent capped there.

use super::*;
use crate::{
    meet_preservation::{
        MeetPreservationBaseline, PreservationPolicy, PreservationViolation, TURN_TANGENT_CAP_DEG,
        tangent_changes,
    },
    optimize::project_preserved_design,
};
use indicatrix::geometry::meet_solver::SolveControl;

/// The standard brilliant in the editor's `block(2, 1, 4)` rough.
fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// `design` with the tier called `name` at `angle_deg` (its mast untouched).
fn with_angle(design: &Design, name: &str, angle_deg: f64) -> Design {
    let mut turned = design.clone();
    turned
        .tiers
        .iter_mut()
        .find(|tier| tier.name == name)
        .expect("the tier exists")
        .angle_deg = angle_deg;
    turned
}

fn tier_named(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .expect("the tier exists")
}

fn tangent(angle_deg: f64) -> f64 {
    angle_deg.to_radians().tan()
}

/// Regression for the diamond-to-sapphire retarget of the standard brilliant (owner run 3,
/// 2026-10-19): its anchored seed kept every original meet, the girdle gauge and the
/// corner band and was still refused as a shape limit, because `RETARGET` bounded the
/// pavilion depth and the culet's travel by a constant 0.08 while a ten-degree turn about
/// the outline moves them by 0.17 of the stone. A turn about the outline is granted the
/// travel it implies; the constant alone refuses it, and nothing else is relaxed.
#[test]
fn a_turn_about_the_girdle_outline_is_granted_the_travel_it_implies() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).unwrap();
    let turned = crown_turned(&design, 10.0);
    let facts = baseline
        .validate(&turned, PreservationPolicy::RETARGET)
        .expect("a turn about the outline keeps every meet and is a legitimate retarget");
    assert_eq!(facts.original_meets, 6);
    // tan 55 - tan 45 = 0.428, and a crown facet reaches from the outline (its girdle edge,
    // half the width from the axis) to the apex: the apex travels 0.214 of the stone, far
    // beyond the constant bound, which refuses the same stone as a shape limit. The gate
    // reads that reach from the original stone and grants 0.214 on top of the constant.
    assert!(
        facts.shape_distance > PreservationPolicy::RETARGET.max_junction_displacement,
        "{}",
        facts.shape_distance
    );
    assert!(
        (facts.shape_distance - 0.214).abs() < 0.005,
        "{}",
        facts.shape_distance
    );
    let constant = PreservationPolicy {
        bounds_follow_turn: false,
        ..PreservationPolicy::RETARGET
    };
    assert!(matches!(
        baseline.validate(&turned, constant),
        Err(PreservationViolation::ShapeLimit)
    ));
    // The allowance is the turn's own travel, not a free pass: a split apex is still a
    // detached meet, and the whole stone grown still moves the gauge.
    let mut split = turned.clone();
    split.tiers[0].angle_deg += 1.0;
    assert!(
        baseline
            .validate(&split, PreservationPolicy::RETARGET)
            .is_err()
    );
    let mut grown = turned;
    for tier in &mut grown.tiers {
        if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
            *mast *= 1.03;
        }
    }
    let error = baseline
        .validate(&grown, PreservationPolicy::RETARGET)
        .unwrap_err();
    assert!(error.to_string().contains("girdle radius"), "{error}");
}

#[test]
fn the_turn_allowance_is_the_travel_and_optimize_grants_none() {
    let turned = PreservationPolicy::RETARGET.for_turn(0.2, 0.1);
    assert!((turned.max_depth_change - 0.28).abs() < 1e-12);
    assert!((turned.max_girdle_change - 0.22).abs() < 1e-12);
    assert!((turned.max_junction_displacement - 0.18).abs() < 1e-12);
    assert_eq!(
        PreservationPolicy {
            max_depth_change: PreservationPolicy::RETARGET.max_depth_change,
            max_girdle_change: PreservationPolicy::RETARGET.max_girdle_change,
            max_junction_displacement: PreservationPolicy::RETARGET.max_junction_displacement,
            ..turned
        },
        PreservationPolicy::RETARGET,
        "the table, the aspect and the gauge lock are not relaxed"
    );
    assert_eq!(
        PreservationPolicy::OPTIMIZE.for_turn(0.2, 0.1),
        PreservationPolicy::OPTIMIZE
    );
    // The bound is never tighter than the constant: a travel that is not a finite positive
    // number grants nothing and takes nothing away.
    for travel in [0.0, -0.1, f64::NAN, f64::INFINITY] {
        assert_eq!(
            PreservationPolicy::RETARGET.for_turn(travel, 0.05),
            PreservationPolicy::RETARGET,
            "travel {travel}"
        );
        assert_eq!(
            PreservationPolicy::RETARGET.for_turn(0.05, travel),
            PreservationPolicy::RETARGET,
            "travel {travel}"
        );
    }
}

#[test]
fn the_tangent_changes_read_the_turned_slopes_and_ignore_flats_walls_and_fixed_tiers() {
    let design = brilliant();
    let still = tangent_changes(&design, &design);
    assert_eq!(still.len(), design.tiers.len());
    assert!(still.iter().all(|change| change.abs() < 1e-15));
    let mut turned = design.clone();
    for tier in &mut turned.tiers {
        match tier.name.as_str() {
            "Pavilion Main" => tier.angle_deg = -51.0,
            "Star" => tier.angle_deg = 18.3,
            // A wall and the flats: their slopes are no travel.
            "Girdle" => tier.angle_deg = 89.9,
            "Table" => tier.constraint = MeetConstraint::ScaleReference(0.36),
            "Culet" => tier.constraint = MeetConstraint::ScaleReference(1.25),
            _ => {}
        }
    }
    let turn = tangent_changes(&design, &turned);
    for (index, tier) in design.tiers.iter().enumerate() {
        let expected = match tier.name.as_str() {
            "Pavilion Main" => tangent(51.0) - tangent(41.0),
            "Star" => tangent(18.3) - tangent(15.0),
            // The crown main and the breaks are not turned; the wall and the flats have no
            // slope to travel by.
            _ => 0.0,
        };
        assert!(
            (turn[index] - expected).abs() < 1e-12,
            "{}: {} vs {expected}",
            tier.name,
            turn[index]
        );
    }
}

#[test]
fn a_relation_followers_derived_angle_counts_as_its_own_turn() {
    // C1 follows C0: when C0 turns ten degrees C1's facet turns with it and carries its own
    // points, so it counts with its own tangent change (owner, 2026-10-19: "each tier's own
    // reach x its own slope change"); the per-tier maximum never adds it to C0's.
    let mut design = octahedron();
    design.ensure_tier_ids();
    let relation = design.parse_relation("C0 + 0").expect("the relation reads");
    design.tier_relations.insert(design.tier_ids[1], relation);
    assert!(design.is_tier_driven(1));
    let requested = crown_turned(&design, 10.0);
    let turn = tangent_changes(&design, &requested);
    let expected = tangent(55.0) - tangent(45.0);
    assert!((turn[0] - expected).abs() < 1e-12, "{}", turn[0]);
    assert!(
        (turn[1] - expected).abs() < 1e-12,
        "the follower turns with its driver: {}",
        turn[1]
    );
    assert!((turn[2] - expected).abs() < 1e-12, "{}", turn[2]);
    assert!(
        turn[4].abs() < 1e-15,
        "the pavilion did not turn: {}",
        turn[4]
    );
}

#[test]
fn a_steep_tiers_contribution_is_capped_at_75_degrees() {
    let design = brilliant();
    let girdle = tier_named(&design, "Girdle");
    // From 70 to 80 degrees the tangent grows by 2.92; capped at 75 it grows by 0.98.
    let seventy = with_angle(&design, "Girdle", 70.0);
    let eighty = with_angle(&design, "Girdle", 80.0);
    let capped = tangent_changes(&seventy, &eighty)[girdle];
    let expected = tangent(TURN_TANGENT_CAP_DEG) - tangent(70.0);
    assert!((capped - expected).abs() < 1e-12, "{capped} vs {expected}");
    assert!(capped < tangent(80.0) - tangent(70.0));
    // A tier that starts beyond the cap contributes nothing however far it turns.
    let steeper = with_angle(&design, "Girdle", 85.0);
    let beyond = tangent_changes(&eighty, &steeper)[girdle];
    assert!(beyond.abs() < 1e-15, "{beyond}");
    // Below the cap nothing changes: the pavilion shift reads its full tangent change.
    let main = tier_named(&design, "Pavilion Main");
    let shift = tangent_changes(&design, &with_angle(&design, "Pavilion Main", -51.0))[main];
    let full = tangent(51.0) - tangent(41.0);
    assert!((shift - full).abs() < 1e-12, "{shift} vs {full}");
}

#[test]
fn an_unturned_tier_contributes_nothing_and_each_tier_counts_with_its_own_reach() {
    let design = brilliant();
    let baseline = MeetPreservationBaseline::capture(&design).expect("the brilliant captures");
    let still = baseline.turn_travel(&tangent_changes(&design, &design));
    assert!(still.abs() < 1e-15, "{still}");
    // The pavilion main reaches from the outline (the wall, 1.0 from the axis) to the culet
    // (0.009 from it): the ten-degree shift carries its culet end by 0.99 (tan 51 - tan 41).
    let main_shift = tangent(51.0) - tangent(41.0);
    let main_only = with_angle(&design, "Pavilion Main", -51.0);
    let main = baseline.turn_travel(&tangent_changes(&design, &main_only));
    assert!(
        0.991_f64.mul_add(-main_shift, main).abs() < 0.01 * main_shift,
        "{main}"
    );
    // The star reaches from the outline to the table's edge, under half as far. Only its own
    // reach counts for its own turn: the untouched pavilion main contributes nothing, where
    // "the longest reach times the largest tangent change" would have counted it.
    let star_shift = tangent(18.3) - tangent(15.0);
    let star_only = with_angle(&design, "Star", 18.3);
    let star = baseline.turn_travel(&tangent_changes(&design, &star_only));
    assert!(star > 0.0);
    assert!(star < 0.6 * 0.991 * star_shift, "star {star}");
    assert!(star < 0.25 * main, "star {star} vs main {main}");
    // Both turned: the travel is the largest product, not a sum.
    let both = with_angle(&main_only, "Star", 18.3);
    let both_travel = baseline.turn_travel(&tangent_changes(&design, &both));
    assert!(
        (both_travel - main).abs() < 1e-12,
        "{both_travel} vs {main}"
    );
}

#[test]
fn the_projections_repair_cannot_enlarge_the_allowance() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).unwrap();
    let control = SolveControl::default();
    // The request turns the crown two degrees; the candidate stands at ten, as a repair
    // that turned further would.
    let request = tangent_changes(&design, &crown_turned(&design, 2.0));
    let candidate = crown_turned(&design, 10.0);
    // Judged by the request's turn (0.036 of the stone granted), the candidate's 0.214 apex
    // travel is a shape limit...
    let by_request = baseline
        .assess(
            &candidate,
            PreservationPolicy::RETARGET,
            &control,
            Some(&request),
        )
        .verdict;
    assert!(
        matches!(by_request, Err(PreservationViolation::ShapeLimit)),
        "{by_request:?}"
    );
    // ...while judged by its own angles it is the legitimate ten-degree turn, and a request
    // that asks for the full turn grants it. (The grant is the smaller of the request's
    // travel and the candidate's own, so a request can never buy more than the turn the
    // candidate really made.)
    baseline
        .assess(&candidate, PreservationPolicy::RETARGET, &control, None)
        .verdict
        .expect("a finished state is judged by its own turn");
    let full = tangent_changes(&design, &candidate);
    baseline
        .assess(
            &candidate,
            PreservationPolicy::RETARGET,
            &control,
            Some(&full),
        )
        .verdict
        .expect("the requested turn is granted");
}

/// The projection itself judges every assessment by the turn it is given
/// (`OptimizeOptions::turn_request`: a Retarget's anchored candidate, which also covers the
/// refit projected after it) and, without one, by its seed's own turn. A feasible state
/// turned further than the request is refused however it came about: its repair starts from
/// a zero residual, so nothing it can do earns the difference.
#[test]
fn the_projection_judges_by_the_request_it_is_given() {
    let design = octahedron();
    let solved = design.solve().expect("the octahedron solves");
    let baseline = std::sync::Arc::new(MeetPreservationBaseline::capture(&design).unwrap());
    let options = |turn_request: Option<&[f64]>| crate::OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: crate::design::tier_hinge_points(&design, &solved),
        preservation_baseline: Some(baseline.clone()),
        preservation_policy: PreservationPolicy::RETARGET,
        turn_request: turn_request.map(std::sync::Arc::from),
        ..Default::default()
    };
    // The ten-degree turn about the outline keeps every meet: judged by its own turn (no
    // request, so the seed's) it is accepted as it is, bit for bit.
    let turned = crown_turned(&design, 10.0);
    let accepted = project_preserved_design(&design, &turned, &options(None), None)
        .expect("a feasible seed is accepted at its first assessment");
    assert_eq!(accepted, turned);
    // The same state handed over as the result of a two-degree request: the request grants
    // 0.036 of the stone, the apex travels 0.214, and the projection is refused for the shape.
    let request = tangent_changes(&design, &crown_turned(&design, 2.0));
    let error =
        project_preserved_design(&design, &turned, &options(Some(request.as_slice())), None)
            .expect_err("a state turned further than its request is not granted the difference");
    assert!(error.to_string().contains("permitted change"), "{error}");
    // A request that asks for the full turn grants it.
    let full = tangent_changes(&design, &turned);
    let granted = project_preserved_design(&design, &turned, &options(Some(full.as_slice())), None)
        .expect("the requested turn is granted");
    assert_eq!(granted, turned);
}

#[test]
fn a_tier_whose_reach_cannot_be_read_is_granted_half_the_length() {
    // A ghost tier, pinned five units out, never touches the stone, so its reach cannot be
    // read; a request that turns it is granted half the stone's length, which for the
    // octahedron is exactly a crown facet's reach (its girdle edge to the apex).
    let mut design = octahedron();
    let mut ghost = design.tiers[0].clone();
    ghost.name = "Ghost".into();
    ghost.indices = vec![12.0];
    ghost.constraint = MeetConstraint::ScaleReference(5.0);
    design.tiers.push(ghost);
    design.ensure_tier_ids();
    let index = design.tiers.len() - 1;
    let baseline =
        MeetPreservationBaseline::capture(&design).expect("a ghost plane does not break the stone");
    let ghost_turned = with_angle(&design, "Ghost", 55.0);
    let ghost_travel = baseline.turn_travel(&tangent_changes(&design, &ghost_turned));
    let mut crown = crown_turned(&design, 10.0);
    crown.tiers[index] = design.tiers[index].clone();
    let crown_travel = baseline.turn_travel(&tangent_changes(&design, &crown));
    assert!(crown_travel > 0.0);
    // The width comes from the display mesh and the reach from the arrangement's junctions:
    // the same planes, so they agree far inside this tolerance (the alternatives, nothing or
    // the whole length, are 0.2 away).
    assert!(
        (ghost_travel - crown_travel).abs() < 1e-6,
        "{ghost_travel} vs {crown_travel}"
    );
    // Run time says so too: the gate counts every turned tier it granted the fallback to,
    // and nothing for a tier whose reach it read.
    assert_eq!(baseline.diagnostics().turn_reach_fallbacks, 0);
    // A policy that does not follow the turn is granted nothing, so the same turn counts no
    // fallback under it (the verdict is not the question here, the counter is).
    let _ = baseline.validate(&ghost_turned, PreservationPolicy::OPTIMIZE);
    assert_eq!(
        baseline.diagnostics().turn_reach_fallbacks,
        0,
        "OPTIMIZE grants no travel, so it used no fallback"
    );
    baseline
        .validate(&ghost_turned, PreservationPolicy::RETARGET)
        .expect("a ghost's turn changes nothing on the stone");
    assert_eq!(baseline.diagnostics().turn_reach_fallbacks, 1);
    baseline
        .validate(&crown, PreservationPolicy::RETARGET)
        .expect("the crown's turn is granted its travel");
    assert_eq!(
        baseline.diagnostics().turn_reach_fallbacks,
        1,
        "the crown facets' reaches were read, so they are not fallbacks"
    );
}
