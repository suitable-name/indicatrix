//! Tests for the session's tier relations: a relation moves its tier in one undo step, a
//! driver edit carries the tiers that follow it, a direct edit of a followed angle is
//! refused, a removal frees the relations reading the removed tier, and a linked step
//! series.

use super::*;
use crate::session::{InlineAngle, angle_nudge_coalesce_key};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, ScheduleState};

const fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn crown(name: &str, angle_deg: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0, 12.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// C1 (40), C2 (38) and C3 (30), added through the session so they carry ids.
fn crown_session() -> EditorSession {
    let mut session = EditorSession::fresh();
    for (index, (name, angle)) in [("C1", 40.0), ("C2", 38.0), ("C3", 30.0)]
        .into_iter()
        .enumerate()
    {
        session
            .apply(Edit::AddTier {
                index,
                tier: crown(name, angle),
            })
            .unwrap();
    }
    session
}

/// [`crown_session`] with C2 = C1 - 5 and C3 = C2 - 3: angles 40, 35 and 32.
fn chained_session() -> EditorSession {
    let mut session = crown_session();
    session.set_tier_relation(1, "C1 - 5").unwrap();
    session.set_tier_relation(2, "C2 - 3").unwrap();
    assert_eq!(angles(&session), vec![40.0, 35.0, 32.0]);
    session
}

fn angles(session: &EditorSession) -> Vec<f64> {
    session
        .design
        .tiers
        .iter()
        .map(|tier| tier.angle_deg)
        .collect()
}

const FOLLOWS_MESSAGE: &str =
    "This angle follows a relation (C2 = C1 - 5). Edit the relation or remove it.";

#[test]
fn a_relation_moves_its_tier_in_the_same_undo_step() {
    let mut session = crown_session();
    let before = session.design.clone();
    let generation = session.current_generation();

    let change = session
        .set_tier_relation(1, "C1 - 5")
        .unwrap()
        .expect("applied");
    assert_eq!(change.generation, generation + 1);
    assert!(!change.tier_count_changed());
    assert_eq!(angles(&session), vec![40.0, 35.0, 30.0]);
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C1 - 5"));
    assert_eq!(session.tier_relation_display(0), None);
    assert!(session.is_driven(1));
    assert!(!session.is_driven(0));
    assert_eq!(session.drivers_of(1), vec![0]);
    assert_eq!(session.dependants_of(0), vec![1]);
    assert_eq!(session.drivers_of(0), Vec::<usize>::new());
    assert_eq!(session.dependants_of(1), Vec::<usize>::new());

    // The relation and the angle it gave come back out together.
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before);
    assert!(session.redo().unwrap().is_some());
    assert_eq!(angles(&session), vec![40.0, 35.0, 30.0]);
    assert!(session.is_driven(1));
}

#[test]
fn setting_the_same_relation_again_spends_no_undo_step() {
    let mut session = crown_session();
    assert!(session.set_tier_relation(1, "C1 - 5").unwrap().is_some());
    let generation = session.current_generation();
    assert_eq!(session.set_tier_relation(1, "= C1 - 5").unwrap(), None);
    assert_eq!(session.current_generation(), generation);
    // Clearing a relation that is not there does nothing either.
    assert_eq!(session.clear_tier_relation(0).unwrap(), None);
    assert_eq!(session.current_generation(), generation);
}

#[test]
fn a_driver_edit_carries_the_tiers_that_follow_it_in_one_undo_step() {
    let mut session = chained_session();
    let outcome = session
        .nudge_angles(&[0], 1.0, ms(0))
        .unwrap()
        .expect("nudged");
    assert_eq!(outcome.clamped_labels, Vec::<String>::new());
    assert_eq!(angles(&session), vec![41.0, 36.0, 33.0]);

    assert!(session.undo().unwrap().is_some());
    assert_eq!(angles(&session), vec![40.0, 35.0, 32.0]);
    assert!(session.is_driven(1) && session.is_driven(2));
    assert!(session.redo().unwrap().is_some());
    assert_eq!(angles(&session), vec![41.0, 36.0, 33.0]);
}

