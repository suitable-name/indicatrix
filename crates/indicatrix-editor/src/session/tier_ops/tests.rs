//! Tests for the tier table's structural edits: duplicate, remove (and its refusal
//! while other tiers still meet the tier by name), move, adopt, mirror, generate steps
//! and the inline angle commit.

use super::*;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::PreformSpec;
use indicatrix_formats::asc::is_asc_safe_tier_name;

fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0],
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn named(names: &[&str]) -> MeetConstraint {
    MeetConstraint::MeetNamed(names.iter().map(|name| (*name).to_string()).collect())
}

fn fresh_session() -> EditorSession {
    let spec = crate::loading::parse_new_design_form(
        96,
        PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        "8",
        true,
        0,
    )
    .expect("the default New Design form parses");
    EditorSession::from_spec(spec)
}

fn session_with_two_tiers() -> EditorSession {
    let mut session = fresh_session();
    let mut crown = tier("C1", 41.0, MeetConstraint::ScaleReference(0.5));
    crown.imported_meet = Some(MeetConstraint::MeetExisting);
    session.design.tiers.push(crown);
    session
        .design
        .tiers
        .push(tier("P1", -41.0, MeetConstraint::ScaleReference(0.9)));
    session
}

/// P1 (pinned), P2 (meets P1), P3 (meets `p1` and P2), C1 (meets nothing), all added
/// through `History` so they carry tier ids.
fn session_with_dependants() -> EditorSession {
    let mut session = fresh_session();
    let tiers = [
        tier("P1", -40.0, MeetConstraint::ScaleReference(0.5)),
        tier("P2", -42.0, named(&["P1"])),
        tier("P3", -44.0, named(&["p1", "P2"])),
        tier("C1", 41.0, MeetConstraint::ScaleReference(0.6)),
    ];
    for (index, t) in tiers.into_iter().enumerate() {
        session.apply(Edit::AddTier { index, tier: t }).unwrap();
    }
    session
}

fn tier_names(session: &EditorSession) -> Vec<&str> {
    session
        .design
        .tiers
        .iter()
        .map(|t| t.name.as_str())
        .collect()
}

#[test]
fn selection_follows_a_removal() {
    assert_eq!(selection_after_remove(Some(2), 2), None);
    assert_eq!(selection_after_remove(Some(3), 2), Some(2));
    assert_eq!(selection_after_remove(Some(1), 2), Some(1));
    assert_eq!(selection_after_remove(None, 2), None);
}

#[test]
fn duplicate_inserts_after_the_source_with_a_counted_name() {
    let mut session = session_with_two_tiers();
    let outcome = session.duplicate_tier(0).unwrap().expect("tier 0 exists");
    assert_eq!(outcome.new_index, 1);
    assert_eq!(outcome.source_label, "C1");
    assert_eq!(outcome.duplicate_label, "C1 (2)");
    assert_eq!(session.design.tiers[1].name, "C1 (2)");
    assert!(session.design.tiers[1].imported_meet.is_none());
    assert_eq!(session.design.tiers.len(), 3);
    assert!(session.duplicate_tier(9).unwrap().is_none());
    // One undo step.
    session.undo().unwrap();
    assert_eq!(session.design.tiers.len(), 2);
}

#[test]
fn remove_reports_the_name_and_facet_count() {
    let mut session = session_with_two_tiers();
    let removed = session.remove_tier(1).unwrap();
    assert_eq!(removed.name, "P1");
    assert_eq!(removed.facet_count, 8);
    assert!(session.remove_tier(5).is_err());
}

#[test]
fn removing_a_met_tier_is_refused_and_names_the_dependants() {
    let mut session = session_with_dependants();
    let generation = session.current_generation();
    let err = session.remove_tier(0).unwrap_err();
    let RemoveTierError::HasDependants {
        subject,
        dependants,
        labels,
    } = &err
    else {
        panic!("expected the dependants error, got {err:?}");
    };
    assert_eq!(subject, "P1");
    assert_eq!(*dependants, vec![1, 2]);
    assert_eq!(
        *labels,
        vec!["tier 2 (P2)".to_string(), "tier 3 (P3)".to_string()]
    );
    let message = err.to_string();
    assert!(
        message.contains("P1")
            && message.contains("tier 2 (P2)")
            && message.contains("tier 3 (P3)"),
        "{message}"
    );
    assert_eq!(tier_names(&session), ["P1", "P2", "P3", "C1"]);
    assert_eq!(session.current_generation(), generation, "nothing changed");

    // A tier nobody meets still goes without any flag.
    assert_eq!(session.remove_tier(3).unwrap().name, "C1");
    // The error for a tier that does not exist is the edit's own.
    assert!(matches!(
        session.remove_tier(9).unwrap_err(),
        RemoveTierError::Edit(_)
    ));
}

