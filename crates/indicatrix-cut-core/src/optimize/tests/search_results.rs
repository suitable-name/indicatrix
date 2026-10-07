//! Tests for [`super::super::optimize_design_with`] and its results: several
//! candidates, anchored tiers, bounds and guards end to end, plus applying results
//! and candidates through `History`.
//!
//! The end-to-end runs score yield only (all three optical weights zero). The yield is
//! pure geometry, so the fast score the search accepts on and the full score the result
//! is gated on agree exactly, and a run is certain to find a better design: some angle
//! always has a direction that keeps more of the rough.

use super::{
    super::{
        AngleChange, MastChange, ObjectiveComponents, ObjectiveWeights, OptimizeCandidate,
        OptimizeConfig, OptimizeOptions, OptimizeOutcome, OptimizeResult, SearchHooks, SearchStage,
        apply_optimize_candidate, apply_optimize_outcome, apply_optimize_result, candidate,
        free_tier_indices_with, guard, optimize_design, optimize_design_with,
    },
    anchored::{anchored_options, hinge_points},
    fixtures::{imported_rbc, rbc_445},
};
use crate::{
    design::{Design, tier_plane_ranges},
    edit::History,
};
use indicatrix::{
    geometry::{
        meet_solver::MeetConstraint,
        stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};
use std::sync::{OnceLock, atomic::AtomicBool};

/// How far from its starting angle a bounded tier may go, in degrees.
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

fn small_config(max_evaluations: usize) -> OptimizeConfig {
    OptimizeConfig {
        weights: yield_only(),
        seed: 7,
        max_evaluations,
        polish_start_step_deg: None,
        ..OptimizeConfig::default()
    }
}

/// Anchored options for `design` with every free tier bounded to `BOUND_DEG` either
/// side of its starting angle, keeping `keep` candidates half a degree apart.
fn bounded_anchored_options(design: &Design, keep: usize) -> OptimizeOptions {
    let mut options = anchored_options(design);
    options.keep_candidates = keep;
    options.candidate_separation_deg = Some(0.5);
    for index in free_tier_indices_with(design, &options) {
        let angle = design.tiers[index].angle_deg;
        options
            .angle_bounds
            .insert(index, (angle - BOUND_DEG, angle + BOUND_DEG));
    }
    options
}

fn pinned_mast(design: &Design, index: usize) -> f64 {
    let MeetConstraint::ScaleReference(mast) = design.tiers[index].constraint else {
        panic!("tier {index} must be a ScaleReference tier");
    };
    mast
}

/// Every free tier's angle in the state `candidate` describes, in free-tier order.
fn state_of(design: &Design, free: &[usize], candidate: &OptimizeCandidate) -> Vec<f64> {
    free.iter()
        .map(|&index| {
            candidate
                .changes
                .iter()
                .find(|change| change.index == index)
                .map_or(design.tiers[index].angle_deg, |change| change.to_deg)
        })
        .collect()
}

// --- nothing free: both entry points agree ---

#[test]
fn the_default_request_is_optimize_design_and_vary_anchored_needs_hinges() {
    let imported = imported_rbc();
    let material = GemMaterial::diamond();
    let config = OptimizeConfig::default();
    let plain = optimize_design(&imported, &material, &config, &SearchHooks::default())
        .expect("imported design must solve");
    // vary_anchored without a single hinge frees nothing, so nothing is searched.
    let options = OptimizeOptions {
        vary_anchored: true,
        keep_candidates: 5,
        ..OptimizeOptions::default()
    };
    let result = optimize_design_with(
        &imported,
        &material,
        &config,
        &options,
        &SearchHooks::default(),
    )
    .expect("imported design must solve");
    assert_eq!(result.outcome, plain);
    assert_eq!(result.outcome.evaluations, 0);
    assert_eq!(result.mast_changes.len(), 0);
    assert_eq!(result.candidates.len(), 0);
    assert!(result.best_candidate().is_none());
}

// --- one anchored run, checked from several sides ---

struct AnchoredRun {
    design: Design,
    options: OptimizeOptions,
    config: OptimizeConfig,
    result: OptimizeResult,
    final_full_reports: usize,
}

fn run_anchored(
    design: &Design,
    options: &OptimizeOptions,
    config: &OptimizeConfig,
) -> (OptimizeResult, usize) {
    let material = GemMaterial::diamond();
    let final_full_reports = std::cell::Cell::new(0usize);
    let on_progress = |_evaluations: usize, stage: SearchStage| {
        if stage == SearchStage::FinalFull {
            final_full_reports.set(final_full_reports.get() + 1);
        }
    };
    let hooks = SearchHooks {
        cancel: None,
        on_progress: Some(&on_progress),
        on_start: None,
    };
    let result = optimize_design_with(design, &material, config, options, &hooks)
        .expect("the imported design must solve");
    (result, final_full_reports.get())
}

fn anchored_config() -> OptimizeConfig {
    OptimizeConfig {
        polish_start_step_deg: Some(0.5),
        polish_max_evaluations: Some(12),
        ..small_config(30)
    }
}

/// The run every test below inspects, done once: an imported design (every tier
/// pinned), anchored tiers varied about their hinges, each within one degree, three
/// candidates wanted.
fn anchored_run() -> &'static AnchoredRun {
    static RUN: OnceLock<AnchoredRun> = OnceLock::new();
    RUN.get_or_init(|| {
        let design = imported_rbc();
        let options = bounded_anchored_options(&design, 3);
        let config = anchored_config();
        let (result, final_full_reports) = run_anchored(&design, &options, &config);
        AnchoredRun {
            design,
            options,
            config,
            result,
            final_full_reports,
        }
    })
}