#[test]
fn a_run_of_nudges_is_one_undo_step_with_the_followers_in_it() {
    let mut session = chained_session();
    for step in 0..3 {
        session.nudge_angles(&[0], 1.0, ms(100 * step)).unwrap();
    }
    assert_eq!(angles(&session), vec![43.0, 38.0, 35.0]);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(angles(&session), vec![40.0, 35.0, 32.0]);
}

#[test]
fn a_dragged_driver_keeps_coalescing_with_its_followers() {
    let mut session = chained_session();
    session.set_tier_angle(0, 41.0, ms(0)).unwrap();
    session.set_tier_angle(0, 42.0, ms(100)).unwrap();
    assert_eq!(angles(&session), vec![42.0, 37.0, 34.0]);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(angles(&session), vec![40.0, 35.0, 32.0]);

    // The coalescing key works through `try_apply_coalescing` directly too.
    session
        .try_apply_coalescing(
            Edit::RetargetAngles {
                changes: vec![(0, 40.0, 44.0)],
            },
            angle_nudge_coalesce_key(&[0]),
            ms(5000),
        )
        .unwrap();
    assert_eq!(angles(&session), vec![44.0, 39.0, 36.0]);
}

#[test]
fn an_edit_of_a_followed_angle_is_refused_and_changes_nothing() {
    let mut session = crown_session();
    session.set_tier_relation(1, "C1 - 5").unwrap();
    let before = session.design.clone();
    let generation = session.current_generation();

    // A drag: the old `apply` family fails with an `EditError` naming the tier, and
    // keeps the typed reason.
    let error = session.set_tier_angle(1, 30.0, ms(0)).unwrap_err();
    assert_eq!(error.index, 1);
    let refusal = session.take_refusal().expect("the reason is kept");
    assert_eq!(
        refusal,
        SessionEditError::Driven(RelationRefusal {
            tier: 1,
            label: "C2".to_string(),
            relation: "C1 - 5".to_string()
        })
    );
    assert_eq!(refusal.to_string(), FOLLOWS_MESSAGE);
    assert!(session.take_refusal().is_none());

    // The typed entry point.
    let mut changed = session.design.tiers[1].clone();
    changed.angle_deg = 20.0;
    let error = session
        .try_apply(Edit::ModifyTier {
            index: 1,
            tier: changed,
        })
        .unwrap_err();
    assert!(matches!(&error, SessionEditError::Driven(refusal) if refusal.tier == 1));

    // The inline cell.
    assert_eq!(
        session.set_tier_angle_from_text(1, "20"),
        Err(FOLLOWS_MESSAGE.to_string())
    );
    // Committing what is there is not a change.
    assert_eq!(
        session.set_tier_angle_from_text(1, "35"),
        Ok(InlineAngle::NoChange)
    );
    // A nudge that includes the followed tier.
    session.nudge_angles(&[0, 1], 1.0, ms(0)).unwrap_err();

    assert_eq!(session.design, before);
    assert_eq!(session.current_generation(), generation);
}

#[test]
fn a_followed_tier_can_still_be_renamed_and_is_free_once_cleared() {
    let mut session = crown_session();
    session.set_tier_relation(1, "C1 - 5").unwrap();
    let mut renamed = session.design.tiers[1].clone();
    renamed.name = "Crown break".to_string();
    session
        .try_apply(Edit::ModifyTier {
            index: 1,
            tier: renamed,
        })
        .unwrap();
    assert_eq!(session.design.tiers[1].name, "Crown break");
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C1 - 5"));

    assert!(session.clear_tier_relation(1).unwrap().is_some());
    assert!(!session.is_driven(1));
    // It keeps the angle it had, and may now be edited.
    assert_eq!(session.design.tiers[1].angle_deg, 35.0);
    assert!(session.set_tier_angle(1, 30.0, ms(0)).unwrap().is_some());
    assert_eq!(session.design.tiers[1].angle_deg, 30.0);
}

