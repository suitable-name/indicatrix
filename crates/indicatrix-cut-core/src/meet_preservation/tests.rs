use super::*;
use crate::{
    design::{ConstraintTier, ScheduleMeta},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Branch diagnosis of refused candidates.
mod branch;
/// The census measurements: raw shape deltas, structure counts and violation kinds.
mod census;
/// Imported meets opened by five-decimal masts: identified, with their slack recorded.
mod quantised;
/// The travel a requested facet turn implies, which the Retarget policy grants.
mod turn;

fn octahedron() -> Design {
    octahedron_in(4.0)
}

/// The octahedron cut from a cubic rough of side `rough`. Every size describes the
/// same stone (six four-plane apexes); a large rough only changes the units the
/// display mesh welds and the arrangement walk normalises in.
fn octahedron_in(rough: f64) -> Design {
    let tier = |name: String, angle_deg, indices| ConstraintTier {
        name,
        angle_deg,
        indices,
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    };
    let mut tiers: Vec<_> = [0.0, 24.0, 48.0, 72.0]
        .into_iter()
        .enumerate()
        .map(|(i, index)| tier(format!("C{i}"), 45.0, vec![index]))
        .collect();
    tiers.push(tier("P".into(), -45.0, vec![0.0, 24.0, 48.0, 72.0]));
    let meta = ScheduleMeta {
        symmetry_order: 4,
        ..ScheduleMeta::standard_round_brilliant()
    };
    Design::new(PreformSpec::block(rough, rough, rough), meta, tiers)
}

/// Roughs from just larger than the stone to hundreds of times larger.
const ROUGH_SIZES: [f64; 3] = [4.0, 64.0, 512.0];

/// The octahedron with every crown facet turned `steeper` degrees about its girdle edge
/// (`mast = r_edge sin t`, `r_edge` the edge's distance from the axis): the equator, this
/// stone's girdle outline, stays where it is, every one of the six four-plane apexes
/// survives, and the top apex travels `r_edge (tan t' - tan t)`: half the width per unit of
/// tangent change, because these facets reach from the outline to the axis.
fn crown_turned(design: &Design, steeper: f64) -> Design {
    let mut turned = design.clone();
    for tier in turned.tiers.iter_mut().filter(|tier| tier.angle_deg > 0.0) {
        let MeetConstraint::ScaleReference(mast) = tier.constraint else {
            unreachable!("the octahedron is pinned")
        };
        let edge = mast / tier.angle_deg.to_radians().sin();
        tier.angle_deg += steeper;
        tier.constraint = MeetConstraint::ScaleReference(edge * tier.angle_deg.to_radians().sin());
    }
    turned
}

fn stretch(design: &Design, axial: f64) -> Design {
    let mut result = design.clone();
    for tier in &mut result.tiers {
        let theta = tier.angle_deg.abs().to_radians();
        let length = theta.sin().hypot(theta.cos() / axial);
        tier.angle_deg = (theta.tan() * axial)
            .atan()
            .to_degrees()
            .copysign(tier.angle_deg);
        if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
            *mast /= length;
        }
    }
    result
}

/// Every tier's angle edit from `before` to `after`.
fn angle_changes(before: &Design, after: &Design) -> Vec<crate::AngleChange> {
    before
        .tiers
        .iter()
        .zip(&after.tiers)
        .enumerate()
        .map(|(index, (a, b))| crate::AngleChange {
            index,
            from_deg: a.angle_deg,
            to_deg: b.angle_deg,
        })
        .collect()
}

/// Every tier's pinned-depth edit from `before` to `after` (all tiers are pinned).
fn mast_changes(before: &Design, after: &Design) -> Vec<crate::MastChange> {
    before
        .tiers
        .iter()
        .zip(&after.tiers)
        .enumerate()
        .map(|(index, (a, b))| {
            let (
                MeetConstraint::ScaleReference(from_mast),
                MeetConstraint::ScaleReference(to_mast),
            ) = (&a.constraint, &b.constraint)
            else {
                unreachable!()
            };
            crate::MastChange {
                index,
                from_mast: *from_mast,
                to_mast: *to_mast,
            }
        })
        .collect()
}

