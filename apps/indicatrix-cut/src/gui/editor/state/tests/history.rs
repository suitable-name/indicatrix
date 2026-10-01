//! `EditorState`'s history/dirty-tracking tests: undo/redo, angle-nudge coalescing,
//! multi-select pruning, the inline-set-angle reject-before-apply contract, the
//! generation/design-epoch counters, and `result_is_stale`.

use super::super::*;
use indicatrix_cut_core::{ConstraintTier, Edit, FreshDesignSpec, MaterialSelection};

#[test]
fn editor_state_add_tier_then_undo_then_redo_round_trips() {
    let mut state = EditorState::fresh();
    let before = state.design.clone();

    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: 0.0,
                name: "T".to_string(),
                indices: vec![],
                constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.32),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .expect("add must apply");
    assert_eq!(state.design.tiers.len(), 1);
    assert!(state.history.can_undo());
    assert!(!state.history.can_redo());

    assert!(state.undo().unwrap());
    assert_eq!(state.design, before);
    assert!(!state.history.can_undo());
    assert!(state.history.can_redo());

    assert!(state.redo().unwrap());
    assert_eq!(state.design.tiers.len(), 1);
}

#[test]
fn editor_state_undo_on_an_empty_history_is_a_harmless_no_op() {
    let mut state = EditorState::fresh();
    assert!(!state.undo().unwrap());
    assert!(!state.redo().unwrap());
}

// --- EditorState::fresh_from_spec ---

#[test]
fn fresh_from_spec_round_trips_gear_symmetry_mirror_and_material() {
    let spec = FreshDesignSpec {
        gear_teeth: 80,
        symmetry_order: 5,
        mirror: false,
        material: MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.55),
            body_colour_override: None,
        },
        preform: indicatrix_cut_core::PreformSpec::cylinder(80, 1.4, 1.0, 1.3),
    };
    let state = EditorState::fresh_from_template(spec, 0);
    assert_eq!(state.design.meta.gear_teeth, 80);
    assert_eq!(state.design.meta.symmetry_order, 5);
    assert!(!state.design.meta.mirror);
    assert_eq!(state.design.material.name.as_deref(), Some("Quartz"));
    assert_eq!(state.design.material.refractive_index_override, Some(1.55));
    assert_eq!(state.design.tiers.len(), 0);
    assert!(!state.history.can_undo());
}

// A `SetMaterial` edit never touches tier geometry, so the ONLY thing that could
// invalidate a material-keyed cache (in `bridge::render_thread`) is the material
// itself changing.

#[test]
fn set_material_edit_leaves_every_tier_untouched_but_bumps_generation() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -40.0,
                name: "P1".to_string(),
                indices: vec![0.0, 12.0],
                constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let tiers_before = state.design.tiers.clone();
    let generation_before = state.generation.load(std::sync::atomic::Ordering::Relaxed);

    state
        .apply(Edit::SetMaterial {
            material: MaterialSelection {
                name: Some("Quartz".to_string()),
                specific_gravity_override: None,
                refractive_index_override: None,
                body_colour_override: None,
            },
        })
        .unwrap();

    assert_eq!(
        state.design.tiers, tiers_before,
        "SetMaterial must not touch geometry"
    );
    assert!(
        state.generation.load(std::sync::atomic::Ordering::Relaxed) > generation_before,
        "SetMaterial must still bump generation, which is what a material-keyed cache checks"
    );
}

// --- angle_nudge_coalesce_key ---

#[test]
fn angle_nudge_coalesce_key_is_order_independent() {
    assert_eq!(
        angle_nudge_coalesce_key(&[3, 4]),
        angle_nudge_coalesce_key(&[4, 3]),
    );
}

#[test]
fn angle_nudge_coalesce_key_distinguishes_a_single_tier_from_a_group_containing_it() {
    // Nudging tier 3 alone must never coalesce with nudging {3, 4} together, even
    // though the same tier is involved in both.
    assert_ne!(
        angle_nudge_coalesce_key(&[3]),
        angle_nudge_coalesce_key(&[3, 4]),
    );
}

