//! Tests for the angle sweep: the plan, the run (rows, order, cancel, workers), the
//! relations, the undo step, the CSV, the chart and the form helpers.

use super::*;
use crate::solve_policy::SolveCostEstimate;
use indicatrix::optics::{materials::GemMaterial, raytracer::LightingPreset};
use indicatrix_cut_core::{ConstraintTier, History, PreformSpec, ScheduleMeta};
use std::sync::Mutex;

/// Positions in the standard round brilliant template.
const CROWN_MAIN: usize = 2;
const UPPER_GIRDLE: usize = 3;
const GIRDLE: usize = 4;
const PAVILION_MAIN: usize = 5;
const LOWER_GIRDLE: usize = 6;
const TABLE: usize = 0;
const CULET: usize = 7;

/// The template: eight pinned tiers that solve and close (Pavilion Main at -41).
fn rbc() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.ensure_tier_ids();
    design
}

fn session_of(design: Design) -> EditorSession {
    EditorSession::with_history(design, History::new())
}

fn range(from_deg: f64, to_deg: f64, step_deg: f64) -> SweepRange {
    SweepRange {
        from_deg,
        to_deg,
        step_deg,
    }
}

fn scene(material: &GemMaterial) -> SweepScene<'_> {
    SweepScene {
        material,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
    }
}

fn options(workers: usize, tilt_average: bool) -> SweepOptions {
    SweepOptions {
        tilt_average,
        workers,
    }
}

fn run(design: &Design, plan: &SweepPlan, workers: usize, tilt_average: bool) -> SweepOutcome {
    let material = GemMaterial::diamond();
    sweep_tier_angle(
        design,
        plan,
        &scene(&material),
        &options(workers, tilt_average),
        &AtomicBool::new(false),
        &|_, _| {},
    )
}

fn pavilion_plan(design: &Design, from_deg: f64, to_deg: f64, step_deg: f64) -> SweepPlan {
    plan_sweep(design, PAVILION_MAIN, range(from_deg, to_deg, step_deg)).expect("a valid plan")
}

fn figures(brilliance: f32, windowing: f32) -> RowMetrics {
    RowMetrics {
        brilliance_pct: brilliance,
        windowing_pct: windowing,
        extinction_pct: 10.0,
        fire_index: 20.0,
        scintillation_pct: 30.0,
        yield_pct: Some(40.0),
        warning_count: 0,
        has_girdle: true,
        tilt: None,
    }
}

fn row(angle_deg: f64, is_current: bool, metrics: Option<RowMetrics>) -> SweepRow {
    SweepRow {
        angle_deg,
        is_current,
        metrics,
        notes: Vec::new(),
    }
}

