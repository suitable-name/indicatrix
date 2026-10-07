//! The Slint-free decisions behind tier relations in the editor: which edit a Tier form
//! save becomes, which rows a change moves, how a refusal is worded and where it is shown.
//!
//! The relations themselves (parsing, evaluation, the refusal of a direct edit, the undo
//! step that carries the followers) live in `indicatrix_editor::session`; this file only
//! turns form state into session calls and session answers into words, so it can be
//! tested without a window.

use std::collections::BTreeSet;

use indicatrix_cut_core::{ConstraintTier, Design, Edit, TierTarget};
use indicatrix_editor::{
    loading::tier_form_error_field,
    session::{RelationNotice, SessionEditError},
    tier_save::tier_save_edit_with_target,
    view_model::live_margin::relation_angle_deg,
};

/// What an attempt to edit the angle of a tier that follows a relation says (the angle cell,
/// the keyboard and the scroll wheel; nothing is edited).
pub(in crate::gui::editor) const DRIVEN_ANGLE_HINT: &str =
    "This angle follows a relation. Edit it in the Tier form.";

/// The magnitude, in degrees, of the stand-in angle a new tier carries while its real angle
/// is still to be worked out from a relation that cannot be read yet (the save is refused
/// with the relation's own error before the stand-in ever lands in the design).
const STAND_IN_ANGLE_DEG: f64 = 45.0;

/// Whether the Angle field's text is a relation (`=C1-4`) rather than a number.
pub(in crate::gui::editor) fn is_relation_text(angle_text: &str) -> bool {
    angle_text.trim_start().starts_with('=')
}

/// The angle the tier form's parse carries while the Angle field holds a relation.
///
/// An existing tier keeps its current angle (the relation, not the form, decides the new
/// one). A new tier takes the angle its relation would give, on the side of the first tier
/// the relation reads; when the relation cannot be read yet, a stand-in on the side its name
/// suggests (a pavilion name such as `P3` is a pavilion tier).
pub(in crate::gui::editor) fn relation_placeholder_angle_deg(
    design: &Design,
    index: i32,
    angle_text: &str,
    name: &str,
) -> f64 {
    if let Some(current) = usize::try_from(index)
        .ok()
        .and_then(|position| design.tiers.get(position))
    {
        return current.angle_deg;
    }
    relation_angle_deg(design, angle_text).unwrap_or_else(|| {
        if indicatrix_cut_core::design::labelling::name_indicates_pavilion(name.trim()) {
            -STAND_IN_ANGLE_DEG
        } else {
            STAND_IN_ANGLE_DEG
        }
    })
}

/// The edit a Tier form save becomes, and the position the saved tier ends up at.
///
/// Without `relation_text` (the Angle field held a number or arithmetic) this is exactly
/// [`tier_save_edit_with_target`]: a plain angle on a tier that follows a relation is then
/// refused by the session, never silently applied.
///
/// With `relation_text` (the field held `=...`) the tier is saved as usual and the tier's
/// relation is set in the same edit, so one Undo reverts both. When only the relation
/// changes (the tier is otherwise as it was) the edit is the lone `SetTierRelation`, which
/// reads best in Edit > Undo. A relation the tier already follows exactly adds nothing.
///
/// # Errors
///
/// [`SessionEditError::Relation`] when the relation text cannot be read (unknown tier, a
/// bracket left open, ...) or the tier cannot follow a relation at all (a table, culet or
/// girdle tier).
pub(in crate::gui::editor) fn plan_tier_save(
    design: &Design,
    index: i32,
    tier: ConstraintTier,
    relation_text: Option<&str>,
    target: Option<TierTarget>,
    selected: Option<usize>,
) -> Result<(usize, Edit), SessionEditError> {
    let Some(text) = relation_text else {
        return Ok(tier_save_edit_with_target(
            design, index, tier, target, selected,
        ));
    };
    let relation = design.parse_relation(text)?;
    let existing = usize::try_from(index).ok();
    if let Some(position) = existing {
        design.check_relation_target(position)?;
    }
    let relation_changed =
        existing.is_none_or(|position| design.tier_relation(position) != Some(&relation));
    let only_the_relation = existing.is_some_and(|position| {
        design.tiers.get(position) == Some(&tier)
            && target.is_none()
            && design.tier_target(position).is_none()
    });
    if !relation_changed {
        return Ok(tier_save_edit_with_target(
            design, index, tier, target, selected,
        ));
    }
    if let Some(position) = existing
        && only_the_relation
    {
        let edit = Edit::SetTierRelation {
            index: position,
            relation: Some(relation),
        };
        return Ok((position, edit));
    }
    let (saved_at, base) = tier_save_edit_with_target(design, index, tier, target, selected);
    let set = Edit::SetTierRelation {
        index: saved_at,
        relation: Some(relation),
    };
    let edit = match base {
        Edit::Batch(mut steps) => {
            steps.push(set);
            Edit::Batch(steps)
        }
        single => Edit::Batch(vec![single, set]),
    };
    Ok((saved_at, edit))
}