#[test]
fn an_imported_design_improves_once_anchored_tiers_may_vary() {
    let run = anchored_run();
    let outcome = &run.result.outcome;
    assert!(outcome.evaluations > 0);
    assert!(
        !outcome.changes.is_empty(),
        "yield can always be improved: some anchored tier must have moved"
    );
    assert!(outcome.after_score <= outcome.before_score);
    assert!(
        outcome.after_yield_loss_pct <= outcome.before_yield_loss_pct,
        "the yield is the objective here"
    );
}

#[test]
fn the_best_candidate_is_the_plain_outcome() {
    let run = anchored_run();
    let best = run
        .result
        .best_candidate()
        .expect("a run that changed something has a best candidate");
    assert_eq!(best.changes, run.result.outcome.changes);
    assert_eq!(best.mast_changes, run.result.mast_changes);
    assert_eq!(best.after, run.result.outcome.after);
    assert_eq!(best.score, run.result.outcome.after_score);
    assert_eq!(best.yield_loss_pct, run.result.outcome.after_yield_loss_pct);
}

#[test]
fn candidates_are_ranked_bounded_and_distinct() {
    let run = anchored_run();
    let candidates = &run.result.candidates;
    assert!(
        (1..=3).contains(&candidates.len()),
        "got {}",
        candidates.len()
    );
    for pair in candidates.windows(2) {
        assert!(
            pair[0].score <= pair[1].score,
            "candidates must be best first"
        );
    }
    for candidate in candidates {
        assert!(candidate.score <= run.result.outcome.before_score);
        assert_ne!(candidate.changes.len(), 0);
    }

    let free = free_tier_indices_with(&run.design, &run.options);
    let states: Vec<Vec<f64>> = candidates
        .iter()
        .map(|c| state_of(&run.design, &free, c))
        .collect();
    for (i, a) in states.iter().enumerate() {
        for b in &states[i + 1..] {
            let apart = a.iter().zip(b).any(|(x, y)| (x - y).abs() >= 0.5 - 1e-9);
            assert!(
                apart,
                "two candidates differ by less than the separation: {a:?} {b:?}"
            );
        }
    }
}

#[test]
fn every_scoring_of_the_result_reports_the_final_stage() {
    let run = anchored_run();
    assert!(
        run.final_full_reports >= run.result.candidates.len(),
        "{} FinalFull reports for {} candidates",
        run.final_full_reports,
        run.result.candidates.len()
    );
    assert!(
        run.final_full_reports <= 3,
        "no more than one per kept candidate"
    );
}

#[test]
fn only_movable_free_tiers_change_and_bounds_hold() {
    let run = anchored_run();
    let free = free_tier_indices_with(&run.design, &run.options);
    for kept in &run.result.candidates {
        for change in &kept.changes {
            assert!(
                free.contains(&change.index),
                "tier {} is not free",
                change.index
            );
            assert!(candidate::angle_is_variable(change.from_deg));
            assert!(
                (change.to_deg - change.from_deg).abs() <= BOUND_DEG + 1e-9,
                "tier {} moved {} -> {}, past its bound",
                change.index,
                change.from_deg,
                change.to_deg
            );
            assert_eq!(
                change.to_deg.is_sign_negative(),
                change.from_deg.is_sign_negative(),
                "tier {} changed side",
                change.index
            );
        }
    }
    // The table (tier 11) and the girdle facets (2, 3) are in no change at all.
    for fixed in [2usize, 3, 11] {
        assert!(
            run.result
                .candidates
                .iter()
                .all(|c| c.changes.iter().all(|change| change.index != fixed)),
            "tier {fixed} must never move"
        );
    }
}