#[test]
fn a_cascading_removal_clears_the_references_in_one_undo_step() {
    let mut session = session_with_dependants();
    let removed = session.remove_tier_with(0, true).unwrap();
    assert_eq!(removed.name, "P1");
    assert_eq!(tier_names(&session), ["P2", "P3", "C1"]);
    // P2 lost its only name; P3 keeps the one that still exists.
    assert_eq!(
        session.design.tiers[0].constraint,
        MeetConstraint::MeetExisting
    );
    assert_eq!(session.design.tiers[1].constraint, named(&["P2"]));

    assert!(session.undo().unwrap().is_some());
    assert_eq!(tier_names(&session), ["P1", "P2", "P3", "C1"]);
    assert_eq!(session.design.tiers[1].constraint, named(&["P1"]));
    assert_eq!(session.design.tiers[2].constraint, named(&["p1", "P2"]));
}

#[test]
fn a_multi_delete_is_refused_while_an_unselected_tier_meets_a_selected_one() {
    let mut session = session_with_dependants();
    session.multi_selected.extend([0, 1]);
    let err = session.remove_multi_selected().unwrap_err();
    let RemoveTierError::HasDependants { dependants, .. } = &err else {
        panic!("expected the dependants error, got {err:?}");
    };
    assert_eq!(*dependants, vec![2], "only P3 sits outside the selection");
    assert_eq!(tier_names(&session), ["P1", "P2", "P3", "C1"]);
    assert_eq!(session.multi_selected, BTreeSet::from([0, 1]));

    // Selecting the dependant too leaves nobody pointing at a removed name.
    session.multi_selected.insert(2);
    assert_eq!(session.remove_multi_selected().unwrap(), 3);
    assert_eq!(tier_names(&session), ["C1"]);
    assert!(session.multi_selected.is_empty());
}

#[test]
fn a_cascading_multi_delete_clears_the_outside_references() {
    let mut session = session_with_dependants();
    session.multi_selected.extend([0, 1]);
    assert_eq!(session.remove_multi_selected_with(true).unwrap(), 2);
    assert_eq!(tier_names(&session), ["P3", "C1"]);
    assert_eq!(
        session.design.tiers[0].constraint,
        MeetConstraint::MeetExisting
    );
}

#[test]
fn move_stops_at_the_ends() {
    let mut session = session_with_two_tiers();
    assert!(session.move_tier(0, -1).unwrap().is_none());
    assert!(session.move_tier(1, 1).unwrap().is_none());
    let moved = session.move_tier(0, 1).unwrap().expect("can move down");
    assert_eq!(moved.target, 1);
    assert_eq!(session.design.tiers[1].name, "C1");
}

#[test]
fn adopt_switches_only_tiers_with_an_imported_meet() {
    let mut session = session_with_two_tiers();
    session.multi_selected.insert(1);
    assert_eq!(session.adopt_selected_imported_meets().unwrap(), 0);
    assert_eq!(session.adopt_all_imported_meets().unwrap(), 1);
    assert_eq!(
        session.design.tiers[0].constraint,
        MeetConstraint::MeetExisting
    );
}

#[test]
fn mirror_appends_the_negated_tier() {
    let mut session = session_with_two_tiers();
    let outcome = session
        .mirror_tier_to_other_block(0, "'")
        .unwrap()
        .expect("tier 0 exists");
    assert_eq!(outcome.new_index, 2);
    assert!(session.design.tiers[2].angle_deg < 0.0);
    assert_eq!(outcome.label, "C1'");
}

#[test]
fn mirroring_twice_never_repeats_a_name() {
    let mut session = session_with_two_tiers();
    let first = session.mirror_tier_to_other_block(0, "'").unwrap().unwrap();
    let second = session.mirror_tier_to_other_block(0, "'").unwrap().unwrap();
    assert_eq!(first.label, "C1'");
    assert_eq!(second.label, "C1'2");
    assert_eq!(session.design.tiers[second.new_index].name, "C1'2");
    // The label clashes case-insensitively with a tier that is only a token of a
    // multi-name too.
    session.design.tiers[0].name = "x/p1'".to_string();
    let third = session.mirror_tier_to_other_block(1, "'").unwrap().unwrap();
    assert_eq!(third.label, "P1'2");
}