fn outcome_of(rows: Vec<SweepRow>, tilt_average: bool) -> SweepOutcome {
    SweepOutcome {
        tier: 0,
        tier_name: "Pavilion Main".to_owned(),
        current_deg: -41.0,
        requested: rows.len(),
        cancelled: false,
        tilt_average,
        rows,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// --- the plan ------------------------------------------------------------------------

#[test]
fn the_plan_lists_the_grid_flattest_first_whichever_end_comes_first() {
    let design = rbc();
    // The pavilion tier stores -41; the grid runs 39, 40, ... 43 degrees from flat and
    // each angle carries the tier's own (negative) side.
    let plan = pavilion_plan(&design, 43.0, 39.0, 1.0);
    assert_eq!(plan.angles, vec![-39.0, -40.0, -41.0, -42.0, -43.0]);
    assert_eq!(plan.tier, PAVILION_MAIN);
    assert_eq!(plan.tier_name, "Pavilion Main");
    assert!(near(plan.current_deg, -41.0));
    assert_eq!(plan.current_row, 2);
    // The ends in the other order, or typed with their stored sign, give the same plan.
    assert_eq!(pavilion_plan(&design, 39.0, 43.0, 1.0), plan);
    assert_eq!(pavilion_plan(&design, -39.0, -43.0, 1.0), plan);
    assert_eq!(pavilion_plan(&design, -43.0, 39.0, 1.0), plan);
}

#[test]
fn a_signed_and_an_unsigned_range_give_the_same_plan_on_either_side_of_the_girdle() {
    let design = rbc();
    let crown = |from: f64, to: f64| {
        plan_sweep(&design, CROWN_MAIN, range(from, to, 0.5)).expect("a valid plan")
    };
    assert_eq!(crown(33.5, 35.5), crown(-33.5, -35.5));
    assert_eq!(crown(33.5, 35.5).angles, vec![33.5, 34.0, 34.5, 35.0, 35.5]);
    let pavilion = |from: f64, to: f64| pavilion_plan(&design, from, to, 0.5);
    assert_eq!(pavilion(40.0, 42.0), pavilion(-40.0, -42.0));
    assert_eq!(
        pavilion(40.0, 42.0).angles,
        vec![-40.0, -40.5, -41.0, -41.5, -42.0]
    );
}

#[test]
fn an_off_grid_current_angle_is_added_as_a_row() {
    let design = rbc();
    let plan = pavilion_plan(&design, 43.0, 40.0, 1.5);
    assert_eq!(plan.angles, vec![-40.0, -41.0, -41.5, -43.0]);
    assert_eq!(plan.current_row, 1);
    // A range that misses the current angle altogether still holds it.
    let far = pavilion_plan(&design, 46.0, 44.0, 1.0);
    assert_eq!(far.angles, vec![-41.0, -44.0, -45.0, -46.0]);
    assert_eq!(far.current_row, 0);
}

#[test]
fn a_grid_angle_next_to_the_current_one_becomes_the_current_row() {
    let mut design = rbc();
    design.tiers[PAVILION_MAIN].angle_deg = -41.000_01;
    let plan = pavilion_plan(&design, -43.0, -39.0, 1.0);
    assert_eq!(plan.angles.len(), 5);
    assert_eq!(plan.angles[2].to_bits(), (-41.000_01_f64).to_bits());
    assert_eq!(plan.current_row, 2);
}

#[test]
fn grid_angles_carry_no_float_noise() {
    let design = rbc();
    let plan = pavilion_plan(&design, 41.3, 40.9, 0.1);
    assert_eq!(plan.angles, vec![-40.9, -41.0, -41.1, -41.2, -41.3]);
}

#[test]
fn a_range_of_one_angle_is_that_angle_alone() {
    let design = rbc();
    let plan = pavilion_plan(&design, -41.0, -41.0, 1.0);
    assert_eq!(plan.angles, vec![-41.0]);
    assert_eq!(plan.current_row, 0);
}

#[test]
fn a_crown_tier_sweeps_on_the_positive_side() {
    let design = rbc();
    let plan = plan_sweep(&design, CROWN_MAIN, range(33.5, 35.5, 0.5)).expect("a valid plan");
    assert_eq!(plan.angles, vec![33.5, 34.0, 34.5, 35.0, 35.5]);
    assert_eq!(plan.current_row, 2);
}

#[test]
fn a_tier_that_cannot_be_swept_is_refused_in_plain_words() {
    let design = rbc();
    let whole = range(-43.0, -39.0, 1.0);
    assert!(matches!(
        plan_sweep(&design, 99, whole),
        Err(SweepError::NoSuchTier {
            tier: 99,
            tier_count: 8
        })
    ));
    assert!(matches!(
        plan_sweep(&design, TABLE, whole),
        Err(SweepError::Flat { .. })
    ));
    assert!(matches!(
        plan_sweep(&design, CULET, whole),
        Err(SweepError::Flat { .. })
    ));
    assert!(matches!(
        plan_sweep(&design, GIRDLE, whole),
        Err(SweepError::Vertical { .. })
    ));
    let message = plan_sweep(&design, TABLE, whole).unwrap_err().to_string();
    assert!(message.starts_with("Table is flat"), "{message}");
}

#[test]
fn a_tier_that_follows_a_relation_cannot_be_swept() {
    let mut session = session_of(rbc());
    session
        .set_tier_relation(UPPER_GIRDLE, "[Crown Main] + 6.5")
        .expect("a relation");
    let error = plan_sweep(&session.design, UPPER_GIRDLE, range(38.0, 42.0, 1.0)).unwrap_err();
    assert!(matches!(error, SweepError::Driven { .. }));
    let message = error.to_string();
    assert!(
        message.starts_with("Upper Girdle follows a relation ("),
        "{message}"
    );
    // The tier it reads is free.
    assert!(plan_sweep(&session.design, CROWN_MAIN, range(33.5, 35.5, 1.0)).is_ok());
}

#[test]
fn the_sweepable_tiers_are_the_free_slanted_ones() {
    let design = rbc();
    // Not the table, the girdle or the culet.
    assert_eq!(
        sweepable_tiers(&design),
        vec![1, CROWN_MAIN, UPPER_GIRDLE, PAVILION_MAIN, LOWER_GIRDLE]
    );
    let mut session = session_of(design);
    session
        .set_tier_relation(UPPER_GIRDLE, "[Crown Main] + 6.5")
        .expect("a relation");
    let tiers = sweepable_tiers(&session.design);
    assert!(!tiers.contains(&UPPER_GIRDLE), "a driven tier is left out");
    assert!(tiers.contains(&CROWN_MAIN), "the tier it reads stays");
    for tier in tiers {
        assert!(
            plan_sweep(&session.design, tier, range(-43.0, -39.0, 1.0)).is_ok()
                || plan_sweep(&session.design, tier, range(30.0, 35.0, 1.0)).is_ok(),
            "tier {tier} is listed, so it must not be refused for what it is"
        );
    }
}

#[test]
fn a_range_out_of_bounds_is_refused_and_a_sign_is_not_a_side() {
    let design = rbc();
    // The side of the girdle comes from the tier, so no sign puts an end on the wrong side.
    assert!(plan_sweep(&design, PAVILION_MAIN, range(39.0, 43.0, 1.0)).is_ok());
    assert!(plan_sweep(&design, CROWN_MAIN, range(-35.0, -33.0, 1.0)).is_ok());
    for bad in [0.0, 0.05, -0.05, 95.0, -95.0, -89.95, f64::NAN] {
        assert!(
            matches!(
                plan_sweep(&design, PAVILION_MAIN, range(bad, 41.0, 1.0)),
                Err(SweepError::OutOfBounds { .. })
            ),
            "{bad}"
        );
    }
    let message = plan_sweep(&design, PAVILION_MAIN, range(95.0, 41.0, 1.0))
        .unwrap_err()
        .to_string();
    assert!(
        message.starts_with("An angle of 95.00 degrees"),
        "{message}"
    );
}

#[test]
fn the_step_must_be_more_than_zero() {
    let design = rbc();
    for step in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            plan_sweep(&design, PAVILION_MAIN, range(-43.0, -39.0, step)),
            Err(SweepError::StepNotPositive),
            "{step}"
        );
    }
}

