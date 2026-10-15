//! Per-tier and per-design field round-trips through the design file
//! (`design_to_string`/`design_from_str`): the meet-intent fields themselves, tier
//! notes, cheater offsets, and tier ids/targets.

use super::fixtures::{design_text, reopen, simple_design};
use crate::{design::TierTarget, native::design_from_str};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// A design file must reproduce every field the save captured: preform, girdle
/// diameter, material, and every tier's authored constraint/detached set -- not just
/// the ones `.asc` itself would have reconstructed via
/// [`crate::design::Design::from_asc_schedule`]'s own pinning.
#[test]
fn a_design_file_reproduces_every_captured_field() {
    let mut design = simple_design();
    // Tiers 0 and 2 ALSO get an authored constraint that differs from what a
    // plain `.asc` import alone would pin them to (their own recorded masts,
    // 1.0 and 0.55) -- not just tier 1's `MeetExisting` override -- so this test
    // exercises the whole tier list, not one hand-picked tier.
    design.tiers[0].constraint = MeetConstraint::ScaleReference(0.61);
    design.tiers[2].constraint = MeetConstraint::ScaleReference(0.72);

    let loaded = reopen(&design);
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.girdle_diameter_mm, design.girdle_diameter_mm);
    assert_eq!(loaded.design.material, design.material);
    // The WHOLE tier list -- every tier's authored constraint/detached/name/
    // indices, including 0 and 2 above -- must survive intact, not just the one
    // tier a narrower field-by-field check happens to look at.
    assert_eq!(loaded.design.tiers, design.tiers);
}

/// A per-tier cutter-authored note (`Design::tier_notes`) must round-trip on the
/// same tier.
#[test]
fn tier_notes_round_trip_through_save_and_load() {
    let mut design = simple_design();
    design.tier_notes.insert(1, "check meet here".to_string());

    let loaded = reopen(&design);
    assert_eq!(loaded.design.tier_note(0), None);
    assert_eq!(loaded.design.tier_note(1), Some("check meet here"));
    assert_eq!(loaded.design.tier_note(2), None);
}

/// A design with no notes at all must round-trip with an empty `tier_notes` --
/// the "nothing to restore" case `tier_notes_round_trip_through_save_and_load`'s
/// positive case doesn't cover.
#[test]
fn a_design_with_no_notes_round_trips_with_no_notes() {
    let loaded = reopen(&simple_design());
    assert!(loaded.design.tier_notes.is_empty());
}

/// A per-tier cutter-authored cheater/azimuth offset (`Design::cheater_offsets_deg`)
/// must round-trip exactly like `tier_notes` does, on the same tier.
#[test]
fn cheater_offset_round_trips_through_save_and_load() {
    let mut design = simple_design();
    design.cheater_offsets_deg.insert(1, -0.75);

    let loaded = reopen(&design);
    assert_eq!(loaded.design.cheater_offset_deg(0), None);
    assert_eq!(loaded.design.cheater_offset_deg(1), Some(-0.75));
    assert_eq!(loaded.design.cheater_offset_deg(2), None);
}

/// A design with no cheater offsets at all must round-trip with an empty
/// `cheater_offsets_deg` -- the "nothing to restore" case
/// `cheater_offset_round_trips_through_save_and_load`'s positive case doesn't cover.
#[test]
fn a_design_with_no_cheater_offsets_round_trips_with_no_cheater_offsets() {
    let loaded = reopen(&simple_design());
    assert!(loaded.design.cheater_offsets_deg.is_empty());
}

/// `TierId`/`TierTarget` must round-trip exactly like `cheater_offsets_deg` does above.
#[test]
fn tier_id_and_target_round_trip_through_save_and_load() {
    let mut design = simple_design();
    let tier1_id = design.tier_id_at(1).expect("tier 1 must have an id");
    design
        .tier_targets
        .insert(tier1_id, TierTarget::DepthMm(3.2));

    let loaded = reopen(&design);
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

/// A file saved before `TierTable::tier_id` existed (every tier's `tier_id` key
/// stripped, simulating an old file) must still load -- with a genuinely fresh,
/// distinct id assigned to every tier instead of `None`/a panic. See
/// `apply_tier_ids_and_targets`'s own doc comment ("assign a fresh id only when the
/// file has none").
#[test]
fn a_file_with_no_tier_ids_still_loads_and_gets_fresh_distinct_ids() {
    let design = simple_design();
    // Simulate a pre-this-field file by stripping every tier's freshly written
    // `tier_id` key.
    let text = design_text(&design)
        .lines()
        .filter(|line| !line.trim_start().starts_with("tier_id"))
        .collect::<Vec<_>>()
        .join("\n");

    let loaded = design_from_str(&text).expect("must still load without tier_id");
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

/// A design's body-color override (`MaterialSelection::body_color_override`) must
/// round-trip through a real save/load bit for bit, like every other `[material]`
/// field.
#[test]
fn body_color_override_round_trips_through_save_and_load() {
    let mut design = simple_design();
    let yellow = [0.2f32, 0.4, 2.8];
    design.material = design.material.clone().with_body_color(Some(yellow));
    let text = design_text(&design);
    assert!(text.contains("body_color_override"));
    // The file shows the cutter's own short decimals, not `f64::from(0.2f32)`'s
    // `0.20000000298023224` -- see `convert::body_color_to_table`.
    // The TOML writer spreads the array over several lines with a trailing comma;
    // compare with all whitespace removed so only the digits matter.
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("body_color_override=[0.2,0.4,2.8"),
        "expected shortest-decimal components, got:\n{text}"
    );

    let loaded = design_from_str(&text).expect("must load");
    let got = loaded
        .design
        .material
        .body_color_override
        .expect("the override must load back");
    assert_eq!(got.map(f32::to_bits), yellow.map(f32::to_bits));
    assert_eq!(loaded.design.material, design.material);
}

/// A design with no override writes no `body_color_override` key at all, and such
/// a file -- exactly what every file written before the field existed looks like --
/// loads back as `None`.
#[test]
fn a_file_without_a_body_color_override_loads_as_none() {
    let design = simple_design();
    assert_eq!(design.material.body_color_override, None);
    let text = design_text(&design);
    assert!(!text.contains("body_color_override"));

    let loaded = design_from_str(&text).expect("must load");
    assert_eq!(loaded.design.material.body_color_override, None);
}