#[test]
fn an_edit_that_leaves_a_result_outside_the_range_is_refused_whole() {
    let mut session = crown_session();
    session.set_tier_relation(1, "C1 + 50").unwrap();
    assert_eq!(angles(&session), vec![40.0, 90.0, 30.0]);
    let before = session.design.clone();
    let generation = session.current_generation();

    session.nudge_angles(&[0], 1.0, ms(0)).unwrap_err();
    match session.take_refusal() {
        Some(SessionEditError::Relation(RelationError::OutOfRange { tier, value })) => {
            assert_eq!(tier, "C2");
            assert_eq!(value, 91.0);
        }
        other => panic!("expected an out-of-range refusal, got {other:?}"),
    }
    assert_eq!(session.design, before);
    assert_eq!(session.current_generation(), generation);
    // The typed entry point says it in words.
    let error = session
        .try_apply(Edit::RetargetAngles {
            changes: vec![(0, 40.0, 45.0)],
        })
        .unwrap_err();
    assert!(
        error.to_string().starts_with("C2 would come out at 95.00"),
        "{error}"
    );
}

#[test]
fn a_relation_that_cannot_be_kept_is_refused_with_the_reason() {
    let mut session = crown_session();
    session.set_tier_relation(0, "C2").unwrap();
    assert_eq!(angles(&session)[0], 38.0);
    let before = session.design.clone();

    let error = session.set_tier_relation(1, "C1").unwrap_err();
    assert_eq!(
        error,
        SessionEditError::Relation(RelationError::Cycle(vec![
            "C1".to_string(),
            "C2".to_string()
        ]))
    );
    assert_eq!(
        error.to_string(),
        "C1 and C2 refer to each other in a loop."
    );
    let error = session.set_tier_relation(2, "C3 + 1").unwrap_err();
    assert_eq!(error.to_string(), "C3 refers to itself.");
    assert!(matches!(
        session.set_tier_relation(2, "Q9"),
        Err(SessionEditError::Relation(RelationError::Parse(_)))
    ));
    assert_eq!(
        session.set_tier_relation(9, "C1"),
        Err(SessionEditError::Relation(RelationError::NoSuchTier {
            index: 9
        }))
    );
    assert_eq!(session.design, before);
}

#[test]
fn a_flat_or_girdle_tier_cannot_follow_a_relation() {
    let mut session = crown_session();
    session
        .apply(Edit::AddTier {
            index: 3,
            tier: crown("Table", 0.0),
        })
        .unwrap();
    session
        .apply(Edit::AddTier {
            index: 4,
            tier: crown("G1", 90.0),
        })
        .unwrap();
    assert!(matches!(
        session.set_tier_relation(3, "C1"),
        Err(SessionEditError::Relation(
            RelationError::HorizontalTier { .. }
        ))
    ));
    assert!(matches!(
        session.set_tier_relation(4, "C1"),
        Err(SessionEditError::Relation(RelationError::GirdleTier { .. }))
    ));
}

#[test]
fn removing_a_tier_other_relations_read_frees_them_with_a_notice() {
    let mut session = chained_session();
    let before = session.design.clone();

    session.remove_tier(0).unwrap();
    assert_eq!(angles(&session), vec![35.0, 32.0]);
    // C2 keeps its angle but follows nothing; C3 still follows C2.
    assert!(!session.is_driven(0));
    assert!(session.is_driven(1));
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C2 - 3"));
    session.design.evaluate_relations().unwrap();

    let notice = session.take_relation_notice().expect("a notice");
    assert_eq!(
        notice.cleared,
        vec![ClearedRelation {
            tier: "C2".to_string(),
            relation: "C1 - 5".to_string()
        }]
    );
    assert_eq!(
        notice.to_string(),
        "C2 (was C2 = C1 - 5) no longer follows a relation because a tier it read was \
         removed. It keeps its current angle."
    );
    assert!(session.take_relation_notice().is_none());

    // The removal and the freed relation are one undo step.
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before);
}

#[test]
fn removing_a_followed_tier_frees_the_relations_reading_it_too() {
    let mut session = chained_session();
    let before = session.design.clone();

    session.remove_tier(1).unwrap();
    assert_eq!(angles(&session), vec![40.0, 32.0]);
    assert!(session.design.tier_relations.is_empty());
    let notice = session.take_relation_notice().expect("a notice");
    assert_eq!(
        notice.cleared,
        vec![ClearedRelation {
            tier: "C3".to_string(),
            relation: "C2 - 3".to_string()
        }]
    );
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before);
    assert!(session.is_driven(1) && session.is_driven(2));
}

