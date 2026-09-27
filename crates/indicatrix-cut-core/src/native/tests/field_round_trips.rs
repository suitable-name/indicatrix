//! Per-tier and per-design field round-trips through `to_native_file`/
//! `load_paired` on a clean (fingerprint-matching) pair: the meet-intent
//! overlay itself, tier notes, cheater offsets, and tier ids/targets.

use super::fixtures::{SIMPLE_ASC, simple_design};
use crate::{
    design::TierTarget,
    native::{
        FingerprintCheck, SaveExtras, TierOverlay, load_paired, to_native_file, to_toml_string,
    },
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// [`crate::native::load_paired`] with a matching fingerprint must reproduce every
/// field [`crate::native::to_native_file`] captured: preform, girdle diameter,
/// material, and every tier's authored constraint/detached set -- not just the ones
/// `.asc` itself would have reconstructed via
/// [`crate::design::Design::from_asc_schedule`]'s own pinning.
#[test]
fn load_paired_reproduces_every_captured_field_on_a_clean_pair() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.fingerprint, FingerprintCheck::Match);
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.girdle_diameter_mm, design.girdle_diameter_mm);
    assert_eq!(loaded.design.material, design.material);
    // Tier 1 was deliberately set to `MeetExisting`, not what `.asc` import alone
    // would pin it to (`ScaleReference`) -- the gap this module exists to close.
    assert_eq!(
        loaded.design.tiers[1].constraint,
        MeetConstraint::MeetExisting
    );
    assert_eq!(loaded.design.tiers[1].detached, vec![0.0, 2.0]);
}

/// A per-tier cutter-authored note (`Design::tier_notes`) must round-trip
/// through `to_native_file`/`load_paired` exactly like the constraint/detached
/// overlay does, on the same tier, by the same array position.
#[test]
fn tier_notes_round_trip_through_save_and_load() {
    let mut design = simple_design();
    design.tier_notes.insert(1, "check meet here".to_string());
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    assert_eq!(loaded.design.tier_note(0), None);
    assert_eq!(loaded.design.tier_note(1), Some("check meet here"));
    assert_eq!(loaded.design.tier_note(2), None);
}

/// A design with no notes at all must round-trip with an empty `tier_notes` --
/// the "nothing to restore" case `tier_notes_round_trip_through_save_and_load`'s
/// positive case doesn't cover.
#[test]
fn a_design_with_no_notes_round_trips_with_no_notes() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert!(loaded.design.tier_notes.is_empty());
}

/// A per-tier cutter-authored cheater/azimuth offset (`Design::cheater_offsets_deg`)
/// must round-trip through `to_native_file`/`load_paired` exactly like `tier_notes`
/// does, on the same tier, by the same array position.
#[test]
fn cheater_offset_round_trips_through_save_and_load() {
    let mut design = simple_design();
    design.cheater_offsets_deg.insert(1, -0.75);
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    assert_eq!(loaded.design.cheater_offset_deg(0), None);
    assert_eq!(loaded.design.cheater_offset_deg(1), Some(-0.75));
    assert_eq!(loaded.design.cheater_offset_deg(2), None);
}

/// A design with no cheater offsets at all must round-trip with an empty
/// `cheater_offsets_deg` -- the "nothing to restore" case
/// `cheater_offset_round_trips_through_save_and_load`'s positive case doesn't cover.
#[test]
fn a_design_with_no_cheater_offsets_round_trips_with_no_cheater_offsets() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert!(loaded.design.cheater_offsets_deg.is_empty());
}

/// `TierId`/`TierTarget` must round-trip through `to_native_file`/`load_paired`
/// exactly like `cheater_offsets_deg` does above.
#[test]
fn tier_id_and_target_round_trip_through_save_and_load() {
    let mut design = simple_design();
    let tier1_id = design.tier_id_at(1).expect("tier 1 must have an id");
    design
        .tier_targets
        .insert(tier1_id, TierTarget::DepthMm(3.2));

    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    // The targeted tier's own id and target both survive...
    assert_eq!(loaded.design.tier_id_at(1), Some(tier1_id));
    assert_eq!(loaded.design.tier_target(1), Some(TierTarget::DepthMm(3.2)));
    // ...every OTHER tier's id survives too, and neither gained a target it
    // never had.
    assert_eq!(loaded.design.tier_id_at(0), design.tier_id_at(0));
    assert_eq!(loaded.design.tier_id_at(2), design.tier_id_at(2));
    assert_eq!(loaded.design.tier_target(0), None);
    assert_eq!(loaded.design.tier_target(2), None);
}

/// A sidecar saved before `TierTable::tier_id` existed (every tier's `tier_id`
/// key stripped, simulating an old file) must still load -- with a genuinely
/// fresh, distinct id assigned to every tier instead of `None`/a panic. See
/// `apply_tier_ids_and_targets`'s own doc comment ("assign a fresh id only
/// when the file has none").
#[test]
fn a_sidecar_with_no_tier_ids_still_loads_and_gets_fresh_distinct_ids() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let mut text = to_toml_string(&native).expect("must serialize");
    // Simulate a pre-this-field sidecar by stripping every tier's freshly
    // written `tier_id` key.
    text = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("tier_id"))
        .collect::<Vec<_>>()
        .join("\n");

    let loaded = load_paired(SIMPLE_ASC, &text, false).expect("must still load without tier_id");
    let mut ids = Vec::new();
    for index in 0..loaded.design.tiers.len() {
        let id = loaded
            .design
            .tier_id_at(index)
            .unwrap_or_else(|| panic!("tier {index} must still get a fresh id"));
        ids.push(id);
    }
    let before = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), before, "every fresh id must be distinct");
}
