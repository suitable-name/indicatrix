use super::*;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    AngleChange, ConstraintTier, MastChange, ObjectiveComponents, PreformSpec, ScheduleMeta,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    optimize::{OptimizationProvenance, authoring_identity},
};
use indicatrix_editor::optimize_view::candidate_outcome;

fn components() -> ObjectiveComponents {
    ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 10.0,
        tilt_brilliance_pct: 80.0,
    }
}

/// A closed bipyramid and its vertical stretch: angles AND masts move while
/// every original meet stays joined. Certify the geometry through the real gate.
fn fixture() -> (EditorSession, Design, OptimizeCandidate) {
    let tiers = [45.0, -45.0]
        .into_iter()
        .enumerate()
        .map(|(index, angle_deg)| ConstraintTier {
            angle_deg,
            name: format!("Tier {index}"),
            indices: vec![0.0, 24.0, 48.0, 72.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    let mut session = EditorSession::fresh();
    let meta = ScheduleMeta {
        symmetry_order: 4,
        ..ScheduleMeta::standard_round_brilliant()
    };
    session.design = Design::new(PreformSpec::block(4.0, 4.0, 4.0), meta, tiers);
    session.design.ensure_tier_ids();
    let original = &session.design;
    let mut accepted = original.clone();
    for tier in &mut accepted.tiers {
        let theta = tier.angle_deg.abs().to_radians();
        let length = theta.sin().hypot(theta.cos() / 1.03);
        tier.angle_deg = (theta.tan() * 1.03)
            .atan()
            .to_degrees()
            .copysign(tier.angle_deg);
        if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
            *mast /= length;
        }
    }
    let baseline = Arc::new(MeetPreservationBaseline::capture(original).expect("closed baseline"));
    let facts = baseline
        .validate(&accepted, PreservationPolicy::OPTIMIZE)
        .expect("an affine stretch preserves the original meets");
    let changes = original
        .tiers
        .iter()
        .zip(&accepted.tiers)
        .enumerate()
        .map(|(index, (before, after))| AngleChange {
            index,
            from_deg: before.angle_deg,
            to_deg: after.angle_deg,
        })
        .collect();
    let mast_changes: Vec<_> = original
        .tiers
        .iter()
        .zip(&accepted.tiers)
        .enumerate()
        .map(|(index, (before, after))| {
            let (MeetConstraint::ScaleReference(from), MeetConstraint::ScaleReference(to)) =
                (&before.constraint, &after.constraint)
            else {
                panic!("pinned fixture")
            };
            MastChange {
                index,
                from_mast: *from,
                to_mast: *to,
            }
        })
        .collect();
    let provenance = OptimizationProvenance {
        source: authoring_identity(original).unwrap(),
        candidate: authoring_identity(&accepted).unwrap(),
        scoring: "geometry-only panel fixture".into(),
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
        after: components(),
        score: 9.0,
        yield_loss_pct: 4.0,
        tone: None,
    };
    (session, accepted, candidate)
}

#[test]
fn the_tone_note_names_the_viewport_lighting() {
    let mut design = EditorSession::fresh().design;
    design.girdle_diameter_mm = Some(6.5);
    let material = indicatrix::optics::materials::GemMaterial::sapphire().with_body_color(
        indicatrix::optics::materials::body_color::BODY_COLOR_PRESETS[1].absorption_rgb,
    );
    let note = |lighting| tone_scale_note(&design, &material, lighting);
    assert!(note(LightingPreset::Daylight).contains("D65 Daylight"));
    assert!(note(LightingPreset::Incandescent).contains("Incandescent (3200K)"));
    assert!(note(LightingPreset::UvLamp365).contains("daylight"));
}

fn outcome_of(candidate: &OptimizeCandidate) -> OptimizeOutcome {
    let base = OptimizeOutcome {
        provenance: None,
        before: components(),
        before_score: 10.0,
        before_yield_loss_pct: 5.0,
        after: components(),
        after_score: 10.0,
        after_yield_loss_pct: 5.0,
        evaluations: 30,
        changes: Vec::new(),
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    };
    candidate_outcome(&base, candidate)
}

fn stored(session: &EditorSession, candidates: Vec<OptimizeCandidate>) -> StoredRun {
    let best = &candidates[0];
    StoredRun {
        result: OptimizeResult {
            outcome: outcome_of(best),
            mast_changes: best.mast_changes.clone(),
            candidates,
            tone_before: None,
            tone_goal: None,
            lighting: indicatrix_cut_core::CANONICAL_LIGHTING_PRESET,
            starts_run: 1,
            best_start: 0,
        },
        design: session.design.clone(),
        generation: session.current_generation(),
        selected: Some(0),
    }
}

#[test]
fn a_candidate_of_the_last_run_is_applied_with_its_masts_in_one_undo_step() {
    let (mut session, accepted, candidate) = fixture();
    let original = session.design.clone();
    let outcome = outcome_of(&candidate);
    with_panel(|panel| panel.run = Some(stored(&session, vec![candidate])));
    let applied = apply_pending(&mut session, &outcome);
    with_panel(|panel| panel.run = None);
    assert_eq!(applied, Ok(2));
    assert_eq!(session.design, accepted);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, original);
    assert!(
        session.undo().unwrap().is_none(),
        "one edit for angles and masts"
    );
    assert!(session.redo().unwrap().is_some());
    assert_eq!(session.design, accepted);
}

#[test]
fn a_certified_outcome_without_panel_state_keeps_its_masts() {
    with_panel(|panel| panel.run = None);
    let (mut session, accepted, candidate) = fixture();
    assert_eq!(apply_pending(&mut session, &outcome_of(&candidate)), Ok(2));
    assert_eq!(
        session.design, accepted,
        "portable outcomes retain the complete result"
    );
}

#[test]
fn an_uncertified_outcome_cannot_borrow_a_matching_panel_candidates_identity() {
    let (mut session, _, candidate) = fixture();
    let original = session.design.clone();
    let mut outcome = outcome_of(&candidate);
    outcome.provenance = None;
    with_panel(|panel| panel.run = Some(stored(&session, vec![candidate])));
    let applied = apply_pending(&mut session, &outcome);
    with_panel(|panel| panel.run = None);
    assert!(applied.is_err());
    assert_eq!(session.design, original);
    assert!(session.undo().unwrap().is_none());
}

#[test]
fn equal_angles_do_not_confuse_candidates_with_different_depth_identity() {
    let (session, _, candidate) = fixture();
    let mut other = candidate.clone();
    // This lookup-only alternative deliberately differs in depth; Apply would
    // still require its accepted geometry to pass certification independently.
    other.mast_changes[0].to_mast += 0.01;
    other.provenance.as_mut().unwrap().masts[0].2 += 0.01;
    let outcome = outcome_of(&other);
    let run = stored(&session, vec![candidate, other.clone()]);
    let found = run
        .candidate_for(&outcome)
        .expect("the outcome's own candidate");
    assert_eq!(found.mast_changes, other.mast_changes);
}

#[test]
fn a_stale_outcome_is_refused_without_changing_history() {
    with_panel(|panel| panel.run = None);
    let (mut session, _, candidate) = fixture();
    session.design.tiers[0].name = "Edited since the run".into();
    let original = session.design.clone();
    assert!(apply_pending(&mut session, &outcome_of(&candidate)).is_err());
    assert_eq!(session.design, original);
    assert!(session.undo().unwrap().is_none());
}
