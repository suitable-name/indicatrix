//! Tests for the girdle/table guard ([`super::super::guard`]) and for the bounded
//! candidate pool ([`super::super::pool`]).

use super::{
    super::{
        CANONICAL_LIGHTING_PRESET, ObjectiveWeights,
        candidate::{self, BaselineWarningCounts, CandidateOutcome},
        guard::{self, ShapeGuard},
        pool::{CandidatePool, PoolEntry},
    },
    fixtures::imported_rbc,
};
use crate::{
    design::{
        ConstraintTier, Design, GirdleBand, KNIFE_EDGE_GAP, ScheduleMeta, girdle_band_in,
        mast_through, tier_hinges, tier_plane_ranges,
    },
    preform::PreformSpec,
};
use glam::DVec3;
use indicatrix::{
    geometry::{
        meet_solver::{MeetConstraint, SolvedTier},
        stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};

// --- girdle thickness rule (pure) ---

#[test]
fn a_design_with_no_girdle_has_nothing_to_keep() {
    assert!(guard::girdle_admits(None, 0.5, None));
    assert!(guard::girdle_admits(None, 0.5, Some(0.01)));
}

#[test]
fn a_vanished_girdle_is_rejected() {
    assert!(!guard::girdle_admits(Some(0.1), 0.5, None));
    assert!(!guard::girdle_admits(Some(0.1), 0.0, Some(0.0)));
}

#[test]
fn a_girdle_thinner_than_the_fraction_is_rejected_and_one_at_it_is_not() {
    assert!(!guard::girdle_admits(Some(0.1), 0.5, Some(0.049)));
    assert!(guard::girdle_admits(Some(0.1), 0.5, Some(0.05)));
    assert!(guard::girdle_admits(Some(0.1), 0.5, Some(0.2)));
}

// --- girdle thinnest-point rule (pure) ---

/// A measured band; with no live wall there is no band at all.
fn band(min_thickness: f64, live_walls: usize) -> Option<GirdleBand> {
    (live_walls > 0).then_some(GirdleBand {
        min_thickness,
        live_walls,
    })
}

#[test]
fn a_design_with_no_girdle_band_has_nothing_to_keep() {
    assert!(guard::band_admits(None, 0.5, None));
    assert!(guard::band_admits(None, 0.5, band(0.0, 3)));
}

#[test]
fn a_vanished_band_or_a_lost_wall_is_rejected() {
    assert!(!guard::band_admits(band(0.01, 16), 0.5, None));
    assert!(!guard::band_admits(band(0.01, 16), 0.5, band(0.01, 15)));
    assert!(guard::band_admits(band(0.01, 16), 0.5, band(0.01, 16)));
    assert!(guard::band_admits(band(0.01, 16), 0.5, band(0.01, 18)));
}

#[test]
fn a_band_that_runs_to_a_knife_edge_is_rejected_whatever_the_fraction() {
    assert!(!guard::band_admits(band(0.01, 16), 0.0, band(0.0, 16)));
    assert!(!guard::band_admits(
        band(0.01, 16),
        0.0,
        band(KNIFE_EDGE_GAP, 16)
    ));
}

#[test]
fn a_band_thinner_at_its_corners_than_the_fraction_is_rejected_and_one_at_it_is_not() {
    assert!(!guard::band_admits(band(0.01, 16), 0.5, band(0.0049, 16)));
    assert!(guard::band_admits(band(0.01, 16), 0.5, band(0.005, 16)));
    assert!(guard::band_admits(band(0.01, 16), 0.5, band(0.02, 16)));
}

#[test]
fn a_band_that_was_a_knife_edge_already_has_nothing_to_keep_but_its_walls() {
    assert!(guard::band_admits(band(0.0, 16), 0.5, band(0.0, 16)));
    assert!(!guard::band_admits(band(0.0, 16), 0.5, band(0.0, 12)));
}

// --- the guard against real solids ---

/// A solved, meshed design: what the guard measures and checks.
struct Measured {
    design: Design,
    solved: Vec<SolvedTier>,
    planes: Vec<(DVec3, f64)>,
    mesh: SolidMesh,
}

fn measured(design: Design) -> Measured {
    let solved = design.solve().expect("design must solve");
    let planes = design.planes_from_solved(&solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("design must close");
    };
    Measured {
        design,
        solved,
        planes,
        mesh,
    }
}

fn guard_for(measured: &Measured, fraction: f64) -> ShapeGuard {
    ShapeGuard::measure(
        &measured.design,
        &measured.solved,
        &measured.planes,
        &measured.mesh,
        fraction,
    )
}

fn admits(guard: &ShapeGuard, measured: &Measured) -> bool {
    guard.admits(
        &measured.design,
        &measured.solved,
        &measured.planes,
        &measured.mesh,
    )
}

/// The fixture's girdle band must be live for the girdle tests below to mean anything.
fn assert_has_girdle(measured: &Measured) {
    let thickness = measure_solid(&measured.planes).and_then(|m| m.girdle_thickness);
    assert!(
        thickness.is_some_and(|t| t > 0.0),
        "RBC-445 must have a live girdle band, got {thickness:?}"
    );
}

#[test]
fn the_starting_design_always_passes_its_own_guard() {
    let start = measured(imported_rbc());
    assert_has_girdle(&start);
    let guard = guard_for(&start, 1.0);
    assert!(
        admits(&guard, &start),
        "a design is exactly as thick as itself"
    );
}

#[test]
fn a_girdle_thinner_than_the_requested_fraction_of_the_start_is_rejected() {
    let start = measured(imported_rbc());
    assert_has_girdle(&start);
    // Asking for 1.5 times the starting band means no design of the same thickness
    // qualifies, which is what a band that has thinned to two thirds looks like from
    // the other side.
    let strict = guard_for(&start, 1.5);
    assert!(!admits(&strict, &start));
    let lenient = guard_for(&start, 0.5);
    assert!(admits(&lenient, &start));
}

#[test]
fn a_lost_table_facet_is_rejected() {
    let start = measured(imported_rbc());
    assert_has_girdle(&start);
    let guard = guard_for(&start, 0.5);
    assert!(admits(&guard, &start));

    // Tier 11 is the +0.0 table. Lift its plane above the stone: the facet no longer
    // reaches the solid, so its ring is gone and the guard must say no.
    let mut lifted = start.design;
    assert_eq!(lifted.tiers[11].angle_deg.to_bits(), 0.0f64.to_bits());
    lifted.tiers[11].constraint = MeetConstraint::ScaleReference(9.0);
    let lifted = measured(lifted);
    assert!(
        !admits(&guard, &lifted),
        "a candidate without the table facet must be rejected"
    );
}

/// The standard round brilliant in a rough deep enough that the preform never touches it.
fn standard_brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// The girdle thickness `measure_solid` reports: the figure the old guard read.
fn measured_thickness(measured: &Measured) -> f64 {
    measure_solid(&measured.planes)
        .and_then(|metrics| metrics.girdle_thickness)
        .expect("the stone has a girdle")
}

/// The standard brilliant's two break facets (`start`'s Upper and Lower Girdle) turned steeper
/// about their girdle edges, the way a retarget re-anchors them, by `share` of the full turn
/// the pinch test below uses: `0.0` is `start` itself, `1.0` the angles 45.3 and -46.24.
fn pinch_turned(start: &Measured, share: f64) -> Measured {
    let hinges = tier_hinges(&start.design, &start.solved);
    let mut turned = start.design.clone();
    for (name, full_turn) in [("Upper Girdle", 45.3), ("Lower Girdle", -46.24)] {
        let index = turned
            .tiers
            .iter()
            .position(|tier| tier.name == name)
            .expect("the tier exists");
        let angle = start.design.tiers[index]
            .angle_deg
            .mul_add(1.0 - share, full_turn * share);
        let hinge = hinges[&index];
        let mast = mast_through(angle, hinge.azimuth_rad, hinge.side, hinge.point);
        turned.tiers[index].angle_deg = angle;
        turned.tiers[index].constraint = MeetConstraint::ScaleReference(mast);
    }
    measured(turned)
}

/// The girdle band of a measured design: its thinnest point and live walls.
fn band_of(measured: &Measured) -> Option<GirdleBand> {
    let ranges = tier_plane_ranges(&measured.design, &measured.solved);
    girdle_band_in(&measured.design, &ranges, &measured.planes, &measured.mesh)
}

/// A facet turned about its girdle edge keeps the extreme wall vertices the measured girdle
/// thickness is read from, so that figure cannot see the band pinch out between the walls.
/// The guard measures the thinnest point too, and says no.
#[test]
fn a_band_that_pinches_at_its_corners_is_rejected_while_the_measured_thickness_stays() {
    let start = measured(standard_brilliant());
    let guard = guard_for(&start, 0.5);
    assert!(admits(&guard, &start), "a design passes its own guard");

    // The owner's emerald case in miniature: both break facets turned steeper about their
    // girdle edges.
    let turned = pinch_turned(&start, 1.0);

    let (before, after) = (measured_thickness(&start), measured_thickness(&turned));
    assert!(
        (before - after).abs() < 1e-6,
        "the measured thickness is {before} before and {after} after"
    );
    assert!(
        guard::girdle_admits(Some(before), 0.5, Some(after)),
        "the thickness check alone is blind to this"
    );
    assert!(
        !admits(&guard, &turned),
        "the band pinches out at the corners and must be refused"
    );
}

/// The pinch-turned brilliant whose band reads `target` times the start's thinnest point at its
/// corners, found by bisecting the share of the full turn (the corners close steadily as the
/// facets turn, and the girdle's extreme vertices, so its overall thickness, stay put).
///
/// The angles are not written down because they depend on the stone's geometry: the share is
/// the one that makes the corners read `target`, whatever the planes give.
fn pinched_to(start: &Measured, target: f64) -> Measured {
    let start_band = band_of(start).expect("the start has a girdle band");
    let keeps_walls_and_target = |turned: &Measured| {
        band_of(turned).is_some_and(|band| {
            band.live_walls >= start_band.live_walls
                && band.min_thickness >= target * start_band.min_thickness
        })
    };
    // `kept` always reads at least the target (the start itself does) and `lost` always less
    // (the full turn pinches the band out).
    let (mut kept, mut lost) = (0.0_f64, 1.0_f64);
    for _ in 0..16 {
        let middle = kept.midpoint(lost);
        if keeps_walls_and_target(&pinch_turned(start, middle)) {
            kept = middle;
        } else {
            lost = middle;
        }
    }
    pinch_turned(start, kept)
}

/// The guard's two floors, one for the overall girdle thickness and one for the band's thinnest
/// point, are each applied to their own figure and no other.
///
/// The candidate is a milder pinch than the one above: its overall thickness is the start's
/// (ratio 1.0) while its corners read about 0.7 of the start's, so the two figures differ. A
/// guard that took the stricter floor for both would answer every case below the way it
/// answers when the floors are the same, which is how this tells separate floors from one.
#[test]
fn the_overall_floor_and_the_thinnest_floor_are_independent() {
    let start = measured(standard_brilliant());
    let turned = pinched_to(&start, 0.7);
    let overall = measured_thickness(&turned) / measured_thickness(&start);
    let thinnest = band_of(&turned).expect("a girdle band").min_thickness
        / band_of(&start).expect("a girdle band").min_thickness;
    assert!(
        (overall - 1.0).abs() < 1e-5,
        "the overall thickness must stay the start's, ratio {overall}"
    );
    assert!(
        (0.55..0.85).contains(&thinnest),
        "the corners must read between the two floors below, ratio {thinnest}"
    );
    let guard_with = |girdle_floor: f64, band_floor: f64| {
        guard_for(&start, girdle_floor).with_thinnest_fraction(Some(band_floor))
    };
    // The start is exactly as thick as itself in both figures, so floors of 1.0 let it through.
    assert!(admits(&guard_with(1.0, 1.0), &start));
    assert!(admits(&guard_with(0.5, 0.5), &start));
    // A strict floor on the overall figure and a lenient one on the corners admit the pinched
    // stone (its overall figure is the start's); the stricter-for-both reading refuses it.
    assert!(
        admits(&guard_with(0.9, 0.5), &turned),
        "ratios {overall}, {thinnest}"
    );
    assert!(!admits(&guard_with(0.9, 0.9), &turned));
    // The other way round: a strict floor on the corners refuses it, and a lenient overall
    // floor does not save it.
    assert!(
        !admits(&guard_with(0.5, 0.9), &turned),
        "ratios {overall}, {thinnest}"
    );
    assert!(admits(&guard_with(0.5, 0.5), &turned));
    // A floor above 1.0 on the overall figure refuses it though the corners' floor is lenient.
    assert!(!admits(&guard_with(1.5, 0.5), &turned));
    // Without a floor of its own the thinnest point takes the girdle fraction, as it always did.
    assert!(admits(
        &guard_for(&start, 0.5).with_thinnest_fraction(None),
        &turned
    ));
    assert!(!admits(
        &guard_for(&start, 0.9).with_thinnest_fraction(None),
        &turned
    ));
    assert!(!admits(
        &guard_for(&start, 1.5).with_thinnest_fraction(None),
        &start
    ));
}

#[test]
fn tier_plane_ranges_cover_every_tier_in_order_after_the_preform() {
    let start = measured(imported_rbc());
    let ranges = tier_plane_ranges(&start.design, &start.solved);
    assert_eq!(ranges.len(), start.design.tiers.len());
    let preform_len = start.design.preform.planes().len();
    assert_eq!(ranges[0].start, preform_len);
    for pair in ranges.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "ranges must be contiguous");
    }
    assert_eq!(ranges.last().map(|r| r.end), Some(start.planes.len()));
}