#[test]
fn several_freed_relations_are_listed_together() {
    let mut session = crown_session();
    session.set_tier_relation(1, "C1 - 5").unwrap();
    session.set_tier_relation(2, "C1 - 8").unwrap();
    session.remove_tier(0).unwrap();
    let notice = session.take_relation_notice().expect("a notice");
    assert_eq!(notice.cleared.len(), 2);
    assert_eq!(
        notice.to_string(),
        "C2 (was C2 = C1 - 5), C3 (was C3 = C1 - 8) no longer follow a relation because a \
         tier they read was removed. They keep their current angles."
    );
}

#[test]
fn adding_and_moving_tiers_leaves_the_relations_attached_to_their_tiers() {
    let mut session = crown_session();
    session.set_tier_relation(1, "C1 - 5").unwrap();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: crown("New", 20.0),
        })
        .unwrap();
    assert_eq!(angles(&session), vec![20.0, 40.0, 35.0, 30.0]);
    assert!(session.is_driven(2));
    assert_eq!(session.drivers_of(2), vec![1]);
    session.move_tier(2, -1).unwrap().expect("moved");
    assert!(session.is_driven(1));
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C1 - 5"));
    assert_eq!(angles(&session), vec![20.0, 35.0, 40.0, 30.0]);
}

#[test]
fn an_invalid_edit_is_still_reported_as_it_was() {
    let mut session = chained_session();
    let error = session.apply(Edit::RemoveTier { index: 9 }).unwrap_err();
    assert_eq!(error.index, 9);
    assert!(session.take_refusal().is_none());
    assert!(matches!(
        session.try_apply(Edit::RemoveTier { index: 9 }),
        Err(SessionEditError::Edit(_))
    ));
}

#[test]
fn the_inline_cell_takes_relations_and_names() {
    let mut session = crown_session();
    // `=` makes a relation.
    let applied = session.set_tier_angle_from_text(1, "=C1-4").unwrap();
    assert!(matches!(applied, InlineAngle::Applied(_)));
    assert_eq!(angles(&session), vec![40.0, 36.0, 30.0]);
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C1 - 4"));
    assert_eq!(
        session.set_tier_angle_from_text(1, "= C1 - 4"),
        Ok(InlineAngle::NoChange)
    );
    // Without `=`, a name is worked out once and nothing follows.
    let applied = session.set_tier_angle_from_text(2, "C1 - 9").unwrap();
    assert!(matches!(applied, InlineAngle::Applied(_)));
    assert_eq!(session.design.tiers[2].angle_deg, 31.0);
    assert!(!session.is_driven(2));
    // Plain arithmetic.
    session.set_tier_angle_from_text(2, "30 + 0.5").unwrap();
    assert_eq!(session.design.tiers[2].angle_deg, 30.5);

    let error = session.set_tier_angle_from_text(2, "Q9 - 9").unwrap_err();
    assert!(
        error.starts_with("Angle 'Q9 - 9' cannot be calculated:"),
        "{error}"
    );
    assert!(error.contains("there is no tier called 'Q9'"), "{error}");
    let error = session.set_tier_angle_from_text(2, "=Q9").unwrap_err();
    assert!(error.contains("there is no tier called 'Q9'"), "{error}");
    let error = session.set_tier_angle_from_text(2, "=").unwrap_err();
    assert!(error.contains("Type a relation"), "{error}");
}

#[test]
fn a_pavilion_tier_keeps_its_side_through_a_relation() {
    let mut session = EditorSession::fresh();
    for (index, (name, angle)) in [("P1", -40.0), ("P2", -38.0)].into_iter().enumerate() {
        session
            .apply(Edit::AddTier {
                index,
                tier: crown(name, angle),
            })
            .unwrap();
    }
    session.set_tier_relation(1, "P1 - 5").unwrap();
    assert_eq!(angles(&session), vec![-40.0, -35.0]);
    session.nudge_angles(&[0], -1.0, ms(0)).unwrap();
    assert_eq!(angles(&session), vec![-41.0, -36.0]);
}

