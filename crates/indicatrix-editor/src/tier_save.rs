//! The tier inspector's Save/Add Tier: which `Edit` a parsed tier form becomes.
//!
//! Moved from the desktop's `tier_actions::tier_form` so the web inspector saves a tier
//! exactly as the desktop does: a new tier inserts right after the selected row, an
//! existing tier modifies itself in place, and a depth / girdle-thickness / table-width
//! target is written (or cleared) in the same undo step.

use crate::manipulate::{dependents::rename_dependant_edits, tiers_meeting};
use indicatrix_cut_core::{ConstraintTier, Design, Edit, TierTarget};

/// Every OTHER tier's own name token, with the tier at `excluded_index` left out.
///
/// The tokens are `ConstraintTier::names()`, split on `/`. Feeds `TierFormFields::other_tier_names` so
/// `parse_tier_form` can reject name collisions. `excluded_index < 0` (a brand-new
/// tier) excludes nothing, since there is no existing row to exempt.
#[must_use]
pub fn other_tier_names_excluding(design: &Design, excluded_index: i32) -> Vec<String> {
    design
        .tiers
        .iter()
        .enumerate()
        .filter(|&(i, _)| excluded_index < 0 || i != excluded_index as usize)
        .flat_map(|(_, tier)| tier.names().into_iter().map(str::to_string))
        .collect()
}

/// Why saving `new_name` on the existing tier `index` must be refused: the tier is
/// being left unnamed while other tiers still meet it by name.
///
/// [`tier_save_edit`] follows a rename by rewriting those tiers' `MeetNamed` lists, but
/// an unnamed tier cannot be a meet target, so there is no name to rewrite them to and
/// their references would dangle. A caller shows this message instead of saving.
/// `None` when the save is safe: a new tier (`index < 0`), a name that is not blank, or
/// a tier nobody meets by name.
#[must_use]
pub fn unnaming_blocked_message(design: &Design, index: i32, new_name: &str) -> Option<String> {
    let index = usize::try_from(index).ok()?;
    if !new_name.trim().is_empty() {
        return None;
    }
    let old = design.tiers.get(index)?;
    let dependants = tiers_meeting(design, index);
    if dependants.is_empty() {
        return None;
    }
    let labels: Vec<String> = dependants
        .iter()
        .filter_map(|&i| {
            design
                .tiers
                .get(i)
                .map(|tier| crate::session::tier_nudge_label(tier, i))
        })
        .collect();
    Some(format!(
        "Cannot clear the name '{}': {} meet it by name. Change those tiers first.",
        old.name,
        labels.join(", ")
    ))
}

/// A tier save's edit and the index the saved tier ends up at.
///
/// A new tier (`index < 0`) inserts right after `selected` (the currently selected row)
/// instead of always appending -- an out-of-range or absent selection falls back to
/// appending at the end -- while an existing tier (`index >= 0`) simply modifies itself in
/// place.
///
/// Renaming an existing tier also rewrites the `MeetNamed` lists of the tiers that meet
/// it by name ([`rename_dependant_edits`]), in the same `Edit::Batch` so one undo reverts
/// both; a save that keeps the name, or that nobody meets, stays a plain
/// `Edit::ModifyTier`. Saving a tier unnamed is the one rename this cannot follow -- see
/// [`unnaming_blocked_message`].
#[must_use]
pub fn tier_save_edit(
    design: &Design,
    index: i32,
    tier: ConstraintTier,
    selected: Option<usize>,
) -> (usize, Edit) {
    if index < 0 {
        let append_index = design.tiers.len();
        let insert_after_selected = selected
            .filter(|&i| i < append_index)
            .map_or(append_index, |i| i + 1);
        (
            insert_after_selected,
            Edit::AddTier {
                index: insert_after_selected,
                tier,
            },
        )
    } else {
        let index = index as usize;
        let dependant_rewrites = rename_dependant_edits(design, index, &tier.name);
        let modify = Edit::ModifyTier { index, tier };
        if dependant_rewrites.is_empty() {
            (index, modify)
        } else {
            let mut steps = vec![modify];
            steps.extend(dependant_rewrites);
            (index, Edit::Batch(steps))
        }
    }
}