#[test]
fn a_sweep_takes_at_most_two_hundred_angles() {
    let design = rbc();
    let at_the_limit = pavilion_plan(&design, -50.9, -31.0, 0.1);
    assert_eq!(at_the_limit.angles.len(), MAX_SWEEP_STEPS);
    assert!(at_the_limit.angles.contains(&-41.0));
    assert_eq!(
        plan_sweep(&design, PAVILION_MAIN, range(-51.0, -31.0, 0.1)),
        Err(SweepError::TooManySteps {
            steps: 201,
            max: MAX_SWEEP_STEPS
        })
    );
    // A tiny step over a wide range is refused before anything is allocated.
    assert!(matches!(
        plan_sweep(&design, PAVILION_MAIN, range(-80.0, -1.0, 1e-9)),
        Err(SweepError::TooManySteps { .. })
    ));
    // The current angle, added to a grid of 200, is the 201st.
    let mut shifted = rbc();
    shifted.tiers[PAVILION_MAIN].angle_deg = -41.05;
    assert!(matches!(
        plan_sweep(&shifted, PAVILION_MAIN, range(-50.9, -31.0, 0.1)),
        Err(SweepError::TooManySteps { steps: 201, .. })
    ));
}

// --- the run -------------------------------------------------------------------------

#[test]
fn a_sweep_returns_a_row_per_angle_flattest_first() {
    let design = rbc();
    let plan = pavilion_plan(&design, -42.0, -40.0, 1.0);
    let outcome = run(&design, &plan, 1, false);

    assert!(!outcome.cancelled);
    assert_eq!(outcome.requested, 3);
    assert_eq!(outcome.tier, PAVILION_MAIN);
    assert_eq!(outcome.tier_name, "Pavilion Main");
    let angles: Vec<f64> = outcome.rows.iter().map(|r| r.angle_deg).collect();
    assert_eq!(angles, plan.angles);

    let current = outcome.current_row().expect("the current row finished");
    assert!(near(current.angle_deg, -41.0));
    assert!(current.is_valid(), "{:?}", current.notes);
    let scored = current.metrics.expect("scored");
    for percent in [
        scored.brilliance_pct,
        scored.windowing_pct,
        scored.extinction_pct,
        scored.scintillation_pct,
    ] {
        assert!((0.0..=100.0).contains(&percent), "{percent}");
    }
    assert!(scored.fire_index >= 0.0);
    assert!(scored.has_girdle);
    assert!(scored.tilt.is_none());
    let yield_pct = scored.yield_pct.expect("the rough has a volume");
    assert!(yield_pct > 0.0 && yield_pct < 100.0, "{yield_pct}");
    assert_eq!(outcome.rows.iter().filter(|r| r.is_current).count(), 1);
    assert!(outcome.valid_count() >= 1);
}

