//! Tests for the three-phase pipeline: phase-1 rank-1/named-meet selection,
//! determinism, cancellation/error contracts, and progress reporting.

use glam::DVec3;

use super::{
    super::{
        MAX_PLANES, MeetConstraint, MeetTierInput, NonFiniteField, SolveControl, SolveError,
        SolvePhase, SolveProgress, SolveStrategy,
    },
    solve_meet_points, solve_meet_points_with,
};

/// Hand-verifiable case: a square girdle wall plus flat table/culet (all three
/// given as scale references) fully cap a box `[-1,1] x [-0.6,0.6] x [-1,1]`. A
/// fourth facet at 45 degrees, azimuth 45 degrees (pointed straight at one of
/// the box's corner edges) must land on a *vertex level* of the box's corner
/// arrangement. Under the rank-1 prior that is the second-highest corner value
/// in its normal direction: for unit normal `n = (0.5, 1/sqrt2, 0.5)` the box
/// corners give values `+-0.5 +- 0.6/sqrt2 +- 0.5`, whose distinct levels are
/// `1 + 0.6/sqrt2` (first touch), then `1 - 0.6/sqrt2` (the rank-1 level this
/// solver must select), then lower ones.
#[test]
fn selects_the_rank1_vertex_level_against_a_capped_box() {
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: 0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: -0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
        },
    ];

    let solved = solve_meet_points(gear, &tiers);
    assert_eq!(solved.len(), 4);
    for s in &solved[..3] {
        assert_eq!(s.strategy, SolveStrategy::ScaleReference);
    }
    assert_eq!(
        solved[3].strategy,
        SolveStrategy::DependencyOrder,
        "detail: {}",
        solved[3].detail
    );
    let expected = std::f64::consts::FRAC_1_SQRT_2.mul_add(-0.6, 1.0);
    assert!(
        (solved[3].mast - expected).abs() < 1e-6,
        "expected the rank-1 vertex level {expected}, solver produced {} (detail: {})",
        solved[3].mast,
        solved[3].detail
    );
}

/// A stated `"Meet <names>"` reference must override the rank-1 prior: the tier
/// must land exactly on the vertex where its named references meet, even when
/// that is not the rank-1 level of its own arrangement.
#[test]
fn a_stated_named_meet_overrides_the_rank1_prior() {
    // Box as above, plus Y (a 60-degree facet solved by rank-1 first), plus X,
    // which names Y + girdle + table: X must pass through the vertex where those
    // three planes meet.
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec!["girdle".to_string()],
        },
        MeetTierInput {
            angle_deg: 0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec!["table".to_string()],
        },
        MeetTierInput {
            angle_deg: -0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec!["culet".to_string()],
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetNamed(vec![
                "Y".to_string(),
                "girdle".to_string(),
                "table".to_string(),
            ]),
            names: vec!["X".to_string()],
        },
        MeetTierInput {
            angle_deg: 60.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec!["Y".to_string()],
        },
    ];

    let solved = solve_meet_points(gear, &tiers);
    assert_eq!(solved.len(), 5);
    let x = &solved[3];
    assert_eq!(
        x.strategy,
        SolveStrategy::DependencyOrder,
        "X should have solved exactly, got {:?}: {}",
        x.strategy,
        x.detail
    );
    assert!(
        x.detail.contains("named reference"),
        "X should have used its stated named references, detail: {}",
        x.detail
    );

    // Verify the incidence directly: X's plane must pass through a point that
    // also lies on Y's plane, a girdle plane, and the table plane.
    let y = &solved[4];
    let theta_x = 45.0_f64.to_radians();
    let theta_y = 60.0_f64.to_radians();
    let phi = std::f64::consts::FRAC_PI_4; // index 0.5 on a 4-tooth gear
    let n_x = DVec3::new(
        theta_x.sin() * phi.cos(),
        theta_x.cos(),
        theta_x.sin() * phi.sin(),
    );
    let n_y = DVec3::new(
        theta_y.sin() * phi.cos(),
        theta_y.cos(),
        theta_y.sin() * phi.sin(),
    );
    // Vertex of {Y, girdle at azimuth 0 (n = +x), table (n = +y)}:
    let m = glam::DMat3::from_cols(n_y, DVec3::X, DVec3::Y).transpose();
    let v = m.inverse() * DVec3::new(y.mast, 1.0, 0.6);
    assert!(
        (n_x.dot(v) - x.mast).abs() < 1e-6,
        "X's mast {} should equal n_x . v = {} at the named meet vertex",
        x.mast,
        n_x.dot(v)
    );
}