#[test]
fn every_turned_tier_has_its_mast_change_and_only_those() {
    let run = anchored_run();
    for candidate in &run.result.candidates {
        let angle_tiers: Vec<usize> = candidate.changes.iter().map(|c| c.index).collect();
        let mast_tiers: Vec<usize> = candidate.mast_changes.iter().map(|m| m.index).collect();
        assert_eq!(angle_tiers, mast_tiers);
        for mast in &candidate.mast_changes {
            assert_eq!(
                mast.from_mast.to_bits(),
                pinned_mast(&run.design, mast.index).to_bits()
            );
            assert_ne!(mast.from_mast.to_bits(), mast.to_mast.to_bits());
        }
    }
}

/// The numeric heart of the feature: after a candidate is applied, each tier it turned
/// still has its first facet through the hinge it turns about.
#[test]
fn applied_candidates_keep_every_changed_facet_on_its_hinge() {
    let run = anchored_run();
    for candidate in &run.result.candidates {
        let mut design = run.design.clone();
        let mut history = History::new();
        apply_optimize_candidate(&mut history, &mut design, candidate)
            .expect("a candidate applies to the design it was computed from");
        let solved = design.solve().expect("the applied design solves");
        let planes = design.planes_from_solved(&solved);
        let ranges = tier_plane_ranges(&design, &solved);
        for mast in &candidate.mast_changes {
            let hinge = run.options.anchor_hinges[&mast.index];
            assert!(!ranges[mast.index].is_empty());
            let (normal, offset) = planes[ranges[mast.index].start];
            assert!(
                (normal.dot(hinge) - offset).abs() < 1e-5,
                "tier {}: n.hinge {} vs offset {offset}",
                mast.index,
                normal.dot(hinge)
            );
        }
        assert!(design.is_closed(), "an applied candidate closes");
    }
}

#[test]
fn applying_the_result_is_one_exact_undo_step() {
    let run = anchored_run();
    let mut design = run.design.clone();
    let mut history = History::new();
    let touched = apply_optimize_result(&mut history, &mut design, &run.result)
        .expect("the result applies to the design it was computed from");
    assert_eq!(touched, run.result.outcome.changes.len());
    assert_ne!(design, run.design);
    for change in &run.result.outcome.changes {
        assert_eq!(design.tiers[change.index].angle_deg, change.to_deg);
    }
    for mast in &run.result.mast_changes {
        assert_eq!(pinned_mast(&design, mast.index), mast.to_mast);
    }

    assert!(history.undo(&mut design).expect("undo"));
    assert_eq!(
        design, run.design,
        "one undo restores angles and masts exactly"
    );
    assert!(!history.undo(&mut design).expect("undo"), "it was one step");
    assert!(history.redo(&mut design).expect("redo"));
    for mast in &run.result.mast_changes {
        assert_eq!(pinned_mast(&design, mast.index), mast.to_mast);
    }
}

#[test]
fn the_same_inputs_give_the_same_result_candidates_and_masts_included() {
    let run = anchored_run();
    let (again, _) = run_anchored(&run.design, &run.options, &run.config);
    assert_eq!(again, run.result);
}

// --- the girdle guard, end to end ---

#[test]
fn a_guarded_run_returns_only_candidates_that_keep_the_girdle_and_the_table() {
    let design = imported_rbc();
    let mut options = bounded_anchored_options(&design, 2);
    options.min_girdle_fraction = Some(0.5);
    let config = small_config(16);
    let (result, _) = run_anchored(&design, &options, &config);

    let solved = design.solve().expect("fixture must solve");
    let baseline_planes = design.planes_from_solved(&solved);
    let baseline_girdle = measure_solid(&baseline_planes)
        .and_then(|m| m.girdle_thickness)
        .expect("RBC-445 has a live girdle band");
    let table_ranges = tier_plane_ranges(&design, &solved);

    for candidate in &result.candidates {
        let mut applied = design.clone();
        let mut history = History::new();
        apply_optimize_candidate(&mut history, &mut applied, candidate).expect("applies");
        let applied_solved = applied.solve().expect("solves");
        let planes = applied.planes_from_solved(&applied_solved);
        let girdle = measure_solid(&planes)
            .and_then(|m| m.girdle_thickness)
            .expect("the girdle band survives");
        assert!(
            girdle >= 0.5_f64.mul_add(baseline_girdle, -1e-9),
            "girdle {girdle} thinner than half of {baseline_girdle}"
        );
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            panic!("an applied candidate must close");
        };
        assert!(
            guard::ring_present(&mesh, &table_ranges[11]),
            "the table facet must survive"
        );
    }
}

