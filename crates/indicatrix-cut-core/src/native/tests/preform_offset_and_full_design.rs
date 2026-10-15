//! The preform's vertical-span offset and plane-set shape surviving a design file round
//! trip, and the whole design (tiers, notes, offsets, targets, schedule header) coming
//! back from the one file with no `.asc` anywhere -- what an autosave restore relies on.

use super::fixtures::{reopen, simple_design, unsolved_design};
use crate::{
    design::TierTarget,
    native::{DesignExtras, design_from_str, design_to_string},
    preform::PreformSpec,
};
use indicatrix::geometry::stone_metrics::ExternalProportions;

/// Asserts `loaded` is the same plane-set preform as `original`: every plane bit for bit,
/// the same source note and the same size fields bit for bit.
fn assert_same_plane_set(loaded: &PreformSpec, original: &PreformSpec) {
    assert!(loaded.is_plane_set());
    assert_eq!(loaded.source(), original.source());
    let (got, want) = (loaded.planes(), original.planes());
    assert_eq!(got.len(), want.len());
    for ((gn, gm), (wn, wm)) in got.iter().zip(&want) {
        assert_eq!(
            [gn.x.to_bits(), gn.y.to_bits(), gn.z.to_bits(), gm.to_bits()],
            [wn.x.to_bits(), wn.y.to_bits(), wn.z.to_bits(), wm.to_bits()]
        );
    }
    assert_eq!(loaded.half_width.to_bits(), original.half_width.to_bits());
    assert_eq!(
        loaded.length_over_width.to_bits(),
        original.length_over_width.to_bits()
    );
    assert_eq!(loaded.depth.to_bits(), original.depth.to_bits());
}

/// A design whose preform is a plane set (the Rough Planner's piece of rough) must come
/// back from the design file with the same planes, the same source note and the same
/// size fields.
#[test]
fn a_plane_set_preform_round_trips_through_the_design_file() {
    let mut design = simple_design();
    design.preform = PreformSpec::from_planes(
        PreformSpec::block(2.0, 1.0, 2.0).planes(),
        "plan 'Aqua'".to_string(),
    )
    .expect("a block's planes close");
    design.preform_y_offset = 0.25;

    let loaded = reopen(&design);
    assert_same_plane_set(&loaded.design.preform, &design.preform);
    assert!((loaded.design.preform_y_offset - 0.25).abs() < 1e-12);
    assert_eq!(loaded.design.preform, design.preform);
}

/// `Design::preform_y_offset` must round-trip through save/open exactly like
/// `girdle_diameter_mm` already does.
#[test]
fn preform_y_offset_round_trips_through_save_and_load() {
    let mut design = simple_design();
    design.preform_y_offset = 0.35;
    let loaded = reopen(&design);
    assert!((loaded.design.preform_y_offset - 0.35).abs() < 1e-12);
}

/// The whole-design round trip: the one design file, with NO `.asc` text anywhere in
/// the picture, must restore every field -- tiers (angle/indices/constraint/detached),
/// material, girdle diameter, cheater offsets and tier notes -- plus the `.asc`-only
/// header fields (`meta`) a catalogue `.asc` carries.
#[test]
fn the_design_file_round_trips_the_full_design_with_no_asc() {
    let mut design = simple_design();
    design
        .tier_notes
        .insert(0, "polish this one last".to_string());
    design.cheater_offsets_deg.insert(2, 1.25);
    let tier1_id = design.tier_id_at(1).expect("tier 1 must have an id");
    design
        .tier_targets
        .insert(tier1_id, TierTarget::TableWidthMm(4.1));

    let loaded = reopen(&design);

    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.meta, design.meta);
    assert_eq!(loaded.design.tiers.len(), design.tiers.len());
    for (loaded_tier, original_tier) in loaded.design.tiers.iter().zip(&design.tiers) {
        assert_eq!(loaded_tier.angle_deg, original_tier.angle_deg);
        assert_eq!(loaded_tier.indices, original_tier.indices);
        assert_eq!(loaded_tier.constraint, original_tier.constraint);
        assert_eq!(loaded_tier.detached, original_tier.detached);
    }
    assert_eq!(loaded.design.girdle_diameter_mm, design.girdle_diameter_mm);
    assert_eq!(loaded.design.material, design.material);
    assert_eq!(loaded.design.tier_notes, design.tier_notes);
    assert_eq!(
        loaded.design.cheater_offsets_deg,
        design.cheater_offsets_deg
    );
    for index in 0..design.tiers.len() {
        assert_eq!(loaded.design.tier_id_at(index), design.tier_id_at(index));
    }
    assert_eq!(
        loaded.design.tier_target(1),
        Some(TierTarget::TableWidthMm(4.1))
    );
}

/// A design that does not solve (no scale-reference anchor anywhere) still round-trips
/// every tier: nothing about the file depends on the design solving.
#[test]
fn a_design_that_does_not_solve_round_trips_every_tier() {
    let mut design = simple_design();
    // No `ScaleReference` tier anywhere -> `design.solve()` fails.
    for tier in &mut design.tiers {
        tier.constraint = indicatrix::geometry::meet_solver::MeetConstraint::MeetExisting;
    }
    assert!(design.solve().is_err());

    let loaded = reopen(&design);
    assert_eq!(loaded.design.tiers.len(), design.tiers.len());
    for (loaded_tier, original_tier) in loaded.design.tiers.iter().zip(&design.tiers) {
        assert_eq!(loaded_tier.angle_deg, original_tier.angle_deg);
        assert_eq!(loaded_tier.indices, original_tier.indices);
    }
    assert_eq!(reopen(&unsolved_design()).design, unsolved_design());
}

/// `printed_proportions`/history round-trip through the one file.
#[test]
fn printed_proportions_and_history_round_trip_through_the_design_file() {
    let design = simple_design();
    let props = ExternalProportions {
        vol_w3: Some(0.5),
        lw: Some(1.0),
        cw: Some(0.55),
        pw: Some(0.43),
        hw: Some(0.6),
    };
    let history = ["Set girdle diameter to 6.50mm".to_string()];
    let extras = DesignExtras {
        history_entries: &history,
        ..DesignExtras::default()
    };
    let text = design_to_string(&design, Some(&props), &extras).expect("must serialize");

    let loaded = design_from_str(&text).expect("must load");
    // `ExternalProportions` derives no `PartialEq` (see
    // `printed_proportions_round_trip_through_save_and_open`'s own comment) --
    // each field is checked individually.
    let restored = loaded
        .printed_proportions
        .expect("printed proportions must survive the round trip");
    assert_eq!(restored.vol_w3, props.vol_w3);
    assert_eq!(restored.lw, props.lw);
    assert_eq!(restored.cw, props.cw);
    assert_eq!(restored.pw, props.pw);
    assert_eq!(restored.hw, props.hw);
    assert_eq!(
        loaded.history_entries,
        vec!["Set girdle diameter to 6.50mm".to_string()]
    );
}
