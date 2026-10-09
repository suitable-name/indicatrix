//! Previewing and applying one Optimize candidate: its angle changes and the mast changes
//! that go with them, with the tiers that follow a relation moved along.
//!
//! The plain [`indicatrix_cut_core::apply_optimize_outcome`] writes angles only. A
//! candidate from a run that varied anchored tiers also moves masts, and a design with
//! tier relations needs the tiers that follow a changed tier to follow it in the same undo
//! step. The session's relation-aware `try_apply` does the second part; this module builds
//! the edit for the first.

use crate::EditorSession;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{Design, Edit, OptimizeCandidate};
use std::sync::atomic::Ordering;

/// A clone of `design` with `candidate` applied: its angles, its masts, and the angles the
/// tiers that follow a relation then have. The candidate a preview or the compare window
/// shows.
///
/// Never touches history. A change naming a tier the design no longer has is skipped, and
/// relations that cannot be worked out leave the followers where they were.
#[must_use]
pub fn build_candidate_preview_design(design: &Design, candidate: &OptimizeCandidate) -> Design {
    let mut preview = design.clone();
    for change in &candidate.changes {
        if let Some(tier) = preview.tiers.get_mut(change.index) {
            tier.angle_deg = change.to_deg;
        }
    }
    for change in &candidate.mast_changes {
        if let Some(tier) = preview.tiers.get_mut(change.index)
            && matches!(tier.constraint, MeetConstraint::ScaleReference(_))
        {
            tier.constraint = MeetConstraint::ScaleReference(change.to_mast);
        }
    }
    if let Ok(updates) = preview.evaluate_relations() {
        for (position, angle_deg) in updates {
            if let Some(tier) = preview.tiers.get_mut(position) {
                tier.angle_deg = angle_deg;
            }
        }
    }
    preview
}