/// The guard is wired into the candidate gate: the same design that is accepted
/// without a guard is rejected under a guard it cannot satisfy.
#[test]
fn evaluate_candidate_rejects_what_the_guard_rejects() {
    let start = measured(imported_rbc());
    assert_has_girdle(&start);
    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights::default();
    let baseline = BaselineWarningCounts::count(&crate::check_manufacturability(
        &start.design,
        &start.solved,
        crate::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    ));
    let evaluate = |guard: Option<&ShapeGuard>| {
        candidate::evaluate_candidate(
            &start.design,
            &material,
            &weights,
            &baseline,
            CANONICAL_LIGHTING_PRESET,
            guard,
            None,
        )
    };
    assert!(matches!(evaluate(None), CandidateOutcome::Accepted { .. }));
    let lenient = guard_for(&start, 0.5);
    assert!(matches!(
        evaluate(Some(&lenient)),
        CandidateOutcome::Accepted { .. }
    ));
    let strict = guard_for(&start, 1.5);
    assert!(matches!(
        evaluate(Some(&strict)),
        CandidateOutcome::Rejected
    ));
}

// --- the candidate pool ---

fn entry(angles: &[f64], score: f32) -> PoolEntry {
    PoolEntry {
        angles: angles.to_vec(),
        score,
    }
}

