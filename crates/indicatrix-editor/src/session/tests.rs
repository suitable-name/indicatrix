//! `EditorSession` tests: the scripted sequence the desktop's identity pin records
//! (same design, same history depth), caller-supplied coalescing timestamps, the
//! nudge clamp, template seeding, and the generation/dirty bookkeeping.

use super::*;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    TierTarget,
    native::{SaveExtras, save_native_only_toml},
};

fn fnv1a(bytes: &[u8]) -> u64 {
    indicatrix_solid::mesh_cache::fnv1a_64(bytes.iter().copied())
}

fn tier(name: &str, angle_deg: f64, indices: &[f64], constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

const fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

/// The desktop's `identity_pins::scripted_edit_sequence_is_pinned`, driven through
/// `EditorSession` with explicit timestamps: add three tiers, three coalescing
/// nudges, undo, redo, mirror, complete orbit. The native TOML hash/length and the
/// history depth are the values the desktop recorded before the move, so the web
/// app (which edits through this type) produces the desktop's exact design.
#[test]
fn the_scripted_sequence_matches_the_desktop_pin() {
    let mut session = EditorSession::fresh();
    let eight = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    let tiers = [
        tier("G1", 90.0, &eight, MeetConstraint::ScaleReference(1.0)),
        tier(
            "P1",
            -40.0,
            &eight,
            MeetConstraint::MeetNamed(vec!["G1".to_string()]),
        ),
        tier(
            "C1",
            34.5,
            &[0.0, 12.0, 24.0],
            MeetConstraint::MeetNamed(vec!["G1".to_string()]),
        ),
    ];
    for (index, t) in tiers.into_iter().enumerate() {
        let change = session.apply(Edit::AddTier { index, tier: t }).unwrap();
        assert!(change.tier_count_changed());
    }
    for step in 0..3 {
        let current = session.design.tiers[1].angle_deg;
        let change = session
            .apply_coalescing(
                Edit::RetargetAngles {
                    changes: vec![(1, current, current - 0.25)],
                },
                angle_nudge_coalesce_key(&[1]),
                ms(100 * step),
            )
            .unwrap();
        assert!(!change.tier_count_changed());
    }
    assert!(session.undo().unwrap().is_some());
    assert!(session.redo().unwrap().is_some());
    session.mirror_indices(2).unwrap();
    assert!(session.complete_orbit(2).unwrap());

    let toml =
        save_native_only_toml(&session.design, "pin.asc", None, &SaveExtras::default()).unwrap();
    assert_eq!(
        (fnv1a(toml.as_bytes()), toml.len()),
        (8_529_034_122_875_928_046, 1078)
    );
    assert_eq!(session.current_generation(), 10);
    assert!(session.is_dirty());

    let mut depth = 0;
    while session.undo().unwrap().is_some() {
        depth += 1;
    }
    assert_eq!(
        depth, 6,
        "three adds + one coalesced nudge + mirror + orbit"
    );
}

#[test]
fn nudges_outside_the_coalescing_window_are_separate_undo_steps() {
    let mut session = EditorSession::fresh();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("P1", -40.0, &[], MeetConstraint::ScaleReference(0.5)),
        })
        .unwrap();
    session.nudge_angles(&[0], -0.5, ms(0)).unwrap();
    // Inside the window: merged into the first nudge.
    session.nudge_angles(&[0], -0.5, ms(1400)).unwrap();
    // Past the window measured from the last nudge: a new undo step.
    session.nudge_angles(&[0], -0.5, ms(1400 + 1600)).unwrap();
    assert!((session.design.tiers[0].angle_deg + 41.5).abs() < 1e-9);
    session.undo().unwrap();
    assert!((session.design.tiers[0].angle_deg + 41.0).abs() < 1e-9);
    session.undo().unwrap();
    assert!((session.design.tiers[0].angle_deg + 40.0).abs() < 1e-9);
}