#[test]
fn a_linked_step_series_follows_its_first_tier() {
    let mut session = EditorSession::fresh();
    let series = session
        .generate_step_series_linked("S", "40", "2", 3, "0 12", "")
        .unwrap();
    assert_eq!((series.start_index, series.added), (0, 3));
    assert_eq!(angles(&session), vec![40.0, 42.0, 44.0]);
    assert!(!session.is_driven(0));
    assert_eq!(session.drivers_of(1), vec![0]);
    assert_eq!(session.drivers_of(2), vec![0]);

    // Moving the first tier moves the ladder, in one undo step.
    session.nudge_angles(&[0], 1.0, ms(0)).unwrap();
    assert_eq!(angles(&session), vec![41.0, 43.0, 45.0]);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(angles(&session), vec![40.0, 42.0, 44.0]);

    // The series and its relations are one step too.
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design.tiers.len(), 0);
    assert!(session.design.tier_relations.is_empty());
}

#[test]
fn a_linked_pavilion_series_steps_away_from_the_girdle() {
    let mut session = EditorSession::fresh();
    session
        .generate_step_series_linked("S", "-40", "-2", 3, "0 12", "")
        .unwrap();
    assert_eq!(angles(&session), vec![-40.0, -42.0, -44.0]);
    session.nudge_angles(&[0], -1.0, ms(0)).unwrap();
    assert_eq!(angles(&session), vec![-41.0, -43.0, -45.0]);
}

/// An Optimize outcome carries signed angles. Applied through the session to a pavilion
/// ladder it moves the driver and the tier that follows it, in one undo step, and both stay
/// on the pavilion side of zero.
#[test]
fn an_optimize_outcome_keeps_a_pavilion_ladder_on_its_side() {
    use indicatrix_cut_core::{AngleChange, ObjectiveComponents, OptimizeOutcome};

    let mut session = EditorSession::fresh();
    for (index, (name, angle)) in [("P1", -40.0), ("P2", -38.0)].into_iter().enumerate() {
        session
            .apply(Edit::AddTier {
                index,
                tier: crown(name, angle),
            })
            .unwrap();
    }
    session.set_tier_relation(1, "P1 - 5").unwrap();
    assert_eq!(angles(&session), vec![-40.0, -35.0]);

    let components = ObjectiveComponents {
        windowing_pct: 0.0,
        extinction_pct: 0.0,
        tilt_brilliance_pct: 0.0,
    };
    let outcome = OptimizeOutcome {
        before: components,
        before_score: 1.0,
        before_yield_loss_pct: 0.0,
        after: components,
        after_score: 0.5,
        after_yield_loss_pct: 0.0,
        evaluations: 1,
        changes: vec![AngleChange {
            index: 0,
            from_deg: -40.0,
            to_deg: -41.5,
        }],
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    };
    let applied = session.apply_optimize_outcome(&outcome).unwrap();
    assert!(applied >= 1, "{applied}");
    assert_eq!(angles(&session), vec![-41.5, -36.5]);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(angles(&session), vec![-40.0, -35.0]);
}

#[test]
fn a_linked_series_that_would_leave_the_range_refuses_the_edit_that_does_it() {
    let mut session = EditorSession::fresh();
    session
        .generate_step_series_linked("S", "80", "5", 3, "0 12", "")
        .unwrap();
    assert_eq!(angles(&session), vec![80.0, 85.0, 90.0]);
    session.nudge_angles(&[0], 1.0, ms(0)).unwrap_err();
    assert!(matches!(
        session.take_refusal(),
        Some(SessionEditError::Relation(RelationError::OutOfRange { value, .. })) if value == 91.0
    ));
    assert_eq!(angles(&session), vec![80.0, 85.0, 90.0]);
}