#[test]
fn the_current_row_matches_a_direct_score_of_the_design() {
    let design = rbc();
    let plan = pavilion_plan(&design, -41.0, -41.0, 1.0);
    let outcome = run(&design, &plan, 1, false);
    let swept = outcome.rows[0].metrics.expect("scored");

    let material = GemMaterial::diamond();
    let solved = design.solve().expect("the template solves");
    let planes = crate::solve_policy::design_to_gpu_planes_from_solved(&design, &solved);
    let direct = evaluate_gem_optical_metrics(
        &planes,
        &material,
        0.0,
        std::f32::consts::FRAC_PI_2,
        scene(&material).environment,
    );
    assert_eq!(
        swept.brilliance_pct.to_bits(),
        direct.brilliance_pct.to_bits()
    );
    assert_eq!(
        swept.windowing_pct.to_bits(),
        direct.windowing_pct.to_bits()
    );
    assert_eq!(
        swept.extinction_pct.to_bits(),
        direct.extinction_pct.to_bits()
    );
}

#[test]
fn the_number_of_workers_does_not_change_the_rows() {
    let design = rbc();
    let plan = pavilion_plan(&design, -43.0, -39.0, 0.5);
    let alone = run(&design, &plan, 1, false);
    let together = run(&design, &plan, 3, false);
    assert_eq!(alone.rows, together.rows);
    assert_eq!(alone, together);
    // More workers than angles is fine too.
    let many = run(&design, &plan, 64, false);
    assert_eq!(alone.rows, many.rows);
}

#[test]
fn a_sweep_does_not_touch_the_design_it_was_given() {
    let design = rbc();
    let before = design.clone();
    let plan = pavilion_plan(&design, -43.0, -39.0, 1.0);
    let _ = run(&design, &plan, 2, false);
    assert_eq!(design, before);
}