fn kept(pool: &CandidatePool) -> Vec<PoolEntry> {
    pool.entries().to_vec()
}

#[test]
fn a_pool_of_one_is_disabled_and_keeps_nothing() {
    let mut pool = CandidatePool::new(1, 0.125);
    assert!(!pool.is_enabled());
    pool.offer(vec![0.0], 1.0);
    assert_eq!(kept(&pool).len(), 0);
    assert!(!CandidatePool::new(0, 0.125).is_enabled());
    assert!(CandidatePool::new(2, 0.125).is_enabled());
}

#[test]
fn the_pool_keeps_the_best_states_sorted_and_bounded() {
    let mut pool = CandidatePool::new(3, 0.5);
    for (angle, score) in [(0.0, 5.0), (1.0, 3.0), (2.0, 4.0), (3.0, 6.0), (4.0, 7.0)] {
        pool.offer(vec![angle], score);
    }
    // Capacity 3, so the three best states stay: 3.0, 4.0 and 5.0.
    assert_eq!(
        kept(&pool),
        vec![entry(&[1.0], 3.0), entry(&[2.0], 4.0), entry(&[0.0], 5.0)]
    );
}

#[test]
fn rivals_leave_out_the_best_state_and_keep_one_slot_free_for_it() {
    let mut pool = CandidatePool::new(3, 0.5);
    for (angle, score) in [(0.0, 5.0), (1.0, 3.0), (2.0, 4.0)] {
        pool.offer(vec![angle], score);
    }
    // The run's end point is the state at 1.0; the capacity of 3 means two rivals.
    let rivals = pool.rivals(&[1.0]);
    assert_eq!(rivals, vec![entry(&[2.0], 4.0), entry(&[0.0], 5.0)]);
}

