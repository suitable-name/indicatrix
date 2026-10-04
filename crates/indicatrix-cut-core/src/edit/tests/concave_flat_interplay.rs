//! Edits where the flat and concave tier lists meet: the shared-name rule on
//! flat adds/modifies, and gear remaps of concave indices.

use super::fixtures::{fresh_design, tier};
use crate::{
    design::{ConcaveTier, ConcaveTool, ToolMotion},
    edit::{Edit, RemapRounding},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn groove(indices: &[f64]) -> ConcaveTier {
    ConcaveTier {
        name: "Groove".to_owned(),
        angle_deg: -40.0,
        indices: indices.to_vec(),
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.0, 0.1],
        diameter_ratio: 0.5,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

#[test]
fn a_flat_tier_cannot_take_a_concave_tiers_name() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddTier {
            index: 0,
            tier: tier("P1", -41.0, MeetConstraint::ScaleReference(0.5), &[0.0]),
        })
        .unwrap();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[0.0, 24.0]),
        })
        .unwrap();
    let before = design.clone();

    // Adding, and renaming (also as one of several `/`-joined names), are refused
    // and leave the design untouched.
    for name in ["Groove", "P2/Groove"] {
        let clashing = tier(name, -30.0, MeetConstraint::MeetExisting, &[0.0]);
        assert!(
            design
                .apply_edit(Edit::AddTier {
                    index: 1,
                    tier: clashing.clone()
                })
                .is_err()
        );
        assert!(
            design
                .apply_edit(Edit::ModifyTier {
                    index: 0,
                    tier: clashing
                })
                .is_err()
        );
    }
    assert_eq!(design, before);
    design.validate_concave_tiers().expect("still valid");

    // A different name is fine.
    design
        .apply_edit(Edit::ModifyTier {
            index: 0,
            tier: tier("P9", -41.0, MeetConstraint::ScaleReference(0.5), &[0.0]),
        })
        .unwrap();
}

#[test]
fn remapping_wraps_a_concave_index_that_rounds_up_to_the_gear() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[0.0, 95.0]),
        })
        .unwrap();
    // 95 * 80 / 96 = 79.17, which Ceil rounds to 80: position 0 of the 80 gear.
    let inverse = design
        .apply_edit(Edit::RemapIndices {
            from_gear: 96,
            to_gear: 80,
            rounding: RemapRounding::Ceil,
        })
        .unwrap();
    assert_eq!(design.concave_tiers[0].indices, vec![0.0, 0.0]);
    design.meta.gear_teeth = 80;
    design
        .validate_concave_tiers()
        .expect("remapped list is valid");
    design.meta.gear_teeth = 96;
    design.apply_edit(inverse).unwrap();
    assert_eq!(design.concave_tiers[0].indices, vec![0.0, 95.0]);
}

#[test]
fn a_remap_that_would_invalidate_a_concave_tier_changes_nothing() {
    let mut design = fresh_design();
    design
        .apply_edit(Edit::AddTier {
            index: 0,
            tier: tier("P1", -41.0, MeetConstraint::ScaleReference(0.5), &[8.0]),
        })
        .unwrap();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(&[8.0]),
        })
        .unwrap();
    let before = design.clone();
    assert!(
        design
            .apply_edit(Edit::RemapIndices {
                from_gear: 96,
                to_gear: 0,
                rounding: RemapRounding::Nearest,
            })
            .is_err()
    );
    assert_eq!(design, before);
}
