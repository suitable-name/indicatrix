//! Tests for [`super::super::candidate`]'s per-candidate machinery:
//! `select_candidate_directions`, `evaluate_survivors`, and
//! `build_free_angle_candidate`.

use super::{
    super::{
        CANONICAL_LIGHTING_PRESET, ObjectiveFidelity, ObjectiveWeights, OptimizeConfig, candidate,
        evaluate_objective, evaluate_objective_under, free_tier_indices, objective,
        space::SearchSpace,
    },
    fixtures::rbc_445,
};
use crate::manufacturability::{DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability};
use indicatrix::optics::{materials::GemMaterial, raytracer::LightingPreset};

/// The equal-weights objective every search test here scores with.
const EQUAL_WEIGHTS: ObjectiveWeights = ObjectiveWeights {
    windowing: 1.0,
    extinction: 1.0,
    tilt_brilliance: 1.0,
    yield_weight: 0.0,
    tone_weight: 0.0,
    tone_goal: crate::optimize::ToneGoal::Lighter,
};

// --- select_candidate_directions / evaluate_survivors ---

#[test]
fn select_candidate_directions_keeps_both_when_both_are_safe() {
    let design = rbc_445();
    // Tier 7 ("A"): angle 31.0 -- +/-1 degree stays on the same side and well clear
    // of the vertical bound, so both directions survive.
    let survivors =
        candidate::select_candidate_directions(&design, 7, 31.0, 1.0, &SearchSpace::default());
    let mut degs: Vec<f64> = survivors.iter().map(|(deg, _)| *deg).collect();
    degs.sort_by(f64::total_cmp);
    assert_eq!(degs, vec![30.0, 32.0]);
}

#[test]
fn select_candidate_directions_keeps_only_the_safe_direction() {
    let design = rbc_445();
    // Tier 0: angle -50.4 -- "+45" (-5.4) stays clear of both bounds, "-45" (-95.4)
    // passes the vertical bound, so only the plus direction survives.
    let survivors =
        candidate::select_candidate_directions(&design, 0, -50.4, 45.0, &SearchSpace::default());
    assert_eq!(survivors.len(), 1);
    assert!((survivors[0].0 - (-5.4)).abs() < 1e-9);
}

#[test]
fn select_candidate_directions_is_empty_when_both_directions_are_unsafe() {
    let design = rbc_445();
    // Tier 0: angle -50.4 -- "+200" (149.6) crosses the zero boundary AND passes the
    // vertical bound; "-200" (-250.4) passes the vertical bound too. Neither survives.
    let survivors =
        candidate::select_candidate_directions(&design, 0, -50.4, 200.0, &SearchSpace::default());
    assert_eq!(survivors.len(), 0);
}

/// The pre-rejected case (both directions unsafe, see
/// `select_candidate_directions_is_empty_when_both_directions_are_unsafe`) must
/// short-circuit to the same empty evaluation a fully-evaluated-but-rejected pair
/// would produce -- without ever entering `std::thread::scope`, since there is
/// nothing left to evaluate.
#[test]
fn evaluate_survivors_with_no_survivors_needs_no_thread_scope_and_matches_the_old_result() {
    let material = GemMaterial::diamond();
    let baseline = candidate::BaselineWarningCounts::default();
    let space = SearchSpace::default();
    let ctx = candidate::SearchContext {
        material: &material,
        weights: &EQUAL_WEIGHTS,
        baseline_warnings: &baseline,
        lighting: CANONICAL_LIGHTING_PRESET,
        space: &space,
        guard: None,
        shape_target: None,
    };
    let result = candidate::evaluate_survivors(Vec::new(), &ctx, f32::MAX);
    assert_eq!(result.best, None);
    assert_eq!(result.evaluations, 0);
    assert_eq!(result.accepted.len(), 0);
}

/// A single surviving direction must be solved (evaluations == 1) without needing a
/// second candidate to pair with -- exercises the inline, no-`thread::scope` path.
#[test]
fn evaluate_survivors_with_one_survivor_evaluates_it_inline() {
    let design = rbc_445();
    let space = SearchSpace::default();
    let survivors = candidate::select_candidate_directions(&design, 0, -50.4, 45.0, &space);
    assert_eq!(
        survivors.len(),
        1,
        "fixture must exercise the one-survivor case"
    );

    let material = GemMaterial::diamond();
    let baseline = candidate::BaselineWarningCounts::default();
    let ctx = candidate::SearchContext {
        material: &material,
        weights: &EQUAL_WEIGHTS,
        baseline_warnings: &baseline,
        lighting: CANONICAL_LIGHTING_PRESET,
        space: &space,
        guard: None,
        shape_target: None,
    };
    let result = candidate::evaluate_survivors(survivors, &ctx, f32::MAX);
    assert_eq!(result.evaluations, 1);
}