/// `seeds` plus every tier whose angle follows one of them, directly or through other
/// followers (positions in `design`, which is the design AFTER the edit). These are the rows
/// an edit of a seed moved, so the table, the stale marks and the preview refresh them too.
pub(in crate::gui::editor) fn with_followers(
    design: &Design,
    seeds: impl IntoIterator<Item = usize>,
) -> BTreeSet<usize> {
    let mut all: BTreeSet<usize> = seeds.into_iter().collect();
    if design.tier_relations.is_empty() {
        return all;
    }
    let mut frontier: Vec<usize> = all.iter().copied().collect();
    while let Some(tier) = frontier.pop() {
        for dependant in design.relation_dependants(tier) {
            if all.insert(dependant) {
                frontier.push(dependant);
            }
        }
    }
    all
}

/// Splits `targets` into the tiers whose angle follows a relation (they take no direct
/// angle edit) and the free ones, each in the given order.
pub(in crate::gui::editor) fn split_driven_targets(
    design: &Design,
    targets: &[usize],
) -> (Vec<usize>, Vec<usize>) {
    targets
        .iter()
        .copied()
        .partition(|&tier| design.is_tier_driven(tier))
}

/// The sentence for relations an edit freed, to follow the edit's own toast: `None` when it
/// freed none. Names only the tiers; the notice's own text says more than a toast has room
/// for.
pub(in crate::gui::editor) fn relation_cleared_sentence(notice: &RelationNotice) -> Option<String> {
    let names = notice
        .cleared
        .iter()
        .map(|cleared| cleared.tier.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    match notice.cleared.len() {
        0 => None,
        1 => Some(format!(
            "Removed the relation of {names}; its angle stays as it is."
        )),
        _ => Some(format!(
            "Removed the relations of {names}; their angles stay as they are."
        )),
    }
}

/// The text a failed Tier form save shows and the form field it belongs to (see
/// `report_tier_form_error`). A refusal that concerns tier angles (a loop, an angle out of
/// range, a direct edit of a driven tier) is always the Angle field's, whatever tier it
/// names; any other failure is classified by its wording as before.
pub(in crate::gui::editor) fn save_error_text_and_field(
    error: &SessionEditError,
) -> (String, &'static str) {
    let text = error.to_string();
    let field = match error {
        SessionEditError::Driven(_) | SessionEditError::Relation(_) => "angle",
        SessionEditError::Edit(_) => tier_form_error_field(&text),
    };
    (text, field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_editor::{
        EditorSession,
        loading::{TierFormFields, parse_tier_form_with_relation},
        session::ClearedRelation,
    };

    fn tier(name: &str, angle_deg: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_owned(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::ScaleReference(0.65),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// P1 at -41 and P2 at -39 (pavilion), C1 at 34.5 (crown), none following anything.
    fn session() -> EditorSession {
        let mut session = EditorSession::fresh();
        for (index, (name, angle)) in [("P1", -41.0), ("P2", -39.0), ("C1", 34.5)]
            .into_iter()
            .enumerate()
        {
            session
                .apply(Edit::AddTier {
                    index,
                    tier: tier(name, angle),
                })
                .expect("a tier is added");
        }
        session
    }

    /// The form fields for a tier called `name` with the angle text `angle`.
    fn fields<'a>(angle: &'a str, name: &'a str) -> TierFormFields<'a> {
        TierFormFields {
            angle,
            constraint_kind: 2,
            constraint_text: "0.65",
            name,
            indices: "0, 24",
            gear_teeth_abs: 96,
            imported_meet: None,
            original_notes: None,
            other_tier_names: Vec::new(),
        }
    }

    /// What saving the form for tier `index` does to `session`, the way the Save callback
    /// maps it: parse (with the stand-in angle), plan, apply.
    fn save(
        session: &mut EditorSession,
        index: i32,
        angle: &str,
        name: &str,
    ) -> Result<(usize, Edit), SessionEditError> {
        let placeholder = relation_placeholder_angle_deg(&session.design, index, angle, name);
        let (mut tier, relation) =
            parse_tier_form_with_relation(fields(angle, name), placeholder).expect("a valid form");
        // The callback's side handling for a saved tier.
        if let Some(current) = usize::try_from(index)
            .ok()
            .and_then(|position| session.design.tiers.get(position))
            && current.angle_deg.is_sign_negative()
        {
            tier.angle_deg = -tier.angle_deg.abs();
        }
        let (at, edit) = plan_tier_save(
            &session.design,
            index,
            tier,
            relation.as_deref(),
            None,
            None,
        )?;
        session.try_apply(edit.clone())?;
        Ok((at, edit))
    }

    #[test]
    fn a_plain_number_and_arithmetic_set_the_angle_and_only_an_equals_sign_makes_a_relation() {
        let mut session = session();
        let placeholder = -39.0;
        let (_, relation) =
            parse_tier_form_with_relation(fields("41.5+0.3", "P2"), placeholder).unwrap();
        assert_eq!(relation, None, "arithmetic is just a number");
        let (tier, relation) =
            parse_tier_form_with_relation(fields("=C1-4", "P2"), placeholder).unwrap();
        assert_eq!(relation.as_deref(), Some("C1-4"));
        assert_eq!(tier.angle_deg.to_bits(), placeholder.to_bits());

        // Arithmetic on a free tier: a plain ModifyTier, no relation anywhere.
        let (at, edit) = save(&mut session, 1, "-38.5+0.5", "P2").unwrap();
        assert_eq!(at, 1);
        assert!(
            matches!(edit, Edit::ModifyTier { index: 1, .. }),
            "{edit:?}"
        );
        assert!(session.design.tier_relations.is_empty());
        assert!((session.design.tiers[1].angle_deg - -38.0).abs() < 1e-12);
    }

    #[test]
    fn typing_a_relation_on_a_free_tier_is_one_lone_undoable_relation_edit() {
        let mut session = session();
        let (at, edit) = save(&mut session, 1, "=P1 - 2", "P2").unwrap();
        assert_eq!(at, 1);
        assert!(
            matches!(
                edit,
                Edit::SetTierRelation {
                    index: 1,
                    relation: Some(_)
                }
            ),
            "{edit:?}"
        );
        assert!(session.is_driven(1));
        assert_eq!(session.tier_relation_display(1).as_deref(), Some("P1 - 2"));
        // P1 is 41 in magnitude, so P2 follows at 39 on its own (pavilion) side.
        assert!((session.design.tiers[1].angle_deg - -39.0).abs() < 1e-9);
        session.undo().unwrap();
        assert!(!session.is_driven(1));
    }

    #[test]
    fn a_relation_with_other_changes_is_one_batch_and_one_undo_step() {
        let mut session = session();
        // Rename P2 to P2b and make it follow P1 in the same save.
        let (_, edit) = save(&mut session, 1, "=P1 - 3", "P2b").unwrap();
        assert!(matches!(&edit, Edit::Batch(steps) if matches!(
            steps.last(),
            Some(Edit::SetTierRelation { index: 1, .. })
        )));
        assert_eq!(session.design.tiers[1].name, "P2b");
        assert!((session.design.tiers[1].angle_deg - -38.0).abs() < 1e-9);
        session.undo().unwrap();
        assert_eq!(session.design.tiers[1].name, "P2");
        assert!(!session.is_driven(1));
        assert!((session.design.tiers[1].angle_deg - -39.0).abs() < 1e-9);
    }

    #[test]
    fn a_new_tier_typed_as_a_relation_is_added_driven_on_the_side_of_the_tier_it_reads() {
        let mut session = session();
        let (at, edit) = save(&mut session, -1, "= C1 + 1.5", "").unwrap();
        assert_eq!(at, 3);
        assert!(
            matches!(&edit, Edit::Batch(steps) if matches!(steps[0], Edit::AddTier { .. })),
            "{edit:?}"
        );
        assert!(session.is_driven(3));
        assert!((session.design.tiers[3].angle_deg - 36.0).abs() < 1e-9);
        // The same text on a pavilion tier lands on the pavilion side.
        save(&mut session, -1, "=P1 - 2", "").unwrap();
        let added = session.design.tiers.len() - 1;
        assert!(session.design.tiers[added].angle_deg < 0.0);
        // The stand-in leans on the name when the relation cannot be read yet.
        assert!(
            relation_placeholder_angle_deg(&session.design, -1, "=Q9", "P7") < 0.0,
            "a pavilion name"
        );
        assert!(relation_placeholder_angle_deg(&session.design, -1, "=Q9", "C7") > 0.0);
    }

    #[test]
    fn saving_a_driven_tier_with_its_relation_unchanged_keeps_following() {
        let mut session = session();
        save(&mut session, 1, "=P1 - 2", "P2").unwrap();
        // Rename it, leaving the relation text as the form shows it.
        let (_, edit) = save(&mut session, 1, "=P1 - 2", "P2c").unwrap();
        assert!(
            matches!(edit, Edit::ModifyTier { index: 1, .. }),
            "{edit:?}"
        );
        assert!(session.is_driven(1));
        assert_eq!(session.design.tiers[1].name, "P2c");
        // Editing the relation replaces it.
        save(&mut session, 1, "=P1 - 5", "P2c").unwrap();
        assert!((session.design.tiers[1].angle_deg - -36.0).abs() < 1e-9);
    }

    #[test]
    fn a_plain_angle_on_a_driven_tier_is_refused_in_words_at_the_angle_field() {
        let mut session = session();
        save(&mut session, 1, "=P1 - 2", "P2").unwrap();
        let error = save(&mut session, 1, "38", "P2").unwrap_err();
        assert!(matches!(error, SessionEditError::Driven(_)), "{error:?}");
        let (text, field) = save_error_text_and_field(&error);
        assert_eq!(
            text,
            "This angle follows a relation (P2 = P1 - 2). Edit the relation or remove it."
        );
        assert_eq!(field, "angle");
        // Nothing changed.
        assert!((session.design.tiers[1].angle_deg - -39.0).abs() < 1e-9);
        // Typing the angle it already has is not an edit, so the rest of the form saves.
        save(&mut session, 1, "39", "P2z").unwrap();
        assert_eq!(session.design.tiers[1].name, "P2z");
    }

    #[test]
    fn relation_mistakes_are_refused_at_the_angle_field_in_plain_words() {
        let mut session = session();
        save(&mut session, 1, "=P1 - 2", "P2").unwrap();
        // A loop: P1 would read P2 while P2 reads P1.
        let (text, field) =
            save_error_text_and_field(&save(&mut session, 0, "=P2 + 2", "P1").unwrap_err());
        assert_eq!(field, "angle");
        assert!(text.contains("loop"), "{text}");
        // A tier that does not exist.
        let (text, field) =
            save_error_text_and_field(&save(&mut session, 2, "=Q9 - 1", "C1").unwrap_err());
        assert_eq!(field, "angle");
        assert!(text.contains("Q9"), "{text}");
        // A result that is not a facet angle.
        let (text, field) =
            save_error_text_and_field(&save(&mut session, 2, "=P1 + 60", "C1").unwrap_err());
        assert_eq!(field, "angle");
        assert!(text.contains("more than 0"), "{text}");
        // None of it changed the design.
        assert!(!session.is_driven(0) && !session.is_driven(2));
    }

    #[test]
    fn a_wrong_edit_that_is_no_relation_problem_is_classified_by_its_wording() {
        let error = SessionEditError::Edit(indicatrix_cut_core::EditError {
            index: 9,
            tier_count: 3,
        });
        let (text, field) = save_error_text_and_field(&error);
        assert_eq!(field, tier_form_error_field(&text));
    }

    #[test]
    fn followers_are_found_through_chains_and_not_upwards() {
        let mut session = session();
        session.set_tier_relation(1, "=P1 - 2").unwrap();
        // A third pavilion tier follows P2, which follows P1.
        session
            .apply(Edit::AddTier {
                index: 3,
                tier: tier("P3", -30.0),
            })
            .unwrap();
        session.set_tier_relation(3, "=P2 - 4").unwrap();
        let design = &session.design;
        assert_eq!(
            with_followers(design, [0]),
            BTreeSet::from([0, 1, 3]),
            "P1 moves P2 and, through it, P3"
        );
        assert_eq!(with_followers(design, [1]), BTreeSet::from([1, 3]));
        assert_eq!(with_followers(design, [3]), BTreeSet::from([3]));
        assert_eq!(with_followers(design, [2]), BTreeSet::from([2]));
        let plain = EditorSession::fresh().design;
        assert_eq!(with_followers(&plain, [4, 2]), BTreeSet::from([2, 4]));
    }

    #[test]
    fn nudge_targets_split_into_driven_and_free() {
        let mut session = session();
        session.set_tier_relation(1, "=P1 - 2").unwrap();
        let (driven, free) = split_driven_targets(&session.design, &[0, 1, 2]);
        assert_eq!(driven, vec![1]);
        assert_eq!(free, vec![0, 2]);
        let (driven, free) = split_driven_targets(&session.design, &[]);
        assert!(driven.is_empty() && free.is_empty());
    }

    #[test]
    fn the_cleared_sentence_names_the_tiers() {
        let cleared = |tier: &str| ClearedRelation {
            tier: tier.to_owned(),
            relation: "P1 - 2".to_owned(),
        };
        assert_eq!(
            relation_cleared_sentence(&RelationNotice {
                cleared: vec![cleared("P2"), cleared("P3")]
            })
            .as_deref(),
            Some("Removed the relations of P2, P3; their angles stay as they are.")
        );
        assert_eq!(
            relation_cleared_sentence(&RelationNotice {
                cleared: vec![cleared("P2")]
            })
            .as_deref(),
            Some("Removed the relation of P2; its angle stays as it is.")
        );
        assert_eq!(
            relation_cleared_sentence(&RelationNotice {
                cleared: Vec::new()
            }),
            None
        );
    }

    #[test]
    fn deleting_a_tier_others_follow_gives_the_notice_the_toast_is_built_from() {
        let mut session = session();
        session.set_tier_relation(1, "=P1 - 2").unwrap();
        session.apply(Edit::RemoveTier { index: 0 }).unwrap();
        let notice = session.take_relation_notice().expect("P2 lost its driver");
        assert_eq!(
            relation_cleared_sentence(&notice).as_deref(),
            Some("Removed the relation of P2; its angle stays as it is.")
        );
        assert!(!session.is_driven(0), "P2 is the first tier now");
        assert!(session.take_relation_notice().is_none(), "taken once");
    }

    #[test]
    fn removing_a_relation_keeps_the_current_angle() {
        let mut session = session();
        session.set_tier_relation(1, "=P1 - 2").unwrap();
        assert!(session.clear_tier_relation(1).unwrap().is_some());
        assert!(!session.is_driven(1));
        assert!((session.design.tiers[1].angle_deg - -39.0).abs() < 1e-9);
        assert!(
            session.clear_tier_relation(1).unwrap().is_none(),
            "already free"
        );
    }

    #[test]
    fn a_table_culet_or_girdle_tier_cannot_be_given_a_relation_by_the_form() {
        let mut session = session();
        session
            .apply(Edit::AddTier {
                index: 3,
                tier: tier("Table", 0.0),
            })
            .unwrap();
        let error = save(&mut session, 3, "=C1 - 4", "Table").unwrap_err();
        let (text, field) = save_error_text_and_field(&error);
        assert_eq!(field, "angle");
        assert!(text.contains("cannot follow a relation"), "{text}");
    }

    #[test]
    fn the_hint_is_the_one_sentence_the_brief_asks_for() {
        assert_eq!(
            DRIVEN_ANGLE_HINT,
            "This angle follows a relation. Edit it in the Tier form."
        );
        assert!(is_relation_text("  =P1-2"));
        assert!(!is_relation_text("41.5+0.3"));
    }
}
