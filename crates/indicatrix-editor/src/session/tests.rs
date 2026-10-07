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

/// The tier table prints a pavilion tier's angle without its minus sign, so a nudge of the
/// SHOWN number has to move the stored value the other way for a pavilion tier.
#[test]
fn a_displayed_nudge_is_signed_by_the_side_of_the_tier() {
    // Crown and table: the stored angle is positive, the delta is taken as it is.
    assert!((displayed_nudge_delta(34.5, 0.1) - 0.1).abs() < f64::EPSILON);
    assert!((displayed_nudge_delta(0.0, -0.1) + 0.1).abs() < f64::EPSILON);
    // Pavilion and culet (a minus zero): the stored angle is negative, the delta flips.
    assert!((displayed_nudge_delta(-40.0, 0.1) + 0.1).abs() < f64::EPSILON);
    assert!((displayed_nudge_delta(-0.0, 0.1) + 0.1).abs() < f64::EPSILON);
    assert!((displayed_nudge_delta(-40.0, -1.0) - 1.0).abs() < f64::EPSILON);
}

/// Up (a positive delta) makes the shown number bigger for a crown and a pavilion tier alike,
/// one Undo steps it back, and the stored sign never changes.
#[test]
fn a_displayed_nudge_makes_the_shown_angle_bigger_on_both_sides() {
    let mut session = EditorSession::fresh();
    for (index, (name, angle)) in [("C1", 34.5), ("P1", -40.0)].into_iter().enumerate() {
        session
            .apply(Edit::AddTier {
                index,
                tier: tier(name, angle, &[], MeetConstraint::ScaleReference(0.5)),
            })
            .unwrap();
    }
    // The tier table's ANGLE column shows the magnitude.
    let shown = |session: &EditorSession| -> Vec<f64> {
        session
            .design
            .tiers
            .iter()
            .map(|tier| tier.angle_deg.abs())
            .collect()
    };

    session
        .nudge_displayed_angles(&[0, 1], 0.5, ms(0))
        .unwrap()
        .unwrap();
    assert_eq!(
        shown(&session),
        [35.0, 40.5],
        "Up made both shown numbers bigger"
    );
    assert!(session.design.tiers[1].angle_deg.is_sign_negative());

    session
        .nudge_displayed_angles(&[1], -2.0, ms(5000))
        .unwrap()
        .unwrap();
    assert_eq!(
        shown(&session),
        [35.0, 38.5],
        "Down made the shown number smaller"
    );
    assert!((session.design.tiers[1].angle_deg + 38.5).abs() < 1e-9);

    // One Undo steps the last nudge back exactly.
    session.undo().unwrap();
    assert!((session.design.tiers[1].angle_deg + 40.5).abs() < 1e-9);
}