#[test]
fn a_linked_series_after_existing_tiers_reads_its_own_first_tier() {
    let mut session = crown_session();
    let series = session
        .generate_step_series_linked("S", "20", "1", 2, "0 12", "")
        .unwrap();
    assert_eq!((series.start_index, series.added), (3, 2));
    assert_eq!(session.drivers_of(4), vec![3]);
    assert!(!session.is_driven(3));
    assert!(!session.is_driven(1));
    // A one-tier series has nothing to link.
    let single = session
        .generate_step_series_linked("T", "25", "1", 1, "0 12", "")
        .unwrap();
    assert_eq!(single.added, 1);
    assert!(!session.is_driven(5));
}

#[test]
fn the_plain_step_series_is_unchanged_and_links_nothing() {
    let mut session = EditorSession::fresh();
    session
        .generate_step_series("S", "40", "2", 3, "0 12", "")
        .unwrap();
    assert_eq!(angles(&session), vec![40.0, 42.0, 44.0]);
    assert!(session.design.tier_relations.is_empty());
}

#[test]
fn the_notice_text_is_plain_english() {
    let notice = RelationNotice {
        cleared: vec![ClearedRelation {
            tier: "P2".to_string(),
            relation: "P1 - 2".to_string(),
        }],
    };
    assert!(
        notice
            .to_string()
            .starts_with("P2 (was P2 = P1 - 2) no longer follows")
    );
    let error = SessionEditError::from(EditError {
        index: 3,
        tier_count: 2,
    });
    assert_eq!(
        error.to_string(),
        "tier index 3 out of range (schedule has 2 tier(s))"
    );
    assert!(std::error::Error::source(&error).is_some());
}

// --- a whole-schedule replacement replaces the relations too ----------------------------

/// The state of `session`'s design with the tier angles set to `new_angles`, in order.
fn state_with_angles(session: &EditorSession, new_angles: [f64; 3]) -> ScheduleState {
    let mut state = ScheduleState::of(&session.design);
    for (tier, angle) in state.tiers.iter_mut().zip(new_angles) {
        tier.angle_deg = angle;
    }
    state
}

fn replace(state: ScheduleState) -> Edit {
    Edit::ReplaceSchedule(Box::new(state))
}

#[test]
fn replacing_the_schedule_may_move_the_tiers_that_follow_a_relation() {
    let mut session = chained_session();
    let before = session.design.clone();
    let steps = session.history_entries().len();
    let state = state_with_angles(&session, [42.0, 37.0, 34.0]);

    session
        .try_apply(replace(state))
        .expect("a replacement that carries its own relations is not refused");
    assert_eq!(angles(&session), vec![42.0, 37.0, 34.0]);
    assert!(session.is_driven(1) && session.is_driven(2));
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C1 - 5"));
    assert_eq!(session.history_entries().len(), steps + 1, "one undo step");

    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before, "undo is exact");
    assert!(session.redo().unwrap().is_some());
    assert_eq!(angles(&session), vec![42.0, 37.0, 34.0]);
}

#[test]
fn a_replacement_brings_its_own_relations() {
    let mut session = chained_session();
    let before = session.design.clone();
    let mut state = state_with_angles(&session, [40.0, 36.0, 32.0]);
    let relation = session.design.parse_relation("C1 - 4").unwrap();
    state
        .tier_relations
        .insert(session.design.tier_ids[1], relation);

    session.try_apply(replace(state)).expect("applied");
    // C2 follows the new relation, and C3 (still C2 - 3) followed it in the same step.
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C1 - 4"));
    assert_eq!(angles(&session), vec![40.0, 36.0, 33.0]);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before);
    assert_eq!(session.tier_relation_display(1).as_deref(), Some("C1 - 5"));
}

#[test]
fn a_replacement_that_breaks_its_own_relations_is_made_true_in_the_same_step() {
    let mut session = chained_session();
    let before = session.design.clone();
    let steps = session.history_entries().len();
    // C2 and C3 carry angles their relations do not give.
    let state = state_with_angles(&session, [44.0, 50.0, 32.0]);

    session.try_apply(replace(state)).expect("applied");
    assert_eq!(angles(&session), vec![44.0, 39.0, 36.0]);
    assert_eq!(session.history_entries().len(), steps + 1, "still one step");
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before);
}

