//! Tests for [`super::super::polish::run_polish`], generic over a plain
//! synthetic scoring closure -- no [`crate::design::Design`], no `solve`, no
//! `#[ignore]` needed (see the parent module's own doc comment, "Coordinate
//! descent stalls on diagonal ridges").

use super::super::polish;

/// The Nelder-Mead polish stage substantially closes the gap on a diagonal ridge a
/// pure axis-aligned (coordinate) search stalls on -- see the parent module's
/// "Coordinate descent stalls on diagonal ridges" section.
///
/// `f(a, b) = (a - b)^2 + 0.01 * (a + b - c)^2` has its minimum at `a == b == c /
/// 2`; its Hessian is ill-conditioned (eigenvalues roughly 4.02 and 0.02, a ~200:1
/// ratio), so a search that only ever moves one of `a`/`b` at a time makes real but
/// very slow progress along the shallow `a == b` valley -- once its step size
/// floors out at a fixed minimum, whatever residual remains along that valley is
/// where it stalls. `(0.0, 6.0)` below stands in for such a stalled point: clearly
/// not optimal, and not on the ridge line either.
#[test]
fn polish_stage_closes_most_of_the_gap_on_a_diagonal_ridge_from_a_stalled_point() {
    let c = 10.0;
    let f = |p: &[f64]| 0.01f64.mul_add((p[0] + p[1] - c).powi(2), (p[0] - p[1]).powi(2)) as f32;
    let start = [0.0, 6.0];
    let start_score = f(&start);

    // The stand-in stalled point is nowhere near the ridge's minimum.
    assert!(
        start_score > 10.0,
        "fixture must actually be far from optimal, got {start_score}"
    );

    let result = polish::run_polish(&start, start_score, 0.5, 300, 1e-6, &|| false, f);

    assert!(
        result.score < start_score / 10.0,
        "polish must substantially improve on the stalled point: start={start_score} end={}",
        result.score
    );
    let midpoint = f64::midpoint(result.point[0], result.point[1]);
    assert!(
        (result.point[0] - result.point[1]).abs() < 0.5,
        "polish should settle close to the ridge line a == b, got {:?}",
        result.point
    );
    assert!(
        (midpoint - c / 2.0).abs() < 0.5,
        "polish should approach the ridge's true minimum near a == b == c/2, got {:?}",
        result.point
    );
}

#[test]
fn polish_stage_is_deterministic_for_identical_inputs() {
    let c = 10.0;
    let f = |p: &[f64]| 0.01f64.mul_add((p[0] + p[1] - c).powi(2), (p[0] - p[1]).powi(2)) as f32;
    let start = [0.0, 6.0];
    let run = || polish::run_polish(&start, f(&start), 0.5, 100, 1e-6, &|| false, f);
    let a = run();
    let b = run();
    assert_eq!(a.point, b.point);
    assert!((a.score - b.score).abs() < f32::EPSILON);
    assert_eq!(a.evaluations, b.evaluations);
    assert_eq!(a.cancelled, b.cancelled);
}

/// `is_cancelled` is polled at the very top of the main loop, before the initial
/// simplex is ever touched further -- only the one evaluation per dimension spent
/// building that initial simplex should ever run.
#[test]
fn polish_stage_stops_when_cancelled() {
    let f = |p: &[f64]| p[1].mul_add(p[1], p[0] * p[0]) as f32;
    let start = [10.0, 10.0];
    let result = polish::run_polish(&start, f(&start), 1.0, 1000, 1e-9, &|| true, f);
    assert!(result.cancelled);
    assert_eq!(result.evaluations, start.len());
}

/// A point the scoring closure rejects (here, anything outside a synthetic `|a|,
/// |b| <= 5` box, scored `f32::INFINITY` -- exactly how
/// `candidate::build_free_angle_candidate` treats an out-of-bounds simplex point,
/// never clamping it back in) must never end up as `run_polish`'s own reported
/// result, even when the unconstrained minimum lies outside that box.
#[test]
fn polish_stage_never_returns_a_point_the_evaluator_rejected() {
    let f = |p: &[f64]| {
        if p[0].abs() > 5.0 || p[1].abs() > 5.0 {
            f32::INFINITY
        } else {
            (p[1] - 10.0).mul_add(p[1] - 10.0, (p[0] - 10.0).powi(2)) as f32
        }
    };
    let start = [0.0, 0.0];
    let result = polish::run_polish(&start, f(&start), 1.0, 200, 1e-6, &|| false, f);
    assert!(
        result.score.is_finite(),
        "polish must never settle on a rejected (infinite-score) point"
    );
    assert!(
        result.score <= f(&start),
        "polish must not regress on its own starting point"
    );
}
