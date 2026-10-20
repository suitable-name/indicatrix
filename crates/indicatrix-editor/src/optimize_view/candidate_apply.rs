//! Previewing and applying one Optimize candidate: its angle changes and the mast changes
//! that go with them, with the tiers that follow a relation moved along.
//!
//! All result forms retain the complete accepted state. The session derives relation
//! followers in the same undo step while retaining their accepted pinned depths.

use crate::EditorSession;
use indicatrix_cut_core::{Design, Edit, OptimizeCandidate, optimize::MaterializeError};
use std::sync::atomic::Ordering;

/// A clone of `design` with `candidate` applied: its angles, its masts, and the angles the
/// tiers that follow a relation then have. The candidate a preview or the compare window
/// shows.
///
/// Never touches history. Invalid or stale input returns an explicit error.
///
/// # Errors
///
/// A message when the candidate is stale, carries no provenance, or fails the geometry gate.
pub fn build_candidate_preview_design(
    design: &Design,
    candidate: &OptimizeCandidate,
) -> Result<Design, String> {
    try_build_candidate_preview_design(design, candidate)
}

/// Fallible preview materialization; also used by Apply.
///
/// # Errors
///
/// As [`build_candidate_preview_design`].
pub fn try_build_candidate_preview_design(
    design: &Design,
    candidate: &OptimizeCandidate,
) -> Result<Design, String> {
    indicatrix_cut_core::optimize::materialize_candidate(design, candidate)
        .map(|(preview, _, _)| preview)
        .map_err(|e| format!("Cannot preview this candidate: {e}"))
}

/// The edits that turn `design` into `candidate`: one `ModifyTier` per changed tier.
///
/// Angle and mast go together, including settled relation followers. Convert the batch
/// with [`edits_for_relation_session`] before committing through `EditorSession`.
///
/// # Errors
///
/// The [`MaterializeError`] that refuses the candidate: [`MaterializeError::Edit`] names the
/// tier of the first change that is gone or no longer has the angle or mast the change started
/// from (the design moved on since the run); [`MaterializeError::Preservation`] is a geometry
/// or identity refusal that belongs to no single tier (it is not reported as "tier 0").
pub fn candidate_edits(
    design: &Design,
    candidate: &OptimizeCandidate,
) -> Result<Vec<Edit>, MaterializeError> {
    let (_, edit, _) = indicatrix_cut_core::optimize::materialize_candidate(design, candidate)?;
    match edit {
        Edit::Batch(edits) => Ok(edits),
        edit => Ok(vec![edit]),
    }
}

/// Let `EditorSession` perform its own relation updates while retaining every mast edit.
/// The complete relation-settled state was already solved and validated before this conversion.
#[must_use]
pub fn edits_for_relation_session(design: &Design, edit: Edit) -> Edit {
    match edit {
        Edit::ModifyTier { index, mut tier } => {
            if design.is_tier_driven(index) {
                tier.angle_deg = design.tiers[index].angle_deg;
            }
            Edit::ModifyTier { index, tier }
        }
        Edit::Batch(edits) => Edit::Batch(
            edits
                .into_iter()
                .map(|edit| edits_for_relation_session(design, edit))
                .collect(),
        ),
        other => other,
    }
}