#[test]
fn angle_nudge_coalesce_key_distinguishes_disjoint_targets() {
    assert_ne!(
        angle_nudge_coalesce_key(&[0]),
        angle_nudge_coalesce_key(&[1]),
    );
}

// --- EditorState::apply_coalescing / multi_selected pruning ---

fn pavilion_tier(name: &str, angle_deg: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![],
        constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

#[test]
fn apply_coalescing_through_editor_state_merges_into_one_undo_step() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: pavilion_tier("P1", -40.0),
        })
        .unwrap();
    let before_nudges = state.design.clone();
    let key = angle_nudge_coalesce_key(&[0]);

    state
        .apply_coalescing(
            Edit::ModifyTier {
                index: 0,
                tier: pavilion_tier("P1", -40.1),
            },
            key,
        )
        .expect("first nudge must apply");
    state
        .apply_coalescing(
            Edit::ModifyTier {
                index: 0,
                tier: pavilion_tier("P1", -40.2),
            },
            key,
        )
        .expect("second nudge must apply");
    assert_eq!(state.design.tiers[0].angle_deg, -40.2);

    // One undo reverts BOTH nudges, all the way back to before the burst started.
    assert!(state.undo().unwrap());
    assert_eq!(state.design, before_nudges);
}

#[test]
fn multi_selected_is_pruned_when_a_tier_it_names_is_removed() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: pavilion_tier("P1", -40.0),
        })
        .unwrap();
    state
        .apply(Edit::AddTier {
            index: 1,
            tier: pavilion_tier("P2", -35.0),
        })
        .unwrap();
    state.multi_selected.insert(0);
    state.multi_selected.insert(1);

    state.apply(Edit::RemoveTier { index: 1 }).unwrap();

    assert!(
        state.multi_selected.contains(&0),
        "tier 0 still exists and must stay selected"
    );
    assert!(
        !state.multi_selected.contains(&1),
        "tier 1 no longer exists -- must be dropped from the selection, not left dangling"
    );
}

// --- inline_set_angle's own rejection path (`loading::tier_form::parse_angle_only`) ---

#[test]
fn inline_set_angle_rejection_leaves_the_design_and_history_untouched() {
    // Drives the very function the inline angle cell's callback calls
    // (`set_tier_angle_from_text`) with text that cannot be an angle: it must report
    // `Err` (which the callback shows as an error toast) and leave the design, the
    // undo history and the generation counter exactly as they were.
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: pavilion_tier("P1", -40.0),
        })
        .unwrap();

    for bad in [
        "not-a-number",
        "",
        "NaN",
        "inf",
        "-inf",
        "1e999",
        "90.01",
        "-410",
    ] {
        let history_before = state.history.clone();
        let design_before = state.design.clone();
        let generation_before = state.generation.load(std::sync::atomic::Ordering::Relaxed);

        let outcome = state.set_tier_angle_from_text(0, bad);

        assert!(
            outcome.is_err(),
            "{bad:?} must be rejected, got {:?}",
            outcome.map(|_| ())
        );
        assert_eq!(state.history, history_before, "{bad:?} touched the history");
        assert_eq!(state.design, design_before, "{bad:?} touched the design");
        assert_eq!(
            state.generation.load(std::sync::atomic::Ordering::Relaxed),
            generation_before,
            "{bad:?} bumped the generation"
        );
    }
}

#[test]
fn inline_set_angle_accepts_a_well_formed_value() {
    // The accept-path counterpart: a valid angle parses to the exact `f64` an
    // `Edit::ModifyTier` would then carry.
    assert_eq!(
        crate::gui::editor::loading::parse_angle_only(" -41.5 ").unwrap(),
        -41.5
    );
}

// --- design_epoch: telling an edit apart from a wholesale replacement ---
//
// A background Deep Solve/Optimize can only distinguish "the cutter edited this
// design" from "the cutter loaded a different one" if these two counters move
// independently -- see `EditorState::design_epoch`'s own doc comment and
// `callbacks::solve_actions::RunProvenance`.

