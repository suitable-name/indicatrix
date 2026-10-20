//! The undo-list labels of the batches a retarget builds, split out of `edit_type.rs`
//! ([`super::Edit::describe`]'s `Batch` arm calls these first).

use super::{Edit, material_display_name};
use crate::{design::Design, material::MaterialSelection};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// A retarget that also re-anchors masts, as the editor session builds it: angle changes plus
/// the [`Edit::SetConstraint`]s that keep each facet's girdle edge in place, and the material
/// change when there is one. `None` for a batch with no constraint change (those keep their
/// older labels) or with any other kind of edit in it.
pub(super) fn describe_anchored_retarget(edits: &[Edit]) -> Option<String> {
    let mut has_angles = false;
    let mut has_constraint = false;
    let mut material: Option<&MaterialSelection> = None;
    for edit in edits {
        match edit {
            Edit::RetargetAngles { .. } => has_angles = true,
            Edit::SetConstraint { .. } => has_constraint = true,
            Edit::SetMaterial {
                material: selection,
            } => material = Some(selection),
            _ => return None,
        }
    }
    if !(has_angles && has_constraint) {
        return None;
    }
    Some(retarget_label(material))
}

/// The atomic batch a checked retarget builds: one [`Edit::ModifyTier`] per tier whose angle or
/// mast moved (plus the material change when there is one, and the angle updates of the tiers
/// that follow a relation).
///
/// A batch reads as a retarget only when it can be nothing else: every `ModifyTier` turns an
/// existing tier of `design` and changes nothing but its angle and its `ScaleReference` mast
/// (same name, indices, targets and constraint kind), at least one angle really changes, and
/// there are at least two modified tiers or a material change (a lone modified tier keeps the
/// label of a modified tier). `None` for any other batch.
///
/// An Optimize Apply builds the same shape; it passes its own label to the session, which is
/// what the history shows.
pub(super) fn describe_modified_tiers_retarget(edits: &[Edit], design: &Design) -> Option<String> {
    let mut modified = 0;
    let mut angle_changed = false;
    let mut material: Option<&MaterialSelection> = None;
    for edit in edits {
        match edit {
            Edit::ModifyTier { index, tier } => {
                let current = design.tiers.get(*index)?;
                let mut expected = current.clone();
                expected.angle_deg = tier.angle_deg;
                if let (MeetConstraint::ScaleReference(_), MeetConstraint::ScaleReference(_)) =
                    (&current.constraint, &tier.constraint)
                {
                    expected.constraint = tier.constraint.clone();
                }
                if expected != *tier {
                    return None;
                }
                angle_changed |= current.angle_deg.to_bits() != tier.angle_deg.to_bits();
                modified += 1;
            }
            Edit::RetargetAngles { .. } => {}
            Edit::SetMaterial {
                material: selection,
            } => material = Some(selection),
            _ => return None,
        }
    }
    if !angle_changed || (modified < 2 && material.is_none()) {
        return None;
    }
    Some(retarget_label(material))
}

/// "Retarget angles", or "Retarget for <material>" when the batch also changes the material.
fn retarget_label(material: Option<&MaterialSelection>) -> String {
    material.map_or_else(
        || "Retarget angles".to_string(),
        |selection| format!("Retarget for {}", material_display_name(selection)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design::{ConstraintTier, ScheduleMeta},
        preform::PreformSpec,
    };

    fn tier(name: &str, angle_deg: f64, mast: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::ScaleReference(mast),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    fn design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::default(),
            vec![tier("P1", -40.0, 0.5), tier("P2", -42.0, 0.6)],
        )
    }

    fn modify(index: usize, new: ConstraintTier) -> Edit {
        Edit::ModifyTier { index, tier: new }
    }

    #[test]
    fn modified_tiers_that_only_turn_and_re_anchor_read_as_a_retarget() {
        let design = design();
        let batch = Edit::Batch(vec![
            modify(0, tier("P1", -41.0, 0.52)),
            modify(1, tier("P2", -43.0, 0.61)),
        ]);
        assert_eq!(batch.describe(&design), "Retarget angles");
    }

    #[test]
    fn a_material_change_in_the_batch_names_the_material() {
        let design = design();
        let material = MaterialSelection {
            name: Some("Quartz".to_string()),
            ..MaterialSelection::default()
        };
        let batch = Edit::Batch(vec![
            modify(0, tier("P1", -41.0, 0.52)),
            Edit::SetMaterial { material },
        ]);
        assert_eq!(batch.describe(&design), "Retarget for Quartz");
    }

    #[test]
    fn a_rename_or_an_index_change_is_not_a_retarget() {
        let design = design();
        let renamed = Edit::Batch(vec![
            modify(0, tier("P1x", -41.0, 0.52)),
            modify(1, tier("P2", -43.0, 0.61)),
        ]);
        assert_eq!(renamed.describe(&design), "2 combined edits");
        let mut other_indices = tier("P2", -43.0, 0.61);
        other_indices.indices = vec![0.0, 48.0];
        let reindexed = Edit::Batch(vec![
            modify(0, tier("P1", -41.0, 0.52)),
            modify(1, other_indices),
        ]);
        assert_eq!(reindexed.describe(&design), "2 combined edits");
    }

    #[test]
    fn masts_alone_or_a_lone_tier_keep_their_older_labels() {
        let design = design();
        let masts_only = Edit::Batch(vec![
            modify(0, tier("P1", -40.0, 0.52)),
            modify(1, tier("P2", -42.0, 0.61)),
        ]);
        assert_eq!(masts_only.describe(&design), "2 combined edits");
        let lone = Edit::Batch(vec![modify(0, tier("P1", -41.0, 0.52))]);
        assert_ne!(lone.describe(&design), "Retarget angles");
    }
}
