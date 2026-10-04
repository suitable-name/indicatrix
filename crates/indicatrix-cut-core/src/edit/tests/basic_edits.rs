//! Basic per-field edit/undo round-trips: `AddTier`, `RemoveTier`,
//! `ModifyTier`, `SetConstraint`, `SetPreform`, `SetGirdleDiameterMm`,
//! `SetMaterial`, `SetMeta`.

use super::fixtures::{fresh_design, tier};
use crate::{
    edit::{Edit, History},
    material::MaterialSelection,
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

#[test]
fn add_tier_then_undo_round_trips() {
    let mut design = fresh_design();
    let mut history = History::new();
    let before = design.clone();

    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier("T", 0.0, MeetConstraint::ScaleReference(0.32), &[]),
            },
        )
        .expect("add must apply");
    assert_eq!(design.tiers.len(), 1);
    assert!(history.can_undo());
    assert!(!history.can_redo());

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(!history.can_undo());
    assert!(history.can_redo());

    assert!(history.redo(&mut design).unwrap());
    assert_eq!(design.tiers.len(), 1);
    assert!(!history.can_redo());
}

#[test]
fn remove_tier_then_undo_restores_it_at_the_same_position() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("A", 10.0, MeetConstraint::ScaleReference(0.5), &[1.0]));
    design
        .tiers
        .push(tier("B", 20.0, MeetConstraint::MeetExisting, &[2.0]));
    design
        .tiers
        .push(tier("C", 30.0, MeetConstraint::MeetExisting, &[3.0]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("remove must apply");
    assert_eq!(
        design
            .tiers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "C"]
    );

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(
        design
            .tiers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B", "C"]
    );
}

#[test]
fn modify_tier_then_undo_restores_the_old_values() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("T", 0.0, MeetConstraint::ScaleReference(0.30), &[]));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("T", 0.0, MeetConstraint::ScaleReference(0.45), &[]),
            },
        )
        .expect("modify must apply");
    assert_eq!(
        design.tiers[0].constraint,
        MeetConstraint::ScaleReference(0.45)
    );

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(
        design.tiers[0].constraint,
        MeetConstraint::ScaleReference(0.30)
    );
}

/// `SetConstraint` changes only the constraint, leaving angle/name/indices
/// alone -- the property that distinguishes it from `ModifyTier`.
#[test]
fn set_constraint_then_undo_touches_only_the_constraint_field() {
    let mut design = fresh_design();
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0],
    ));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetConstraint {
                index: 0,
                constraint: MeetConstraint::MeetNamed(vec!["G1".to_string()]),
            },
        )
        .expect("set constraint must apply");
    assert_eq!(
        design.tiers[0].constraint,
        MeetConstraint::MeetNamed(vec!["G1".to_string()])
    );
    assert_eq!(design.tiers[0].angle_deg, -41.0);
    assert_eq!(design.tiers[0].name, "P1");
    assert_eq!(design.tiers[0].indices, vec![0.0, 24.0]);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

#[test]
fn set_preform_then_undo_restores_the_old_preform() {
    let mut design = fresh_design();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetPreform {
                preform: PreformSpec::cylinder(96, 1.2, 1.0, 0.9),
            },
        )
        .expect("set preform must apply");
    assert_ne!(design.preform, before.preform);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

/// `SetGirdleDiameterMm` then undo must restore the previous scale
/// anchor (`None`, for a fresh design) exactly, the same round-trip
/// `set_preform_then_undo_restores_the_old_preform` above already checks for
/// `SetPreform`.
#[test]
fn set_girdle_diameter_mm_then_undo_restores_the_old_value() {
    let mut design = fresh_design();
    assert_eq!(design.girdle_diameter_mm, None);
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(8.2),
            },
        )
        .expect("set girdle diameter must apply");
    assert_eq!(design.girdle_diameter_mm, Some(8.2));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert_eq!(design.girdle_diameter_mm, None);
}

/// `SetMaterial` then undo must restore the previous material
/// selection wholesale.
#[test]
fn set_material_then_undo_restores_the_old_selection() {
    let mut design = fresh_design();
    assert_eq!(design.material, MaterialSelection::none());
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetMaterial {
                material: MaterialSelection {
                    name: Some("Diamond".to_string()),
                    specific_gravity_override: None,
                    refractive_index_override: None,
                    body_color_override: None,
                },
            },
        )
        .expect("set material must apply");
    assert_eq!(design.material.name.as_deref(), Some("Diamond"));

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}

/// `SetMeta` then undo must restore the previous headers/footnotes/gear
/// reference angle wholesale, and applying it must not force a re-solve --
/// none of those three fields feed `solve_meet_points`.
#[test]
fn set_meta_then_undo_restores_the_old_value() {
    let mut design = fresh_design();
    assert_eq!(design.meta.headers, Vec::<String>::new());
    assert_eq!(design.meta.footnotes, Vec::<String>::new());
    assert_eq!(design.meta.gear_reference_angle, 0.0);
    let before = design.clone();
    let before_solved = design.solve().expect("fresh design must solve");
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetMeta {
                headers: vec!["My Design".to_string()],
                footnotes: vec!["Cut for a client".to_string()],
                gear_reference_angle: 1.5,
            },
        )
        .expect("set meta must apply");
    assert_eq!(design.meta.headers, vec!["My Design".to_string()]);
    assert_eq!(design.meta.footnotes, vec!["Cut for a client".to_string()]);
    assert_eq!(design.meta.gear_reference_angle, 1.5);
    // Purely metadata: the solved masts are untouched by the edit.
    let after_solved = design.solve().expect("design must still solve");
    let before_masts: Vec<f64> = before_solved.iter().map(|t| t.mast).collect();
    let after_masts: Vec<f64> = after_solved.iter().map(|t| t.mast).collect();
    assert_eq!(before_masts, after_masts);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
}
