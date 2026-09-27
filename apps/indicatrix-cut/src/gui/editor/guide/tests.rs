//! The step content's own invariants, and the goal predicates behind the
//! automatic advance.

use super::{
    MANUAL, NEW_DESIGN_CREATED,
    progress::{goal_reached, same_index_set},
    steps::{EIGHT_FOLD_INDICES, Group, STEPS},
};
use crate::gui::editor::state::EditorState;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design, Edit};

/// Every completion key a step may use: [`MANUAL`], [`NEW_DESIGN_CREATED`] (an
/// event), and the state goals `progress::goal_reached` evaluates. A typo in a
/// step's key would otherwise silently mean "never completes".
const COMPLETION_KEYS: &[&str] = &[
    MANUAL,
    NEW_DESIGN_CREATED,
    "tier_named:G1",
    "tier_named:P1",
    "tier_named:C1",
    "tier_named:T",
    "material:Diamond",
    "solved_closed",
    "yield_applied",
];

/// Every `highlight_target` some `.slint` component actually checks for (the New
/// Design dialog -- plus the New Design... button and empty-state card while it is
/// closed -- the inspector's Tier tab, Design Settings, the Solve button, the tier
/// table, the inspector's Preform tab), or `""` for nothing. A typo here would
/// silently mean "no highlight".
const HIGHLIGHT_TARGETS: &[&str] = &[
    "",
    "new_design_dialog",
    "inspector_tier",
    "design_settings",
    "solve_button",
    "tier_table",
    "preform_tab",
];

#[test]
fn every_step_uses_a_known_completion_key_and_a_recognized_highlight_target() {
    for step in STEPS {
        assert!(
            COMPLETION_KEYS.contains(&step.completion),
            "step {:?} has an unknown completion key {:?}",
            step.title,
            step.completion
        );
        assert!(
            HIGHLIGHT_TARGETS.contains(&step.highlight_target),
            "step {:?} has an unrecognized highlight_target {:?}",
            step.title,
            step.highlight_target
        );
    }
}

#[test]
fn every_action_step_says_what_to_do_and_what_it_waits_for() {
    for step in STEPS {
        assert_ne!(step.title, "");
        assert_ne!(step.intro, "");
        if step.completion != MANUAL {
            assert!(
                !step.actions.is_empty(),
                "step {:?} has no actions",
                step.title
            );
            assert_ne!(
                step.waiting, "",
                "step {:?} has no waiting text",
                step.title
            );
            assert_ne!(step.check, "", "step {:?} has no check line", step.title);
        }
        for action in step.actions {
            assert_ne!(*action, "", "step {:?} has a blank action line", step.title);
        }
    }
}

#[test]
fn step_titles_are_unique() {
    for (i, step) in STEPS.iter().enumerate() {
        for other in &STEPS[i + 1..] {
            assert_ne!(step.title, other.title, "duplicate guide step title");
        }
    }
}

/// The walkthrough as the manual's chapter 7 lays it out: ten steps, the two
/// reading steps (8, "Check the orbits", and 10, the closing note) wait for Next,
/// and the closing step locks nothing.
#[test]
fn the_walkthrough_is_ten_steps_and_ends_unlocked() {
    assert_eq!(STEPS.len(), 10);
    let manual: Vec<&str> = STEPS
        .iter()
        .filter(|step| step.completion == MANUAL)
        .map(|step| step.title)
        .collect();
    assert_eq!(manual, ["Check the orbits", "You have a working design"]);
    let last = STEPS.last().expect("ten steps");
    for group in [
        Group::NewDesign,
        Group::TierForm,
        Group::TierTable,
        Group::DesignSettings,
        Group::Solve,
        Group::PreformTab,
        Group::Advanced,
        Group::ViewTabs,
        Group::FileOps,
        Group::History,
    ] {
        assert!(last.allow.contains(&group), "closing step locks {group:?}");
    }
}