#[test]
fn cancelling_stops_early_and_keeps_the_finished_rows() {
    let design = rbc();
    let plan = pavilion_plan(&design, -43.0, -39.0, 1.0);
    let material = GemMaterial::diamond();
    let cancel = AtomicBool::new(false);
    let progress = |done: usize, _total: usize| {
        if done >= 2 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let outcome = sweep_tier_angle(
        &design,
        &plan,
        &scene(&material),
        &options(1, false),
        &cancel,
        &progress,
    );
    assert!(outcome.cancelled);
    assert_eq!(outcome.requested, 5);
    assert_eq!(outcome.rows.len(), 2);
    let angles: Vec<f64> = outcome.rows.iter().map(|r| r.angle_deg).collect();
    assert_eq!(angles, vec![-39.0, -40.0], "the flattest angles come first");
    assert!(outcome.current_row().is_none());
}

#[test]
fn a_sweep_cancelled_before_it_starts_returns_no_rows() {
    let design = rbc();
    let plan = pavilion_plan(&design, -43.0, -39.0, 1.0);
    let material = GemMaterial::diamond();
    let outcome = sweep_tier_angle(
        &design,
        &plan,
        &scene(&material),
        &options(2, false),
        &AtomicBool::new(true),
        &|_, _| {},
    );
    assert!(outcome.cancelled);
    assert_eq!(outcome.rows, Vec::<SweepRow>::new());
}

#[test]
fn progress_counts_each_finished_row() {
    let design = rbc();
    let plan = pavilion_plan(&design, -42.0, -40.0, 1.0);
    let material = GemMaterial::diamond();
    let seen = Mutex::new(Vec::new());
    let progress = |done: usize, total: usize| seen.lock().expect("lock").push((done, total));
    let _ = sweep_tier_angle(
        &design,
        &plan,
        &scene(&material),
        &options(1, false),
        &AtomicBool::new(false),
        &progress,
    );
    assert_eq!(*seen.lock().expect("lock"), vec![(1, 3), (2, 3), (3, 3)]);
}

#[test]
fn a_relation_that_cannot_be_satisfied_makes_an_invalid_row_with_the_reason() {
    let mut session = session_of(rbc());
    session
        .set_tier_relation(UPPER_GIRDLE, "[Crown Main] + 6.5")
        .expect("a relation");
    let design = session.design.clone();
    // At 84 to 86 degrees the Upper Girdle would have to be steeper than 90.
    let plan = plan_sweep(&design, CROWN_MAIN, range(84.0, 86.0, 1.0)).expect("a plan");
    assert_eq!(plan.angles, vec![34.5, 84.0, 85.0, 86.0]);
    let outcome = run(&design, &plan, 2, false);

    assert_eq!(outcome.rows.len(), 4);
    let current = &outcome.rows[0];
    assert!(
        current.is_current && current.is_valid(),
        "{:?}",
        current.notes
    );
    for refused in &outcome.rows[1..] {
        assert!(!refused.is_valid());
        assert!(refused.metrics.is_none());
        assert!(
            refused.notes_text().contains("Upper Girdle"),
            "{:?}",
            refused.notes
        );
    }
}

#[test]
fn a_driven_tier_follows_the_swept_one_on_every_row() {
    let mut session = session_of(rbc());
    session
        .set_tier_relation(LOWER_GIRDLE, "[Pavilion Main] + 1.5")
        .expect("a relation");
    let design = session.design.clone();
    let plan = pavilion_plan(&design, -42.0, -40.0, 1.0);
    let with_relation = run(&design, &plan, 1, false);

    // The same sweep with the Lower Girdle fixed at -42.5 differs from the one where it
    // moves with the Pavilion Main, except at the current angle where both agree.
    let free = run(&rbc(), &plan, 1, false);
    assert_eq!(with_relation.rows.len(), free.rows.len());
    assert_eq!(
        with_relation.rows[1], free.rows[1],
        "at the current angle both designs are the same stone"
    );
    assert_ne!(
        with_relation.rows[0], free.rows[0],
        "away from it the relation moves the Lower Girdle"
    );
}

#[test]
fn tilt_averages_are_computed_when_asked_for() {
    let design = rbc();
    let plan = pavilion_plan(&design, -41.0, -41.0, 1.0);
    let outcome = run(&design, &plan, 1, true);
    assert!(outcome.tilt_average);
    let tilt = outcome.rows[0]
        .metrics
        .expect("scored")
        .tilt
        .expect("tilt averages");
    for percent in [tilt.brilliance_pct, tilt.windowing_pct, tilt.extinction_pct] {
        assert!((0.0..=100.0).contains(&percent), "{percent}");
    }
}

// --- using an angle ------------------------------------------------------------------

#[test]
fn using_an_angle_is_one_undo_step_that_carries_the_followers() {
    let mut session = session_of(rbc());
    session
        .set_tier_relation(LOWER_GIRDLE, "[Pavilion Main] + 1.5")
        .expect("a relation");
    let before = session.design.clone();

    let change = apply_sweep_angle(&mut session, PAVILION_MAIN, -40.5).expect("applied");
    assert!(change.is_some());
    assert!(near(session.design.tiers[PAVILION_MAIN].angle_deg, -40.5));
    assert!(near(session.design.tiers[LOWER_GIRDLE].angle_deg, -42.0));

    assert!(session.undo().expect("undo").is_some());
    assert_eq!(session.design, before);
}

#[test]
fn using_the_angle_a_tier_already_has_changes_nothing() {
    let mut session = session_of(rbc());
    let generation = session.current_generation();
    assert!(matches!(
        apply_sweep_angle(&mut session, PAVILION_MAIN, -41.0),
        Ok(None)
    ));
    assert_eq!(session.current_generation(), generation);
}

#[test]
fn using_an_angle_on_a_tier_that_now_follows_a_relation_is_refused() {
    let mut session = session_of(rbc());
    session
        .set_tier_relation(LOWER_GIRDLE, "[Pavilion Main] + 1.5")
        .expect("a relation");
    assert!(matches!(
        apply_sweep_angle(&mut session, LOWER_GIRDLE, -40.0),
        Err(SessionEditError::Driven(_))
    ));
    assert!(matches!(
        apply_sweep_angle(&mut session, 99, -40.0),
        Err(SessionEditError::Edit(_))
    ));
}

// --- the figures, the best rows, the CSV ---------------------------------------------

#[test]
fn the_best_rows_are_marked_per_figure_among_the_valid_ones() {
    let rows = vec![
        row(40.0, false, Some(figures(70.0, 5.0))),
        row(41.0, true, Some(figures(80.0, 3.0))),
        row(42.0, false, None),
        row(43.0, false, Some(figures(80.001, 9.0))),
    ];
    let flags = best_flags(&rows);
    assert_eq!(flags.len(), 4);
    assert!(flags[1].brilliance, "the highest brilliance");
    assert!(flags[3].brilliance, "a tie at two decimals is a tie");
    assert!(!flags[0].brilliance);
    assert!(flags[1].windowing, "the lowest windowing is the best");
    assert!(!flags[0].windowing && !flags[3].windowing);
    assert_eq!(
        flags[2],
        BestFlags::default(),
        "an invalid row is never best"
    );
    // Extinction is 10 in every valid row: nothing stands out.
    assert!(!flags[0].extinction && !flags[1].extinction && !flags[3].extinction);
    assert!(flags[1].get(SweepMetric::Brilliance));
    assert!(!flags[0].get(SweepMetric::Windowing));
}

#[test]
fn the_metrics_know_their_columns_and_direction() {
    assert_eq!(SweepMetric::ALL.len(), 9);
    for (index, metric) in SweepMetric::ALL.into_iter().enumerate() {
        assert_eq!(metric.index(), index);
        assert_eq!(SweepMetric::from_index(index), Some(metric));
    }
    assert_eq!(SweepMetric::from_index(9), None);
    assert!(SweepMetric::Brilliance.higher_is_better());
    assert!(!SweepMetric::Windowing.higher_is_better());
    assert!(!SweepMetric::TiltExtinction.higher_is_better());
    assert!(SweepMetric::TiltBrilliance.is_tilt() && !SweepMetric::Yield.is_tilt());
    let valid = row(41.0, true, Some(figures(80.0, 3.0)));
    assert_eq!(SweepMetric::Brilliance.cell_text(&valid), "80.00");
    assert_eq!(SweepMetric::Yield.cell_text(&valid), "40.00");
    assert_eq!(SweepMetric::TiltBrilliance.cell_text(&valid), "-");
    assert_eq!(
        SweepMetric::Brilliance.cell_text(&row(1.0, false, None)),
        "-"
    );
}

#[test]
fn the_csv_has_a_header_fixed_precision_and_quotes_where_needed() {
    let mut refused = row(-40.0, false, None);
    refused.notes.push("It does not close, sorry".to_owned());
    let mut outcome = outcome_of(
        vec![row(-41.0, true, Some(figures(80.0, 3.0))), refused],
        false,
    );
    outcome.tier_name = "Pavilion \"main\", 1".to_owned();

    let csv = sweep_csv(&outcome);
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert_eq!(lines.len(), 4, "{csv:?}");
    assert_eq!(lines[3], "", "the last line ends with a line break");
    assert_eq!(
        lines[0],
        "tier,angle_deg,current,valid,brilliance_pct,windowing_pct,extinction_pct,\
         fire_index,scintillation_pct,yield_pct,facet_warnings,notes"
    );
    let tier = "\"Pavilion \"\"main\"\", 1\"";
    // The rows carry the stored negative angles; the file shows magnitudes, like the table.
    assert_eq!(
        lines[1],
        format!("{tier},41.0000,yes,yes,80.0000,3.0000,10.0000,20.0000,30.0000,40.0000,0,")
    );
    // Six empty figures, an empty warning count, then the quoted note.
    assert_eq!(
        lines[2],
        format!(
            "{tier},40.0000,no,no{}\"It does not close, sorry\"",
            ",".repeat(8)
        )
    );
}

#[test]
fn the_csv_uses_a_point_and_adds_the_tilt_columns_only_when_asked() {
    let mut with_tilt = figures(80.5, 3.25);
    with_tilt.tilt = Some(TiltAverages {
        brilliance_pct: 61.5,
        windowing_pct: 12.25,
        extinction_pct: 26.0,
    });
    let csv = sweep_csv(&outcome_of(vec![row(-41.0, true, Some(with_tilt))], true));
    let lines: Vec<&str> = csv.lines().collect();
    assert!(
        lines[0].contains("yield_pct,tilt_brilliance_pct,tilt_windowing_pct,tilt_extinction_pct,"),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains("80.5000,3.2500"), "{}", lines[1]);
    assert!(lines[1].contains("61.5000,12.2500,26.0000"), "{}", lines[1]);
    assert!(!lines[1].contains("80,5"));
    let plain = sweep_csv(&outcome_of(vec![row(-41.0, true, Some(with_tilt))], false));
    assert!(!plain.contains("tilt_"));
    // A sweep with no rows is the header alone.
    assert_eq!(sweep_csv(&outcome_of(Vec::new(), false)).lines().count(), 1);
}

// --- the chart -----------------------------------------------------------------------

fn three_rows() -> Vec<SweepRow> {
    vec![
        row(10.0, false, Some(figures(50.0, 1.0))),
        row(20.0, true, Some(figures(100.0, 3.0))),
        row(30.0, false, Some(figures(75.0, 2.0))),
    ]
}

#[test]
fn a_line_is_scaled_to_its_own_range_inside_the_margins() {
    let rows = three_rows();
    assert_eq!(
        chart_path(&rows, SweepMetric::Brilliance),
        "M 2.00 94.00 L 50.00 6.00 L 98.00 50.00"
    );
    // Windowing is lower-is-better but the line still just follows the value.
    assert_eq!(
        chart_path(&rows, SweepMetric::Windowing),
        "M 2.00 94.00 L 50.00 6.00 L 98.00 50.00"
    );
}

#[test]
fn a_constant_figure_is_a_line_across_the_middle() {
    let rows = three_rows();
    assert_eq!(
        chart_path(&rows, SweepMetric::Extinction),
        "M 2.00 50.00 L 50.00 50.00 L 98.00 50.00"
    );
}

#[test]
fn an_invalid_row_breaks_the_line_and_a_missing_figure_draws_nothing() {
    let mut rows = three_rows();
    rows[1].metrics = None;
    assert_eq!(
        chart_path(&rows, SweepMetric::Brilliance),
        "M 2.00 94.00 M 98.00 6.00"
    );
    assert_eq!(chart_path(&rows, SweepMetric::TiltBrilliance), "");
    assert_eq!(chart_path(&[], SweepMetric::Brilliance), "");
}

#[test]
fn rows_sit_on_the_x_axis_by_angle_and_the_pointer_finds_the_nearest() {
    let rows = three_rows();
    assert!(near(chart_x_percent(&rows, 0), 2.0));
    assert!(near(chart_x_percent(&rows, 1), 50.0));
    assert!(near(chart_x_percent(&rows, 2), 98.0));
    assert!(near(chart_x_percent(&rows, 9), 50.0));
    assert_eq!(current_x_percent(&rows), Some(50.0));
    assert_eq!(current_x_percent(&[row(1.0, false, None)]), None);
    assert!(near(chart_x_percent(&rows[..1], 0), 50.0));

    // A pavilion sweep stores negative angles, flattest first: the axis still runs from
    // the flattest on the left to the steepest on the right.
    let pavilion = vec![
        row(-40.0, false, Some(figures(50.0, 1.0))),
        row(-41.0, true, Some(figures(100.0, 3.0))),
        row(-42.0, false, Some(figures(75.0, 2.0))),
    ];
    assert!(near(chart_x_percent(&pavilion, 0), 2.0));
    assert!(near(chart_x_percent(&pavilion, 1), 50.0));
    assert!(near(chart_x_percent(&pavilion, 2), 98.0));
    assert_eq!(
        chart_path(&pavilion, SweepMetric::Brilliance),
        "M 2.00 94.00 L 50.00 6.00 L 98.00 50.00"
    );
    assert_eq!(nearest_row(&pavilion, 0.9), Some(2));

    assert_eq!(nearest_row(&rows, 0.0), Some(0));
    assert_eq!(nearest_row(&rows, 0.2), Some(0));
    assert_eq!(nearest_row(&rows, 0.3), Some(1));
    assert_eq!(nearest_row(&rows, 0.5), Some(1));
    assert_eq!(nearest_row(&rows, 1.0), Some(2));
    assert_eq!(nearest_row(&rows, 7.0), Some(2));
    assert_eq!(nearest_row(&[], 0.5), None);
}

#[test]
fn the_readout_names_the_angle_the_figures_and_why_a_row_is_not_valid() {
    let mut rows = three_rows();
    let metrics = [SweepMetric::Brilliance, SweepMetric::Windowing];
    assert_eq!(
        hover_text(&rows, 1, &metrics),
        "20.00\u{b0} (current): Brilliance 100.00 %, Windowing 3.00 %"
    );
    assert_eq!(hover_text(&rows, 0, &[]), "10.00\u{b0}");
    rows[2].metrics = None;
    rows[2].notes.push("It does not close.".to_owned());
    assert_eq!(
        hover_text(&rows, 2, &metrics),
        "30.00\u{b0}: not valid - It does not close."
    );
    assert_eq!(hover_text(&rows, 9, &metrics), "");
    // A pavilion row stores -41 and reads 41.
    let pavilion = vec![row(-41.0, true, Some(figures(80.0, 3.0)))];
    assert_eq!(
        hover_text(&pavilion, 0, &metrics),
        "41.00\u{b0} (current): Brilliance 80.00 %, Windowing 3.00 %"
    );
    assert_eq!(
        series_range_text(&rows, SweepMetric::Brilliance),
        "Brilliance 50.00 to 100.00 %"
    );
    assert_eq!(
        series_range_text(&rows, SweepMetric::Fire),
        "Fire 20.00 to 20.00"
    );
    assert_eq!(
        series_range_text(&rows, SweepMetric::TiltBrilliance),
        "Tilt brilliance: no values"
    );
}

// --- the form ------------------------------------------------------------------------

#[test]
fn the_default_range_is_five_degrees_either_side_as_magnitudes() {
    // A pavilion tier at -41 and a crown tier at 41 start with the same fields.
    assert_eq!(default_range(-41.0), (36.0, 46.0));
    assert_eq!(default_range(41.0), (36.0, 46.0));
    assert_eq!(default_range(34.5), (29.5, 39.5));
    assert_eq!(default_range(88.0), (83.0, 89.5));
    assert_eq!(default_range(2.0), (0.5, 7.0));
    assert_eq!(default_range(-2.0), (0.5, 7.0));
    // Whatever it returns is a range the plan takes.
    let design = rbc();
    let (from, to) = default_range(-41.0);
    assert!(
        plan_sweep(
            &design,
            PAVILION_MAIN,
            range(from, to, DEFAULT_SWEEP_STEP_DEG)
        )
        .is_ok()
    );
}

#[test]
fn angles_become_field_text_as_magnitudes_without_trailing_zeros() {
    assert_eq!(format_angle_input(-41.0), "41");
    assert_eq!(format_angle_input(34.5), "34.5");
    assert_eq!(format_angle_input(-41.099_999_999_999_994), "41.1");
}

#[test]
fn the_fields_take_arithmetic_and_leave_the_sign_to_the_plan() {
    let read = parse_sweep_range("46", "36 + 1", "0.25 * 2").expect("fields");
    assert_eq!(read, range(46.0, 37.0, 0.5));
    // A signed entry is read as typed; the plan reads its magnitude, so a signed and an
    // unsigned entry make the same plan for either side of the girdle.
    let signed = parse_sweep_range("-46", "-36 + 1", "0.5").expect("fields");
    assert_eq!(signed, range(-46.0, -35.0, 0.5));
    let design = rbc();
    let unsigned_plan = pavilion_plan(&design, 40.0, 42.0, 0.5);
    let typed = parse_sweep_range("-40", "-42", "0.5").expect("fields");
    assert_eq!(
        plan_sweep(&design, PAVILION_MAIN, typed).expect("a plan"),
        unsigned_plan
    );
    let typed = parse_sweep_range("40", "42", "0.5").expect("fields");
    assert_eq!(
        plan_sweep(&design, PAVILION_MAIN, typed).expect("a plan"),
        unsigned_plan
    );
}

#[test]
fn a_field_that_is_not_a_number_is_named_in_the_message() {
    assert_eq!(
        parse_sweep_range("x", "1", "1"),
        Err(SweepError::BadNumber(
            "From 'x' is not a number.".to_owned()
        ))
    );
    assert_eq!(
        parse_sweep_range("1", "", "1"),
        Err(SweepError::BadNumber("To '' is not a number.".to_owned()))
    );
    let Err(SweepError::BadNumber(message)) = parse_sweep_range("1", "1", "1 / 0") else {
        panic!("a division by zero is not a number");
    };
    assert!(
        message.starts_with("Step '1 / 0' cannot be calculated"),
        "{message}"
    );
}

#[test]
fn the_estimate_grows_with_rows_and_tilt_and_shrinks_with_workers() {
    let cost = SolveCostEstimate {
        planes: 100,
        meet_derived_tiers: 0,
        has_tier_targets: false,
    };
    let plain = estimate_seconds(20, false, 1, cost);
    assert!(plain > 0.0 && plain < 10.0, "{plain}");
    assert!(estimate_seconds(40, false, 1, cost) > plain);
    let tilt = estimate_seconds(20, true, 1, cost);
    assert!(tilt > plain + 20.0, "{tilt}");
    assert!(estimate_seconds(20, true, 4, cost) < tilt / 2.0);
    // A worker count of 0 is the same as 1, and tier targets cost more.
    assert!(near(estimate_seconds(20, false, 0, cost), plain));
    let targeted = SolveCostEstimate {
        has_tier_targets: true,
        ..cost
    };
    assert!(estimate_seconds(20, false, 1, targeted) > plain);
}

#[test]
fn durations_read_as_a_rough_phrase() {
    assert_eq!(format_duration_estimate(0.4), "under a second");
    assert_eq!(format_duration_estimate(1.0), "about 1 second");
    assert_eq!(format_duration_estimate(4.4), "about 4 seconds");
    assert_eq!(format_duration_estimate(12.0), "about 10 seconds");
    assert_eq!(format_duration_estimate(33.0), "about 35 seconds");
    assert_eq!(format_duration_estimate(95.0), "about 2 minutes");
    assert_eq!(format_duration_estimate(200.0), "about 3 minutes");
}

#[test]
fn the_summary_names_the_count_and_the_range() {
    let design = rbc();
    let plan = pavilion_plan(&design, -43.0, -39.0, 1.0);
    assert_eq!(
        plan_summary(&plan, "about 3 seconds"),
        "5 angles from 39.00 to 43.00 degrees. This takes about 3 seconds."
    );
}

#[test]
fn the_default_worker_count_is_at_least_one() {
    let workers = default_worker_count();
    assert!(workers >= 1);
    assert!(SweepOptions::default().workers >= 1);
    assert!(!SweepOptions::default().tilt_average);
}