#[test]
fn split_four_facet_apex_is_rejected_even_though_the_solid_still_closes() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("original network");
    let mut damaged = design;
    damaged.tiers[0].angle_deg += 1.0;
    assert!(
        damaged.is_closed(),
        "closure alone must not certify a split meet"
    );
    assert!(
        baseline
            .validate(&damaged, PreservationPolicy::OPTIMIZE)
            .is_err()
    );
}

#[test]
fn all_six_apexes_are_captured_whatever_the_rough_size() {
    for rough in ROUGH_SIZES {
        let design = octahedron_in(rough);
        let baseline = MeetPreservationBaseline::capture(&design)
            .unwrap_or_else(|error| panic!("rough {rough}: {error}"));
        let facts = baseline
            .validate(&design, PreservationPolicy::OPTIMIZE)
            .unwrap_or_else(|error| panic!("rough {rough}: {error}"));
        assert_eq!(facts.original_meets, 6, "rough {rough}");
        assert_eq!(facts.added_meets, 0, "rough {rough}");
    }
}

#[test]
fn a_split_apex_is_rejected_whatever_the_rough_size() {
    for rough in ROUGH_SIZES {
        let design = octahedron_in(rough);
        let baseline = MeetPreservationBaseline::capture(&design)
            .unwrap_or_else(|error| panic!("rough {rough}: {error}"));
        let mut damaged = design.clone();
        damaged.tiers[0].angle_deg += 1.0;
        assert!(
            damaged.is_closed(),
            "rough {rough}: closure alone must not certify a split meet"
        );
        assert!(
            baseline
                .validate(&damaged, PreservationPolicy::OPTIMIZE)
                .is_err(),
            "rough {rough}"
        );
    }
}

#[test]
fn a_moving_apex_preserves_the_network_whatever_the_rough_size() {
    for rough in ROUGH_SIZES {
        let design = octahedron_in(rough);
        let baseline = MeetPreservationBaseline::capture(&design)
            .unwrap_or_else(|error| panic!("rough {rough}: {error}"));
        let facts = baseline
            .validate(&stretch(&design, 1.04), PreservationPolicy::RETARGET)
            .unwrap_or_else(|error| panic!("rough {rough}: {error}"));
        assert!(facts.shape_distance > 0.001, "rough {rough}");
        assert!(facts.max_normalized_residual <= 2e-6, "rough {rough}");
    }
}

#[test]
fn a_candidate_that_loses_a_facet_does_not_blame_the_original() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).unwrap();
    let mut candidate = design;
    // Pinned five units out, the crown facet no longer touches the stone.
    candidate.tiers[0].constraint = MeetConstraint::ScaleReference(5.0);
    let error = baseline
        .validate(&candidate, PreservationPolicy::OPTIMIZE)
        .unwrap_err();
    assert!(
        !matches!(error, PreservationViolation::InvalidBaseline(_)),
        "{error}"
    );
    assert!(
        !error
            .to_string()
            .contains("Cannot establish the original meets"),
        "{error}"
    );
}

#[test]
fn growing_the_whole_stone_moves_the_girdle_gauge() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).unwrap();
    let mut grown = design;
    for tier in &mut grown.tiers {
        if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
            *mast *= 1.03;
        }
    }
    // Every junction keeps its supports and the shape ratios are unchanged: only the
    // outline's absolute reach differs, and the presets hold that.
    let error = baseline
        .validate(&grown, PreservationPolicy::RETARGET)
        .unwrap_err();
    assert!(
        matches!(
            &error,
            PreservationViolation::AtSurfaces { .. } | PreservationViolation::GirdleGauge { .. }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("girdle radius"), "{error}");
    let free = PreservationPolicy {
        lock_girdle_gauge: false,
        ..PreservationPolicy::RETARGET
    };
    baseline
        .validate(&grown, free)
        .expect("without the gauge lock a uniform growth is a valid candidate");
}

#[test]
fn a_moving_four_facet_apex_preserves_the_network() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("original network");
    let candidate = stretch(&design, 1.04);
    assert_ne!(candidate.tiers, design.tiers);
    let facts = baseline
        .validate(&candidate, PreservationPolicy::RETARGET)
        .expect("affine preserved meets");
    assert!(facts.shape_distance > 0.001);
    assert!(facts.max_normalized_residual <= 2e-6);
}

