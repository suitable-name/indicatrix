//! The gate grants a retarget the travel its requested facet turn implies.
//!
//! Owner run 3 (2026-10-19) left the three diamond-to-sapphire check tests red with
//! `ShapeLimit`: the anchored seed kept every original meet, the girdle outline and the
//! corner band, and was refused because `PreservationPolicy::RETARGET` bounded the pavilion
//! depth change and the culet's travel by a constant 0.08 of the stone, while a ten-degree
//! pavilion turn about the girdle outline (the critical-angle shift from n 2.42 to 1.77)
//! carries the culet by `(R - r_culet) (tan 51 - tan 41) = 0.18 W`. The policy now follows
//! the turn (`PreservationPolicy::bounds_follow_turn`, tightened by the owner the same day):
//! the gate grants the largest per-tier product of a turned tier's inward reach from the
//! outline and its tangent change, the tangent change read from the REQUESTED angles (the
//! anchored seed, so the projection's repair cannot enlarge its allowance), only tiers the
//! request turned counting, tangents capped at 75 degrees. Its constants bound what the
//! projection's repair adds. For this stone the pavilion main reaches 0.99 R inward, so the
//! grant is 0.18 W against the 0.17 W the stone deepens by.

use super::*;
use indicatrix_cut_core::meet_preservation::{PreservationViolation, tangent_changes};

#[test]
fn a_large_retarget_is_granted_the_depth_its_turn_implies() {
    let design = brilliant();
    let (_, result) = check(&design, "Sapphire", CrownShift::default());
    assert_eq!(
        result.validity.status,
        ValidityStatus::Valid,
        "{:?}",
        result.validity.reasons
    );
    let figures = result.validity.figures;

    // The point of the test: the stone deepens by far more than the constant depth bound
    // (0.08 width units are 8 points of H/W), so the constant alone refuses it.
    let (depth_was, depth_now) = (
        figures.depth_was.expect("the depth is measured"),
        figures.depth_now.expect("the depth is measured"),
    );
    assert!(
        depth_now - depth_was > 100.0 * PreservationPolicy::RETARGET.max_depth_change,
        "depth {depth_was} % became {depth_now} %"
    );

    // The girdle OUTLINE is the contract: the corner band is exactly what it was...
    let (thinnest_was, thinnest_now) = (
        figures
            .thinnest_was
            .expect("the thinnest point is measured"),
        figures
            .thinnest_now
            .expect("the thinnest point is measured"),
    );
    assert!(
        (thinnest_now - thinnest_was).abs() < LEVEL_NOISE_PERCENT,
        "thinnest point {thinnest_was} % became {thinnest_now} %"
    );
    assert_corners_kept(&result);

    // ...and the table keeps its size through the refit.
    let (table_was, table_now) = (
        figures.table_was.expect("the table is measured"),
        figures.table_now.expect("the table is measured"),
    );
    assert!(
        (table_now - table_was).abs() <= 0.01 * table_was,
        "table {table_was} % became {table_now} %"
    );

    // The checked state applies, and its meets survive saving and reopening.
    let mut applied = design.clone();
    let edit = result.validated_edit(&design).expect("preserved retarget");
    History::new()
        .apply(&mut applied, edit)
        .expect("the batch applies");
    assert_eq!(
        Some(&applied),
        result.candidate.as_ref(),
        "Apply uses the exact checked state"
    );
    assert_meets_survive_saving(&result, &applied);
}

#[test]
fn without_the_turn_allowance_the_same_stone_is_refused_for_its_shape() {
    let design = brilliant();
    let (_, result) = check(&design, "Sapphire", CrownShift::default());
    let candidate = result
        .candidate
        .as_ref()
        .expect("a valid check carries its candidate");
    let baseline = result.baseline.as_ref().expect("original meet network");
    let constant = PreservationPolicy {
        bounds_follow_turn: false,
        ..PreservationPolicy::RETARGET
    };
    let refused = baseline.validate(candidate, constant);
    assert!(
        matches!(refused, Err(PreservationViolation::ShapeLimit)),
        "{refused:?}"
    );
    baseline
        .validate(candidate, PreservationPolicy::RETARGET)
        .expect("the turn's own travel is granted, nothing else is relaxed");
}

/// A refit is built on the PROJECTED plain attempt and projected again, so its seed carries
/// the first projection's angles. The gate's allowance is the retarget's REQUEST
/// (`Attempt::requested`, handed to both projections as `OptimizeOptions::turn_request`),
/// not the state the refit is built on: the same projected sapphire state refitted as the
/// result of its own request is valid, and refitted as the result of a small request is
/// refused for the shape, however far its seed's angles were turned.
#[test]
fn a_refit_is_judged_by_the_request_not_by_the_projected_state_it_refits() {
    let live = brilliant();
    let original = analyze(&live, true).expect("original geometry and meet network");
    let never = || false;
    let sapphire = selection("Sapphire").resolve(&BuiltinMaterials);
    let plan = build_plan(&live, &sapphire, CrownShift::default(), &[]);
    let moves = plan.moving_angles();
    let (requested, anchors) = anchored_candidate(&live, &plan, &original);
    let base = Attempt::new(
        &live,
        &original,
        Proposal {
            design: requested.clone(),
            anchors,
            strategy: RetargetStrategy::Anchored,
            requested,
        },
        &moves,
        &never,
    );
    // The real flow: the refit inherits the ten-degree request and is the valid result.
    let honest = refit_attempt(&live, &original, &base, &moves, &never)
        .expect("the deeper pavilion refits the table and the culet");
    assert!(honest.reasons.is_empty(), "{:?}", honest.reasons);

    // The same projected state presented as the refit of a retarget to n 2.30, a turn of
    // about a degree and a half: its request grants a few hundredths of the width, the stone
    // deepens by 0.17 W, and the ten-degree angles the seed carries buy nothing.
    let near = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(2.30),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
    .resolve(&BuiltinMaterials);
    let near_plan = build_plan(&live, &near, CrownShift::default(), &[]);
    let (near_requested, _) = anchored_candidate(&live, &near_plan, &original);
    let main = tier_named(&live, "Pavilion Main");
    let (near_turn, turn) = (
        tangent_changes(&live, &near_requested)[main],
        tangent_changes(&live, &base.requested)[main],
    );
    assert!(
        near_turn < turn / 3.0,
        "near {near_turn} vs sapphire {turn}"
    );
    let mut overturned = base;
    overturned.requested = near_requested;
    let judged = refit_attempt(&live, &original, &overturned, &moves, &never)
        .expect("the refit proposal is built the same way");
    assert!(
        judged.reasons.iter().any(|reason| matches!(
            reason,
            InvalidReason::Preservation(text) if text.contains("permitted change")
        )),
        "{:?}",
        judged.reasons
    );
}