#[test]
fn rivals_leave_out_anything_that_resembles_the_best_state() {
    let mut pool = CandidatePool::new(4, 0.5);
    pool.offer(vec![0.0], 1.0);
    pool.offer(vec![3.0], 2.0);
    // The end point sits a tenth of a degree from the first state: that state is the
    // end point for all practical purposes, not an alternative to it.
    let rivals = pool.rivals(&[0.1]);
    assert_eq!(rivals, vec![entry(&[3.0], 2.0)]);
}

#[test]
fn a_worse_look_alike_is_dropped_and_a_better_one_replaces_the_original() {
    let mut pool = CandidatePool::new(3, 0.5);
    pool.offer(vec![1.0], 3.0);
    pool.offer(vec![1.2], 3.5);
    assert_eq!(
        kept(&pool),
        vec![entry(&[1.0], 3.0)],
        "1.2 is within half a degree of 1.0 and scores worse: dropped"
    );
    pool.offer(vec![1.2], 2.0);
    assert_eq!(
        kept(&pool),
        vec![entry(&[1.2], 2.0)],
        "a better look-alike evicts the state it resembles"
    );
}

#[test]
fn states_exactly_the_separation_apart_are_distinct() {
    let mut pool = CandidatePool::new(3, 0.125);
    pool.offer(vec![0.0], 1.0);
    pool.offer(vec![0.125], 2.0);
    pool.offer(vec![0.1], 3.0);
    assert_eq!(
        kept(&pool),
        vec![entry(&[0.0], 1.0), entry(&[0.125], 2.0)],
        "0.125 is distinct from 0.0; 0.1 resembles both and scores worse than both"
    );
}