#[test]
fn a_nudge_stops_at_zero_instead_of_crossing_blocks() {
    let mut session = EditorSession::fresh();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("P1", -0.5, &[], MeetConstraint::ScaleReference(0.5)),
        })
        .unwrap();
    let outcome = session.nudge_angles(&[0], 2.0, ms(0)).unwrap().unwrap();
    assert_eq!(outcome.clamped_labels, ["tier 1 (P1)"]);
    let angle = session.design.tiers[0].angle_deg;
    assert!(angle == 0.0 && angle.is_sign_negative(), "{angle}");
    assert_eq!(session.nudge_angles(&[5], 1.0, ms(0)).unwrap(), None);
    assert_eq!(session.nudge_angles(&[], 1.0, ms(0)).unwrap(), None);
}

#[test]
fn clamp_nudge_to_side_keeps_the_original_sign() {
    assert!(clamp_nudge_to_side(-1.0, 1.0).is_sign_negative());
    assert!(!clamp_nudge_to_side(1.0, -1.0).is_sign_negative());
    assert!((clamp_nudge_to_side(2.0, 3.0) - 3.0).abs() < f64::EPSILON);
}

fn fresh_spec() -> FreshDesignSpec {
    FreshDesignSpec {
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        material: indicatrix_cut_core::MaterialSelection::none(),
        preform: PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
    }
}

#[test]
fn a_template_tier_accepts_an_authored_target() {
    let mut session = EditorSession::from_template(fresh_spec(), 1);
    let tier_count = session.design.tiers.len();
    assert!(tier_count > 1, "template 1 seeds a real schedule");
    assert_eq!(
        session.design.tier_ids.len(),
        tier_count,
        "every seeded tier carries a TierId"
    );
    let distinct: BTreeSet<_> = session.design.tier_ids.iter().collect();
    assert_eq!(distinct.len(), tier_count, "and no two share one");

    for index in [0, tier_count - 1] {
        let target = TierTarget::DepthMm(2.0);
        session
            .apply(Edit::SetTierTarget {
                index,
                target: Some(target),
            })
            .expect("a template tier can carry a target");
        assert_eq!(session.design.tier_target(index), Some(target));
    }
    // Undo clears it again: the seeded ids are real identities, not placeholders.
    session.undo().unwrap();
    assert_eq!(session.design.tier_target(tier_count - 1), None);
    // Nothing the template seeded is undoable.
    session.undo().unwrap();
    assert!(!session.history.can_undo());
}

#[test]
fn from_template_seeds_the_template_tiers_without_history() {
    let spec = fresh_spec();
    let session = EditorSession::from_template(spec.clone(), 1);
    assert_eq!(
        session.design.tiers,
        indicatrix_cut_core::templates::TEMPLATES[0].tiers()
    );
    assert!(!session.history.can_undo());
    for empty in [0, -3, 99] {
        assert_eq!(
            EditorSession::from_template(spec.clone(), empty)
                .design
                .tiers,
            Vec::new()
        );
    }
}

#[test]
fn a_replacement_continues_the_generation_counter_and_reads_clean() {
    let mut session = EditorSession::fresh();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("T", 0.0, &[], MeetConstraint::ScaleReference(0.3)),
        })
        .unwrap();
    let captured = Arc::clone(&session.generation);
    assert!(session.is_dirty());
    session.replace_with(EditorSession::fresh());
    assert_eq!(captured.load(Ordering::Relaxed), 2);
    assert!(!session.is_dirty());
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("T", 0.0, &[], MeetConstraint::ScaleReference(0.3)),
        })
        .unwrap();
    assert!(session.is_dirty());
    session.mark_saved();
    assert!(!session.is_dirty());
}

#[test]
fn an_edit_prunes_the_selection_to_live_tiers() {
    let mut session = EditorSession::fresh();
    for index in 0..2 {
        session
            .apply(Edit::AddTier {
                index,
                tier: tier(
                    &format!("P{index}"),
                    -40.0,
                    &[],
                    MeetConstraint::ScaleReference(0.5),
                ),
            })
            .unwrap();
    }
    session.multi_selected.extend([0, 1]);
    let change = session.apply(Edit::RemoveTier { index: 1 }).unwrap();
    assert!(change.tier_count_changed());
    assert_eq!(session.multi_selected, BTreeSet::from([0]));
}