#[test]
fn aliases_and_native_roundtrip_keep_the_same_original_connections() {
    let mut design = octahedron();
    let mut alias = design.tiers[0].clone();
    alias.name = "Alias".into();
    design.tiers.push(alias);
    design.ensure_tier_ids();
    let baseline = MeetPreservationBaseline::capture(&design).expect("duplicate surface aliases");
    let candidate = stretch(&design, 1.02);
    let text =
        crate::native::design_to_string(&candidate, None, &crate::native::DesignExtras::default())
            .expect("native serialization");
    let reopened = crate::native::design_from_str(&text)
        .expect("native load")
        .design;
    baseline
        .validate(&reopened, PreservationPolicy::RETARGET)
        .expect("stored authored state reproduces meets");
    assert_eq!(candidate.planes().unwrap(), reopened.planes().unwrap());
}

#[test]
fn projection_retains_a_nonzero_coupled_authored_deformation() {
    let design = octahedron();
    let solved = design.solve().unwrap();
    let baseline = std::sync::Arc::new(MeetPreservationBaseline::capture(&design).unwrap());
    let options = crate::OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: crate::design::tier_hinge_points(&design, &solved),
        preservation_baseline: Some(baseline.clone()),
        ..Default::default()
    };
    let mut proposed = stretch(&design, 1.04);
    proposed.tiers[0].angle_deg += 0.2;
    let projected = crate::optimize::project_preserved_design(&design, &proposed, &options, None)
        .expect("bounded coupled projection");
    MeetPreservationBaseline::capture(&design)
        .unwrap()
        .validate(&projected, PreservationPolicy::OPTIMIZE)
        .expect("actual authoring solve preserves original meets");
    // The solve budget of one projection: twelve iterations, each a validation plus one
    // finite-difference probe per variable and a short line search. A tier contributes at
    // most an angle and a depth, so `2 * tiers` bounds the variables.
    let variables = u64::try_from(2 * design.tiers.len()).unwrap();
    let solve_calls = baseline.diagnostics().phase_calls[2];
    assert!(
        solve_calls <= 12 * (variables + 8),
        "{solve_calls} solves for {variables} variables"
    );
    assert!(
        projected
            .tiers
            .iter()
            .zip(&design.tiers)
            .any(|(a, b)| (a.angle_deg - b.angle_deg).abs() > 0.01)
    );
    assert!(
        projected
            .tiers
            .iter()
            .zip(&design.tiers)
            .filter(|(a, b)| a != b)
            .count()
            >= 2
    );
}

#[test]
fn stale_identity_and_hand_constructed_damage_fail_before_history_changes() {
    let mut design = octahedron();
    let before = design.clone();
    let mut history = crate::History::new();
    let damage = crate::AngleChange {
        index: 0,
        from_deg: 45.0,
        to_deg: 46.0,
    };
    let result = crate::optimize::materialize_preserved_changes(
        &design,
        &[damage],
        &[],
        None,
        PreservationPolicy::OPTIMIZE,
    );
    assert!(result.is_err());
    assert_eq!(design, before);
    assert!(!history.can_undo());
    let candidate = stretch(&design, 1.03);
    let angles = angle_changes(&before, &candidate);
    let masts = mast_changes(&before, &candidate);
    let (_, edit, _) = crate::optimize::materialize_preserved_changes(
        &design,
        &angles,
        &masts,
        None,
        PreservationPolicy::OPTIMIZE,
    )
    .unwrap();
    history.apply(&mut design, edit).unwrap();
    assert_eq!(design.planes().unwrap(), candidate.planes().unwrap());
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.planes().unwrap(), candidate.planes().unwrap());
}

#[test]
fn a_deduplicated_alias_uses_the_retained_planes_cheater_transform() {
    let mut design = octahedron();
    let mut alias = design.tiers[0].clone();
    alias.name = "Near duplicate".into();
    alias.constraint = MeetConstraint::ScaleReference(0.5001);
    let alias_index = design.tiers.len();
    design.tiers.push(alias);
    design.ensure_tier_ids();
    // Production deduplicates before rotating: this discarded alias does not add
    // a new, rotated plane to the stone and must inherit the retained surface.
    design.cheater_offsets_deg.insert(alias_index, 12.0);
    let baseline = MeetPreservationBaseline::capture(&design).expect("production aliases");
    baseline
        .validate(&design, PreservationPolicy::OPTIMIZE)
        .expect("identity is valid");
    let mut split = design.clone();
    split.tiers[alias_index].angle_deg += 1.0;
    assert!(
        baseline
            .validate(&split, PreservationPolicy::OPTIMIZE)
            .is_err()
    );
}

