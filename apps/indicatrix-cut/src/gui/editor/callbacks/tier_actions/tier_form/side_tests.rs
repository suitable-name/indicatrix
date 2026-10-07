//! The form shows an angle without its sign, so a save hands the form's magnitude back and
//! [`preserve_saved_tier_side_and_detached`] is what keeps a pavilion tier a pavilion tier.

use super::*;
use indicatrix_cut_core::{Design, Edit};
use indicatrix_editor::EditorSession;

fn tier(name: &str, angle_deg: f64) -> indicatrix_cut_core::ConstraintTier {
    indicatrix_cut_core::ConstraintTier {
        angle_deg,
        name: name.to_owned(),
        indices: vec![0.0, 24.0],
        constraint: MeetConstraint::ScaleReference(0.65),
        imported_meet: None,
        original_notes: None,
        detached: vec![24.0],
    }
}

/// P1 at -41 (pavilion) and C1 at 34.5 (crown).
fn session() -> EditorSession {
    let mut session = EditorSession::fresh();
    for (index, (name, angle)) in [("P1", -41.0), ("C1", 34.5)].into_iter().enumerate() {
        session
            .apply(Edit::AddTier {
                index,
                tier: tier(name, angle),
            })
            .expect("a tier is added");
    }
    session
}

fn design() -> Design {
    session().design
}

/// What the Save action does with a NEW tier left unnamed, angle field `angle`: the
/// form's parse (with the relation's stand-in angle), `settle_saved_tier`, the planned
/// edit applied through the session. Returns the tier as it ends up in the design.
fn save_new_unnamed_tier(
    session: &mut EditorSession,
    angle: &str,
) -> indicatrix_cut_core::ConstraintTier {
    let other_tier_names =
        indicatrix_editor::tier_save::other_tier_names_excluding(&session.design, -1);
    let placeholder = relation_placeholder_angle_deg(&session.design, -1, angle, "");
    let (mut saved, relation) = parse_tier_form_with_relation(
        loading::TierFormFields {
            angle,
            constraint_kind: 2,
            constraint_text: "0.65",
            name: "",
            indices: "0, 24",
            gear_teeth_abs: 96,
            imported_meet: None,
            original_notes: None,
            other_tier_names: other_tier_names.clone(),
        },
        placeholder,
    )
    .expect("a valid form");
    settle_saved_tier(&mut saved, &session.design, -1, &other_tier_names);
    let (at, edit) = plan_tier_save(&session.design, -1, saved, relation.as_deref(), None, None)
        .expect("a valid relation");
    session.try_apply(edit).expect("the edit applies");
    session.design.tiers[at].clone()
}

#[test]
fn a_saved_pavilion_tier_keeps_its_negative_side_and_its_detached_marks() {
    let design = design();
    let mut saved = tier("P1", 39.0);
    saved.detached = Vec::new();
    preserve_saved_tier_side_and_detached(&mut saved, &design, 0);
    assert_eq!(saved.angle_deg, -39.0);
    assert_eq!(saved.detached, vec![24.0]);

    // A typed zero is the same side too: minus zero.
    let mut zero = tier("P1", 0.0);
    preserve_saved_tier_side_and_detached(&mut zero, &design, 0);
    assert!(zero.angle_deg == 0.0 && zero.angle_deg.is_sign_negative());
}

#[test]
fn a_saved_crown_tier_stays_on_the_crown_side() {
    let design = design();
    let mut saved = tier("C1", 36.0);
    preserve_saved_tier_side_and_detached(&mut saved, &design, 1);
    assert_eq!(saved.angle_deg, 36.0);
}

#[test]
fn a_new_tier_is_a_pavilion_tier_only_when_its_name_says_so() {
    let design = design();
    let mut pavilion = tier("P7", 40.0);
    preserve_saved_tier_side_and_detached(&mut pavilion, &design, -1);
    assert_eq!(pavilion.angle_deg, -40.0);
    let mut crown = tier("C7", 40.0);
    preserve_saved_tier_side_and_detached(&mut crown, &design, -1);
    assert_eq!(crown.angle_deg, 40.0);
    // A row that no longer exists changes nothing.
    let mut stale = tier("P1", 40.0);
    preserve_saved_tier_side_and_detached(&mut stale, &design, 9);
    assert_eq!(stale.angle_deg, 40.0);
}

#[test]
fn a_new_unnamed_tier_with_a_relation_to_a_pavilion_tier_is_named_for_its_final_side() {
    let mut session = session();
    // Reads the pavilion tier P1 (41), so it lands on the pavilion side at 39 ...
    let added = save_new_unnamed_tier(&mut session, "=P1 - 2");
    assert!(
        added.angle_deg.is_sign_negative() && (added.angle_deg + 39.0).abs() < 1e-9,
        "{added:?}"
    );
    // ... and its block name says so: the next free pavilion name, not a crown one.
    assert_eq!(added.name, "P2");
    assert!(session.is_driven(2), "the relation is kept");

    // The same text on the crown tier C1 (34.5) gives a crown name.
    let added = save_new_unnamed_tier(&mut session, "=C1 + 1.5");
    assert!(added.angle_deg > 0.0 && (added.angle_deg - 36.0).abs() < 1e-9);
    assert_eq!(added.name, "C2");
}

#[test]
fn a_new_unnamed_tier_without_a_relation_is_named_for_the_side_it_is_typed_on() {
    let mut session = session();
    assert_eq!(save_new_unnamed_tier(&mut session, "38").name, "C2");
    let pavilion = save_new_unnamed_tier(&mut session, "-38");
    assert_eq!(pavilion.name, "P2");
    assert!(pavilion.angle_deg.is_sign_negative());
}

#[test]
fn the_name_of_a_new_tier_always_agrees_with_the_side_the_design_ends_up_with() {
    let mut session = session();
    for text in [
        "=P1 - 2",
        "=C1 + 3",
        "=P1 + 1",
        "=(P1 + C1) / 2",
        "-20",
        "20",
    ] {
        let added = save_new_unnamed_tier(&mut session, text);
        assert_eq!(
            indicatrix_cut_core::design::labelling::name_indicates_pavilion(&added.name),
            added.angle_deg.is_sign_negative(),
            "{text}: {} at {}",
            added.name,
            added.angle_deg
        );
    }
}

#[test]
fn a_name_the_cutter_typed_and_an_existing_tier_are_left_alone() {
    let design = design();
    let others = vec!["P1".to_owned(), "C1".to_owned()];
    let mut typed = tier("Keel", 40.0);
    settle_saved_tier(&mut typed, &design, -1, &others);
    assert_eq!(typed.name, "Keel");
    // Deliberately blanking an existing tier is not undone by the auto-name.
    let mut blanked = tier("", 36.0);
    settle_saved_tier(&mut blanked, &design, 1, &others);
    assert_eq!(blanked.name, "");
}
