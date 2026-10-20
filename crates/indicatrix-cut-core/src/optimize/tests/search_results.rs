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
fn default_request_matches_plain_search_and_flat_depths_can_be_unlocked() {
    let imported = imported_rbc();
    let material = GemMaterial::diamond();
    let config = small_config(4);
    let plain = optimize_design(&imported, &material, &config, &SearchHooks::default()).unwrap();
    let default = optimize_design_with(
        &imported,
        &material,
        &config,
        &OptimizeOptions::default(),
        &SearchHooks::default(),
    )
    .unwrap();
    assert_eq!(default.outcome, plain);
    assert_eq!(plain.evaluations, 0);
    // No hinge is needed for a flat depth; all independent angles remain locked.
    let options = OptimizeOptions {
        vary_anchored: true,
        ..Default::default()
    };
    let result = optimize_design_with(
        &imported,
        &material,
        &config,
        &options,
        &SearchHooks::default(),
    )
    .unwrap();
    assert!(result.outcome.evaluations > 0);
    assert_eq!(result.outcome.changes.len(), 0);
    for candidate in result.candidates {
        assert_eq!(candidate.changes.len(), 0);
        assert_ne!(candidate.mast_changes.len(), 0);
        let accepted = crate::optimize::materialize_candidate(&imported, &candidate)
            .unwrap()
            .0;
        crate::meet_preservation::MeetPreservationBaseline::capture(&imported)
            .unwrap()
            .validate(
                &accepted,
                crate::meet_preservation::PreservationPolicy::OPTIMIZE,
            )
            .unwrap();
    }
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
        assert!(!candidate.changes.is_empty() || !candidate.mast_changes.is_empty());
    }

    let free = free_tier_indices_with(&run.design, &run.options);
    let states: Vec<Vec<f64>> = candidates
        .iter()
        .map(|c| state_of(&run.design, &free, c))
        .collect();
    for (i, a) in states.iter().enumerate() {
        for (j, b) in states.iter().enumerate().skip(i + 1) {
            let apart = a.iter().zip(b).any(|(x, y)| (x - y).abs() >= 0.5 - 1e-9);
            let depth_apart = (0..run.design.tiers.len()).any(|tier| {
                let mast = |candidate: &OptimizeCandidate| {
                    candidate
                        .mast_changes
                        .iter()
                        .find(|m| m.index == tier)
                        .map_or_else(|| pinned_mast(&run.design, tier), |m| m.to_mast)
                };
                (mast(&candidates[i]) - mast(&candidates[j])).abs() > 1e-9
            });
            assert!(
                apart || depth_apart,
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

/// Angles and depths are independent variables of the projection: a flat pinned tier (the
/// table) is a depth variable with no angle, and a projected tier may keep its angle while
/// its depth moves. So the old "every turned tier has a mast change and only those" is not
/// an invariant any more; the invariant is that a depth only ever moves on a tier the
/// projection was allowed to move, from the mast the design pinned.
#[test]
fn every_mast_change_is_on_a_movable_tier_and_starts_from_the_pinned_mast() {
    let run = anchored_run();
    let movable = crate::optimize::constrained::movable_masts(&run.design, &run.options);
    for candidate in &run.result.candidates {
        for mast in &candidate.mast_changes {
            assert!(
                movable.contains(&mast.index),
                "tier {} has a mast change but its depth is locked",
                mast.index
            );
            assert_eq!(
                mast.from_mast.to_bits(),
                pinned_mast(&run.design, mast.index).to_bits()
            );
            assert_ne!(mast.from_mast.to_bits(), mast.to_mast.to_bits());
        }
    }
}

/// What an applied candidate guarantees is the projection's contract, not a hinge
/// incidence: the whole original meet network survives within the policy. The hinge only
/// seeds the proposal; the projection moves every variable (the junction that was the
/// hinge may travel up to `max_junction_displacement`), so "the first facet still passes
/// through its hinge" is no longer an invariant.
#[test]
fn applied_candidates_keep_the_original_meets_within_the_policy() {
    let run = anchored_run();
    let baseline = crate::meet_preservation::MeetPreservationBaseline::capture(&run.design)
        .expect("the fixture's network is captured");
    let policy = run.options.preservation_policy;
    for candidate in &run.result.candidates {
        let mut design = run.design.clone();
        let mut history = History::new();
        apply_optimize_candidate(&mut history, &mut design, candidate)
            .expect("a candidate applies to the design it was computed from");
        let facts = baseline
            .validate(&design, policy)
            .expect("an applied candidate keeps every original meet");
        assert!(
            facts.shape_distance <= policy.max_junction_displacement,
            "junctions travelled {} of the stone, limit {}",
            facts.shape_distance,
            policy.max_junction_displacement
        );
        assert!(
            facts.max_normalized_residual <= 2e-6,
            "meet residual {}",
            facts.max_normalized_residual
        );
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
    // The count is of tiers, not of angle changes: a tier whose depth moves while its angle
    // stays (the table, which the turned facets drag along) is one more modified tier, and a
    // tier with both an angle and a depth change is one.
    let listed: std::collections::BTreeSet<usize> = run
        .result
        .outcome
        .changes
        .iter()
        .map(|change| change.index)
        .chain(run.result.mast_changes.iter().map(|mast| mast.index))
        .collect();
    assert_eq!(touched, listed.len());
    let differing = design
        .tiers
        .iter()
        .zip(&run.design.tiers)
        .filter(|(now, before)| now != before)
        .count();
    assert_eq!(
        touched, differing,
        "exactly the tiers that differ from the original were touched"
    );
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

#[test]
fn a_candidate_that_no_longer_validates_is_dropped_not_fatal() {
    let run = anchored_run();
    assert!(
        !run.result.candidates.is_empty(),
        "the fixture run must keep candidates"
    );
    let baseline =
        crate::meet_preservation::MeetPreservationBaseline::capture(&run.design).unwrap();
    // No junction may travel at all: every changed candidate fails its re-validation,
    // while the unchanged design still validates.
    let mut options = run.options.clone();
    options.preservation_policy = crate::meet_preservation::PreservationPolicy {
        max_junction_displacement: 0.0,
        ..crate::meet_preservation::PreservationPolicy::OPTIMIZE
    };
    let mut result = run.result.clone();
    crate::optimize::provenance::stamp_result(
        &run.design,
        &GemMaterial::diamond(),
        &run.config,
        &options,
        &baseline,
        &mut result,
    )
    .expect("a failing rival must not become a solver error");
    assert_eq!(result.candidates.len(), 0);
    assert_eq!(result.outcome.changes.len(), 0);
    assert_eq!(result.mast_changes.len(), 0);
    assert_eq!(
        result.outcome.after_score.to_bits(),
        result.outcome.before_score.to_bits()
    );
    assert!(
        result.outcome.provenance.is_some(),
        "the unchanged design is still stamped"
    );
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
fn cancellation_before_baseline_capture_returns_an_explicit_cancelled_error() {
    let design = imported_rbc();
    let options = bounded_anchored_options(&design, 3);
    let material = GemMaterial::diamond();
    let cancel = AtomicBool::new(true);
    let hooks = SearchHooks {
        cancel: Some(&cancel),
        on_progress: None,
        on_start: None,
    };
    let result = optimize_design_with(&design, &material, &small_config(30), &options, &hooks);
    assert!(matches!(
        result,
        Err(crate::DesignSolveError::Preservation(
            crate::meet_preservation::PreservationViolation::Cancelled
        ))
    ));
}

// --- applying results and candidates (no search) ---

fn dummy_outcome(changes: Vec<AngleChange>) -> OptimizeOutcome {
    let components = ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 10.0,
        tilt_brilliance_pct: 80.0,
    };
    OptimizeOutcome {
        provenance: None,
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
fn an_uncertified_angle_and_depth_result_is_rejected() {
    let original = imported_rbc();
    let result = result_for(&original, 0, 1.25, 0.015);
    let mut design = original.clone();
    let mut history = History::new();
    assert!(apply_optimize_result(&mut history, &mut design, &result).is_err());
    assert_eq!(design, original);
    assert!(!history.can_undo());
}

#[test]
fn an_uncertified_ranked_alternative_is_rejected() {
    let original = imported_rbc();
    let result = result_for(&original, 8, -0.75, -0.01);
    let candidate = OptimizeCandidate {
        provenance: None,
        preservation_baseline: None,
        changes: result.outcome.changes.clone(),
        mast_changes: result.mast_changes.clone(),
        after: result.outcome.after,
        score: result.outcome.after_score,
        yield_loss_pct: 0.0,
        tone: None,
    };
    let mut design = original.clone();
    let mut history = History::new();
    assert!(apply_optimize_candidate(&mut history, &mut design, &candidate).is_err());
    assert_eq!(design, original);
    assert!(!history.can_undo());
}

#[test]
fn an_uncertified_mast_only_result_is_rejected() {
    let original = imported_rbc();
    let mut result = result_for(&original, 0, 0.0, 0.02);
    result.outcome.changes.clear();
    let mut design = original.clone();
    let mut history = History::new();
    assert!(apply_optimize_result(&mut history, &mut design, &result).is_err());
    assert_eq!(design, original);
    assert!(!history.can_undo());
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
    let original = design.clone();
    let result = optimize_design_with(
        &design,
        &GemMaterial::diamond(),
        &small_config(0),
        &OptimizeOptions::default(),
        &SearchHooks::default(),
    )
    .expect("a fully pinned source returns a certified unchanged result");
    assert_eq!(result.outcome.changes.len(), 0);
    assert_eq!(result.mast_changes.len(), 0);
    let mut history = History::new();
    assert_eq!(
        apply_optimize_result(&mut history, &mut design, &result).expect("applies"),
        0
    );
    assert_eq!(design, original);
    assert!(!history.undo(&mut design).expect("undo"));
}

#[test]
fn an_uncertified_outcome_cannot_apply_only_the_angles() {
    let original = imported_rbc();
    let result = result_for(&original, 0, 1.0, 0.05);
    let mut design = original.clone();
    let mut history = History::new();
    assert!(apply_optimize_outcome(&mut history, &mut design, &result.outcome).is_err());
    assert_eq!(design, original);
    assert!(!history.can_undo());
}

#[test]
fn a_certified_outcome_applies_the_same_masts_as_the_rich_result() {
    let run = anchored_run();
    assert!(
        !run.result.mast_changes.is_empty(),
        "the regression must exercise depth changes"
    );
    let mut rich = run.design.clone();
    let mut legacy = run.design.clone();
    let mut rich_history = History::new();
    let mut legacy_history = History::new();
    let rich_count = apply_optimize_result(&mut rich_history, &mut rich, &run.result)
        .expect("certified rich result");
    let legacy_count =
        apply_optimize_outcome(&mut legacy_history, &mut legacy, &run.result.outcome)
            .expect("certified portable outcome");
    assert_eq!(legacy_count, rich_count);
    assert_eq!(legacy, rich);
    assert!(legacy_count > 0);
    assert!(legacy_history.undo(&mut legacy).unwrap());
    assert_eq!(legacy, run.design);
    assert!(!legacy_history.can_undo());
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