// --- cancelling ---

#[test]
fn a_cancelled_run_scores_its_end_point_only_and_proposes_nothing_it_did_not_reach() {
    let design = imported_rbc();
    let options = bounded_anchored_options(&design, 3);
    let material = GemMaterial::diamond();
    let cancel = AtomicBool::new(true);
    let hooks = SearchHooks {
        cancel: Some(&cancel),
        on_progress: None,
        on_start: None,
    };
    let result = optimize_design_with(&design, &material, &small_config(30), &options, &hooks)
        .expect("fixture must solve");
    assert!(result.outcome.cancelled);
    assert_eq!(result.outcome.evaluations, 0);
    assert_eq!(result.outcome.changes.len(), 0);
    assert_eq!(result.candidates.len(), 0);
}

// --- applying results and candidates (no search) ---

fn dummy_outcome(changes: Vec<AngleChange>) -> OptimizeOutcome {
    let components = ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 10.0,
        tilt_brilliance_pct: 80.0,
    };
    OptimizeOutcome {
        before: components,
        before_score: 10.0,
        before_yield_loss_pct: 0.0,
        after: components,
        after_score: 5.0,
        after_yield_loss_pct: 0.0,
        evaluations: 4,
        changes,
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    }
}

fn result_for(design: &Design, index: usize, delta_deg: f64, mast_delta: f64) -> OptimizeResult {
    let from_deg = design.tiers[index].angle_deg;
    let from_mast = pinned_mast(design, index);
    OptimizeResult {
        outcome: dummy_outcome(vec![AngleChange {
            index,
            from_deg,
            to_deg: from_deg + delta_deg,
        }]),
        mast_changes: vec![MastChange {
            index,
            from_mast,
            to_mast: from_mast + mast_delta,
        }],
        candidates: Vec::new(),
        tone_before: None,
        tone_goal: None,
        lighting: crate::optimize::CANONICAL_LIGHTING_PRESET,
        starts_run: 1,
        best_start: 0,
    }
}

#[test]
fn apply_optimize_result_moves_the_angle_and_the_mast_in_one_undo_step() {
    let original = imported_rbc();
    let result = result_for(&original, 0, 1.25, 0.015);
    let mut design = original.clone();
    let mut history = History::new();
    let touched = apply_optimize_result(&mut history, &mut design, &result).expect("applies");
    assert_eq!(touched, 1, "angle and mast of one tier are one edit");
    assert_eq!(
        design.tiers[0].angle_deg,
        original.tiers[0].angle_deg + 1.25
    );
    assert_eq!(pinned_mast(&design, 0), pinned_mast(&original, 0) + 0.015);
    assert!(history.undo(&mut design).expect("undo"));
    assert_eq!(design, original);
    assert!(!history.undo(&mut design).expect("undo"));
}

#[test]
fn apply_optimize_candidate_applies_a_ranked_alternative() {
    let original = imported_rbc();
    let result = result_for(&original, 8, -0.75, -0.01);
    let candidate = OptimizeCandidate {
        changes: result.outcome.changes.clone(),
        mast_changes: result.mast_changes.clone(),
        after: result.outcome.after,
        score: result.outcome.after_score,
        yield_loss_pct: 0.0,
        tone: None,
    };
    let mut design = original.clone();
    let mut history = History::new();
    assert_eq!(
        apply_optimize_candidate(&mut history, &mut design, &candidate).expect("applies"),
        1
    );
    assert_eq!(
        design.tiers[8].angle_deg,
        original.tiers[8].angle_deg - 0.75
    );
    assert!(history.undo(&mut design).expect("undo"));
    assert_eq!(design, original);
}