/// [`tier_save_edit`], extended for depth / girdle-thickness / table-width targets.
///
/// Wraps its edit in an [`Edit::Batch`] with [`Edit::SetTierTarget`] whenever `target`
/// is `Some`, or whenever the tier CURRENTLY at `index` already carries one -- which
/// must then be explicitly cleared (`Edit::SetTierTarget { target: None }`) the moment
/// the cutter saves with a plain Meets kind (0/1/2), or it would silently keep
/// resolving against a target the form no longer shows. A brand-new tier (`index < 0`)
/// never has one to clear.
#[must_use]
pub fn tier_save_edit_with_target(
    design: &Design,
    index: i32,
    tier: ConstraintTier,
    target: Option<TierTarget>,
    selected: Option<usize>,
) -> (usize, Edit) {
    let had_target = usize::try_from(index)
        .ok()
        .and_then(|i| design.tier_target(i))
        .is_some();
    let (dirty_index, edit) = tier_save_edit(design, index, tier, selected);
    let edit = if target.is_some() || had_target {
        Edit::Batch(vec![
            edit,
            Edit::SetTierTarget {
                index: dirty_index,
                target,
            },
        ])
    } else {
        edit
    };
    (dirty_index, edit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorSession;

    fn tier(name: &str) -> ConstraintTier {
        ConstraintTier {
            angle_deg: -40.0,
            name: name.to_string(),
            indices: vec![0.0],
            constraint: indicatrix::geometry::meet_solver::MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    #[test]
    fn other_tier_names_skips_only_the_excluded_row() {
        let mut design = EditorSession::fresh().design;
        design.tiers = vec![tier("P1"), tier("P2/P3"), tier("C1")];
        assert_eq!(
            other_tier_names_excluding(&design, 1),
            vec!["P1".to_string(), "C1".to_string()]
        );
        assert_eq!(other_tier_names_excluding(&design, -1).len(), 4);
    }

    #[test]
    fn a_new_tier_goes_after_the_selection_or_at_the_end() {
        let mut design = EditorSession::fresh().design;
        design.tiers = vec![tier("P1"), tier("P2"), tier("C1")];
        let (at, _) = tier_save_edit(&design, -1, tier("N"), Some(0));
        assert_eq!(at, 1);
        let (at, _) = tier_save_edit(&design, -1, tier("N"), None);
        assert_eq!(at, 3);
        let (at, _) = tier_save_edit(&design, -1, tier("N"), Some(9));
        assert_eq!(at, 3);
        let (at, edit) = tier_save_edit(&design, 2, tier("N"), Some(0));
        assert_eq!(at, 2);
        assert!(matches!(edit, Edit::ModifyTier { index: 2, .. }));
    }

    /// P1 pinned at 0.5, P2 meeting it by a differently-cased name, C1 meeting nothing.
    fn session_with_a_dependant() -> EditorSession {
        use indicatrix::geometry::meet_solver::MeetConstraint;
        let mut session = EditorSession::fresh();
        let mut p1 = tier("P1");
        p1.constraint = MeetConstraint::ScaleReference(0.5);
        let mut p2 = tier("P2");
        p2.constraint = MeetConstraint::MeetNamed(vec!["p1".to_string()]);
        for (index, t) in [p1, p2, tier("C1")].into_iter().enumerate() {
            session.apply(Edit::AddTier { index, tier: t }).unwrap();
        }
        session
    }

    #[test]
    fn renaming_a_tier_rewrites_the_tiers_meeting_it_in_one_undo_step() {
        use indicatrix::geometry::meet_solver::MeetConstraint;
        let mut session = session_with_a_dependant();
        let mut renamed = session.design.tiers[0].clone();
        renamed.name = "Main".to_string();
        let (at, edit) = tier_save_edit(&session.design, 0, renamed, None);
        assert_eq!(at, 0);
        assert!(matches!(&edit, Edit::Batch(steps) if steps.len() == 2));
        session.apply(edit).unwrap();
        assert_eq!(session.design.tiers[0].name, "Main");
        assert_eq!(
            session.design.tiers[1].constraint,
            MeetConstraint::MeetNamed(vec!["Main".to_string()])
        );
        session.undo().unwrap();
        assert_eq!(session.design.tiers[0].name, "P1");
        assert_eq!(
            session.design.tiers[1].constraint,
            MeetConstraint::MeetNamed(vec!["p1".to_string()])
        );
    }

    #[test]
    fn a_save_that_keeps_the_name_stays_a_plain_modify() {
        let session = session_with_a_dependant();
        let mut steeper = session.design.tiers[0].clone();
        steeper.angle_deg = -41.0;
        let (_, edit) = tier_save_edit(&session.design, 0, steeper, None);
        assert!(matches!(edit, Edit::ModifyTier { index: 0, .. }));
        // Renaming a tier nobody meets is plain too.
        let mut other = session.design.tiers[2].clone();
        other.name = "Q".to_string();
        let (_, edit) = tier_save_edit(&session.design, 2, other, None);
        assert!(matches!(edit, Edit::ModifyTier { index: 2, .. }));
    }

    #[test]
    fn leaving_a_met_tier_unnamed_is_blocked_with_the_dependants_named() {
        let session = session_with_a_dependant();
        let message = unnaming_blocked_message(&session.design, 0, "  ").expect("P2 meets P1");
        assert!(message.contains("tier 2 (P2)"), "{message}");
        assert!(unnaming_blocked_message(&session.design, 0, "Main").is_none());
        assert!(unnaming_blocked_message(&session.design, 1, "").is_none());
        assert!(unnaming_blocked_message(&session.design, -1, "").is_none());
        assert!(unnaming_blocked_message(&session.design, 9, "").is_none());
    }

    #[test]
    fn a_target_wraps_the_edit_in_a_batch_and_a_plain_save_leaves_it_alone() {
        let mut design = EditorSession::fresh().design;
        design.tiers = vec![tier("P1")];
        let (_, plain) = tier_save_edit_with_target(&design, 0, tier("P1"), None, None);
        assert!(matches!(plain, Edit::ModifyTier { .. }));
        let (_, with) = tier_save_edit_with_target(
            &design,
            0,
            tier("P1"),
            Some(TierTarget::DepthMm(2.0)),
            None,
        );
        assert!(matches!(with, Edit::Batch(edits) if edits.len() == 2));
    }
}