/// A shown number stops at 0 for a pavilion tier too, on its own side, and says so.
#[test]
fn a_displayed_nudge_down_stops_a_pavilion_tier_at_zero_on_its_own_side() {
    let mut session = EditorSession::fresh();
    session
        .apply(Edit::AddTier {
            index: 0,
            tier: tier("P1", -0.5, &[], MeetConstraint::ScaleReference(0.5)),
        })
        .unwrap();
    let outcome = session
        .nudge_displayed_angles(&[0], -2.0, ms(0))
        .unwrap()
        .unwrap();
    assert_eq!(outcome.clamped_labels, ["tier 1 (P1)"]);
    let angle = session.design.tiers[0].angle_deg;
    assert!(angle == 0.0 && angle.is_sign_negative(), "{angle}");
    // Up from there moves the shown number up again, still a pavilion tier.
    session
        .nudge_displayed_angles(&[0], 0.25, ms(5000))
        .unwrap()
        .unwrap();
    assert!((session.design.tiers[0].angle_deg + 0.25).abs() < 1e-9);
    // The same call for a tier that does not exist, or no tiers, applies nothing.
    assert_eq!(
        session.nudge_displayed_angles(&[5], 1.0, ms(0)).unwrap(),
        None
    );
    assert_eq!(
        session.nudge_displayed_angles(&[], 1.0, ms(0)).unwrap(),
        None
    );
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

/// Where a caller's own selected table row sits after `change`, as the desktop works it
/// out from the map an undo, redo or jump returns.
fn row_after(
    row: usize,
    change: &EditChange,
    renumbering: &TierIndexMap,
    session: &EditorSession,
) -> Option<usize> {
    renumbering.map_table_row(
        row,
        change.tier_count_before,
        change.tier_count_after,
        session.design.concave_tiers.len(),
    )
}

/// The failure the review found: tiers P0..P4 with P3 selected, a tier added above it
/// (the selection moves to row 4), then Undo. The session's own selection follows the
/// tier back, and so must a row the caller keeps -- not stay at row 4 and name P4.
#[test]
fn undo_returns_the_renumbering_a_caller_selected_row_follows() {
    let mut session = five_tier_session();
    session
        .apply(Edit::AddTier {
            index: 2,
            tier: tier("New", -40.0, &[], MeetConstraint::ScaleReference(0.5)),
        })
        .unwrap();
    assert_eq!(tier_names(&session), ["P0", "P1", "New", "P2", "P3", "P4"]);
    // P3 sits at row 4 now; both the session and the caller's row follow it.
    let mut row = 4;
    session.multi_selected.insert(row);

    let (change, renumbering) = session.undo_mapped().unwrap().expect("something to undo");
    assert_eq!(tier_names(&session), ["P0", "P1", "P2", "P3", "P4"]);
    row = row_after(row, &change, &renumbering, &session).expect("P3 is still there");
    assert_eq!(row, 3);
    assert_eq!(session.design.tiers[row].name, "P3");
    assert_eq!(
        session.multi_selected,
        BTreeSet::from([row]),
        "the session and the caller's row agree"
    );
    assert!(!change.concave_count_changed());

    // The tier that was added is gone after the undo: a row on it names nothing.
    assert_eq!(row_after(2, &change, &renumbering, &session), None);

    // Redo puts it back and the row follows again.
    let (change, renumbering) = session.redo_mapped().unwrap().expect("something to redo");
    row = row_after(row, &change, &renumbering, &session).expect("P3 is still there");
    assert_eq!(row, 4);
    assert_eq!(session.design.tiers[row].name, "P3");
    assert_eq!(session.multi_selected, BTreeSet::from([row]));
}

/// An undo that removes the selected tier clears the row, and one that restores a removed
/// tier pushes the rows below it down.
#[test]
fn a_selected_row_is_cleared_when_the_move_removes_its_tier() {
    let mut session = five_tier_session();
    session.apply(Edit::RemoveTier { index: 1 }).unwrap();
    // [P0, P2, P3, P4]: P2 is row 1.
    let (change, renumbering) = session.undo_mapped().unwrap().expect("something to undo");
    assert_eq!(
        row_after(1, &change, &renumbering, &session),
        Some(2),
        "P2 is row 2 once P1 is back"
    );
    // Redo removes P1 again: a row on it names nothing.
    let (change, renumbering) = session.redo_mapped().unwrap().expect("something to redo");
    assert_eq!(row_after(1, &change, &renumbering, &session), None);
    assert_eq!(row_after(4, &change, &renumbering, &session), Some(3));
}

/// Nothing to undo or redo is no move at all, and gives no map.
#[test]
fn undo_and_redo_with_nothing_to_move_return_no_map() {
    let mut session = EditorSession::fresh();
    assert!(session.undo_mapped().unwrap().is_none());
    assert!(session.redo_mapped().unwrap().is_none());
}

/// The mapped forms change nothing the plain ones did: the same design, the same
/// generation and the same selection after the same step.
#[test]
fn the_mapped_undo_and_redo_leave_the_session_as_the_plain_ones_do() {
    let mut plain = five_tier_session();
    let mut mapped = five_tier_session();
    plain.multi_selected.extend([0, 1]);
    mapped.multi_selected.extend([0, 1]);
    for session in [&mut plain, &mut mapped] {
        session.apply(Edit::RemoveTier { index: 0 }).unwrap();
    }
    let plain_change = plain.undo().unwrap().expect("undone");
    let (mapped_change, _) = mapped.undo_mapped().unwrap().expect("undone");
    assert_eq!(plain_change, mapped_change);
    assert_eq!(plain.design, mapped.design);
    assert_eq!(plain.multi_selected, mapped.multi_selected);
    let plain_change = plain.redo().unwrap().expect("redone");
    let (mapped_change, _) = mapped.redo_mapped().unwrap().expect("redone");
    assert_eq!(plain_change, mapped_change);
    assert_eq!(plain.design, mapped.design);
    assert_eq!(plain.multi_selected, mapped.multi_selected);
}

/// A jump is several undos: the map it returns composes all of them, so one row can be
/// followed through the whole jump.
#[test]
fn a_jump_returns_one_map_for_every_step_it_walked() {
    let mut session = five_tier_session();
    // Step 6 moves P0 to row 3; step 7 inserts a tier at row 1.
    session.apply(Edit::MoveTier { from: 0, to: 3 }).unwrap();
    session
        .apply(Edit::AddTier {
            index: 1,
            tier: tier("New", -40.0, &[], MeetConstraint::ScaleReference(0.5)),
        })
        .unwrap();
    assert_eq!(tier_names(&session), ["P1", "New", "P2", "P3", "P0", "P4"]);
    // P0 is row 4 and selected.
    let mut row = 4;
    session.multi_selected.insert(row);

    // Back to the five tiers as they were first added (position 5): both steps undone.
    let (result, renumbering) = session.jump_to_mapped(5);
    let outcome = result.expect("the jump works");
    let change = outcome.change.expect("something moved");
    assert_eq!(outcome.position, 5);
    assert_eq!(tier_names(&session), ["P0", "P1", "P2", "P3", "P4"]);
    row = row_after(row, &change, &renumbering, &session).expect("P0 is still there");
    assert_eq!(row, 0);
    assert_eq!(session.multi_selected, BTreeSet::from([row]));
    // The tier added in step 7 (row 1) is not part of the design at position 5; P2 (row 2
    // before) is row 2 in it, the move and the insertion having cancelled out.
    assert_eq!(row_after(1, &change, &renumbering, &session), None);
    assert_eq!(row_after(2, &change, &renumbering, &session), Some(2));

    // And forward again: P0 is row 4 once more.
    let (result, renumbering) = session.jump_to_mapped(7);
    let change = result.expect("the jump works").change.expect("moved");
    row = row_after(row, &change, &renumbering, &session).expect("P0 is still there");
    assert_eq!(row, 4);
    assert_eq!(session.multi_selected, BTreeSet::from([row]));
}

/// A jump that moves nothing, or that is refused, returns an empty map.
#[test]
fn a_jump_that_moves_nothing_returns_an_identity_map() {
    let mut session = five_tier_session();
    let (result, renumbering) = session.jump_to_mapped(5);
    assert_eq!(result.expect("already there").change, None);
    assert_eq!(renumbering, TierIndexMap::default());
    let (result, renumbering) = session.jump_to_mapped(99);
    assert!(result.is_err(), "past the last step");
    assert_eq!(renumbering, TierIndexMap::default());
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

/// The tier table prints an angle without its sign, so every edit that starts from what
/// the cutter sees (a typed magnitude, an arrow nudge, a handle drag that reports a
/// magnitude) must leave a pavilion tier on the negative side: the sign is the only thing
/// that says the tier is a pavilion.
#[test]
fn every_direct_angle_edit_leaves_a_pavilion_tier_negative() {
    let mut session = drag_session();
    let negative = |session: &EditorSession| session.design.tiers[0].angle_deg.is_sign_negative();

    // The inline cell: the table shows `40`, the cutter types `41`.
    assert!(matches!(
        session.set_tier_angle_from_text(0, "41").unwrap(),
        InlineAngle::Applied(_)
    ));
    assert_eq!(session.design.tiers[0].angle_deg, -41.0);

    // Arrow nudges, up and down.
    session.nudge_angles(&[0], 0.25, ms(0)).unwrap();
    assert_eq!(session.design.tiers[0].angle_deg, -40.75);
    session.nudge_angles(&[0], -0.25, ms(5000)).unwrap();
    assert_eq!(session.design.tiers[0].angle_deg, -41.0);

    // A handle drag reports a value; one on the other side of zero is held on the side.
    session.set_tier_angle(0, 43.0, ms(10_000)).unwrap();
    assert!(
        negative(&session),
        "a handle value on the crown side is clamped to minus zero"
    );
    session.set_tier_angle(0, -43.0, ms(20_000)).unwrap();
    assert_eq!(session.design.tiers[0].angle_deg, -43.0);

    // A nudge that would cross the girdle stops at minus zero.
    session.nudge_angles(&[0], 500.0, ms(30_000)).unwrap();
    assert!(negative(&session), "a nudge past zero stops at minus zero");

    // A tier at minus zero is still a pavilion tier for the next typed value.
    session.set_tier_angle_from_text(0, "5").unwrap();
    assert_eq!(session.design.tiers[0].angle_deg, -5.0);

    // Undo walks back through negative angles only.
    while session.undo().unwrap().is_some() {
        if session.design.tiers.len() == 2 {
            assert!(negative(&session), "{}", session.design.tiers[0].angle_deg);
        }
    }
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
