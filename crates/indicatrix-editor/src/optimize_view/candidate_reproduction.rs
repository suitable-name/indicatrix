//! Positive geometry fixture for the certified preview/session/History path.
use super::*;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    AngleChange, ConstraintTier, MastChange, ObjectiveComponents, PreformSpec,
    design::ScheduleMeta,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    optimize::{OptimizationProvenance, authoring_identity},
};
use std::sync::Arc;

/// `design` with every angle tangent scaled by `factor` (the stone stretched along y) and
/// every mast rescaled to match, then the authoring evaluator's follower angles applied.
fn stretched(design: &Design, factor: f64) -> Design {
    let mut stretched = design.clone();
    for tier in &mut stretched.tiers {
        let theta = tier.angle_deg.abs().to_radians();
        let length = theta.sin().hypot(theta.cos() / factor);
        tier.angle_deg = (theta.tan() * factor)
            .atan()
            .to_degrees()
            .copysign(tier.angle_deg);
        if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
            *mast /= length;
        }
    }
    // The authoring evaluator, not an independent optimizer parameter, owns C1.
    for (index, angle) in stretched.evaluate_relations().unwrap() {
        stretched.tiers[index].angle_deg = angle;
    }
    stretched
}

fn fixture() -> (Design, Design, OptimizeCandidate) {
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
    let mut original = Design::new(PreformSpec::block(4.0, 4.0, 4.0), meta, tiers);
    original.ensure_tier_ids();
    original
        .tier_relations
        .insert(original.tier_ids[1], original.parse_relation("C0").unwrap());
    let accepted = stretched(&original, 1.03);
    let baseline = Arc::new(MeetPreservationBaseline::capture(&original).unwrap());
    let facts = baseline
        .validate(&accepted, PreservationPolicy::OPTIMIZE)
        .unwrap();
    let changes = original
        .tiers
        .iter()
        .zip(&accepted.tiers)
        .enumerate()
        .filter(|(i, _)| !original.is_tier_driven(*i))
        .map(|(index, (a, b))| AngleChange {
            index,
            from_deg: a.angle_deg,
            to_deg: b.angle_deg,
        })
        .collect();
    let mast_changes: Vec<_> = original
        .tiers
        .iter()
        .zip(&accepted.tiers)
        .enumerate()
        .map(|(index, (a, b))| {
            let (
                MeetConstraint::ScaleReference(from_mast),
                MeetConstraint::ScaleReference(to_mast),
            ) = (&a.constraint, &b.constraint)
            else {
                unreachable!()
            };
            MastChange {
                index,
                from_mast: *from_mast,
                to_mast: *to_mast,
            }
        })
        .collect();
    let provenance = OptimizationProvenance {
        source: authoring_identity(&original).unwrap(),
        candidate: authoring_identity(&accepted).unwrap(),
        scoring: "geometry-only fixture".into(),
        policy: PreservationPolicy::OPTIMIZE,
        masts: mast_changes
            .iter()
            .map(|m| (m.index, m.from_mast, m.to_mast))
            .collect(),
        diagnostics: baseline.diagnostics(),
        preservation: facts,
    };
    let candidate = OptimizeCandidate {
        provenance: Some(provenance),
        preservation_baseline: Some(baseline),
        changes,
        mast_changes,
        after: ObjectiveComponents {
            windowing_pct: 0.0,
            extinction_pct: 0.0,
            tilt_brilliance_pct: 0.0,
        },
        score: 0.0,
        tone: None,
        yield_loss_pct: 0.0,
    };
    (original, accepted, candidate)
}

#[test]
fn preview_apply_undo_redo_reproduce_the_certified_state_with_followers() {
    let (original, accepted, candidate) = fixture();
    assert_eq!(
        build_candidate_preview_design(&original, &candidate).unwrap(),
        accepted
    );
    let mut session = EditorSession::fresh();
    session.design = original.clone();
    assert_eq!(
        apply_candidate(&mut session, &candidate).unwrap(),
        original.tiers.len()
    );
    assert_eq!(session.design, accepted);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, original);
    assert!(session.redo().unwrap().is_some());
    assert_eq!(session.design, accepted);
    assert!(session.undo().unwrap().is_some());
    assert!(
        session.undo().unwrap().is_none(),
        "the entire result is one edit"
    );
}

#[test]
fn altered_or_stale_certified_candidates_never_touch_history() {
    let (original, _, mut candidate) = fixture();
    candidate.mast_changes.pop();
    let mut session = EditorSession::fresh();
    session.design = original.clone();
    assert!(apply_candidate(&mut session, &candidate).is_err());
    assert_eq!(session.design, original);
    assert!(session.undo().unwrap().is_none());
    let (_, _, candidate) = fixture();
    session.design.tiers[4].name = "Edited since search".into();
    let before = session.design.clone();
    assert!(apply_candidate(&mut session, &candidate).is_err());
    assert_eq!(session.design, before);
}

#[test]
fn a_shifted_source_retains_the_pre_shift_geometry_gate() {
    let (original, accepted, mut candidate) = fixture();
    let seeded = stretched(&original, 1.01);
    for change in &mut candidate.changes {
        change.from_deg = seeded.tiers[change.index].angle_deg;
    }
    for change in &mut candidate.mast_changes {
        let MeetConstraint::ScaleReference(mast) = seeded.tiers[change.index].constraint else {
            unreachable!()
        };
        change.from_mast = mast;
    }
    let provenance = candidate.provenance.as_mut().unwrap();
    provenance.source = authoring_identity(&seeded).unwrap();
    provenance.masts = candidate
        .mast_changes
        .iter()
        .map(|m| (m.index, m.from_mast, m.to_mast))
        .collect();
    assert_ne!(seeded, original);
    assert_eq!(
        build_candidate_preview_design(&seeded, &candidate).unwrap(),
        accepted
    );
    assert!(
        build_candidate_preview_design(&original, &candidate).is_err(),
        "the parameter source is still exact"
    );
}