/// Two identical calls must produce byte-identical results (the old solver's
/// convex-hull library was seeded per-process and made the whole pipeline
/// nondeterministic; this pins the fix).
#[test]
fn solving_is_deterministic() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 96 0.0\n\
         y 6 y\n\
         I 1.72\n\
         H PC 45.149  Round Trichecker-12\n\
         a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
         a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
         a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
         a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
         a 10.000000 0.48799664 96 n C 16 32 48 64 80\n",
    )
    .expect("must parse");
    let mut tiers = super::super::meet_tier_inputs_from_asc(&schedule);
    // No stated scale references in this design: anchor one tier per block the
    // same way the corpus harness does (pavilion tier 0, crown tier 2, and the
    // girdle tier 1).
    tiers[0].constraint = MeetConstraint::ScaleReference(schedule.tiers[0].mast);
    tiers[1].constraint = MeetConstraint::ScaleReference(schedule.tiers[1].mast);
    tiers[2].constraint = MeetConstraint::ScaleReference(schedule.tiers[2].mast);

    let a = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
    let b = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert!(
            x.mast.to_bits() == y.mast.to_bits(),
            "nondeterministic mast"
        );
        assert_eq!(x.strategy, y.strategy);
        assert_eq!(x.detail, y.detail);
    }
}

/// The named-meet fixture above, run through [`solve_meet_points_with`]
/// with a plain [`SolveControl::default`], must reproduce
/// [`solve_meet_points`] bit for bit -- an unused control must change
/// nothing about the result.
#[test]
fn solve_meet_points_with_a_default_control_matches_solve_meet_points_bitwise() {
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: 0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: -0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
        },
    ];
    let plain = solve_meet_points(gear, &tiers);
    let via_with = solve_meet_points_with(gear, &tiers, &SolveControl::default())
        .expect("a default control never cancels and this fixture is under MAX_PLANES");
    assert_eq!(plain.len(), via_with.len());
    for (a, b) in plain.iter().zip(&via_with) {
        assert_eq!(a.mast.to_bits(), b.mast.to_bits());
        assert_eq!(a.strategy, b.strategy);
        assert_eq!(a.detail, b.detail);
    }
}

/// A control whose cancel flag is already set before the solve starts
/// must return `Err(SolveCancelled)` (i.e. [`SolveError::Cancelled`])
/// immediately, without needing a slow fixture or a background thread --
/// the deterministic half of the cancellation contract. The
/// wall-clock-latency half (cancelling mid-solve, on the real 103-tier
/// fixture) is proven at the `indicatrix-cut-core` level, where that
/// fixture lives -- see `design::tests::cancel_stops_a_large_real_solve_quickly`.
#[test]
fn solve_meet_points_with_a_precancelled_control_returns_cancelled() {
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
        },
    ];
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let control = SolveControl::with_cancel(&cancel);
    let result = solve_meet_points_with(gear, &tiers, &control);
    assert!(matches!(result, Err(SolveError::Cancelled)), "{result:?}");
}

/// Above [`MAX_PLANES`], `solve_meet_points_with` must return a distinct
/// [`SolveError::TooManyPlanes`] naming the actual plane count and the
/// cap, while the legacy `solve_meet_points` keeps its old silent
/// behavior (every tier `SolveStrategy::Failed`, except the anchor,
/// which keeps its given mast) -- the two must never diverge on what
/// counts as "too many planes".
#[test]
fn solve_meet_points_with_reports_too_many_planes_above_the_cap() {
    let gear = 400;
    // One tier with 401 index instances -- one plane instance over
    // MAX_PLANES all by itself (`total_planes = 6 blank + 401`).
    let indices: Vec<f64> = (0..401).map(f64::from).collect();
    let tiers = vec![MeetTierInput {
        angle_deg: 90.0,
        indices,
        constraint: MeetConstraint::ScaleReference(1.0),
        names: vec![],
    }];

    let via_with = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match via_with {
        Err(SolveError::TooManyPlanes { planes, max }) => {
            assert_eq!(max, MAX_PLANES);
            assert!(planes > MAX_PLANES, "planes: {planes}");
        }
        other => panic!("expected SolveError::TooManyPlanes, got {other:?}"),
    }

    let legacy = solve_meet_points(gear, &tiers);
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].strategy, SolveStrategy::ScaleReference);
    assert!((legacy[0].mast - 1.0).abs() < 1e-12);
}

