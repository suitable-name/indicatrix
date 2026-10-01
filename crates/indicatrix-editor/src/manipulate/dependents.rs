//! Which tiers meet a tier by name (a drag drags them along; a rename or removal keeps
//! them consistent) and which tiers' solved mast actually moved during a drag.

use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier};
use indicatrix_cut_core::{ConstraintTier, Design, Edit};

/// The tiers whose `MeetNamed` constraint names one of `design.tiers[tier]`'s names.
///
/// Names resolve the way `ConstraintTier::meet_target_indices` does (first exact match,
/// then ASCII case-insensitive). Ascending; never contains `tier` itself. `MeetExisting`
/// and `ScaleReference` tiers name nothing, so they never appear. Empty when `tier` does
/// not exist or has no name.
#[must_use]
pub fn tiers_meeting(design: &Design, tier: usize) -> Vec<usize> {
    if design.tiers.get(tier).is_none() {
        return Vec::new();
    }
    design
        .tiers
        .iter()
        .enumerate()
        .filter(|&(index, other)| {
            index != tier && other.meet_target_indices(&design.tiers).contains(&tier)
        })
        .map(|(index, _)| index)
        .collect()
}

/// The tiers, other than `exclude`, whose solved mast moved by more than `tol`.
///
/// `exclude` is the dragged tier; the masts are compared between `before` and `after`.
/// Ascending. Empty when the two lists are not the same length (the tier list changed
/// under the drag, so nothing is comparable).
#[must_use]
pub fn moved_tiers(
    before: &[SolvedTier],
    after: &[SolvedTier],
    exclude: usize,
    tol: f64,
) -> Vec<usize> {
    if before.len() != after.len() {
        return Vec::new();
    }
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|&(index, (old, new))| index != exclude && (new.mast - old.mast).abs() > tol)
        .map(|(index, _)| index)
        .collect()
}

/// The position of the tier a `MeetNamed` token resolves to: the first tier bearing the
/// name exactly, else the first bearing it ASCII case-insensitively -- the precedence
/// `ConstraintTier::meet_target_indices` applies.
fn resolve_token(token: &str, tiers: &[ConstraintTier]) -> Option<usize> {
    tiers
        .iter()
        .position(|tier| tier.names().contains(&token))
        .or_else(|| {
            tiers
                .iter()
                .position(|tier| tier.names().iter().any(|n| n.eq_ignore_ascii_case(token)))
        })
}

/// `constraint` with every `MeetNamed` token that resolves to tier `target` replaced by
/// `replacement(token)` (dropped when that returns `None`, and repeats collapsed); a
/// list that loses every name becomes `MeetExisting`, since an empty `MeetNamed` is not
/// a valid constraint. `None` when the constraint names `target` nowhere.
fn rewrite_meet_names(
    constraint: &MeetConstraint,
    tiers: &[ConstraintTier],
    target: usize,
    replacement: &impl Fn(&str) -> Option<String>,
) -> Option<MeetConstraint> {
    let MeetConstraint::MeetNamed(names) = constraint else {
        return None;
    };
    let mut changed = false;
    let mut rewritten: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let new_name = if resolve_token(name, tiers) == Some(target) {
            changed = true;
            replacement(name.as_str())
        } else {
            Some(name.clone())
        };
        if let Some(new_name) = new_name
            && !rewritten.contains(&new_name)
        {
            rewritten.push(new_name);
        }
    }
    if !changed {
        return None;
    }
    Some(if rewritten.is_empty() {
        MeetConstraint::MeetExisting
    } else {
        MeetConstraint::MeetNamed(rewritten)
    })
}

/// One `Edit::ModifyTier` per tier that meets `design.tiers[target]` by name
/// ([`tiers_meeting`]), each with its `constraint` (and its `imported_meet`, when that
/// also names the tier) rewritten by `replacement`; ascending by tier.
fn dependant_edits(
    design: &Design,
    target: usize,
    replacement: &impl Fn(&str) -> Option<String>,
) -> Vec<Edit> {
    tiers_meeting(design, target)
        .into_iter()
        .filter_map(|index| {
            let tier = design.tiers.get(index)?;
            let constraint =
                rewrite_meet_names(&tier.constraint, &design.tiers, target, replacement);
            let imported = tier
                .imported_meet
                .as_ref()
                .and_then(|meet| rewrite_meet_names(meet, &design.tiers, target, replacement));
            if constraint.is_none() && imported.is_none() {
                return None;
            }
            let mut updated = tier.clone();
            if let Some(constraint) = constraint {
                updated.constraint = constraint;
            }
            if let Some(imported) = imported {
                updated.imported_meet = Some(imported);
            }
            Some(Edit::ModifyTier {
                index,
                tier: updated,
            })
        })
        .collect()
}