/// A session of five pavilion tiers named `P0` to `P4`, in that order.
fn five_tier_session() -> EditorSession {
    let mut session = EditorSession::fresh();
    for index in 0..5 {
        session
            .apply(Edit::AddTier {
                index,
                tier: tier(
                    &format!("P{index}"),
                    -40.0,
                    &[],
                    MeetConstraint::ScaleReference(0.5),
                ),
            })
            .unwrap();
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
fn removing_a_tier_keeps_the_selection_on_the_tiers_it_named() {
    let mut session = five_tier_session();
    session.multi_selected.extend([0, 1]);
    session.apply(Edit::RemoveTier { index: 0 }).unwrap();
    assert_eq!(session.multi_selected, BTreeSet::from([0]));
    assert_eq!(
        session.design.tiers[0].name, "P1",
        "row 0 is the former tier 1"
    );

    // A second Delete removes the tier that was selected, not a stranger.
    assert_eq!(session.remove_multi_selected().unwrap(), 1);
    assert_eq!(tier_names(&session), ["P2", "P3", "P4"]);
}

#[test]
fn undo_and_redo_renumber_the_selection_back_and_forth() {
    let mut session = five_tier_session();
    session.multi_selected.extend([0, 1]);
    session.apply(Edit::RemoveTier { index: 0 }).unwrap();
    assert_eq!(session.multi_selected, BTreeSet::from([0]));

    // The removed tier comes back at row 0, so the tier that was selected is row 1 again.
    session.undo().unwrap();
    assert_eq!(tier_names(&session), ["P0", "P1", "P2", "P3", "P4"]);
    assert_eq!(session.multi_selected, BTreeSet::from([1]));

    session.redo().unwrap();
    assert_eq!(tier_names(&session), ["P1", "P2", "P3", "P4"]);
    assert_eq!(session.multi_selected, BTreeSet::from([0]));
}

#[test]
fn moving_and_adding_tiers_renumber_the_selection() {
    let mut session = five_tier_session();
    session.multi_selected.extend([1, 2]);
    // [P0, P1, P2, P3, P4] with P0 moved to row 3 is [P1, P2, P3, P0, P4].
    session.apply(Edit::MoveTier { from: 0, to: 3 }).unwrap();
    assert_eq!(session.multi_selected, BTreeSet::from([0, 1]));
    assert_eq!(tier_names(&session), ["P1", "P2", "P3", "P0", "P4"]);

    session.undo().unwrap();
    assert_eq!(session.multi_selected, BTreeSet::from([1, 2]));

    // A tier inserted above the selection pushes it down.
    session
        .apply(Edit::AddTier {
            index: 1,
            tier: tier("New", -40.0, &[], MeetConstraint::ScaleReference(0.5)),
        })
        .unwrap();
    assert_eq!(session.multi_selected, BTreeSet::from([2, 3]));
    assert_eq!(session.design.tiers[2].name, "P1");
    assert_eq!(session.design.tiers[3].name, "P2");
}

#[test]
fn edits_that_move_no_rows_leave_the_selection_alone() {
    let mut session = five_tier_session();
    session.multi_selected.extend([0, 3]);
    session.nudge_angles(&[1], -0.5, ms(0)).unwrap();
    session
        .apply(Edit::SetConstraint {
            index: 2,
            constraint: MeetConstraint::MeetExisting,
        })
        .unwrap();
    assert_eq!(session.multi_selected, BTreeSet::from([0, 3]));
}

/// A session holding P1 (pinned at 0.5, angle -40) and P2 (meets P1 by name).
fn drag_session() -> EditorSession {
    let mut session = EditorSession::fresh();
    let tiers = [
        tier(
            "P1",
            -40.0,
            &[0.0, 24.0],
            MeetConstraint::ScaleReference(0.5),
        ),
        tier(
            "P2",
            -42.0,
            &[0.0, 24.0],
            MeetConstraint::MeetNamed(vec!["P1".to_string()]),
        ),
    ];
    for (index, t) in tiers.into_iter().enumerate() {
        session.apply(Edit::AddTier { index, tier: t }).unwrap();
    }
    session
}

#[test]
fn an_angle_drag_is_one_undo_step_and_undo_restores_the_angle_bit_for_bit() {
    let mut session = drag_session();
    let original = session.design.tiers[0].angle_deg;
    for (step, angle) in [-40.5, -41.0, -41.3].into_iter().enumerate() {
        let change = session
            .set_tier_angle(0, angle, ms(10 * step as u64))
            .unwrap()
            .expect("the angle changed");
        assert!(!change.tier_count_changed());
    }
    assert_eq!(session.design.tiers[0].angle_deg, -41.3);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(
        session.design.tiers[0].angle_deg.to_bits(),
        original.to_bits()
    );
    assert_eq!(session.design.tiers.len(), 2, "the drag was ONE step");
    // The next undo removes P2, so the three drag calls really were a single step.
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design.tiers.len(), 1);
}

#[test]
fn set_tier_angle_clamps_at_zero_and_ignores_unchanged_or_missing_tiers() {
    let mut session = drag_session();
    session.set_tier_angle(0, 5.0, ms(0)).unwrap();
    let angle = session.design.tiers[0].angle_deg;
    assert!(angle == 0.0 && angle.is_sign_negative(), "{angle}");
    assert_eq!(session.set_tier_angle(0, 5.0, ms(10)).unwrap(), None);
    assert_eq!(session.set_tier_angle(9, -1.0, ms(10)).unwrap(), None);
    assert_eq!(session.set_tier_angle(0, f64::NAN, ms(10)).unwrap(), None);
}

#[test]
fn pinning_a_meet_tier_reports_and_undo_restores_the_replaced_meet() {
    let mut session = drag_session();
    let outcome = session
        .pin_tier_mast(1, 0.7, ms(0))
        .unwrap()
        .expect("the constraint changed");
    assert_eq!(
        outcome.replaced_meet,
        Some(MeetConstraint::MeetNamed(vec!["P1".to_string()]))
    );
    assert_eq!(
        session.design.tiers[1].constraint,
        MeetConstraint::ScaleReference(0.7)
    );
    // A later call of the same drag finds the tier already pinned.
    let again = session.pin_tier_mast(1, 0.75, ms(10)).unwrap().unwrap();
    assert_eq!(again.replaced_meet, None);
    assert_eq!(session.pin_tier_mast(1, 0.75, ms(20)).unwrap(), None);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(
        session.design.tiers[1].constraint,
        MeetConstraint::MeetNamed(vec!["P1".to_string()])
    );
    assert_eq!(session.pin_tier_mast(9, 0.7, ms(30)).unwrap(), None);
}

#[test]
fn rotating_indices_by_zero_is_nothing_and_a_drag_of_turns_is_one_undo_step() {
    let mut session = drag_session();
    assert_eq!(session.rotate_tier_indices(0, 0, ms(0)).unwrap(), None);
    let before = session.design.tiers[0].indices.clone();
    session.rotate_tier_indices(0, 1, ms(0)).unwrap().unwrap();
    session.rotate_tier_indices(0, 2, ms(10)).unwrap().unwrap();
    assert_eq!(session.design.tiers[0].indices, vec![3.0, 27.0]);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(session.design.tiers[0].indices, before);
    assert_eq!(session.design.tiers.len(), 2);
    assert!(session.rotate_tier_indices(9, 1, ms(20)).is_err());
}

#[test]
fn drags_of_different_handles_or_tiers_do_not_merge() {
    let mut session = drag_session();
    session.set_tier_angle(0, -41.0, ms(0)).unwrap();
    session.pin_tier_mast(0, 0.6, ms(10)).unwrap();
    session.set_tier_angle(1, -43.0, ms(20)).unwrap();
    session.undo().unwrap();
    assert_eq!(session.design.tiers[1].angle_deg, -42.0);
    assert_eq!(session.design.tiers[0].angle_deg, -41.0);
    session.undo().unwrap();
    assert_eq!(
        session.design.tiers[0].constraint,
        MeetConstraint::ScaleReference(0.5)
    );
}

#[test]
fn result_staleness_follows_the_generation() {
    assert!(!result_is_stale(None, 7));
    assert!(!result_is_stale(Some(7), 7));
    assert!(result_is_stale(Some(6), 7));
}