#[test]
fn a_feasible_preference_cannot_bypass_requested_angle_bounds() {
    let design = octahedron();
    let solved = design.solve().unwrap();
    let options = crate::OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: crate::design::tier_hinge_points(&design, &solved),
        angle_bounds: design
            .tiers
            .iter()
            .enumerate()
            .map(|(i, tier)| (i, (tier.angle_deg, tier.angle_deg)))
            .collect(),
        ..Default::default()
    };
    let preference = stretch(&design, 1.04);
    assert!(
        MeetPreservationBaseline::capture(&design)
            .unwrap()
            .validate(&preference, PreservationPolicy::OPTIMIZE)
            .is_ok()
    );
    assert!(
        crate::optimize::project_preserved_design(&design, &preference, &options, None).is_err()
    );
}

#[test]
fn a_proposal_that_adopts_a_pinned_depth_to_a_meet_is_stale_and_does_not_panic() {
    let design = octahedron();
    let solved = design.solve().unwrap();
    let options = crate::OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: crate::design::tier_hinge_points(&design, &solved),
        ..Default::default()
    };
    let mut proposed = design.clone();
    proposed.tiers[0].constraint = MeetConstraint::MeetExisting;
    let result = crate::optimize::project_preserved_design(&design, &proposed, &options, None);
    assert!(
        matches!(result, Err(PreservationViolation::StaleCandidate)),
        "{result:?}"
    );
}

#[test]
fn projection_rejects_changed_membership_without_indexing_a_stale_baseline() {
    let design = octahedron();
    let mut preference = design.clone();
    preference.tiers.push(preference.tiers[0].clone());
    preference.ensure_tier_ids();
    assert!(
        crate::optimize::project_preserved_design(
            &design,
            &preference,
            &crate::OptimizeOptions::default(),
            None
        )
        .is_err()
    );
}

#[test]
fn a_flat_depth_can_move_without_unlocking_any_angles() {
    let mut design = octahedron();
    let mut table = design.tiers[0].clone();
    table.name = "Table".into();
    table.angle_deg = 0.0;
    table.constraint = MeetConstraint::ScaleReference(0.3);
    design.tiers.push(table);
    design.ensure_tier_ids();
    let index = design.tiers.len() - 1;
    let options = crate::OptimizeOptions {
        vary_anchored: true,
        only_tiers: Some(std::iter::once(index).collect()),
        ..Default::default()
    };
    let mut preferred = design.clone();
    preferred.tiers[index].constraint = MeetConstraint::ScaleReference(0.303);
    let candidate =
        crate::optimize::project_preserved_design(&design, &preferred, &options, None).unwrap();
    let MeetConstraint::ScaleReference(mast) = candidate.tiers[index].constraint else {
        panic!("the table stays pinned");
    };
    assert!(
        (mast - 0.303).abs() < 1e-12,
        "the requested depth must survive projection: {mast}"
    );
    MeetPreservationBaseline::capture(&design)
        .unwrap()
        .validate(&candidate, PreservationPolicy::OPTIMIZE)
        .expect("preserved flat-depth move");
    for (a, b) in candidate.tiers.iter().zip(&design.tiers) {
        assert_eq!(a.angle_deg, b.angle_deg);
    }
    assert_eq!(&candidate.tiers[..index], &design.tiers[..index]);
}

#[test]
fn cancellation_reaches_capture_and_projection_through_a_predicate() {
    let design = octahedron();
    let stop = || true;
    let control = indicatrix::geometry::meet_solver::SolveControl::with_stop(&stop);
    assert!(matches!(
        MeetPreservationBaseline::capture_controlled(&design, &control),
        Err(PreservationViolation::Cancelled)
    ));
    assert!(matches!(
        crate::optimize::project_preserved_design_controlled(
            &design,
            &design,
            &crate::OptimizeOptions::default(),
            &control
        ),
        Err(PreservationViolation::Cancelled)
    ));
}