/// Applies `candidate` to `session` as ONE undo step: its angles and masts, and the tiers
/// that follow a relation moved to the angles their relations give.
///
/// Returns how many tiers changed, followers included.
///
/// The session's generation moves on after a success (as for any edit) and after a refusal
/// (the candidate was computed against a design that has since moved).
///
/// # Errors
///
/// A plain-English message when the design changed since the run (a tier moved or was
/// removed), when a change would move a tier that follows a relation, or when the relations
/// cannot be satisfied afterwards. Nothing changes on `Err`.
pub fn apply_candidate(
    session: &mut EditorSession,
    candidate: &OptimizeCandidate,
) -> Result<usize, String> {
    let (_, batch, _) =
        indicatrix_cut_core::optimize::materialize_candidate(&session.design, candidate).map_err(
            |error| {
                session.generation.fetch_add(1, Ordering::Relaxed);
                error.to_string()
            },
        )?;
    let edits = match batch {
        Edit::Batch(edits) => edits,
        edit => vec![edit],
    };
    if edits.is_empty() {
        return Ok(0);
    }
    let before = session.design.tiers.clone();
    let edit = edits_for_relation_session(&session.design, Edit::Batch(edits));
    match session.try_apply(edit) {
        Ok(_) => Ok(session
            .design
            .tiers
            .iter()
            .zip(&before)
            .filter(|(now, was)| now != was)
            .count()),
        Err(error) => {
            session.generation.fetch_add(1, Ordering::Relaxed);
            Err(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{AngleChange, ConstraintTier, MastChange, ObjectiveComponents};

    fn tier(name: &str, angle_deg: f64, mast: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: vec![0.0],
            constraint: MeetConstraint::ScaleReference(mast),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// `A` (tier 1) at 40 degrees with mast 0.6, `B` at 38 degrees with mast 0.5, and `C`
    /// at 20 degrees.
    fn session() -> EditorSession {
        let mut session = EditorSession::fresh();
        session.design.tiers.push(tier("A", 40.0, 0.6));
        session.design.tiers.push(tier("B", 38.0, 0.5));
        session.design.tiers.push(tier("C", 20.0, 0.4));
        session.design.ensure_tier_ids();
        session
    }

    /// Makes `B` follow `A - 2`.
    fn follow(session: &mut EditorSession) {
        let relation = session.design.parse_relation("A - 2").expect("reads");
        let id = session.design.tier_ids[1];
        session.design.tier_relations.insert(id, relation);
    }

    fn candidate(changes: Vec<AngleChange>, mast_changes: Vec<MastChange>) -> OptimizeCandidate {
        OptimizeCandidate {
            provenance: None,
            preservation_baseline: None,
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
        }
    }

    fn move_a() -> OptimizeCandidate {
        candidate(
            vec![AngleChange {
                index: 0,
                from_deg: 40.0,
                to_deg: 42.0,
            }],
            vec![MastChange {
                index: 0,
                from_mast: 0.6,
                to_mast: 0.62,
            }],
        )
    }

    fn mast(design: &Design, index: usize) -> f64 {
        let MeetConstraint::ScaleReference(mast) = design.tiers[index].constraint else {
            panic!("tier {index} must be pinned");
        };
        mast
    }

    // --- the preview ---

    #[test]
    fn uncertified_preview_is_rejected_without_mutation() {
        let session = session();
        let preview = build_candidate_preview_design(&session.design, &move_a());
        assert!(
            preview.is_err(),
            "uncertified hand-built candidates cannot be previewed"
        );
        assert_eq!(session.design.tiers[0].angle_deg, 40.0);
        assert_eq!(mast(&session.design, 0), 0.6);
    }

    #[test]
    fn relations_do_not_authorize_an_uncertified_preview() {
        let mut session = session();
        follow(&mut session);
        let preview = build_candidate_preview_design(&session.design, &move_a());
        assert!(preview.is_err(), "relations do not replace source identity");
    }

    #[test]
    fn the_preview_rejects_a_change_for_a_missing_tier() {
        let session = session();
        let gone = candidate(
            vec![AngleChange {
                index: 9,
                from_deg: 1.0,
                to_deg: 2.0,
            }],
            vec![MastChange {
                index: 9,
                from_mast: 1.0,
                to_mast: 2.0,
            }],
        );
        let preview = build_candidate_preview_design(&session.design, &gone);
        assert!(preview.is_err(), "missing tiers cannot be silently skipped");
    }

    // --- the edits ---

    #[test]
    fn uncertified_parameters_cannot_be_converted_to_edits() {
        let session = session();
        assert!(candidate_edits(&session.design, &move_a()).is_err());
    }

    /// The tier a refusal names, `None` for a geometry refusal that belongs to no tier.
    fn refused_tier(error: &MaterializeError) -> Option<usize> {
        match error {
            MaterializeError::Edit(edit) => Some(edit.index),
            MaterializeError::Preservation(_) => None,
        }
    }

    #[test]
    fn an_uncertified_candidate_is_a_geometry_refusal_that_names_no_tier() {
        // Tier 0 is a real, in-range tier: reporting a provenance refusal as "tier 0" would
        // point the cutter at a facet that is not the problem.
        let session = session();
        let error = candidate_edits(&session.design, &move_a()).unwrap_err();
        assert_eq!(refused_tier(&error), None, "{error}");
    }

    #[test]
    fn a_change_that_no_longer_fits_the_design_is_refused_by_tier() {
        let session = session();
        let wrong_angle = candidate(
            vec![AngleChange {
                index: 2,
                from_deg: 21.0,
                to_deg: 22.0,
            }],
            Vec::new(),
        );
        assert_eq!(
            refused_tier(&candidate_edits(&session.design, &wrong_angle).unwrap_err()),
            Some(2)
        );
        let wrong_mast = candidate(
            Vec::new(),
            vec![MastChange {
                index: 1,
                from_mast: 0.55,
                to_mast: 0.6,
            }],
        );
        assert_eq!(
            refused_tier(&candidate_edits(&session.design, &wrong_mast).unwrap_err()),
            Some(1)
        );
        let missing = candidate(
            vec![AngleChange {
                index: 7,
                from_deg: 40.0,
                to_deg: 41.0,
            }],
            Vec::new(),
        );
        assert_eq!(
            refused_tier(&candidate_edits(&session.design, &missing).unwrap_err()),
            Some(7)
        );
    }

    // --- applying ---

    #[test]
    fn uncertified_changes_leave_history_empty() {
        let mut session = session();
        let original = session.design.clone();
        assert!(apply_candidate(&mut session, &move_a()).is_err());
        assert_eq!(session.design, original);
        assert!(session.undo().expect("undo").is_none());
    }

    #[test]
    fn uncertified_changes_cannot_move_followers() {
        let mut session = session();
        follow(&mut session);
        let original = session.design.clone();
        assert!(apply_candidate(&mut session, &move_a()).is_err());
        assert_eq!(session.design, original);
        assert!(session.undo().expect("undo").is_none());
    }

    #[test]
    fn a_stale_candidate_changes_nothing_and_says_which_tier() {
        let mut session = session();
        session.design.tiers[0].angle_deg = 41.0;
        let before = session.design.clone();
        let generation = session.current_generation();
        let error = apply_candidate(&mut session, &move_a()).unwrap_err();
        assert_ne!(error.len(), 0);
        assert_eq!(session.design, before);
        assert_ne!(
            session.current_generation(),
            generation,
            "a result computed against an older design is invalidated"
        );
    }

    #[test]
    fn a_change_to_a_tier_that_follows_a_relation_is_refused() {
        let mut session = session();
        follow(&mut session);
        let before = session.design.clone();
        let direct = candidate(
            vec![AngleChange {
                index: 1,
                from_deg: 38.0,
                to_deg: 36.0,
            }],
            Vec::new(),
        );
        let error = apply_candidate(&mut session, &direct).unwrap_err();
        assert!(error.contains("follows a relation"), "{error}");
        assert_eq!(session.design, before);
    }

    #[test]
    fn an_empty_uncertified_candidate_is_rejected() {
        let mut session = session();
        let before = session.design.clone();
        assert!(apply_candidate(&mut session, &candidate(Vec::new(), Vec::new())).is_err());
        assert_eq!(session.design, before);
        assert!(session.undo().expect("undo").is_none());
    }
}

#[cfg(test)]
#[path = "candidate_reproduction.rs"]
mod reproduction_tests;