#[test]
fn a_mirrored_name_is_whitespace_free_and_an_unnamed_source_gets_a_block_name() {
    let mut session = session_with_two_tiers();
    session.design.tiers[0].name = "Crown Main".to_string();
    let spaced = session
        .mirror_tier_to_other_block(0, " a b")
        .unwrap()
        .unwrap();
    assert_eq!(spaced.label, "Crown_Maina_b");
    assert!(is_asc_safe_tier_name(&spaced.label));

    session.design.tiers[1].name = String::new();
    let unnamed = session.mirror_tier_to_other_block(1, "'").unwrap().unwrap();
    assert_eq!(
        unnamed.label, "C1",
        "the mirror of a pavilion tier is a crown tier"
    );
    assert!(session.design.tiers[unnamed.new_index].angle_deg > 0.0);
}

#[test]
fn generate_appends_a_ladder_in_one_step() {
    let mut session = session_with_two_tiers();
    let series = session
        .generate_step_series("Step", "30", "2", 3, "0", "")
        .unwrap();
    assert_eq!((series.start_index, series.added), (2, 3));
    assert_eq!(session.design.tiers.len(), 5);
    assert_eq!(
        tier_names(&session),
        ["C1", "P1", "Step1", "Step2", "Step3"]
    );
    session.undo().unwrap();
    assert_eq!(session.design.tiers.len(), 2);
    assert!(
        session
            .generate_step_series("Step", "x", "2", 3, "0", "")
            .is_err()
    );
}

#[test]
fn a_second_ladder_with_the_same_prefix_continues_the_numbering() {
    let mut session = session_with_two_tiers();
    session
        .generate_step_series("Step", "30", "2", 2, "0", "")
        .unwrap();
    session
        .generate_step_series("step", "20", "2", 2, "0", "")
        .unwrap();
    assert_eq!(
        tier_names(&session),
        ["C1", "P1", "Step1", "Step2", "step3", "step4"]
    );
}

#[test]
fn a_blank_prefix_names_the_ladder_by_block() {
    let mut session = session_with_two_tiers();
    session
        .generate_step_series("", "30", "2", 2, "0", "")
        .unwrap();
    assert_eq!(tier_names(&session), ["C1", "P1", "C2", "C3"]);
}

#[test]
fn generate_refuses_a_runaway_count_and_bad_angles_without_touching_the_design() {
    let mut session = session_with_two_tiers();
    for count in [0, 65, i32::MAX] {
        let message = session
            .generate_step_series("Step", "30", "1", count, "0", "")
            .unwrap_err();
        assert!(message.contains("1 to 64"), "{message}");
    }
    // 80, 85, 90, 95: past a right angle.
    let past_ninety = session
        .generate_step_series("Step", "80", "5", 4, "0", "")
        .unwrap_err();
    assert!(past_ninety.contains("Tier 4"), "{past_ninety}");
    // -40, -10, +20: the third rung crosses into the crown block.
    let crossing = session
        .generate_step_series("Step", "-40", "30", 3, "0", "")
        .unwrap_err();
    assert!(crossing.contains("Tier 3"), "{crossing}");
    assert_eq!(session.design.tiers.len(), 2);
    assert_eq!(session.current_generation(), 0, "nothing was applied");
}

#[test]
fn inline_angle_text_is_a_no_op_when_bit_identical() {
    let mut session = session_with_two_tiers();
    let before = session.current_generation();
    assert_eq!(
        session.set_tier_angle_from_text(0, "41").unwrap(),
        InlineAngle::NoChange
    );
    assert_eq!(session.current_generation(), before);
    assert!(matches!(
        session.set_tier_angle_from_text(0, "42.5").unwrap(),
        InlineAngle::Applied(_)
    ));
    assert_eq!(
        session.design.tiers[0].angle_deg.to_bits(),
        42.5_f64.to_bits()
    );
    assert_eq!(
        session.set_tier_angle_from_text(9, "1").unwrap(),
        InlineAngle::Missing
    );
    assert!(session.set_tier_angle_from_text(0, "abc").is_err());
}
