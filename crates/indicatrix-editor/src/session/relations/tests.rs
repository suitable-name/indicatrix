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

include!("tests_continued.rs");
