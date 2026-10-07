//! Tests for the multi-start search: the Halton points and the start box, the generic
//! driver on a synthetic two-basin function (no `Design`), and the real search end to
//! end on the anchored fixture (yield-only objective, small budgets).

use super::{
    super::{
        ObjectiveWeights, OptimizeConfig, OptimizeOptions, OptimizeResult, SearchHooks,
        SearchStage, effective_starts, free_tier_indices_with, inclusive_max_evaluations_for,
        multistart::{
            DescentRun, DriverHooks, MultiOutcome, MultiSpec, StartEngine, StartPoint, StartState,
            halton_points, map_point, run_multistart, screening_draws,
        },
        optimize_design_with,
        pool::CandidatePool,
        space::SearchSpace,
    },
    anchored::anchored_options,
    fixtures::imported_rbc,
};
use crate::design::Design;
use indicatrix::optics::materials::GemMaterial;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

// --- Halton points ---

#[test]
fn halton_points_are_deterministic_per_seed_and_differ_between_seeds() {
    let a = halton_points(7, 5, 16);
    assert_eq!(a, halton_points(7, 5, 16));
    let b = halton_points(8, 5, 16);
    assert_eq!(a.len(), b.len());
    assert_ne!(a, b, "the scrambling shift depends on the seed");
}

#[test]
fn halton_points_fill_the_unit_cube_without_repeats() {
    let points = halton_points(3, 4, 32);
    assert_eq!(points.len(), 32);
    for point in &points {
        assert_eq!(point.len(), 4);
        assert!(point.iter().all(|&u| (0.0..1.0).contains(&u)));
    }
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            assert_ne!(a, b, "a Halton sequence never repeats a point");
        }
    }
    // The first points do not depend on how many are asked for.
    assert_eq!(points[..8], halton_points(3, 4, 8)[..]);
}

#[test]
fn halton_points_fall_back_to_uniforms_beyond_a_hundred_dimensions() {
    let points = halton_points(1, 120, 3);
    assert_eq!(points.len(), 3);
    assert!(
        points
            .iter()
            .all(|p| p.len() == 120 && p.iter().all(|&u| (0.0..1.0).contains(&u)))
    );
    assert_ne!(points[0], points[1]);
}

#[test]
fn mapped_points_stay_inside_their_boxes_and_local_ones_hug_the_incumbent() {
    let boxes = [(-9.0, 9.0), (40.0, 60.0)];
    let incumbent = [-6.0, 50.0];
    for point in halton_points(5, 2, 24) {
        let global = map_point(&point, &incumbent, &boxes, false);
        let local = map_point(&point, &incumbent, &boxes, true);
        for (angle, &(low, high)) in global.iter().chain(&local).zip(boxes.iter().cycle()) {
            assert!(*angle >= low && *angle <= high);
        }
        // 30 % of the reach: at most 0.3 * 15 = 4.5 and 0.3 * 10 = 3 from the incumbent.
        assert!((local[0] - incumbent[0]).abs() <= 4.5 + 1e-9);
        assert!((local[1] - incumbent[1]).abs() <= 3.0 + 1e-9);
    }
}

// --- The start box ---

#[test]
fn the_start_box_is_the_tiers_bounds_or_the_default_range_on_its_own_side() {
    let mut options = OptimizeOptions::default();
    options.angle_bounds.insert(2, (4.0, -3.0));
    let space = SearchSpace::new(&options, BTreeMap::new());
    assert_eq!(space.start_box(2, 0.7), (-3.0, 4.0));
    assert_eq!(space.start_box(0, 52.0), (47.0, 57.0));
    assert_eq!(space.start_box(0, 3.0), (0.5, 8.0));
    assert_eq!(space.start_box(0, 88.0), (83.0, 89.5));
    assert_eq!(space.start_box(0, -40.0), (-45.0, -35.0));
    assert_eq!(space.start_box(0, -88.0), (-89.5, -83.0));
}

#[test]
fn the_cost_rule_gives_every_start_four_sweeps() {
    let config = OptimizeConfig {
        max_evaluations: 200,
        starts: 8,
        ..OptimizeConfig::default()
    };
    // 7 free tiers: 8 * 7 = 56 evaluations per start, so 200 affords three.
    assert_eq!(effective_starts(&config, 7), 3);
    assert_eq!(effective_starts(&config, 25), 1);
    assert_eq!(effective_starts(&OptimizeConfig::default(), 7), 1);
    assert_eq!(screening_draws(1), 0);
    assert_eq!(screening_draws(3), 8);
    assert_eq!(screening_draws(32), 64);
}

// --- The driver on a synthetic objective ---

/// A narrow trap at (-6, -6) worth 5 (the incumbent sits in it) and a wide bowl with its
/// minimum 0 at (6, 6): a single coordinate descent from the trap never leaves it.
struct TwoBasins;