/// The edits that keep every tier meeting `design.tiers[index]` by name pointed at it
/// once that tier is renamed to `new_name`.
///
/// Each dependant's `MeetNamed` list (and a named `imported_meet`) has the old name
/// replaced by the new one at the same position of a `/`-joined multi-name, else by the
/// first new name. Empty when the name does not change, when `new_name` names nothing
/// (an unnamed tier cannot be a meet target, so there is nothing to point at), or when
/// no tier meets this one by name. Apply them in the same `Edit::Batch` as the rename.
#[must_use]
pub fn rename_dependant_edits(design: &Design, index: usize, new_name: &str) -> Vec<Edit> {
    let Some(old) = design.tiers.get(index) else {
        return Vec::new();
    };
    let new_tokens: Vec<&str> = new_name.split('/').filter(|t| !t.is_empty()).collect();
    if new_tokens.is_empty() || old.name == new_name {
        return Vec::new();
    }
    let old_tokens = old.names();
    let replacement = |token: &str| -> Option<String> {
        let position = old_tokens
            .iter()
            .position(|known| *known == token)
            .or_else(|| {
                old_tokens
                    .iter()
                    .position(|known| known.eq_ignore_ascii_case(token))
            })
            .unwrap_or(0);
        new_tokens
            .get(position)
            .or_else(|| new_tokens.first())
            .map(|new| (*new).to_string())
    };
    dependant_edits(design, index, &replacement)
}

/// The edits that take every name pointing at `design.tiers[index]` out of the tiers
/// meeting it by name, for removing that tier without leaving dangling references.
///
/// A dependant left with no names meets whatever vertex the solver finds
/// (`MeetExisting`) instead. Empty when no tier meets this one by name. Apply them in
/// the same `Edit::Batch`, before the `Edit::RemoveTier`, so their indices still hold.
#[must_use]
pub fn clear_dependant_edits(design: &Design, index: usize) -> Vec<Edit> {
    let drop_name = |_: &str| -> Option<String> { None };
    dependant_edits(design, index, &drop_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::PreformSpec;

    fn tier(name: &str, constraint: MeetConstraint) -> ConstraintTier {
        ConstraintTier {
            angle_deg: -40.0,
            name: name.to_string(),
            indices: vec![0.0],
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    fn design_of(tiers: Vec<ConstraintTier>) -> Design {
        let mut design = Design::fresh(PreformSpec::cylinder(96, 1.5, 1.0, 1.5), 96, 8, 1.54);
        design.tiers = tiers;
        design
    }

    fn named(names: &[&str]) -> MeetConstraint {
        MeetConstraint::MeetNamed(names.iter().map(|name| (*name).to_string()).collect())
    }

    fn apply_all(design: &mut Design, edits: Vec<Edit>) {
        for edit in edits {
            design.apply_edit(edit).expect("a dependant edit applies");
        }
    }

    #[test]
    fn a_rename_rewrites_every_name_pointing_at_the_tier() {
        let mut design = design_of(vec![
            tier("P1", MeetConstraint::ScaleReference(0.5)),
            tier("P2", named(&["p1", "C9"])),
            tier("C1", named(&["P1", "P2"])),
            tier("X", MeetConstraint::MeetExisting),
        ]);
        let edits = rename_dependant_edits(&design, 0, "Main");
        assert_eq!(edits.len(), 2, "only the two tiers that name P1");
        apply_all(&mut design, edits);
        assert_eq!(design.tiers[1].constraint, named(&["Main", "C9"]));
        assert_eq!(design.tiers[2].constraint, named(&["Main", "P2"]));
        assert_eq!(design.tiers[3].constraint, MeetConstraint::MeetExisting);
    }

    #[test]
    fn a_rename_keeps_the_position_of_a_multi_name_token() {
        let mut design = design_of(vec![
            tier("P2/P3", MeetConstraint::ScaleReference(0.5)),
            tier("B", named(&["P3"])),
        ]);
        let edits = rename_dependant_edits(&design, 0, "Q/R");
        apply_all(&mut design, edits);
        assert_eq!(design.tiers[1].constraint, named(&["R"]));

        // Fewer new names than old ones: a reference falls back to the first.
        let mut design = design_of(vec![
            tier("P2/P3", MeetConstraint::ScaleReference(0.5)),
            tier("B", named(&["P3"])),
        ]);
        let edits = rename_dependant_edits(&design, 0, "Q");
        apply_all(&mut design, edits);
        assert_eq!(design.tiers[1].constraint, named(&["Q"]));
    }

    #[test]
    fn a_rename_that_changes_nothing_or_names_nothing_rewrites_nothing() {
        let design = design_of(vec![
            tier("P1", MeetConstraint::ScaleReference(0.5)),
            tier("P2", named(&["P1"])),
        ]);
        let none: Vec<Edit> = Vec::new();
        assert_eq!(rename_dependant_edits(&design, 0, "P1"), none);
        assert_eq!(rename_dependant_edits(&design, 0, ""), none);
        assert_eq!(
            rename_dependant_edits(&design, 1, "Q"),
            none,
            "nobody meets P2"
        );
        assert_eq!(rename_dependant_edits(&design, 9, "Q"), none);
    }

    #[test]
    fn clearing_drops_the_names_and_falls_back_to_meet_existing() {
        let mut design = design_of(vec![
            tier("P1", MeetConstraint::ScaleReference(0.5)),
            tier("P2", named(&["P1"])),
            tier("C1", named(&["P1", "X"])),
            tier("X", MeetConstraint::ScaleReference(0.9)),
        ]);
        design.tiers[2].imported_meet = Some(named(&["P1"]));
        let edits = clear_dependant_edits(&design, 0);
        apply_all(&mut design, edits);
        assert_eq!(design.tiers[1].constraint, MeetConstraint::MeetExisting);
        assert_eq!(design.tiers[2].constraint, named(&["X"]));
        assert_eq!(
            design.tiers[2].imported_meet,
            Some(MeetConstraint::MeetExisting)
        );
        assert_eq!(tiers_meeting(&design, 0), Vec::<usize>::new());
    }
}