/// A design with no relation is the quick way out of `with_relations_folded`; a replacement
/// that brings relations must not take it, or its driven angles would stand as given.
#[test]
fn a_replacement_that_brings_relations_to_a_design_without_any_is_evaluated() {
    let mut session = crown_session();
    assert!(session.design.tier_relations.is_empty());
    let before = session.design.clone();
    let steps = session.history_entries().len();
    let mut state = ScheduleState::of(&session.design);
    let relation = session.design.parse_relation("C1 - 5").unwrap();
    state
        .tier_relations
        .insert(session.design.tier_ids[1], relation);
    // C2 stands at 38 in the state; its relation gives 35.
    assert_eq!(state.tiers[1].angle_deg, 38.0);

    session.try_apply(replace(state)).expect("applied");
    assert_eq!(angles(&session), vec![40.0, 35.0, 30.0]);
    assert!(session.is_driven(1));
    assert_eq!(session.history_entries().len(), steps + 1, "one undo step");
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before, "undo is exact");
}

#[test]
fn a_labelled_apply_words_the_step_and_reports_the_renumbering() {
    let mut session = chained_session();
    let words = "Open variant \"Wider crown\"";
    let state = state_with_angles(&session, [42.0, 37.0, 34.0]);

    let (change, renumbering) = session
        .try_apply_mapped(replace(state), Some(words))
        .expect("applied");
    assert_eq!(session.history_entries().last().unwrap().label, words);
    assert_eq!(session.history.undo_label(), Some(words));
    assert_eq!(session.undo_hint(), words, "the Undo hint says so too");
    assert_eq!(session.redo_hint(), "");
    assert_eq!(change.tier_count_after, 3);
    assert_eq!(
        [0, 1, 2].map(|row| renumbering.map(row)),
        [None, None, None],
        "no flat row can be followed through a replacement"
    );
    session.undo().unwrap().expect("undone");
    assert_eq!(session.redo_hint(), words, "and so does the Redo hint");
    session.redo().unwrap().expect("redone");

    // Without a label the step is worded by the edit, as `try_apply` always did.
    let state = state_with_angles(&session, [43.0, 38.0, 35.0]);
    session
        .try_apply_mapped(replace(state), None)
        .expect("applied");
    assert_eq!(
        session.history_entries().last().unwrap().label,
        "Edit instructions as text"
    );
    assert_eq!(session.history.undo_label(), None);
    assert_eq!(
        session.undo_hint(),
        "Edit instructions as text",
        "no words of its own: the edit's words, as before"
    );
}

/// The relation updates a replacement needs are folded into its step; the step keeps the
/// caller's words rather than reading "2 combined edits".
#[test]
fn a_labelled_replacement_that_needs_relation_updates_is_still_one_worded_step() {
    let mut session = chained_session();
    let steps = session.history_entries().len();
    let words = "Open variant \"Wider crown\"";
    // C2 and C3 carry angles their relations do not give.
    let state = state_with_angles(&session, [44.0, 50.0, 32.0]);

    session
        .try_apply_mapped(replace(state), Some(words))
        .expect("applied");
    assert_eq!(angles(&session), vec![44.0, 39.0, 36.0]);
    let entries = session.history_entries();
    assert_eq!(entries.len(), steps + 1, "one step");
    assert_eq!(entries.last().unwrap().label, words);
}

#[test]
fn a_batch_with_a_replacement_is_not_refused_but_a_later_move_of_a_driven_tier_is() {
    let mut session = chained_session();
    let state = state_with_angles(&session, [42.0, 37.0, 34.0]);
    let before = session.design.clone();

    // A replacement and an edit that touches nothing driven.
    let batch = Edit::Batch(vec![
        replace(state.clone()),
        Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(7.0),
        },
    ]);
    session.try_apply(batch).expect("applied");
    assert_eq!(angles(&session), vec![42.0, 37.0, 34.0]);
    assert_eq!(session.design.girdle_diameter_mm, Some(7.0));
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design, before);

    // The same replacement followed by a direct move of C2 from the NEW schedule.
    let mut moved = state.tiers[1].clone();
    moved.angle_deg = 20.0;
    let batch = Edit::Batch(vec![
        replace(state),
        Edit::ModifyTier {
            index: 1,
            tier: moved,
        },
    ]);
    let error = session.try_apply(batch).unwrap_err();
    assert!(matches!(&error, SessionEditError::Driven(refusal) if refusal.tier == 1));
    assert_eq!(session.design, before, "nothing changed");
}