impl TwoBasins {
    fn value(angles: &[f64]) -> f32 {
        let (trap_x, trap_y) = (angles[0] + 6.0, angles[1] + 6.0);
        let (bowl_x, bowl_y) = (angles[0] - 6.0, angles[1] - 6.0);
        let trap = trap_x.mul_add(trap_x, trap_y * trap_y);
        let bowl = bowl_x.mul_add(bowl_x, bowl_y * bowl_y);
        50.0_f64.mul_add(trap, 5.0).min(0.5 * bowl) as f32
    }
}

impl StartEngine for TwoBasins {
    fn score(&self, angles: &[f64]) -> Option<f32> {
        Some(Self::value(angles))
    }

    fn descend(&self, run: &DescentRun<'_>, progress: &dyn Fn(usize, SearchStage)) -> StartState {
        let mut point = run.angles.to_vec();
        let mut score = run.score;
        let mut step = 2.0;
        let mut evaluations = 0usize;
        while step >= 0.125 && evaluations < run.max_evaluations {
            let mut improved = false;
            for axis in 0..point.len() {
                for delta in [step, -step] {
                    let mut trial = point.clone();
                    trial[axis] += delta;
                    let trial_score = Self::value(&trial);
                    evaluations += 1;
                    progress(evaluations, SearchStage::Coordinate);
                    if trial_score < score {
                        point = trial;
                        score = trial_score;
                        improved = true;
                    }
                }
            }
            if !improved {
                step /= 2.0;
            }
        }
        StartState {
            angles: point,
            score,
            evaluations,
            polish_evaluations: 0,
            polish_improvement: 0.0,
            cancelled: false,
            pool: self.new_pool(),
        }
    }

    fn polish(&self, state: &StartState, _progress: &dyn Fn(usize, SearchStage)) -> StartState {
        state.clone()
    }

    fn new_pool(&self) -> CandidatePool {
        CandidatePool::new(3, 0.5)
    }
}

fn two_basin_run(max_lanes: usize, starts: usize) -> (MultiOutcome, Vec<(usize, SearchStage)>) {
    let spec = MultiSpec {
        starts,
        max_lanes,
        budget: 400,
        polish_keep: 1,
        seed: 11,
        separation_deg: 0.5,
        cancel: None,
    };
    let incumbent = StartPoint {
        angles: vec![-6.0, -6.0],
        score: TwoBasins::value(&[-6.0, -6.0]),
    };
    let reports = RefCell::new(Vec::new());
    let report = |evaluations: usize, stage: SearchStage| {
        reports.borrow_mut().push((evaluations, stage));
    };
    let outcome = run_multistart(
        &TwoBasins,
        &spec,
        &incumbent,
        &[(-9.0, 9.0), (-9.0, 9.0)],
        &DriverHooks {
            report: Some(&report),
            on_start: None,
        },
    );
    (outcome, reports.into_inner())
}

#[test]
fn several_starts_find_the_deeper_basin_the_single_descent_misses() {
    let incumbent = [-6.0, -6.0];
    let single = TwoBasins.descend(
        &DescentRun {
            angles: &incumbent,
            score: TwoBasins::value(&incumbent),
            max_evaluations: 400,
            seed: 11,
        },
        &|_, _| {},
    );
    assert_eq!(single.score, 5.0, "the trap holds a descent from inside it");

    let (outcome, _) = two_basin_run(1, 4);
    let best_score = TwoBasins::value(&outcome.best_angles);
    assert!(
        best_score < 1.0,
        "a drawn start must reach the wide bowl, got {best_score}"
    );
    assert_ne!(outcome.best_start, 0);
    assert!(outcome.starts_run >= 2);
    assert!((outcome.best_angles[0] - 6.0).abs() < 2.0);
}

