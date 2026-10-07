//! `Edit::ReplaceSchedule`: swapping the whole tier list in one step, its exact undo and
//! redo, and what it refuses.

use super::fixtures::{fresh_design, tier};
use crate::{
    design::{ConcaveTier, ConcaveTool, Design, ToolMotion},
    edit::{Edit, EditError, History, ScheduleState},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Three tiers, P1 (-41), P2 (-35) and C1 (30), with ids 0, 1 and 2, a note on P2 and a
/// cheater offset on C1.
fn three_tier_design() -> Design {
    let mut design = fresh_design();
    for (name, angle) in [("P1", -41.0), ("P2", -35.0), ("C1", 30.0)] {
        design.tiers.push(tier(
            name,
            angle,
            MeetConstraint::ScaleReference(0.5),
            &[0.0, 24.0],
        ));
    }
    design.ensure_tier_ids();
    design.tier_notes.insert(1, "check the meet".to_owned());
    design.cheater_offsets_deg.insert(2, 1.5);
    design
}

#[test]
fn replacing_the_schedule_then_undo_and_redo_is_exact() {
    let mut design = three_tier_design();
    let before = design.clone();
    let mut state = ScheduleState::of(&design);
    // Drop P2 (and its note), add Z1, and change the gear.
    state.tiers.remove(1);
    let dropped = state.tier_ids.remove(1);
    state.tier_notes.clear();
    state.cheater_offsets_deg = [(1, 1.5)].into();
    state.tiers.push(tier(
        "Z1",
        10.0,
        MeetConstraint::ScaleReference(0.2),
        &[0.0],
    ));
    state
        .tier_ids
        .push(crate::design::TierId(state.next_tier_id));
    state.next_tier_id += 1;
    state.meta.gear_teeth = 80;
    let after_state = state.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::ReplaceSchedule(Box::new(state)))
        .expect("the state applies");
    let names: Vec<&str> = design.tiers.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["P1", "C1", "Z1"]);
    assert_eq!(design.meta.gear_teeth, 80);
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.cheater_offset_deg(1), Some(1.5));
    assert!(!design.tier_ids.contains(&dropped));
    assert_eq!(ScheduleState::of(&design).tiers, after_state.tiers);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(design.tier_ids_eq(&before));
    assert_eq!(design.tier_note(1), Some("check the meet"));

    assert!(history.redo(&mut design).unwrap());
    let names: Vec<&str> = design.tiers.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["P1", "C1", "Z1"]);
    assert_eq!(design.meta.gear_teeth, 80);
}

#[test]
fn an_id_handed_out_by_the_replacement_is_never_handed_out_again_after_undo() {
    let mut design = three_tier_design();
    let mut state = ScheduleState::of(&design);
    state.tiers.push(tier(
        "Z1",
        10.0,
        MeetConstraint::ScaleReference(0.2),
        &[0.0],
    ));
    state
        .tier_ids
        .push(crate::design::TierId(state.next_tier_id));
    state.next_tier_id += 1;
    let counter_after = state.next_tier_id;
    let mut history = History::new();

    history
        .apply(&mut design, Edit::ReplaceSchedule(Box::new(state)))
        .unwrap();
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design.next_tier_id, counter_after,
        "undo must not rewind the id counter"
    );
}

/// What opening a saved variant records: the step reads as the cutter's action, not as the
/// edit it is made of ("Edit instructions as text"), and keeps those words for good.
#[test]
fn a_labelled_replacement_keeps_its_words_through_undo_redo_and_the_log() {
    let mut design = three_tier_design();
    let mut state = ScheduleState::of(&design);
    state.tiers[0].angle_deg = -42.0;
    let replacement = Edit::ReplaceSchedule(Box::new(state));
    assert_eq!(
        replacement.describe(&design),
        "Edit instructions as text",
        "the edit's own words are the text editor's"
    );
    let mut history = History::new();
    assert_eq!(history.undo_label(), None, "no step, no words");

    history
        .apply_labeled(&mut design, replacement, "Open variant \"Steeper crown\"")
        .expect("the state applies");
    let words = "Open variant \"Steeper crown\"";
    assert_eq!(history.entries()[0].label, words);
    assert_eq!(history.description_log(), [words]);
    assert_eq!(
        history.undo_label(),
        Some(words),
        "the Undo hint says so too"
    );
    assert_eq!(history.redo_label(), None);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        history.entries()[0].label,
        words,
        "undone, still the same step"
    );
    assert_eq!(history.undo_label(), None);
    assert_eq!(history.redo_label(), Some(words));

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(history.entries()[0].label, words);
    assert_eq!(history.undo_label(), Some(words));
}

