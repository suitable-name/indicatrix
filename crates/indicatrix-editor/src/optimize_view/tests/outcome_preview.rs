//! Positive coverage of the legacy-outcome preview: an outcome the optimizer certified (with its
//! provenance) rebuilds exactly the accepted state, and the same outcome is refused once the
//! design it was computed against has moved on.
//!
//! The fixture is the one of `candidate_apply`'s reproduction tests (a four-fold design with a
//! relation follower, stretched along y by 3 %), which legitimately passes the original-meet gate.
use super::*;
use indicatrix_cut_core::{
    MastChange, PreformSpec,
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

/// The original design, the accepted (stretched) state, and the rich candidate that describes
/// the step from one to the other, with its provenance.
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
fn build_optimize_preview_design_reproduces_a_certified_outcome() {
    let (original, accepted, candidate) = fixture();
    // The outcome the candidate stands for: its changes and provenance in the best result's
    // place, which is what the preview, the compare window and the change table read.
    let outcome = candidate_outcome(&sample_outcome(true, false), &candidate);
    assert_eq!(outcome.changes, candidate.changes);
    assert!(outcome.provenance.is_some());

    let preview =
        build_optimize_preview_design(&original, &outcome).expect("a certified outcome previews");
    assert_eq!(
        preview, accepted,
        "the preview is the accepted state, masts and followers included"
    );
    assert_ne!(preview, original, "and it is not the design as it was");
}

#[test]
fn a_certified_outcome_is_refused_once_the_design_has_moved_on() {
    let (original, _, candidate) = fixture();
    let outcome = candidate_outcome(&sample_outcome(true, false), &candidate);
    let mut moved = original;
    moved.tiers[4].name = "Edited since search".into();
    let before = moved.clone();
    assert!(
        build_optimize_preview_design(&moved, &outcome).is_err(),
        "an untouched tier's name is part of the source identity"
    );
    assert_eq!(moved, before, "the design is never mutated");
}