fn a_tier() -> ConstraintTier {
    ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![0.0, 12.0],
        constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

#[test]
fn an_ordinary_edit_bumps_generation_but_never_the_design_epoch() {
    let mut state = EditorState::fresh();
    let epoch_before = state
        .design_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    let generation_before = state.generation.load(std::sync::atomic::Ordering::Relaxed);

    state
        .apply(Edit::AddTier {
            index: 0,
            tier: a_tier(),
        })
        .unwrap();

    assert_ne!(
        state.generation.load(std::sync::atomic::Ordering::Relaxed),
        generation_before,
        "an edit must still look stale to a run in flight"
    );
    assert_eq!(
        state
            .design_epoch
            .load(std::sync::atomic::Ordering::Relaxed),
        epoch_before,
        "an edit is not a new design -- a finished run's verdict is still about this \
         one, and is shown with a caveat rather than discarded"
    );
}

#[test]
fn replace_wholesale_bumps_the_design_epoch_too() {
    let mut state = EditorState::fresh();
    let epoch_before = state
        .design_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    let generation_before = state.generation.load(std::sync::atomic::Ordering::Relaxed);

    state.replace_wholesale(EditorState::fresh());

    assert_ne!(
        state.generation.load(std::sync::atomic::Ordering::Relaxed),
        generation_before
    );
    assert_ne!(
        state
            .design_epoch
            .load(std::sync::atomic::Ordering::Relaxed),
        epoch_before,
        "New/Load must be distinguishable from an edit"
    );
}

#[test]
fn a_clone_captured_before_a_replacement_still_observes_the_epoch_bump() {
    // The whole point of carrying the `Arc` across the replacement instead of
    // adopting the replacement's own fresh one: a completion closure captured its
    // clone minutes ago and is the only thing that will ever read it.
    let mut state = EditorState::fresh();
    let captured = std::sync::Arc::clone(&state.design_epoch);
    let started = captured.load(std::sync::atomic::Ordering::Relaxed);

    state.replace_wholesale(EditorState::fresh());

    assert_ne!(
        captured.load(std::sync::atomic::Ordering::Relaxed),
        started,
        "a fresh Arc here would silently defeat the check for exactly the runs it \
         exists to catch"
    );
}

#[test]
fn the_epoch_keeps_counting_across_several_replacements() {
    let mut state = EditorState::fresh();
    let captured = std::sync::Arc::clone(&state.design_epoch);
    let started = captured.load(std::sync::atomic::Ordering::Relaxed);

    state.replace_wholesale(EditorState::fresh());
    state.replace_wholesale(EditorState::fresh());

    assert_eq!(
        captured.load(std::sync::atomic::Ordering::Relaxed),
        started + 2,
        "loading design B then design C must not land back on A's own epoch"
    );
}

// --- result_is_stale ---

#[test]
fn a_result_with_no_generation_stamped_is_never_stale() {
    assert!(!result_is_stale(None, 0));
    assert!(!result_is_stale(None, 42));
}

#[test]
fn a_result_stamped_with_the_current_generation_is_not_stale() {
    assert!(!result_is_stale(Some(5), 5));
}

#[test]
fn a_result_is_stale_one_generation_later() {
    // A result computed at generation N is stale at N+1.
    assert!(result_is_stale(Some(5), 6));
}

#[test]
fn a_result_is_stale_many_generations_later_too() {
    assert!(result_is_stale(Some(0), 100));
}

#[test]
fn deep_solve_result_generation_starts_unset_on_a_fresh_state() {
    let state = EditorState::fresh();
    assert_eq!(state.deep_solve_result_generation, None);
}

#[test]
fn replace_wholesale_carries_over_the_replacements_own_unset_deep_solve_result() {
    // A brand-new/loaded design has never had its own Deep Solve run yet, however
    // stale the PREVIOUS design's verdict was -- see `EditorState::
    // deep_solve_result_generation`'s own doc comment for why this must not
    // survive a wholesale replacement.
    let mut state = EditorState::fresh();
    state.deep_solve_result_generation = Some(3);
    state.replace_wholesale(EditorState::fresh());
    assert_eq!(state.deep_solve_result_generation, None);
}
