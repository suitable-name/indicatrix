//! The preform's vertical-span offset surviving a native round trip, and the
//! self-contained (no paired `.asc`) native-only save/load path used for
//! autosave restore.

use super::fixtures::{SIMPLE_ASC, fully_anchored_design, simple_design};
use crate::{
    design::TierTarget,
    native::{
        LoadNativeOnlyError, SaveExtras, load_native_only, load_paired, parse_toml_string,
        save_native_only_toml, save_paired, to_native_file, to_toml_string,
    },
};
use indicatrix::geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions};

/// `Design::preform_y_offset` must round-trip through save/open exactly like
/// `girdle_diameter_mm` already does.
#[test]
fn preform_y_offset_round_trips_through_save_and_load() {
    let mut design = simple_design();
    design.preform_y_offset = 0.35;
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!((saved.native.preform.y_offset - 0.35).abs() < 1e-12);

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert!((loaded.design.preform_y_offset - 0.35).abs() < 1e-12);
}

/// A sidecar saved before `PreformTable::y_offset` existed must load as `0.0` -- the
/// always-centred span every such file's preform actually had.
#[test]
fn a_preform_table_with_no_y_offset_key_loads_as_zero() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let mut text = to_toml_string(&native).expect("must serialize");
    // Simulate a sidecar saved before this field existed, by stripping the
    // (freshly written) key.
    text = text
        .lines()
        .filter(|line| !line.starts_with("y_offset"))
        .collect::<Vec<_>>()
        .join("\n");
    let parsed = parse_toml_string(&text).expect("must still parse without y_offset");
    assert_eq!(parsed.preform.y_offset, 0.0);
}

/// The autosave round trip: `save_native_only` then
/// `load_native_only`, with NO `.asc` text anywhere in the picture, must restore
/// every field the acceptance criterion names -- tiers (angle/indices/constraint/
/// detached), material, girdle diameter, cheater offsets and tier notes -- plus the
/// `.asc`-only header fields (`meta`) an ordinary native file never carries at all.
#[test]
fn save_native_only_then_load_native_only_round_trips_the_full_design() {
    let mut design = simple_design();
    design
        .tier_notes
        .insert(0, "polish this one last".to_string());
    design.cheater_offsets_deg.insert(2, 1.25);
    // `tier_id`/`target` must round-trip through the self-contained (no paired
    // `.asc`) path too, not just `load_paired`'s.
    let tier1_id = design.tier_id_at(1).expect("tier 1 must have an id");
    design
        .tier_targets
        .insert(tier1_id, TierTarget::TableWidthMm(4.1));

    let native_toml = save_native_only_toml(&design, "design.asc", None, &SaveExtras::default())
        .expect("must serialize");

    let loaded = load_native_only(&native_toml).expect("must load with no .asc present");

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

/// `load_native_only` must refuse (not silently guess `gear_teeth`/`symmetry_order`
/// defaults) an ORDINARY paired-mode native file -- one that never went through
/// `save_native_only` -- since such a file's `tiers` carry no `angle_deg`/`indices`
/// at all, and it carries no stashed `meta` either.
///
/// `fully_anchored_design`, not `simple_design`: this test needs a design that
/// actually SOLVES (an ordinary, non-draft save), and `simple_design`'s own
/// tier-1 `MeetExisting` override leaves the Crown block with no
/// `ScaleReference` anchor at all -- `save_paired` on THAT design always falls
/// back to a draft (see the sibling `load_native_only_opens_an_ordinary_draft_save`
/// test), which is a different case entirely.
#[test]
fn load_native_only_refuses_an_ordinary_paired_mode_file() {
    let design = fully_anchored_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!(saved.draft_reason.is_none(), "fixture must actually solve");

    let err = load_native_only(&saved.native_toml)
        .expect_err("an ordinary overlay-only native file has no self-contained meta");
    assert!(matches!(err, LoadNativeOnlyError::NotSelfContained));
}

/// A draft save (`design` does not currently solve) already writes full per-tier
/// `angle_deg`/`indices` via `super::save::draft_tier_tables`, and also stashes `meta` via `stash_schedule_meta` -- exactly
/// like `save_native_only` -- so `load_native_only` can open this same ordinary
/// draft sidecar with no paired `.asc` at all. Without this, a draft `.asc`
/// (itself just a placeholder-mast reconstruction) getting lost or moved away
/// from its sidecar meant the design underneath -- which the sidecar's tier list
/// holds in full -- could not be recovered at all.
#[test]
fn load_native_only_opens_an_ordinary_draft_save() {
    let mut design = simple_design();
    // No `ScaleReference` tier anywhere -> `design.solve()` fails -> draft save.
    for tier in &mut design.tiers {
        tier.constraint = MeetConstraint::MeetExisting;
    }
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must draft-save");
    assert!(saved.draft_reason.is_some());

    let loaded = load_native_only(&saved.native_toml)
        .expect("a draft sidecar now stashes its own schedule meta and opens alone");
    assert_eq!(loaded.design.tiers.len(), design.tiers.len());
    for (loaded_tier, original_tier) in loaded.design.tiers.iter().zip(&design.tiers) {
        assert_eq!(loaded_tier.angle_deg, original_tier.angle_deg);
        assert_eq!(loaded_tier.indices, original_tier.indices);
    }
}

/// `printed_proportions`/history round-trip through the self-contained path exactly
/// like the paired path (Items 172/178) -- same mechanism, same expectation.
#[test]
fn save_native_only_round_trips_printed_proportions_and_history() {
    let design = simple_design();
    let props = ExternalProportions {
        vol_w3: Some(0.5),
        lw: Some(1.0),
        cw: Some(0.55),
        pw: Some(0.43),
        hw: Some(0.6),
    };
    let extras = SaveExtras {
        history_entries: &["Set girdle diameter to 6.50mm".to_string()],
        ..SaveExtras::default()
    };
    let native_toml = save_native_only_toml(&design, "design.asc", Some(&props), &extras)
        .expect("must serialize");

    let loaded = load_native_only(&native_toml).expect("must load");
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
