//! `Edit::Batch`: applying/undoing every sub-edit as one step, rejecting a
//! partially-invalid batch without mutating the design, and `describe`'s
//! cutter-facing sentences across representative variants.

use super::fixtures::{fresh_design, tier};
use crate::{
    edit::{Edit, EditError, History, RemapRounding},
    material::MaterialSelection,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

#[test]
fn batch_applies_every_sub_edit_and_undoes_them_all_in_one_step() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::Batch(vec![
                Edit::RetargetAngles {
                    changes: vec![(0, -40.0, -42.0)],
                },
                Edit::SetMaterial {
                    material: MaterialSelection {
                        name: Some("Quartz".to_string()),
                        specific_gravity_override: None,
                        refractive_index_override: None,
                        body_color_override: None,
                        body_color_bands_override: None,
                        absorption_path_scale_override: None,
                    },
                },
            ]),
        )
        .expect("batch must apply");
    assert_eq!(design.tiers[0].angle_deg, -42.0);
    assert_eq!(design.material.name.as_deref(), Some("Quartz"));
    assert!(history.can_undo());

    // One atomic undo step reverts BOTH sub-edits at once.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(!history.can_undo());

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -42.0);
    assert_eq!(design.material.name.as_deref(), Some("Quartz"));
}

#[test]
fn batch_rejects_a_partially_invalid_batch_without_mutating_the_design() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();

    // The first sub-edit is valid on its own; the second names a tier index the
    // design doesn't have. A naive "apply each in turn against `self`" implementation
    // would leave the first sub-edit's effect in place -- this must not happen.
    let err = design
        .apply_edit(Edit::Batch(vec![
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -42.0)],
            },
            Edit::RemoveTier { index: 5 },
        ]))
        .expect_err("index 5 is out of range");
    assert_eq!(
        err,
        EditError {
            index: 5,
            tier_count: 1
        }
    );
    assert_eq!(
        design, before,
        "a batch that fails partway must not leave the design half-applied"
    );
}

#[test]
fn describe_reads_as_a_cutters_sentence_for_representative_variants() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    design
        .tiers
        .push(tier("C1", 30.0, MeetConstraint::ScaleReference(0.5), &[]));

    assert_eq!(
        Edit::RetargetAngles {
            changes: vec![(0, -40.0, -41.0)],
        }
        .describe(&design),
        // The tier is stored at -41; the sentence reads the plain number.
        "Set P1 angle to 41.0 degrees"
    );
    assert_eq!(
        Edit::RemapIndices {
            from_gear: 96,
            to_gear: 80,
            rounding: RemapRounding::Nearest,
        }
        .describe(&design),
        "Remap gear 96 to 80"
    );
    assert_eq!(
        Edit::RemoveTier { index: 1 }.describe(&design),
        "Remove tier C1"
    );
    let optimize_changes: Vec<(usize, f64, f64)> = (0..12).map(|i| (i, 0.0, 1.0)).collect();
    assert_eq!(
        Edit::RetargetAngles {
            changes: optimize_changes,
        }
        .describe(&design),
        "Optimize 12 tiers"
    );
    assert_eq!(
        Edit::Batch(vec![
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -41.0)],
            },
            Edit::SetMaterial {
                material: MaterialSelection {
                    name: Some("Quartz".to_string()),
                    specific_gravity_override: None,
                    refractive_index_override: None,
                    body_color_override: None,
                    body_color_bands_override: None,
                    absorption_path_scale_override: None,
                },
            },
        ])
        .describe(&design),
        "Retarget for Quartz"
    );
}

/// An old-style tier name (`1`, `A`) reads as the standard code the tier table shows, and a
/// pavilion angle reads as a plain number, in every sentence that names a tier.
#[test]
fn describe_uses_the_standard_code_for_an_old_style_name_and_a_positive_angle() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    design
        .tiers
        .push(tier("C1", 30.0, MeetConstraint::ScaleReference(0.5), &[]));
    let codes = crate::design::compute_tier_labels(&design.tiers);
    let code = codes[0].code.as_str();
    // A pavilion tier named by an old-style digit is a P tier.
    assert!(code.starts_with('P'), "{code}");

    assert_eq!(
        Edit::RetargetAngles {
            changes: vec![(0, -40.0, -41.5)],
        }
        .describe(&design),
        format!("Set {code} angle to 41.5 degrees")
    );
    assert_eq!(
        Edit::RemoveTier { index: 0 }.describe(&design),
        format!("Remove tier {code}")
    );
    // Two changes to the one tier (a nudge plus the tiers that follow) read the same way.
    assert_eq!(
        Edit::Batch(vec![
            Edit::RetargetAngles {
                changes: vec![(0, -40.0, -41.0)],
            },
            Edit::RetargetAngles {
                changes: vec![(0, -41.0, -42.0)],
            },
        ])
        .describe(&design),
        format!("Set {code} angle to 42.0 degrees")
    );
    // A crown tier's angle is unchanged by the rule.
    assert_eq!(
        Edit::RetargetAngles {
            changes: vec![(1, 30.0, 31.0)],
        }
        .describe(&design),
        "Set C1 angle to 31.0 degrees"
    );
}

/// A body-color override is named in the undo label: the preset's own label when
/// the triple matches one, "custom color" otherwise, and nothing extra when unset.
#[test]
fn describe_names_a_body_color_override() {
    let design = fresh_design();
    let yellow = indicatrix::optics::materials::body_color::BODY_COLOR_PRESETS[5];
    let sapphire = MaterialSelection {
        name: Some("Sapphire".to_string()),
        ..MaterialSelection::none()
    };
    let describe = |material: MaterialSelection| Edit::SetMaterial { material }.describe(&design);
    assert_eq!(describe(sapphire.clone()), "Set material to Sapphire");
    assert_eq!(
        describe(
            sapphire
                .clone()
                .with_body_color(Some(yellow.absorption_rgb))
        ),
        "Set material to Sapphire (Yellow)"
    );
    assert_eq!(
        describe(sapphire.with_body_color(Some([0.5, 0.5, 0.5]))),
        "Set material to Sapphire (custom color)"
    );
}
