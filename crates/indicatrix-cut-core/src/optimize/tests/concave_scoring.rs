//! The optimizer scores the stone WITH its concave tools (the Compare window's stone),
//! and a flat design is scored on exactly its planes, bit for bit as before.

use super::{
    super::{
        CANONICAL_LIGHTING_PRESET, ObjectiveFidelity, ObjectiveWeights, candidate,
        evaluate_objective_under, objective, score_finished_design,
    },
    fixtures::rbc_445,
};
use crate::{
    design::Design,
    manufacturability::{DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability},
};
use indicatrix::{
    geometry::stone_metrics::{SolidMetrics, measure_solid},
    optics::materials::GemMaterial,
};

/// `design` with its concave tiers taken away: the flat shell the optimizer used to score.
fn without_tools(design: &Design) -> Design {
    let mut flat = design.clone();
    flat.concave_tiers.clear();
    flat.concave_tier_ids.clear();
    flat
}

#[test]
fn a_flat_design_is_scored_on_exactly_its_planes() {
    let design = rbc_445();
    let material = GemMaterial::diamond();
    let solved = design.solve().expect("RBC-445 must solve");
    let planes = design.planes_from_solved(&solved);
    let stone = candidate::ScoredStone::of(&design, &planes);
    assert!(stone.geometry().is_convex(), "a flat design has no tools");

    let (via_stone, tone, _) = candidate::measure_with_tone(
        stone.geometry(),
        &material,
        &ObjectiveWeights::default(),
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    let direct = evaluate_objective_under(
        &objective::to_gpu_planes(&planes),
        &material,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    assert_eq!(via_stone, direct);
    assert!(tone.is_none(), "an unweighted Fast score measures no tone");
}

#[test]
fn a_concave_design_is_scored_on_the_stone_with_its_tools() {
    let design = Design::concave_fixture();
    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights::default();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);

    let stone = candidate::ScoredStone::of(&design, &planes);
    assert!(!stone.geometry().is_convex());
    assert_eq!(
        stone.geometry().tools.len(),
        design.concave_placement_count(),
        "one tool per placement"
    );
    let (with_tools, _, _) = candidate::measure_with_tone(
        stone.geometry(),
        &material,
        &weights,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    let shell = evaluate_objective_under(
        &objective::to_gpu_planes(&planes),
        &material,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
    );
    assert_ne!(
        with_tools, shell,
        "the grooves and dimples change the figures"
    );

    // The public scorer the Retarget report uses measures the same stone.
    let flat_design = without_tools(&design);
    let carved = score_finished_design(
        &design,
        &planes,
        &material,
        &weights,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
        None,
    );
    let flat = score_finished_design(
        &flat_design,
        &planes,
        &material,
        &weights,
        ObjectiveFidelity::Fast,
        CANONICAL_LIGHTING_PRESET,
        None,
    );
    assert_eq!(carved.after, with_tools);
    assert_eq!(flat.after, shell);

    // And so does every candidate of the search.
    let baseline = candidate::BaselineWarningCounts::count(&check_manufacturability(
        &design,
        &solved,
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    ));
    let score_of = |candidate_design: &Design| match candidate::evaluate_candidate(
        candidate_design,
        &material,
        &weights,
        &baseline,
        CANONICAL_LIGHTING_PRESET,
        None,
        None,
        candidate::preform_volume(candidate_design),
    ) {
        candidate::CandidateOutcome::Accepted { score, .. } => score,
        candidate::CandidateOutcome::Rejected => panic!("the fixture must be accepted"),
    };
    let carved_score = score_of(&design);
    assert_ne!(carved_score.to_bits(), score_of(&flat_design).to_bits());
    assert_eq!(
        carved_score.to_bits(),
        score_of(&design).to_bits(),
        "scoring is deterministic"
    );
}

/// The yield term reads the carved volume (a groove costs material), and a flat design's
/// metrics reach it untouched. The shape penalty reads no volume, so it has nothing to move.
#[test]
fn the_yield_term_reads_the_carved_volume_and_a_flat_design_is_untouched() {
    let flat_design = rbc_445();
    let flat_solved = flat_design.solve().expect("RBC-445 must solve");
    let flat_planes = flat_design.planes_from_solved(&flat_solved);
    let flat_metrics = measure_solid(&flat_planes);
    let flat_stone = candidate::ScoredStone::of(&flat_design, &flat_planes);
    assert_eq!(
        flat_stone.yield_metrics(&flat_planes, flat_metrics.as_ref()),
        flat_metrics,
        "no tools: the very metrics the optimizer always read"
    );

    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let flat_of_fixture = measure_solid(&planes).expect("the flat stone measures");
    let stone = candidate::ScoredStone::of(&design, &planes);
    let carved = stone
        .yield_metrics(&planes, Some(&flat_of_fixture))
        .expect("the tools leave a stone");
    assert!(
        carved.volume < flat_of_fixture.volume,
        "carved {} flat {}",
        carved.volume,
        flat_of_fixture.volume
    );
    assert_eq!(
        SolidMetrics {
            volume: flat_of_fixture.volume,
            ..carved
        },
        flat_of_fixture,
        "only the volume differs: widths, heights and the table stay the flat stone's"
    );

    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights::default();
    let score = |scored: &Design| {
        score_finished_design(
            scored,
            &planes,
            &material,
            &weights,
            ObjectiveFidelity::Fast,
            CANONICAL_LIGHTING_PRESET,
            None,
        )
        .yield_loss_pct
    };
    assert!(
        score(&design) > score(&without_tools(&design)),
        "the carved stone yields less of the same rough"
    );
    assert_eq!(
        score(&design).to_bits(),
        candidate::yield_loss_pct(&design, Some(&carved)).to_bits()
    );
}

/// Meet preservation judges the planar shell and leaves the tools alone: a concave design
/// is captured, validates against its own capture with nothing to correct, is still scored
/// as the carved stone, and a candidate that touches a tool is a different design (stale),
/// not a meet violation.
#[test]
fn a_concave_design_preserves_its_planar_meets_and_keeps_its_tools_fixed() {
    use crate::meet_preservation::{
        MeetPreservationBaseline, PreservationPolicy, PreservationViolation,
    };
    let design = Design::concave_fixture();
    let baseline = MeetPreservationBaseline::capture(&design)
        .expect("the planar shell of a concave design has a capturable network");
    let facts = baseline
        .validate(&design, PreservationPolicy::OPTIMIZE)
        .expect("an unchanged concave design validates against its own capture");
    assert!(facts.original_meets > 0);
    assert_eq!(facts.added_meets, 0);

    // Every residual of the unchanged design is zero: the projection has nothing to correct.
    let residuals = baseline
        .residuals(
            &design,
            &indicatrix::geometry::meet_solver::SolveControl::default(),
        )
        .expect("the residuals of a concave design form");
    assert!(residuals.iter().all(|r| r.abs() < 2e-6));

    // The scored stone is still the carved one.
    let planes = design.planes().expect("the design solves");
    assert!(
        !candidate::ScoredStone::of(&design, &planes)
            .geometry()
            .is_convex()
    );

    // A tool is not something a search may edit: that is another design.
    let mut edited = design;
    edited.concave_tiers[0].diameter_ratio = 0.3;
    assert!(matches!(
        baseline.validate(&edited, PreservationPolicy::OPTIMIZE),
        Err(PreservationViolation::StaleCandidate)
    ));
}

/// Tiers that do not resolve leave the flat stone to score, as every display path draws
/// it: a half-resolved set of tools would measure a stone nobody holds.
#[test]
fn tiers_that_do_not_resolve_are_scored_as_the_flat_stone() {
    let mut design = Design::concave_fixture();
    design.concave_tiers[1].diameter_ratio = 0.0;
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let stone = candidate::ScoredStone::of(&design, &planes);
    assert!(stone.geometry().is_convex());
}