/// A NaN `ScaleReference` mast must be rejected up front, never
/// silently folded away by the scale-prior/domination-limit computation and
/// never reach candidate-vertex geometry as a NaN plane offset.
#[test]
fn solve_meet_points_with_rejects_a_nan_mast() {
    let gear = 96;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 24.0, 48.0, 72.0],
            constraint: MeetConstraint::ScaleReference(f64::NAN),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: 41.0,
            indices: vec![12.0, 36.0, 60.0, 84.0],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
        },
    ];
    let result = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match result {
        Err(SolveError::NonFiniteInput { tier, field }) => {
            assert_eq!(tier, 0);
            assert_eq!(field, NonFiniteField::Mast);
        }
        other => panic!(
            "expected Err(SolveError::NonFiniteInput), got {other:?} (never a solved/Closed result)"
        ),
    }
}

/// An infinite index must be rejected the same way a NaN mast is --
/// checked before `SolveContext::new` derives anything from it.
#[test]
fn solve_meet_points_with_rejects_an_infinite_index() {
    let gear = 96;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 24.0, 48.0, f64::INFINITY],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
        },
        MeetTierInput {
            angle_deg: 41.0,
            indices: vec![12.0, 36.0, 60.0, 84.0],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
        },
    ];
    let result = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match result {
        Err(SolveError::NonFiniteInput { tier, field }) => {
            assert_eq!(tier, 0);
            assert_eq!(field, NonFiniteField::Index);
        }
        other => panic!(
            "expected Err(SolveError::NonFiniteInput), got {other:?} (never a solved/Closed result)"
        ),
    }
}

/// A non-finite `angle_deg` is rejected the same way.
#[test]
fn solve_meet_points_with_rejects_a_nan_angle() {
    let gear = 96;
    let tiers = vec![MeetTierInput {
        angle_deg: f64::NAN,
        indices: vec![0.0],
        constraint: MeetConstraint::ScaleReference(1.0),
        names: vec![],
    }];
    let result = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match result {
        Err(SolveError::NonFiniteInput { tier, field }) => {
            assert_eq!(tier, 0);
            assert_eq!(field, NonFiniteField::AngleDeg);
        }
        other => panic!("expected Err(SolveError::NonFiniteInput), got {other:?}"),
    }
}

/// The progress callback must report [`SolvePhase::Constructive`] before
/// [`SolvePhase::LeastSquares`] before [`SolvePhase::Refine`] (the fixed
/// phase order the module docs describe), `sweep` must never decrease
/// within one phase's own run of reports, and every phase must reach
/// `blocks_done == blocks_total` at least once (each phase fully
/// processes every tier, or reports zero-work-to-do, before moving on).
#[test]
fn solve_meet_points_with_reports_monotonic_progress_reaching_every_phases_total() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 96 0.0\n\
         y 6 y\n\
         I 1.72\n\
         H PC 45.149  Round Trichecker-12\n\
         a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
         a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
         a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
         a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
         a 10.000000 0.48799664 96 n C 16 32 48 64 80\n",
    )
    .expect("must parse");
    let mut tiers = super::super::meet_tier_inputs_from_asc(&schedule);
    tiers[0].constraint = MeetConstraint::ScaleReference(schedule.tiers[0].mast);
    tiers[1].constraint = MeetConstraint::ScaleReference(schedule.tiers[1].mast);
    tiers[2].constraint = MeetConstraint::ScaleReference(schedule.tiers[2].mast);

    let reports: std::cell::RefCell<Vec<SolveProgress>> = std::cell::RefCell::new(Vec::new());
    let record = |p: SolveProgress| reports.borrow_mut().push(p);
    let control = SolveControl::default().reporting(&record);
    let _ = solve_meet_points_with(schedule.gear_teeth_abs(), &tiers, &control)
        .expect("under MAX_PLANES, never cancelled");

    let reports = reports.into_inner();
    assert!(!reports.is_empty(), "no progress reported at all");

    // Fixed phase order, no phase revisited once left.
    let phase_rank = |p: SolvePhase| match p {
        SolvePhase::Constructive => 0,
        SolvePhase::LeastSquares => 1,
        SolvePhase::Refine => 2,
    };
    let mut last_rank = 0;
    let mut last_sweep_in_phase = 0u32;
    let mut reached_total: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for r in &reports {
        let rank = phase_rank(r.phase);
        assert!(
            rank >= last_rank,
            "phase went backwards: {:?} after rank {last_rank}",
            r.phase
        );
        if rank != last_rank {
            last_sweep_in_phase = 0;
        }
        assert!(
            r.sweep >= last_sweep_in_phase,
            "sweep decreased within {:?}: {} after {last_sweep_in_phase}",
            r.phase,
            r.sweep
        );
        last_sweep_in_phase = r.sweep;
        last_rank = rank;
        assert!(
            r.blocks_done <= r.blocks_total,
            "blocks_done exceeded blocks_total: {r:?}"
        );
        assert_eq!(r.blocks_total, tiers.len() as u32);
        if r.blocks_done == r.blocks_total {
            reached_total.insert(rank);
        }
    }
    assert!(
        reached_total.contains(&0),
        "Constructive phase never reported blocks_done == blocks_total: {reports:?}"
    );
    assert!(
        reached_total.contains(&2),
        "Refine phase never reported blocks_done == blocks_total: {reports:?}"
    );
}