#[test]
fn a_mast_change_alone_applies() {
    let original = imported_rbc();
    let mut result = result_for(&original, 0, 0.0, 0.02);
    result.outcome.changes.clear();
    let mut design = original.clone();
    let mut history = History::new();
    assert_eq!(
        apply_optimize_result(&mut history, &mut design, &result).expect("applies"),
        1
    );
    assert_eq!(design.tiers[0].angle_deg, original.tiers[0].angle_deg);
    assert_eq!(pinned_mast(&design, 0), pinned_mast(&original, 0) + 0.02);
}

#[test]
fn a_stale_mast_is_rejected_without_touching_the_design() {
    let original = imported_rbc();
    let mut result = result_for(&original, 0, 1.0, 0.01);
    result.mast_changes[0].from_mast += 1e-9;
    let mut design = original.clone();
    let mut history = History::new();
    let error = apply_optimize_result(&mut history, &mut design, &result)
        .expect_err("a mast that no longer matches is stale");
    assert_eq!(error.index, 0);
    assert_eq!(design, original, "all or nothing");
    assert!(!history.undo(&mut design).expect("undo"));
}

#[test]
fn a_mast_change_on_a_tier_that_is_not_anchored_is_rejected() {
    // Tier 1 of the meet-structure fixture is derived from a meet, not a ScaleReference.
    let original = rbc_445();
    assert!(!matches!(
        original.tiers[1].constraint,
        MeetConstraint::ScaleReference(_)
    ));
    let result = OptimizeResult {
        outcome: dummy_outcome(Vec::new()),
        mast_changes: vec![MastChange {
            index: 1,
            from_mast: 0.5,
            to_mast: 0.6,
        }],
        candidates: Vec::new(),
        tone_before: None,
        tone_goal: None,
        lighting: crate::optimize::CANONICAL_LIGHTING_PRESET,
        starts_run: 1,
        best_start: 0,
    };
    let mut design = original.clone();
    let mut history = History::new();
    assert!(apply_optimize_result(&mut history, &mut design, &result).is_err());
    assert_eq!(design, original);
}

#[test]
fn a_mast_change_past_the_last_tier_is_rejected() {
    let original = imported_rbc();
    let tier_count = original.tiers.len();
    let result = OptimizeResult {
        outcome: dummy_outcome(Vec::new()),
        mast_changes: vec![MastChange {
            index: tier_count + 2,
            from_mast: 0.5,
            to_mast: 0.6,
        }],
        candidates: Vec::new(),
        tone_before: None,
        tone_goal: None,
        lighting: crate::optimize::CANONICAL_LIGHTING_PRESET,
        starts_run: 1,
        best_start: 0,
    };
    let mut design = original;
    let mut history = History::new();
    let error =
        apply_optimize_result(&mut history, &mut design, &result).expect_err("no such tier");
    assert_eq!(error.index, tier_count + 2);
    assert_eq!(error.tier_count, tier_count);
}

#[test]
fn an_empty_result_applies_nothing_and_records_no_edit() {
    let mut design = imported_rbc();
    let result = OptimizeResult {
        outcome: dummy_outcome(Vec::new()),
        mast_changes: Vec::new(),
        candidates: Vec::new(),
        tone_before: None,
        tone_goal: None,
        lighting: crate::optimize::CANONICAL_LIGHTING_PRESET,
        starts_run: 1,
        best_start: 0,
    };
    let mut history = History::new();
    assert_eq!(
        apply_optimize_result(&mut history, &mut design, &result).expect("applies"),
        0
    );
    assert!(!history.undo(&mut design).expect("undo"));
}

/// `apply_optimize_outcome` keeps applying angles only, as before.
#[test]
fn apply_optimize_outcome_still_leaves_masts_alone() {
    let original = imported_rbc();
    let result = result_for(&original, 0, 1.0, 0.05);
    let mut design = original.clone();
    let mut history = History::new();
    apply_optimize_outcome(&mut history, &mut design, &result.outcome).expect("applies");
    assert_eq!(design.tiers[0].angle_deg, original.tiers[0].angle_deg + 1.0);
    assert_eq!(pinned_mast(&design, 0), pinned_mast(&original, 0));
}

// --- the hinge helper agrees with the search space about what is free ---

#[test]
fn the_run_options_hold_a_hinge_for_every_tier_the_run_may_change() {
    let run = anchored_run();
    let hinges = hinge_points(&run.design);
    assert_eq!(run.options.anchor_hinges, hinges);
    assert_eq!(
        free_tier_indices_with(&run.design, &run.options),
        hinges.keys().copied().collect::<Vec<_>>()
    );
}