#[test]
fn a_replacement_that_drops_a_tier_and_its_relation_is_clean() {
    let mut session = chained_session();
    // Without C1 (position 0) C2 cannot keep reading it: the state drops that relation and
    // keeps C3 = C2 - 3.
    let mut state = ScheduleState::of(&session.design);
    let c2 = state.tier_ids[1];
    state.tiers.remove(0);
    state.tier_ids.remove(0);
    state.tier_relations.remove(&c2);

    session.try_apply(replace(state)).expect("applied");
    assert_eq!(angles(&session), vec![35.0, 32.0]);
    assert!(!session.is_driven(0));
    assert!(session.is_driven(1));
    assert!(
        session.take_relation_notice().is_none(),
        "the replacement carries its relations whole, so nothing was cleared behind its back"
    );
}

// --- the notice of a removal does not outlive its command ----------------------------------

/// [`chained_session`] after removing C1 without taking the notice: C2 is freed.
fn session_with_untaken_notice() -> EditorSession {
    let mut session = chained_session();
    session.remove_tier(0).unwrap();
    assert!(session.relation_notice.is_some(), "a notice is waiting");
    session
}

#[test]
fn a_notice_nobody_took_is_dropped_by_the_next_edit() {
    let mut session = session_with_untaken_notice();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: crown("New", 20.0),
        })
        .unwrap();
    assert!(session.take_relation_notice().is_none());
}

#[test]
fn a_notice_nobody_took_is_dropped_by_an_undo_or_a_jump() {
    let mut session = session_with_untaken_notice();
    assert!(session.undo().unwrap().is_some());
    assert!(session.take_relation_notice().is_none());

    let mut session = session_with_untaken_notice();
    let position = session.history_position();
    session.jump_to(position - 1).expect("the jump works");
    assert!(session.take_relation_notice().is_none());

    let mut session = session_with_untaken_notice();
    session.jump_to(position).expect("a jump to where it is");
    assert!(session.take_relation_notice().is_none());
}

#[test]
fn a_notice_nobody_took_is_dropped_by_a_whole_schedule_replacement() {
    let mut session = session_with_untaken_notice();
    // C2 (free now) moves to 36; C3 still follows it.
    let mut state = ScheduleState::of(&session.design);
    state.tiers[0].angle_deg = 36.0;
    session.try_apply(replace(state)).expect("applied");
    assert_eq!(angles(&session), vec![36.0, 33.0]);
    assert!(session.take_relation_notice().is_none());
}

#[test]
fn a_notice_that_was_taken_is_not_left_behind() {
    let mut session = session_with_untaken_notice();
    assert!(session.take_relation_notice().is_some());
    assert!(session.take_relation_notice().is_none());
}

#[test]
fn removing_several_tiers_reports_every_freed_relation_once() {
    let mut session = crown_session();
    // C1 follows C3, so removing C3 (the first removal: highest row first) frees C1, and
    // C1 itself goes in the second removal, which frees nothing more.
    session.set_tier_relation(0, "C3 + 5").unwrap();
    session.multi_selected = [0, 2].into_iter().collect();
    assert_eq!(session.remove_multi_selected().unwrap(), 2);
    let notice = session.take_relation_notice().expect("C1 was freed");
    assert_eq!(notice.cleared.len(), 1);
    assert_eq!(notice.cleared[0].tier, "C1");
    assert!(notice.cleared[0].relation.contains("C3"));
    assert!(session.take_relation_notice().is_none());

    // The command is over: a stray notice no longer survives the next edit.
    let mut session = crown_session();
    session.set_tier_relation(0, "C3 + 5").unwrap();
    session.multi_selected = std::iter::once(2).collect();
    session.remove_multi_selected().unwrap();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: crown("New", 20.0),
        })
        .unwrap();
    assert!(session.take_relation_notice().is_none());
}
