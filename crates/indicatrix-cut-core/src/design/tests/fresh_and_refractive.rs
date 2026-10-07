//! [`crate::design::FreshDesignSpec`]/[`crate::design::Design::fresh`] compatibility,
//! [`crate::design::Design::effective_refractive_index`], and the `.asc` export
//! tests that depend on it.

use crate::{
    design::{ConstraintTier, Design, FreshDesignSpec},
    edit::{Edit, History},
    material::MaterialSelection,
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

// --- FreshDesignSpec / Design::fresh_from_spec / Design::fresh compatibility ---

/// [`crate::design::Design::fresh_from_spec`] must build exactly the
/// gear/symmetry/mirror/material the spec names, with no tiers yet -- the same
/// "closed on its own" property `fresh_design_alone_is_closed` already checks for
/// [`crate::design::Design::fresh`].
#[test]
fn fresh_from_spec_builds_the_requested_gear_symmetry_mirror_and_material() {
    let spec = FreshDesignSpec {
        gear_teeth: 80,
        symmetry_order: 8,
        mirror: false,
        material: MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        },
        preform: PreformSpec::block(1.0, 1.0, 2.0),
    };
    let design = Design::fresh_from_spec(spec);

    assert_eq!(design.meta.gear_teeth, 80);
    assert_eq!(design.meta.symmetry_order, 8);
    assert!(!design.meta.mirror);
    assert_eq!(design.material.name.as_deref(), Some("Quartz"));
    assert_eq!(design.tiers, [] as [ConstraintTier; 0]);
    assert!(
        design.is_closed(),
        "a fresh design must already be a closed solid"
    );
}

/// [`crate::design::Design::fresh`] is documented as a thin wrapper over
/// [`crate::design::Design::fresh_from_spec`] kept for source compatibility -- this
/// pins its exact behavior: `mirror` always `true`, no material selection, and the
/// passed `refractive_index` landing verbatim on `meta.refractive_index`, not
/// derived from a material.
#[test]
fn fresh_is_a_thin_wrapper_that_matches_its_pre_a2_behavior_exactly() {
    let design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.62);
    assert_eq!(design.meta.gear_teeth, 96);
    assert_eq!(design.meta.symmetry_order, 8);
    assert!(design.meta.mirror);
    assert_eq!(design.meta.refractive_index, 1.62);
    assert_eq!(design.material, MaterialSelection::none());
    assert_eq!(design.tiers, [] as [ConstraintTier; 0]);
}

// --- Design::effective_refractive_index ---

#[test]
fn effective_refractive_index_prefers_the_override_over_everything_else() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.54);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.70),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    assert_eq!(design.effective_refractive_index(), 1.70);
}

#[test]
fn effective_refractive_index_falls_back_to_the_resolved_material_when_there_is_no_override() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.54);
    design.material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let expected = crate::material::built_in_refractive_index("Quartz").unwrap();
    assert!((design.effective_refractive_index() - expected).abs() < 1e-9);
    assert_ne!(
        design.effective_refractive_index(),
        design.meta.refractive_index,
        "the resolved material's own n_D must win over the legacy schedule field"
    );
}

/// With neither an override nor a material name that resolves, the legacy
/// `ScheduleMeta::refractive_index` is what survives -- the untouched-import
/// round-trip case.
#[test]
fn effective_refractive_index_falls_back_to_the_legacy_schedule_value_when_unset() {
    let design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.62);
    assert_eq!(design.material, MaterialSelection::none());
    assert_eq!(design.effective_refractive_index(), 1.62);

    let mut unresolved = design;
    unresolved.material = MaterialSelection {
        name: Some("Not A Real Material".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    assert_eq!(unresolved.effective_refractive_index(), 1.62);
}

/// `to_asc_schedule` must write the EFFECTIVE refractive index, not the raw legacy
/// field, exercised end to end through a real solved export.
#[test]
fn to_asc_schedule_writes_the_effective_refractive_index_not_the_legacy_field() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.54);
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design.material.refractive_index_override = Some(1.90);

    let schedule = design
        .to_asc_schedule()
        .expect("single anchored tier must solve");
    assert_eq!(schedule.refractive_index, 1.90);
    assert_ne!(schedule.refractive_index, design.meta.refractive_index);
}

/// A "golden `.asc` export for a retargeted design": applying a `RetargetAngles`
/// edit and exporting must reflect the NEW angle in the resulting `.asc` schedule
/// for the retargeted tier, while every untouched tier's own export stays
/// byte-identical.
#[test]
fn to_asc_schedule_reflects_a_retargeted_angle() {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 8, 1.54);
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.9),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });

    let before_schedule = design
        .to_asc_schedule()
        .expect("must solve before retarget");

    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::RetargetAngles {
                changes: vec![(1, -40.0, -45.0)],
            },
        )
        .expect("retarget must apply");

    let after_schedule = design.to_asc_schedule().expect("must solve after retarget");
    assert_eq!(after_schedule.tiers[1].angle_deg, -45.0);
    // The untouched table tier's own export must be completely unaffected.
    assert_eq!(after_schedule.tiers[0], before_schedule.tiers[0]);

    // Undo must restore the exact original schedule, angle included.
    assert!(history.undo(&mut design).unwrap());
    let restored_schedule = design.to_asc_schedule().expect("must solve after undo");
    assert_eq!(restored_schedule, before_schedule);
}