/// Solved masts must scale exactly proportionally with a design's
/// absolute `ScaleReference` anchors, and every tier must settle on the same
/// [`SolveStrategy`] regardless of the design's absolute scale. Before
/// `SolveContext::scale_norm`'s normalisation, this RBC-shaped design (masts
/// of order `k`) diverged at k=70 (a crown tier's `mast/k` differed from the
/// k=1 baseline) and every meet-derived tier fell to
/// [`SolveStrategy::LeastSquaresFallback`] at k=100 -- because
/// [`super::super::EPS_FEAS`]/[`super::super::EPS_INCIDENT`]/[`super::super::LEVEL_TOL`]/
/// [`super::super::BLANK_HALF_EXTENT`] are absolute constants tuned for masts
/// of order 1. Regression coverage for that measured divergence.
#[test]
fn solve_meet_points_scales_masts_proportionally_with_scale_reference() {
    const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

    let rbc = |k: f64| -> Vec<MeetTierInput> {
        let t = |angle_deg: f64, indices: &[f64], constraint: MeetConstraint| MeetTierInput {
            angle_deg,
            indices: indices.to_vec(),
            constraint,
            names: vec![],
        };
        vec![
            t(0.0, &[], MeetConstraint::ScaleReference(0.32 * k)),
            t(15.0, &STAR, MeetConstraint::MeetExisting),
            t(34.5, &MAIN, MeetConstraint::MeetExisting),
            t(41.0, &BREAK, MeetConstraint::MeetExisting),
            t(90.0, &GIRDLE, MeetConstraint::ScaleReference(1.0 * k)),
            t(-41.0, &MAIN, MeetConstraint::MeetExisting),
            t(-42.5, &BREAK, MeetConstraint::MeetExisting),
            t(-0.0, &[], MeetConstraint::ScaleReference(0.88 * k)),
        ]
    };

    let gear = 96;
    let baseline = solve_meet_points(gear, &rbc(1.0));
    for &k in &[0.001, 0.01, 0.1, 10.0, 20.0, 40.0, 70.0, 100.0] {
        let solved = solve_meet_points(gear, &rbc(k));
        assert_eq!(solved.len(), baseline.len());
        for (i, (s, b)) in solved.iter().zip(&baseline).enumerate() {
            assert_eq!(
                s.strategy, b.strategy,
                "tier {i}: strategy changed at k={k} ({:?} vs baseline {:?})",
                s.strategy, b.strategy
            );
            let rel = (s.mast / k - b.mast).abs() / b.mast.abs().max(1e-9);
            assert!(
                rel < 1e-6,
                "tier {i}: mast/k {} vs baseline {} at k={k} (rel {rel})",
                s.mast / k,
                b.mast
            );
        }
    }
}