/// Two surviving directions must both be solved (evaluations == 2) via the
/// `std::thread::scope` path -- kept as a regression guard that splitting the
/// function did not change this behavior. Every accepted direction is reported in
/// `accepted` (what a multi-candidate run feeds its pool), the winner or not.
#[test]
fn evaluate_survivors_with_two_survivors_evaluates_both() {
    let design = rbc_445();
    let space = SearchSpace::default();
    let survivors = candidate::select_candidate_directions(&design, 7, 31.0, 1.0, &space);
    assert_eq!(
        survivors.len(),
        2,
        "fixture must exercise the two-survivor case"
    );

    let material = GemMaterial::diamond();
    let baseline = candidate::BaselineWarningCounts::default();
    let ctx = candidate::SearchContext {
        material: &material,
        weights: &EQUAL_WEIGHTS,
        baseline_warnings: &baseline,
        lighting: CANONICAL_LIGHTING_PRESET,
        space: &space,
        guard: None,
        shape_target: None,
    };
    let result = candidate::evaluate_survivors(survivors, &ctx, f32::MAX);
    assert_eq!(result.evaluations, 2);
    // With the comparison score at `f32::MAX` every accepted direction beats it, so
    // `accepted` lists them all and `best` is the lowest of them.
    let lowest = result
        .accepted
        .iter()
        .map(|&(_, score)| score)
        .min_by(f32::total_cmp);
    assert_eq!(result.best.map(|(_, score)| score), lowest);
}

/// The lighting preset the search is configured with must reach the objective call:
/// the default config scores under the canonical preset, `evaluate_objective` is that
/// preset's convenience form, and a candidate scores differently under a preset whose
/// illumination differs (an ISO hemisphere lights every upward direction, the ring rig
/// only its sources).
#[test]
fn the_configured_lighting_preset_reaches_the_candidate_score() {
    assert_eq!(
        OptimizeConfig::default().lighting,
        CANONICAL_LIGHTING_PRESET
    );

    let design = rbc_445();
    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights::default();
    let solved = design.solve().expect("RBC-445 must solve");
    let planes = design.planes_from_solved(&solved);
    let baseline = candidate::BaselineWarningCounts::count(&check_manufacturability(
        &design,
        &solved,
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    ));

    let gpu_planes = objective::to_gpu_planes(&planes);
    let canonical = evaluate_objective(&gpu_planes, &material, ObjectiveFidelity::Fast);
    let explicit = evaluate_objective_under(
        &gpu_planes,
        &material,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    assert_eq!(canonical, explicit);

    let score_under = |lighting: LightingPreset| match candidate::evaluate_candidate(
        &design, &material, &weights, &baseline, lighting, None, None,
    ) {
        candidate::CandidateOutcome::Accepted { score } => score,
        candidate::CandidateOutcome::Rejected => {
            panic!("the unmodified RBC-445 must be accepted under {lighting:?}")
        }
    };
    let ring = score_under(LightingPreset::RingLights);
    let iso = score_under(LightingPreset::IsoHemisphere);
    assert!(
        (ring - iso).abs() > 1e-3,
        "the preset must change the score: ring {ring}, iso {iso}"
    );
}

/// [`candidate::build_free_angle_candidate`] -- the real, `Design`-backed bridge
/// [`run_polish_stage`] uses to turn one simplex vertex into a scoreable candidate
/// -- must only ever write `free`'s own tier indices, exactly like every other
/// angle-only edit this module makes (see the parent module's doc comment, "A
/// tier's `angle_deg` is the only field this module ever changes on a free tier").
#[test]
fn build_free_angle_candidate_never_touches_a_tier_outside_free() {
    let design = rbc_445();
    let free = free_tier_indices(&design);
    let reference: Vec<f64> = free.iter().map(|&i| design.tiers[i].angle_deg).collect();
    let angles: Vec<f64> = reference.iter().map(|&a| a + 0.1).collect();
    let candidate = candidate::build_free_angle_candidate(
        &design,
        &free,
        &reference,
        &angles,
        &SearchSpace::default(),
    )
    .expect("a small +0.1 degree nudge must stay within bounds for every free tier");
    for (i, tier) in design.tiers.iter().enumerate() {
        if !free.contains(&i) {
            assert_eq!(
                candidate.tiers[i].angle_deg, tier.angle_deg,
                "tier {i} is not free and must be untouched"
            );
        }
    }
}

/// [`candidate::build_free_angle_candidate`] rejects a candidate outright (returns
/// `None`, rather than clamping it back in bounds) the moment any single free
/// tier's proposed angle is unsafe relative to its own reference angle -- the same
/// zero-crossing/vertical-bound gate [`candidate::candidate_angle_is_safe`] applies
/// to the coordinate stage's own candidates.
#[test]
fn build_free_angle_candidate_rejects_an_out_of_bounds_angle_in_any_position() {
    let design = rbc_445();
    let free = free_tier_indices(&design);
    let reference: Vec<f64> = free.iter().map(|&i| design.tiers[i].angle_deg).collect();
    assert_ne!(
        reference[0], 0.0,
        "fixture must exercise a real zero-crossing, not the always-safe authored-at-zero case"
    );
    // Cross zero on the first free tier only -- every other position keeps its own
    // reference angle unchanged (a safe, zero-delta "move").
    let mut angles = reference.clone();
    angles[0] = -reference[0].signum() * 0.5;
    assert!(
        candidate::build_free_angle_candidate(
            &design,
            &free,
            &reference,
            &angles,
            &SearchSpace::default()
        )
        .is_none(),
        "a single unsafe axis must reject the whole candidate point"
    );
}