/// The edits that turn `design` into `candidate`: one `ModifyTier` per changed tier.
///
/// Angle and mast go together, in first-touched order. The edits do not move the tiers that
/// follow a relation; the session does that when it applies them.
///
/// # Errors
///
/// The index of the first change whose tier is gone, or no longer has the angle or mast
/// the change started from (the design moved on since the run).
pub fn candidate_edits(design: &Design, candidate: &OptimizeCandidate) -> Result<Vec<Edit>, usize> {
    let mut tiers = design.tiers.clone();
    let mut touched: Vec<usize> = Vec::new();
    for change in &candidate.changes {
        let tier = tiers.get_mut(change.index).ok_or(change.index)?;
        if tier.angle_deg.to_bits() != change.from_deg.to_bits() {
            return Err(change.index);
        }
        tier.angle_deg = change.to_deg;
        if !touched.contains(&change.index) {
            touched.push(change.index);
        }
    }
    for change in &candidate.mast_changes {
        let tier = tiers.get_mut(change.index).ok_or(change.index)?;
        let MeetConstraint::ScaleReference(current) = tier.constraint else {
            return Err(change.index);
        };
        if current.to_bits() != change.from_mast.to_bits() {
            return Err(change.index);
        }
        tier.constraint = MeetConstraint::ScaleReference(change.to_mast);
        if !touched.contains(&change.index) {
            touched.push(change.index);
        }
    }
    Ok(touched
        .into_iter()
        .map(|index| Edit::ModifyTier {
            index,
            tier: tiers[index].clone(),
        })
        .collect())
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
    let edits = match candidate_edits(&session.design, candidate) {
        Ok(edits) => edits,
        Err(index) => {
            session.generation.fetch_add(1, Ordering::Relaxed);
            return Err(format!(
                "The design changed since Optimize ran (tier #{} is not as it was). Run \
                 Optimize again.",
                index + 1
            ));
        }
    };
    if edits.is_empty() {
        return Ok(0);
    }
    let before = session.design.tiers.clone();
    match session.try_apply(Edit::Batch(edits)) {
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
    fn the_preview_has_the_candidates_angles_and_masts_and_leaves_the_design_alone() {
        let session = session();
        let preview = build_candidate_preview_design(&session.design, &move_a());
        assert_eq!(preview.tiers[0].angle_deg, 42.0);
        assert_eq!(mast(&preview, 0), 0.62);
        assert_eq!(preview.tiers[1], session.design.tiers[1]);
        assert_eq!(session.design.tiers[0].angle_deg, 40.0);
        assert_eq!(mast(&session.design, 0), 0.6);
    }

    #[test]
    fn the_preview_moves_the_tiers_that_follow_a_relation() {
        let mut session = session();
        follow(&mut session);
        let preview = build_candidate_preview_design(&session.design, &move_a());
        assert!((preview.tiers[1].angle_deg - 40.0).abs() < 1e-9);
        assert_eq!(mast(&preview, 1), 0.5, "the follower keeps its mast");
    }

    #[test]
    fn the_preview_skips_a_change_for_a_tier_that_is_gone() {
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
        assert_eq!(preview.tiers, session.design.tiers);
    }

    // --- the edits ---

    #[test]
    fn a_tier_with_an_angle_and_a_mast_change_is_one_edit() {
        let session = session();
        let edits = candidate_edits(&session.design, &move_a()).expect("fits");
        assert_eq!(edits.len(), 1);
        match &edits[0] {
            Edit::ModifyTier { index, tier } => {
                assert_eq!(*index, 0);
                assert_eq!(tier.angle_deg, 42.0);
                assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.62));
            }
            other => panic!("expected a ModifyTier, got {other:?}"),
        }
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
            candidate_edits(&session.design, &wrong_angle).unwrap_err(),
            2
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
            candidate_edits(&session.design, &wrong_mast).unwrap_err(),
            1
        );
        let missing = candidate(
            vec![AngleChange {
                index: 7,
                from_deg: 40.0,
                to_deg: 41.0,
            }],
            Vec::new(),
        );
        assert_eq!(candidate_edits(&session.design, &missing).unwrap_err(), 7);
    }

    // --- applying ---

    #[test]
    fn applying_changes_the_angle_and_the_mast_in_one_undo_step() {
        let mut session = session();
        let original = session.design.clone();
        let generation = session.current_generation();
        assert_eq!(apply_candidate(&mut session, &move_a()), Ok(1));
        assert_eq!(session.design.tiers[0].angle_deg, 42.0);
        assert_eq!(mast(&session.design, 0), 0.62);
        assert_ne!(session.current_generation(), generation);

        assert!(session.undo().expect("undo").is_some());
        assert_eq!(session.design, original);
        assert!(session.undo().expect("undo").is_none(), "it was one step");
    }

    #[test]
    fn applying_moves_the_followers_in_the_same_undo_step() {
        let mut session = session();
        follow(&mut session);
        let original = session.design.clone();
        assert_eq!(
            apply_candidate(&mut session, &move_a()),
            Ok(2),
            "the tier it changed and the tier that follows it"
        );
        assert_eq!(session.design.tiers[0].angle_deg, 42.0);
        assert!((session.design.tiers[1].angle_deg - 40.0).abs() < 1e-9);
        assert_eq!(mast(&session.design, 1), 0.5);

        assert!(session.undo().expect("undo").is_some());
        assert_eq!(session.design, original, "one undo restores both tiers");
        assert!(session.undo().expect("undo").is_none());
    }

    #[test]
    fn a_stale_candidate_changes_nothing_and_says_which_tier() {
        let mut session = session();
        session.design.tiers[0].angle_deg = 41.0;
        let before = session.design.clone();
        let generation = session.current_generation();
        let error = apply_candidate(&mut session, &move_a()).unwrap_err();
        assert!(error.contains("tier #1"), "{error}");
        assert!(error.contains("Run Optimize again"), "{error}");
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
    fn an_empty_candidate_applies_nothing() {
        let mut session = session();
        let before = session.design.clone();
        assert_eq!(
            apply_candidate(&mut session, &candidate(Vec::new(), Vec::new())),
            Ok(0)
        );
        assert_eq!(session.design, before);
        assert!(session.undo().expect("undo").is_none());
    }
}
