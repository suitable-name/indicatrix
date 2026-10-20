//! Apply uses the completed preservation check, not a legacy angle-only proposal.
use super::{
    apply::{RetargetApplyError, apply_pending_retarget},
    check_run::set_check_for_test,
};
use crate::gui::editor::state::EditorState;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    BuiltinMaterials, CANONICAL_LIGHTING_PRESET, ConstraintTier, Design, History,
    MaterialSelection, PreformSpec, ScheduleMeta,
};
use indicatrix_editor::retarget::{
    CrownShift, RetargetProposal, build_plan,
    check::{CheckInputs, RetargetCheck, check_retarget},
};
use std::sync::atomic::Ordering;

/// Isolate the thread-local worker result even when an assertion unwinds.
struct HeldCheck;

impl HeldCheck {
    fn new(generation: u64, check: Option<RetargetCheck>) -> Self {
        set_check_for_test(generation, check);
        Self
    }
}

impl Drop for HeldCheck {
    fn drop(&mut self) {
        set_check_for_test(0, None);
    }
}

struct ShiftFixture {
    state: EditorState,
    proposal: RetargetProposal,
    check: RetargetCheck,
    material: MaterialSelection,
}

/// The same 1.72 -> sapphire brilliant used by the shared Retarget regressions.
/// It has real closed geometry and needs checked mast changes, including the table.
fn checked_shift() -> ShiftFixture {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.material = MaterialSelection {
        refractive_index_override: Some(1.72),
        ..MaterialSelection::default()
    };
    let material = MaterialSelection {
        name: Some("Sapphire".into()),
        refractive_index_override: Some(1.7681),
        ..MaterialSelection::default()
    };
    let target = material.resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &target, CrownShift::default(), &[]);
    let check = check_retarget(&CheckInputs {
        girdle: None,
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: CANONICAL_LIGHTING_PRESET,
    });
    assert!(check.allows_apply(), "{:?}", check.validity.reasons);
    check
        .validated_edit(&design)
        .expect("a certified Shift candidate");
    let mut state = EditorState::fresh();
    state.design = design;
    state.history = History::new();
    ShiftFixture {
        state,
        proposal: plan.proposal(),
        check,
        material,
    }
}

#[test]
fn apply_pending_retarget_commits_the_checked_geometry_and_solves() {
    let ShiftFixture {
        mut state,
        proposal,
        check,
        ..
    } = checked_shift();
    let original = state.design.clone();
    let accepted = check.candidate.clone().expect("checked geometry");
    let count = proposal.rows.len();
    assert_ne!(
        accepted.tiers, original.tiers,
        "this fixture must actually move"
    );
    let generation = state.generation.load(Ordering::Relaxed);
    let _held = HeldCheck::new(generation, Some(check));
    let applied = apply_pending_retarget(&mut state, (proposal, generation), None)
        .unwrap_or_else(|_| panic!("the current checked candidate must apply"));
    assert_eq!(applied, count);
    assert_eq!(state.design, accepted);
    assert!(state.design.solve().is_ok());
    assert!(state.undo().unwrap());
    assert_eq!(state.design, original);
    assert!(!state.history.can_undo(), "angles and masts form one edit");
}

#[test]
fn apply_pending_retarget_combines_a_material_change_into_one_undo_step() {
    let ShiftFixture {
        mut state,
        proposal,
        check,
        material,
    } = checked_shift();
    let original = state.design.clone();
    let mut accepted = check.candidate.clone().expect("checked geometry");
    accepted.material = material.clone();
    let generation = state.generation.load(Ordering::Relaxed);
    let _held = HeldCheck::new(generation, Some(check));
    apply_pending_retarget(&mut state, (proposal, generation), Some(material))
        .unwrap_or_else(|_| panic!("checked geometry and material must apply"));
    assert_eq!(state.design, accepted);
    assert!(state.undo().unwrap());
    assert_eq!(state.design, original);
    assert!(!state.history.can_undo());
    assert!(state.redo().unwrap());
    assert_eq!(state.design, accepted);
}

#[test]
fn apply_pending_retarget_commits_checked_masts_angles_and_material_together() {
    let ShiftFixture {
        mut state,
        proposal,
        check,
        material,
    } = checked_shift();
    let original = state.design.clone();
    let anchors = check.anchors.clone();
    assert!(
        anchors
            .iter()
            .any(|a| (a.new_mast - a.old_mast).abs() > 1e-9)
    );
    let generation = state.generation.load(Ordering::Relaxed);
    let _held = HeldCheck::new(generation, Some(check));
    apply_pending_retarget(&mut state, (proposal, generation), Some(material.clone()))
        .unwrap_or_else(|_| panic!("checked mast changes must apply"));
    for anchor in anchors {
        assert!(matches!(
            state.design.tiers[anchor.tier_index].constraint,
            MeetConstraint::ScaleReference(mast) if (mast - anchor.new_mast).abs() < 1e-12
        ));
    }
    assert_eq!(state.design.material, material);
    assert!(state.undo().unwrap());
    assert_eq!(state.design, original);
    assert!(!state.history.can_undo());
}

#[test]
fn apply_pending_retarget_refuses_a_stale_generation() {
    let ShiftFixture {
        mut state,
        proposal,
        check,
        ..
    } = checked_shift();
    let original = state.design.clone();
    let generation = state.generation.load(Ordering::Relaxed);
    let _held = HeldCheck::new(generation, Some(check));
    let result = apply_pending_retarget(&mut state, (proposal, generation + 1), None);
    assert!(matches!(result, Err(RetargetApplyError::Stale)));
    assert_eq!(state.design, original);
    assert!(!state.history.can_undo());
}

#[test]
fn apply_pending_retarget_refuses_a_check_from_another_generation() {
    let ShiftFixture {
        mut state,
        proposal,
        check,
        ..
    } = checked_shift();
    let original = state.design.clone();
    let generation = state.generation.load(Ordering::Relaxed);
    let _held = HeldCheck::new(generation + 1, Some(check));
    let result = apply_pending_retarget(&mut state, (proposal, generation), None);
    assert!(matches!(result, Err(RetargetApplyError::Stale)));
    assert_eq!(state.design, original);
    assert!(!state.history.can_undo());
}

#[test]
fn a_plan_without_a_finished_check_cannot_be_applied() {
    let ShiftFixture {
        mut state,
        proposal,
        ..
    } = checked_shift();
    let original = state.design.clone();
    let generation = state.generation.load(Ordering::Relaxed);
    let _held = HeldCheck::new(generation, None);
    let result = apply_pending_retarget(&mut state, (proposal, generation), None);
    assert!(matches!(result, Err(RetargetApplyError::Stale)));
    assert_eq!(state.design, original);
    assert!(!state.history.can_undo());
}