#[test]
fn the_driver_gives_the_same_outcome_for_any_lane_count() {
    let (one, one_reports) = two_basin_run(1, 4);
    let (four, four_reports) = two_basin_run(4, 4);
    assert_eq!(one.best_angles, four.best_angles);
    assert_eq!(one.evaluations, four.evaluations);
    assert_eq!(one.starts_run, four.starts_run);
    assert_eq!(one.best_start, four.best_start);
    assert_eq!(one.polish_evaluations, four.polish_evaluations);

    // The screening reports come first, one per draw, then the counter never goes back.
    for reports in [&one_reports, &four_reports] {
        let draws = screening_draws(4);
        assert!(
            reports[..draws]
                .iter()
                .enumerate()
                .all(|(i, &(count, stage))| count == i + 1 && stage == SearchStage::Screening)
        );
        assert!(reports.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }
}

// --- The real search on the anchored fixture ---

const BOUND_DEG: f64 = 1.0;

fn yield_only() -> ObjectiveWeights {
    ObjectiveWeights {
        windowing: 0.0,
        extinction: 0.0,
        tilt_brilliance: 0.0,
        yield_weight: 1.0,
        ..ObjectiveWeights::default()
    }
}

fn bounded_options(design: &Design) -> OptimizeOptions {
    let mut options = anchored_options(design);
    options.keep_candidates = 3;
    options.candidate_separation_deg = Some(0.5);
    for index in free_tier_indices_with(design, &options) {
        let angle = design.tiers[index].angle_deg;
        options
            .angle_bounds
            .insert(index, (angle - BOUND_DEG, angle + BOUND_DEG));
    }
    options
}

fn config(max_evaluations: usize, starts: usize, max_lanes: usize) -> OptimizeConfig {
    OptimizeConfig {
        weights: yield_only(),
        seed: 7,
        max_evaluations,
        polish_start_step_deg: Some(0.5),
        polish_max_evaluations: Some(12),
        starts,
        max_lanes,
        ..OptimizeConfig::default()
    }
}

struct Fixture {
    design: Design,
    options: OptimizeOptions,
    free: usize,
}

fn fixture() -> Fixture {
    let design = imported_rbc();
    let options = bounded_options(&design);
    let free = free_tier_indices_with(&design, &options).len();
    Fixture {
        design,
        options,
        free,
    }
}

fn run(fixture: &Fixture, config: &OptimizeConfig) -> (OptimizeResult, Vec<SearchStage>) {
    let stages = RefCell::new(Vec::new());
    let on_progress = |_evaluations: usize, stage: SearchStage| stages.borrow_mut().push(stage);
    let hooks = SearchHooks {
        cancel: None,
        on_progress: Some(&on_progress),
        on_start: None,
    };
    let result = optimize_design_with(
        &fixture.design,
        &GemMaterial::diamond(),
        config,
        &fixture.options,
        &hooks,
    )
    .expect("the imported design must solve");
    (result, stages.into_inner())
}

#[test]
fn one_start_is_the_search_it_always_was() {
    let fixture = fixture();
    let (plain, plain_stages) = run(&fixture, &config(30, 1, 0));
    let (lanes, _) = run(&fixture, &config(30, 1, 5));
    let (zero, _) = run(&fixture, &config(30, 0, 0));
    assert_eq!(plain, lanes);
    assert_eq!(plain, zero);
    assert_eq!(plain.starts_run, 1);
    assert_eq!(plain.best_start, 0);
    assert!(!plain_stages.contains(&SearchStage::Screening));
}

#[test]
fn the_result_does_not_depend_on_the_lane_count() {
    let fixture = fixture();
    let budget = 3 * 8 * fixture.free;
    let (serial, _) = run(&fixture, &config(budget, 3, 1));
    let (parallel, _) = run(&fixture, &config(budget, 3, 3));
    assert_eq!(serial, parallel);
}

#[test]
fn a_multi_start_run_stays_inside_its_inclusive_budget_and_reports_its_starts() {
    let fixture = fixture();
    let budget = 3 * 8 * fixture.free;
    let cfg = config(budget, 3, 2);
    assert_eq!(effective_starts(&cfg, fixture.free), 3);
    let (result, stages) = run(&fixture, &cfg);
    assert!((1..=3).contains(&result.starts_run));
    assert!(result.best_start < result.starts_run);
    let limit = inclusive_max_evaluations_for(&cfg, fixture.options.keep_candidates, fixture.free);
    assert!(
        result.outcome.evaluations <= limit + 2 * result.starts_run,
        "{} evaluations against a limit of {limit}",
        result.outcome.evaluations
    );
    assert_eq!(stages.first(), Some(&SearchStage::BaselineFull));
    assert!(stages.contains(&SearchStage::Screening));
    assert_eq!(stages.last(), Some(&SearchStage::FinalFull));
    assert!(result.outcome.after_score <= result.outcome.before_score);
}

#[test]
fn a_budget_below_eight_sweeps_per_tier_degrades_to_the_single_start() {
    let fixture = fixture();
    let budget = 8 * fixture.free - 1;
    let cfg = config(budget, 4, 2);
    assert_eq!(effective_starts(&cfg, fixture.free), 1);
    let (result, stages) = run(&fixture, &cfg);
    let (single, _) = run(&fixture, &config(budget, 1, 0));
    assert_eq!(result, single);
    assert_eq!(result.starts_run, 1);
    assert!(!stages.contains(&SearchStage::Screening));
}

#[test]
fn cancelling_from_the_first_screening_report_still_returns_a_result_no_worse_than_the_start() {
    let fixture = fixture();
    let budget = 3 * 8 * fixture.free;
    let cancel = AtomicBool::new(false);
    let on_progress = |_evaluations: usize, stage: SearchStage| {
        if matches!(stage, SearchStage::Screening | SearchStage::Coordinate) {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let hooks = SearchHooks {
        cancel: Some(&cancel),
        on_progress: Some(&on_progress),
        on_start: None,
    };
    let result = optimize_design_with(
        &fixture.design,
        &GemMaterial::diamond(),
        &config(budget, 3, 2),
        &fixture.options,
        &hooks,
    )
    .expect("the imported design must solve");
    assert!(result.outcome.cancelled);
    assert!(result.outcome.after_score <= result.outcome.before_score);
}