#[test]
fn distinctness_is_over_the_whole_angle_vector() {
    let mut pool = CandidatePool::new(3, 0.5);
    pool.offer(vec![0.0, 0.0], 1.0);
    // One angle differs by a full degree: distinct, though the other matches.
    pool.offer(vec![0.0, 1.0], 2.0);
    // Both angles within the separation of the first state: a worse look-alike.
    pool.offer(vec![0.2, 0.2], 3.0);
    assert_eq!(
        kept(&pool),
        vec![entry(&[0.0, 0.0], 1.0), entry(&[0.0, 1.0], 2.0)]
    );
}

#[test]
fn equal_scores_keep_their_arrival_order_and_bad_scores_are_ignored() {
    let mut pool = CandidatePool::new(4, 0.5);
    pool.offer(vec![0.0], 1.0);
    pool.offer(vec![5.0], 1.0);
    pool.offer(vec![9.0], f32::NAN);
    pool.offer(vec![8.0], f32::INFINITY);
    assert_eq!(kept(&pool), vec![entry(&[0.0], 1.0), entry(&[5.0], 1.0)]);
}

#[test]
fn a_zero_separation_is_raised_so_identical_states_are_never_distinct() {
    let mut pool = CandidatePool::new(3, 0.0);
    pool.offer(vec![1.0], 2.0);
    pool.offer(vec![1.0], 3.0);
    assert_eq!(kept(&pool), vec![entry(&[1.0], 2.0)]);
}