/// A step worded by `Edit::describe` leaves the hints to the caller, and a blank label is no
/// label.
#[test]
fn an_unlabelled_or_blank_labelled_replacement_is_worded_by_the_edit() {
    let mut design = three_tier_design();
    let mut history = History::new();
    let state = ScheduleState::of(&design);
    history
        .apply(&mut design, Edit::ReplaceSchedule(Box::new(state.clone())))
        .expect("applies");
    assert_eq!(history.entries()[0].label, "Edit instructions as text");
    assert_eq!(history.undo_label(), None);

    history
        .apply_labeled(&mut design, Edit::ReplaceSchedule(Box::new(state)), "   ")
        .expect("applies");
    assert_eq!(
        history.entries()[1].label,
        "Edit instructions as text",
        "a blank label falls back to the edit's words"
    );
    assert_eq!(history.undo_label(), None);
}

/// A refused edit leaves no step, labelled or not.
#[test]
fn a_refused_labelled_replacement_records_nothing() {
    let mut design = three_tier_design();
    let before = design.clone();
    let mut state = ScheduleState::of(&design);
    state.meta.gear_teeth = 0;
    let mut history = History::new();
    assert!(
        history
            .apply_labeled(
                &mut design,
                Edit::ReplaceSchedule(Box::new(state)),
                "Open it"
            )
            .is_err()
    );
    assert_eq!(design, before);
    assert_eq!(history.len_total(), 0);
    assert_eq!(history.description_log(), Vec::<String>::new());
}

#[test]
fn a_zero_gear_or_symmetry_is_refused_and_nothing_changes() {
    let mut design = three_tier_design();
    let before = design.clone();
    for (gear, symmetry) in [(0, 4), (96, 0)] {
        let mut state = ScheduleState::of(&design);
        state.meta.gear_teeth = gear;
        state.meta.symmetry_order = symmetry;
        let result = design.apply_edit(Edit::ReplaceSchedule(Box::new(state)));
        assert!(result.is_err(), "gear {gear}, symmetry {symmetry}");
    }
    assert_eq!(design, before);
}

#[test]
fn ids_that_do_not_fit_the_tiers_are_refused_and_nothing_changes() {
    let mut design = three_tier_design();
    let before = design.clone();

    let mut short = ScheduleState::of(&design);
    short.tier_ids.pop();
    assert!(
        design
            .apply_edit(Edit::ReplaceSchedule(Box::new(short)))
            .is_err()
    );

    let mut repeated = ScheduleState::of(&design);
    repeated.tier_ids[2] = repeated.tier_ids[0];
    assert!(
        design
            .apply_edit(Edit::ReplaceSchedule(Box::new(repeated)))
            .is_err()
    );

    let mut stray_note = ScheduleState::of(&design);
    stray_note.tier_notes.insert(7, "nobody".to_owned());
    assert_eq!(
        design.apply_edit(Edit::ReplaceSchedule(Box::new(stray_note))),
        Err(EditError {
            index: 7,
            tier_count: 3
        })
    );

    let mut stray_target = ScheduleState::of(&design);
    stray_target.tier_targets.insert(
        crate::design::TierId(99),
        crate::design::TierTarget::DepthMm(3.0),
    );
    assert!(
        design
            .apply_edit(Edit::ReplaceSchedule(Box::new(stray_target)))
            .is_err()
    );
    assert_eq!(design, before);
}

#[test]
fn a_relation_that_reads_a_missing_tier_is_refused() {
    let mut design = three_tier_design();
    let before = design.clone();
    let relation = design.parse_relation("P1 - 6").expect("the relation reads");
    let mut state = ScheduleState::of(&design);
    // P1 is removed but P2 still follows it.
    let driven = state.tier_ids[1];
    state.tier_relations.insert(driven, relation);
    state.tiers.remove(0);
    state.tier_ids.remove(0);
    state.tier_notes.clear();
    state.cheater_offsets_deg.clear();
    assert!(
        design
            .apply_edit(Edit::ReplaceSchedule(Box::new(state)))
            .is_err()
    );
    assert_eq!(design, before);
}

#[test]
fn a_tier_named_like_a_concave_tier_is_refused() {
    let mut design = three_tier_design();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: ConcaveTier {
                name: "Groove".to_owned(),
                angle_deg: -40.0,
                indices: vec![0.0, 24.0],
                instructions: String::new(),
                tool: ConcaveTool::Cylinder,
                tool_azimuth_deg: 0.0,
                displacement: [0.0, 0.0, 0.1],
                diameter_ratio: 0.5,
                tool_angle_deg: None,
                motion: ToolMotion::Reciprocating,
            },
        })
        .unwrap();
    let before = design.clone();
    let mut state = ScheduleState::of(&design);
    state.tiers[1].name = "Groove".to_owned();
    assert_eq!(
        design.apply_edit(Edit::ReplaceSchedule(Box::new(state))),
        Err(EditError {
            index: 1,
            tier_count: 3
        })
    );
    assert_eq!(design, before);
}

#[test]
fn the_history_label_reads_in_plain_words() {
    let design = three_tier_design();
    let edit = Edit::ReplaceSchedule(Box::new(ScheduleState::of(&design)));
    assert_eq!(edit.describe(&design), "Edit instructions as text");
}