#[test]
fn physical_accuracy_and_shape_limits_are_reported() {
    let mut design = octahedron();
    design.girdle_diameter_mm = Some(8.0);
    let baseline = MeetPreservationBaseline::capture(&design).unwrap();
    let facts = baseline
        .validate(&design, PreservationPolicy::RETARGET)
        .unwrap();
    assert_eq!(facts.added_meets, 0);
    assert!(facts.max_residual_mm.unwrap().is_finite());
    // The constant bound alone: the stretch is also a turn (every slope steepens), and the
    // Retarget preset grants a turn its own travel, which is exactly this apex's.
    let policy = PreservationPolicy {
        max_junction_displacement: 0.0,
        bounds_follow_turn: false,
        ..PreservationPolicy::RETARGET
    };
    assert!(baseline.validate(&stretch(&design, 1.03), policy).is_err());
}

#[test]
fn exported_schedule_is_checked_after_parsing_the_written_text() {
    let design = octahedron();
    let schedule = design.to_asc_schedule_from_solved(&design.solve().unwrap());
    let text = indicatrix_formats::asc::to_asc_string(&schedule).unwrap();
    let reopened = indicatrix_formats::asc::parse_asc(&text).unwrap();
    check_schedule_precision(&schedule, &reopened).expect("identity schedule export");
    let mut damaged = reopened;
    damaged.tiers[0].angle_deg += 1.0;
    assert!(check_schedule_precision(&schedule, &damaged).is_err());
}

#[test]
fn rich_and_legacy_results_reproduce_one_state_and_reject_stale_inputs() {
    let design = octahedron();
    let candidate = stretch(&design, 1.03);
    let baseline = std::sync::Arc::new(MeetPreservationBaseline::capture(&design).unwrap());
    let facts = baseline
        .validate(&candidate, PreservationPolicy::OPTIMIZE)
        .unwrap();
    let changes = angle_changes(&design, &candidate);
    let masts = mast_changes(&design, &candidate);
    let provenance = crate::optimize::OptimizationProvenance {
        source: crate::optimize::authoring_identity(&design).unwrap(),
        candidate: crate::optimize::authoring_identity(&candidate).unwrap(),
        scoring: "geometry-only test fixture".into(),
        policy: PreservationPolicy::OPTIMIZE,
        masts: masts
            .iter()
            .map(|m| (m.index, m.from_mast, m.to_mast))
            .collect(),
        diagnostics: baseline.diagnostics(),
        preservation: facts,
    };
    let wire = serde_json::to_string(&provenance).unwrap();
    let provenance = serde_json::from_str(&wire).unwrap();
    let metrics = crate::ObjectiveComponents {
        windowing_pct: 0.0,
        extinction_pct: 0.0,
        tilt_brilliance_pct: 0.0,
    };
    let rich = crate::OptimizeCandidate {
        preservation_baseline: Some(baseline),
        provenance: Some(provenance),
        changes: changes.clone(),
        mast_changes: masts,
        after: metrics,
        score: 0.0,
        yield_loss_pct: 0.0,
        tone: None,
    };
    let legacy = crate::OptimizeOutcome {
        provenance: rich.provenance.clone(),
        before: metrics,
        before_score: 0.0,
        before_yield_loss_pct: 0.0,
        after: metrics,
        after_score: 0.0,
        after_yield_loss_pct: 0.0,
        evaluations: 0,
        changes,
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    };
    let a = crate::optimize::materialize_candidate(&design, &rich)
        .unwrap()
        .0;
    let b = crate::optimize::materialize_outcome(&design, &legacy)
        .unwrap()
        .0;
    assert_eq!(a.planes().unwrap(), candidate.planes().unwrap());
    assert_eq!(a, b);
    let mut history = crate::History::new();
    let mut applied = design.clone();
    crate::apply_optimize_outcome(&mut history, &mut applied, &legacy).unwrap();
    assert_eq!(applied, candidate);
    assert!(history.undo(&mut applied).unwrap());
    assert_eq!(applied, design);
    assert!(history.redo(&mut applied).unwrap());
    assert_eq!(applied, candidate);
    let mut stale = design.clone();
    stale.tiers[4].name = "Edited outside candidate angles".into();
    assert!(crate::optimize::materialize_outcome(&stale, &legacy).is_err());
    let mut tampered = legacy.clone();
    tampered.provenance.as_mut().unwrap().masts.clear();
    assert!(crate::optimize::materialize_outcome(&design, &tampered).is_err());
}
