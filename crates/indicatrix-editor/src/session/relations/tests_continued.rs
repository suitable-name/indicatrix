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
fn an_uncertified_optimize_outcome_cannot_move_a_pavilion_ladder() {
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
        provenance: None,
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
    assert!(session.apply_optimize_outcome(&outcome).is_err(), "a relation is not operation provenance");
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
        Some(SessionEditError::Relation(RelationError::OutOfRange { value, .. })) if (value - 91.0).abs() < 1e-9
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