/// Each action step unlocks the control its own actions name.
#[test]
fn each_action_step_unlocks_its_own_control() {
    let unlocks = |completion: &str, group: Group| {
        STEPS
            .iter()
            .find(|step| step.completion == completion)
            .is_some_and(|step| step.allow.contains(&group))
    };
    assert!(unlocks(NEW_DESIGN_CREATED, Group::NewDesign));
    for tier in ["G1", "P1", "C1", "T"] {
        assert!(unlocks(&format!("tier_named:{tier}"), Group::TierForm));
    }
    assert!(unlocks("material:Diamond", Group::DesignSettings));
    assert!(unlocks("solved_closed", Group::Solve));
    assert!(unlocks("yield_applied", Group::PreformTab));
    // The first step must not leave file actions open: a Load Selected there would
    // replace the design the walkthrough is about to build.
    assert!(!unlocks(NEW_DESIGN_CREATED, Group::FileOps));
}

fn tier(name: &str, angle_deg: f64, indices: &[f64]) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(1.0),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// A fresh 96-tooth, 8-fold design (`EditorState::fresh`) with `tiers` added
/// through `History`, the way the tier form adds them.
fn design_with(tiers: &[ConstraintTier]) -> Design {
    let mut state = EditorState::fresh();
    for (index, tier) in tiers.iter().enumerate() {
        state
            .apply(Edit::AddTier {
                index,
                tier: tier.clone(),
            })
            .expect("add tier");
    }
    state.design
}

#[test]
fn tier_goals_need_the_right_name_angle_and_indices() {
    let good = design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    assert!(goal_reached("tier_named:G1", &good, false));
    // Case and surrounding whitespace in the typed name do not matter.
    assert!(goal_reached(
        "tier_named:G1",
        &design_with(&[tier(" g1", 90.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    // The classic mistake the step warns about: 0.0 instead of 90.0.
    assert!(!goal_reached(
        "tier_named:G1",
        &design_with(&[tier("G1", 0.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    // One index short is not the girdle the step asked for.
    assert!(!goal_reached(
        "tier_named:G1",
        &design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES[..7])]),
        false
    ));
    // Right tier, wrong name.
    assert!(!goal_reached(
        "tier_named:G1",
        &design_with(&[tier("Girdle", 90.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    // A different step's tier does not complete this one.
    assert!(!goal_reached("tier_named:P1", &good, false));
    assert!(goal_reached(
        "tier_named:P1",
        &design_with(&[tier("P1", -40.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    assert!(goal_reached(
        "tier_named:C1",
        &design_with(&[tier("C1", 34.5, &EIGHT_FOLD_INDICES)]),
        false
    ));
    assert!(goal_reached(
        "tier_named:T",
        &design_with(&[tier("T", 0.0, &[])]),
        false
    ));
    assert!(!goal_reached(
        "tier_named:T",
        &design_with(&[tier("T", 0.0, &[12.0])]),
        false
    ));
}

#[test]
fn index_sets_compare_as_gear_positions() {
    assert!(same_index_set(&[84.0, 0.0, 12.0], &[0.0, 12.0, 84.0], 96.0));
    assert!(same_index_set(&[96.0], &[0.0], 96.0));
    assert!(same_index_set(&[], &[], 96.0));
    assert!(same_index_set(&[0.0], &[], 96.0));
    assert!(!same_index_set(&[0.0, 12.0], &[0.0, 24.0], 96.0));
    assert!(!same_index_set(&[0.0, 0.0], &[0.0, 12.0], 96.0));
}

#[test]
fn material_solve_and_yield_goals_read_the_design() {
    let mut design = design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    assert!(!goal_reached("material:Diamond", &design, false));
    design.material.name = Some("Diamond".to_string());
    assert!(goal_reached("material:Diamond", &design, false));

    assert!(goal_reached("solved_closed", &design, true));
    assert!(!goal_reached("solved_closed", &design, false));
    // A zero-tier design "solves" to its bare preform, which is not the stone.
    assert!(!goal_reached("solved_closed", &design_with(&[]), true));

    assert!(!goal_reached("yield_applied", &design, false));
    design.girdle_diameter_mm = Some(6.5);
    assert!(goal_reached("yield_applied", &design, false));
}

/// Neither a reading step nor the new-design event can be completed by a design
/// predicate: the first only advances on Next, the second only through the
/// explicit `notify` in `do_new_design_create`'s success path, so a rejected form
/// (which never reaches it) can never advance.
#[test]
fn manual_and_event_keys_never_complete_from_state() {
    let design = design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    assert!(!goal_reached(MANUAL, &design, true));
    assert!(!goal_reached(NEW_DESIGN_CREATED, &design, true));
    assert!(!goal_reached("tier_named:X9", &design, true));
}
