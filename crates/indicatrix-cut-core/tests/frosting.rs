//! Frosted tiers and facets through the public API of `indicatrix-cut-core`: the undoable edit,
//! the native `.isdd` round trip, the `.gcs` import and writer, the planes the renderer is told
//! to frost and the cutting sheet. The oracle for "a design without frosting changes nothing" is
//! byte equality with what the same design produced before frosting existed: an empty frosting
//! map must add no key, no plane, no attribute and no word.

use indicatrix_cut_core::{
    CutSheetRow, Design, Edit, History, MaterialSelection, PreformSpec, RemapRounding,
    ScheduleState, TierFrosting,
    native::{DesignExtras, design_from_str, design_to_string},
};
use indicatrix_formats::asc::AscSchedule;
use indicatrix_formats::gcs::{
    GcsTierFrosting, gcs_frosting, gcs_to_asc_schedule, parse_gcs, to_gcs_string,
    to_gcs_string_with_frosting,
};

/// A five-tier round brilliant on a 16-tooth gear: girdle, two pavilion tiers, two crown tiers,
/// four facets each. Tier 1 is `-42` degrees at `0 4 8 12`, tier 2 is `-38` at `2 6 10 14`,
/// tier 3 is `32` at `0 4 8 12`, tier 4 is `40` at `2 6 10 14`.
fn fixture() -> Design {
    const ASC: &str = "GemCad 5.0\n\
         g 16 0.0\n\
         y 4 n\n\
         I 1.54\n\
         a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
         a -42.000000 0.60000000 0 4 8 12 P1 Set stone size\n\
         a -38.000000 0.55000000 2 6 10 14 P2 Set stone size\n\
         a 32.000000 0.45000000 0 4 8 12 C1 Set stone size\n\
         a 40.000000 0.40000000 2 6 10 14 C2 Set stone size\n";
    let schedule = indicatrix_formats::asc::parse_asc(ASC).expect("fixture .asc must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.54),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design.ensure_tier_ids();
    design
}

fn set(design: &mut Design, index: usize, frosting: Option<TierFrosting>) -> Edit {
    design
        .apply_edit(Edit::SetTierFrosting { index, frosting })
        .expect("the tier exists")
}

fn native_text(design: &Design) -> String {
    design_to_string(design, None, &DesignExtras::default()).expect("the design saves")
}

fn version_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| line.starts_with("version"))
        .collect()
}

// ---- the edit ----

#[test]
fn setting_tier_frosting_is_one_undo_step_and_redoes() {
    let mut design = fixture();
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(
            &mut design,
            Edit::SetTierFrosting {
                index: 2,
                frosting: Some(TierFrosting::Tier),
            },
        )
        .expect("applies");
    assert!(design.tier_is_frosted(2));
    assert!(!design.tier_is_frosted(1));
    assert!(design.has_frosting());
    let after = design.clone();

    assert!(history.undo(&mut design).expect("undoes"));
    assert_eq!(design, before);
    assert!(!design.has_frosting());

    assert!(history.redo(&mut design).expect("redoes"));
    assert_eq!(design, after);
}

#[test]
fn facet_frosting_is_normalised_and_clearing_it_is_the_inverse() {
    let mut design = fixture();
    let before = design.clone();
    // Out-of-order positions come back ascending; the inverse of the set is the clear.
    let inverse = set(
        &mut design,
        1,
        Some(TierFrosting::Facets(vec![8.0, 0.0, 8.0])),
    );
    assert_eq!(
        design.tier_frosting(1),
        Some(&TierFrosting::Facets(vec![0.0, 8.0]))
    );
    design.apply_edit(inverse).expect("the inverse applies");
    assert_eq!(design, before);
    // A frosting of no facet is a clear.
    set(&mut design, 1, Some(TierFrosting::Facets(Vec::new())));
    assert_eq!(design.tier_frosting(1), None);
    assert!(!design.has_frosting());
}

#[test]
fn an_unknown_tier_is_an_error_and_changes_nothing() {
    let mut design = fixture();
    let before = design.clone();
    let result = design.apply_edit(Edit::SetTierFrosting {
        index: 99,
        frosting: Some(TierFrosting::Tier),
    });
    assert!(result.is_err());
    assert_eq!(design, before);
}

#[test]
fn removing_a_frosted_tier_and_undoing_restores_its_frosting() {
    let mut design = fixture();
    set(&mut design, 2, Some(TierFrosting::Facets(vec![2.0, 6.0])));
    set(&mut design, 3, Some(TierFrosting::Tier));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::RemoveTier { index: 2 })
        .expect("removes");
    // The removed tier's frosting goes with it; the one that was tier 3 is tier 2 now and keeps
    // its own (the map is keyed by the stable tier id, not by position).
    assert_eq!(design.tiers.len(), before.tiers.len() - 1);
    assert_eq!(design.tier_frosting(2), Some(&TierFrosting::Tier));
    assert_eq!(design.tier_frosting.len(), 1);

    assert!(history.undo(&mut design).expect("undoes"));
    assert_eq!(design, before);
    assert_eq!(
        design.tier_frosting(2),
        Some(&TierFrosting::Facets(vec![2.0, 6.0]))
    );
}

#[test]
fn moving_a_tier_keeps_its_frosting_with_it() {
    let mut design = fixture();
    set(&mut design, 1, Some(TierFrosting::Tier));
    let id = design.tier_ids[1];
    design
        .apply_edit(Edit::MoveTier { from: 1, to: 3 })
        .expect("moves");
    let at = design.tier_ids.iter().position(|&t| t == id).expect("kept");
    assert_eq!(at, 3);
    assert_eq!(design.tier_frosting(at), Some(&TierFrosting::Tier));
    assert_eq!(design.tier_frosting.len(), 1);
}

#[test]
fn remapping_the_gear_remaps_the_frosted_facet_positions_and_undo_restores_them() {
    let mut design = fixture();
    set(&mut design, 2, Some(TierFrosting::Facets(vec![2.0, 6.0])));
    set(&mut design, 1, Some(TierFrosting::Tier));
    let before = design.clone();
    let inverse = design
        .apply_edit(Edit::RemapIndices {
            from_gear: 16,
            to_gear: 32,
            rounding: RemapRounding::Nearest,
        })
        .expect("a remap never fails");
    assert_eq!(
        design.tier_frosting(2),
        Some(&TierFrosting::Facets(vec![4.0, 12.0])),
        "the facet positions double with the gear"
    );
    assert_eq!(design.tier_frosting(1), Some(&TierFrosting::Tier));
    // The remapped positions are the tier's own remapped indices.
    for &position in &[4.0, 12.0] {
        assert!(design.tiers[2].indices.contains(&position));
    }
    design.apply_edit(inverse).expect("the inverse applies");
    assert_eq!(design, before);
}

#[test]
fn replacing_the_schedule_keeps_the_frosting_of_surviving_tiers_and_undo_restores_all() {
    let mut design = fixture();
    set(&mut design, 1, Some(TierFrosting::Tier));
    set(&mut design, 3, Some(TierFrosting::Facets(vec![4.0])));
    let before = design.clone();

    let mut state = ScheduleState::of(&design);
    assert_eq!(
        state.tier_frosting.len(),
        2,
        "the state carries the frosting"
    );
    // Drop tier 1 (frosted) and keep tier 3 (frosted).
    state.tiers.remove(1);
    let dropped = state.tier_ids.remove(1);
    state.tier_frosting.remove(&dropped);
    let mut history = History::new();
    history
        .apply(&mut design, Edit::ReplaceSchedule(Box::new(state)))
        .expect("the state applies");
    assert_eq!(design.tiers.len(), before.tiers.len() - 1);
    assert_eq!(design.tier_frosting.len(), 1);
    assert_eq!(
        design.tier_frosting(2),
        Some(&TierFrosting::Facets(vec![4.0]))
    );

    assert!(history.undo(&mut design).expect("undoes"));
    assert_eq!(design, before);
}

#[test]
fn toggling_one_facet_frosts_it_and_a_fully_frosted_tier_becomes_whole() {
    let mut design = fixture();
    // Tier 3's facets are 0 4 8 12.
    let first = design.frosting_with_facet_toggled(3, 4.0);
    assert_eq!(first, Some(TierFrosting::Facets(vec![4.0])));
    set(&mut design, 3, first);
    assert_eq!(design.tier_frosting_text(3), "Frosted: 1 of 4");
    for position in [0.0, 8.0, 12.0] {
        let next = design.frosting_with_facet_toggled(3, position);
        set(&mut design, 3, next);
    }
    assert_eq!(design.tier_frosting(3), Some(&TierFrosting::Tier));
    assert_eq!(design.tier_frosting_text(3), "Frosted");
    // Toggling one facet of a whole frosted tier leaves the other three.
    let partly = design.frosting_with_facet_toggled(3, 4.0);
    assert_eq!(partly, Some(TierFrosting::Facets(vec![0.0, 8.0, 12.0])));
    // Toggling the last frosted facet clears the tier; a position that is not a facet changes
    // nothing.
    set(&mut design, 3, Some(TierFrosting::Facets(vec![4.0])));
    assert_eq!(design.frosting_with_facet_toggled(3, 4.0), None);
    assert_eq!(
        design.frosting_with_facet_toggled(3, 5.0),
        Some(TierFrosting::Facets(vec![4.0]))
    );
    assert_eq!(design.tier_frosting_text(0), "");
}

// ---- the native file ----

#[test]
fn a_design_without_frosting_writes_no_frosting_key_and_the_same_version() {
    let design = fixture();
    let text = native_text(&design);
    assert!(!text.contains("frost"), "no key for a polished design");
    // Byte-identical across a round trip.
    let again = design_from_str(&text).expect("loads").design;
    assert_eq!(native_text(&again), text);
}

#[test]
fn frosting_round_trips_through_the_native_file_without_a_version_bump() {
    let plain = native_text(&fixture());
    let mut design = fixture();
    set(&mut design, 1, Some(TierFrosting::Tier));
    set(&mut design, 4, Some(TierFrosting::Facets(vec![2.0, 10.0])));
    let text = native_text(&design);
    assert!(text.contains("frosted = true"));
    assert!(text.contains("frosted_facets"));
    assert_eq!(
        version_lines(&text),
        version_lines(&plain),
        "an old reader keeps the keys, so the version stays"
    );
    assert_ne!(version_lines(&text).len(), 0);

    let loaded = design_from_str(&text).expect("loads").design;
    assert_eq!(loaded.tier_frosting(1), Some(&TierFrosting::Tier));
    assert_eq!(
        loaded.tier_frosting(4),
        Some(&TierFrosting::Facets(vec![2.0, 10.0]))
    );
    assert_eq!(loaded.tier_frosting.len(), 2);
    assert_eq!(native_text(&loaded), text, "saving again is byte-identical");
}

#[test]
fn a_file_from_before_frosting_reads_unfrosted() {
    let mut design = fixture();
    set(&mut design, 1, Some(TierFrosting::Tier));
    set(&mut design, 4, Some(TierFrosting::Facets(vec![2.0])));
    let text = native_text(&design);
    // What a build that predates the keys wrote: the same file without them.
    let old: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("frosted"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!old.contains("frost"));
    let loaded = design_from_str(&old).expect("an old file loads").design;
    assert!(!loaded.has_frosting());
    assert!(loaded.tier_frosting.is_empty());
    assert_eq!(loaded.tiers.len(), design.tiers.len());
}

// ---- the planes the renderer frosts ----

#[test]
fn a_scene_marks_exactly_the_planes_of_the_frosted_tier() {
    let mut design = fixture();
    let solved = design.solve().expect("the fixture solves");
    assert!(
        design.frosted_planes_from_solved(&solved).is_empty(),
        "a polished design marks nothing"
    );
    let preform_len = design.preform.planes().len();
    let planes = design.planes_from_solved(&solved);

    set(&mut design, 1, Some(TierFrosting::Tier));
    let tier1 = design.frosted_planes_from_solved(&solved);
    assert_eq!(tier1.len(), 4, "four facets, four planes");
    assert!(tier1.windows(2).all(|w| w[0] < w[1]), "ascending");
    assert!(
        tier1.iter().all(|&i| i as usize >= preform_len),
        "the preform's own planes are never frosted"
    );
    // The stone's axis is `+Y` (`StandardGemCuts::from_asc_schedule` builds the normal as
    // `(sin t cos p, +-cos t, sin t sin p)` for tier angle `t` and facet azimuth `p`): the height
    // of a tier is the normal's `y`, and the azimuth only turns the horizontal part. `z` is
    // `sin t sin p`, which differs from facet to facet of one tier (0 at `p = 0` and `p = 180`
    // degrees, `+-sin t` at `p = 90` and `270`), so it is not a tier's height.
    let abs_y = |i: u32| planes[i as usize].0.y.abs();
    let y1 = abs_y(tier1[0]);
    assert!(tier1.iter().all(|&i| (abs_y(i) - y1).abs() < 1e-9));
    // And they are the planes of THE frosted tier, the `-42` degree pavilion one: its `y` is
    // `-cos 42`, its horizontal length `sin 42` (the planes are `f32` data, so to `f32` precision).
    let t1 = 42.0_f64.to_radians();
    assert!(
        tier1.iter().all(|&i| {
            let normal = planes[i as usize].0;
            (normal.y + t1.cos()).abs() < 1e-6 && (normal.x.hypot(normal.z) - t1.sin()).abs() < 1e-6
        }),
        "the frosted planes are tier 1's own (`-42` degrees)"
    );

    // A different tier marks a disjoint set, with a different angle.
    set(&mut design, 1, None);
    set(&mut design, 3, Some(TierFrosting::Tier));
    let tier3 = design.frosted_planes_from_solved(&solved);
    assert_eq!(tier3.len(), 4);
    assert!(tier3.iter().all(|i| !tier1.contains(i)));
    assert!((abs_y(tier3[0]) - y1).abs() > 1e-3);
    let t3 = 32.0_f64.to_radians();
    assert!(
        tier3.iter().all(|&i| {
            let normal = planes[i as usize].0;
            (normal.y - t3.cos()).abs() < 1e-6 && (normal.x.hypot(normal.z) - t3.sin()).abs() < 1e-6
        }),
        "the frosted planes are tier 3's own (`32` degrees)"
    );

    // Both together are exactly the union.
    set(&mut design, 1, Some(TierFrosting::Tier));
    let mut union = tier1.clone();
    union.extend(&tier3);
    union.sort_unstable();
    assert_eq!(design.frosted_planes_from_solved(&solved), union);

    // One facet of a tier marks one plane of that tier.
    set(&mut design, 3, None);
    set(&mut design, 1, Some(TierFrosting::Facets(vec![4.0])));
    let one = design.frosted_planes_from_solved(&solved);
    assert_eq!(one.len(), 1);
    assert!(tier1.contains(&one[0]));
}

#[test]
fn a_partial_arrangement_marks_the_same_planes_by_value() {
    let mut design = fixture();
    set(&mut design, 3, Some(TierFrosting::Tier));
    let solved = design.solve().expect("solves");
    let planes = design.planes_from_solved(&solved);
    let whole = design.frosted_planes_from_solved(&solved);
    assert_eq!(design.frosted_planes_in(&solved, &planes), whole);

    // The same planes in reverse order: each frosted plane moves to its mirrored slot.
    let reversed: Vec<_> = planes.iter().rev().copied().collect();
    let mut expected: Vec<u32> = whole
        .iter()
        .map(|&i| u32::try_from(planes.len() - 1 - i as usize).expect("fits"))
        .collect();
    expected.sort_unstable();
    assert_eq!(design.frosted_planes_in(&solved, &reversed), expected);

    // A polished design marks nothing, whatever the arrangement.
    let polished = fixture();
    assert_eq!(polished.frosted_planes_in(&solved, &planes).len(), 0);
}

// ---- the cutting sheet ----

#[test]
fn the_cutting_sheet_says_frosted_and_only_for_frosted_tiers() {
    let mut design = fixture();
    let solved = design.solve().expect("solves");
    let plain = design.cutting_sheet(&solved);
    assert!(plain.rows.iter().all(|row| row.frosting.is_none()));
    assert!(!plain.to_text().contains("Frosted"));

    set(&mut design, 1, Some(TierFrosting::Tier));
    set(&mut design, 3, Some(TierFrosting::Facets(vec![4.0, 12.0])));
    let sheet = design.cutting_sheet(&solved);
    let frosted: Vec<_> = sheet.rows.iter().filter(|r| r.frosting.is_some()).collect();
    assert_eq!(frosted.len(), 2);
    let whole = frosted
        .iter()
        .find(|row| row.frosting == Some(TierFrosting::Tier))
        .expect("the whole frosted tier has a row");
    assert!(
        whole.instruction().ends_with("(Frosted)"),
        "{}",
        whole.instruction()
    );
    let partly = frosted
        .iter()
        .find(|row| matches!(row.frosting, Some(TierFrosting::Facets(_))))
        .expect("the partly frosted tier has a row");
    assert!(
        partly.instruction().contains("(Frosted facets: "),
        "{}",
        partly.instruction()
    );
    let text = sheet.to_text();
    assert!(text.contains("(Frosted)"));
    assert!(text.contains("(Frosted facets: "));
    // Polished rows read exactly as they did.
    let polished_text: Vec<String> = plain.rows.iter().map(CutSheetRow::instruction).collect();
    for (row, before) in sheet.rows.iter().zip(&polished_text) {
        if row.frosting.is_none() {
            assert_eq!(&row.instruction(), before);
        }
    }
}

// ---- `.gcs` ----

#[test]
fn gcs_import_maps_tier_and_facet_frosting_into_the_design() {
    let schedule: AscSchedule = fixture().to_asc_schedule().expect("exports");
    let frosting = vec![
        (1, GcsTierFrosting::Whole),
        (3, GcsTierFrosting::Facets(vec![1, 3])),
    ];
    let text = to_gcs_string_with_frosting(&schedule, &frosting).expect("writes");
    let parsed = parse_gcs(&text).expect("parses");
    let stated = gcs_frosting(&parsed);
    assert_eq!(stated, frosting);

    let converted = gcs_to_asc_schedule(&parsed);
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &converted);
    design.apply_imported_frosting(&stated);
    assert_eq!(design.tier_frosting(1), Some(&TierFrosting::Tier));
    let slots: Vec<f64> = [1usize, 3]
        .iter()
        .map(|&slot| design.tiers[3].indices[slot])
        .collect();
    assert_eq!(design.tier_frosting(3), Some(&TierFrosting::Facets(slots)));
    assert_eq!(design.tier_frosting.len(), 2);
}

#[test]
fn the_gcs_writer_keeps_the_designs_frosting_through_a_round_trip() {
    let mut design = fixture();
    set(&mut design, 2, Some(TierFrosting::Tier));
    set(&mut design, 4, Some(TierFrosting::Facets(vec![6.0])));
    let schedule = design.to_asc_schedule().expect("exports");
    let spec = design.gcs_frosting_spec();
    assert_eq!(spec.len(), 2);
    let text = to_gcs_string_with_frosting(&schedule, &spec).expect("writes");

    let parsed = parse_gcs(&text).expect("parses");
    let mut reread = Design::from_asc_schedule(
        PreformSpec::block(2.0, 1.0, 2.0),
        &gcs_to_asc_schedule(&parsed),
    );
    reread.apply_imported_frosting(&gcs_frosting(&parsed));
    // The same tiers are frosted, by the same positions.
    assert_eq!(reread.tier_frosting(2), Some(&TierFrosting::Tier));
    assert_eq!(
        reread.tier_frosting(4),
        Some(&TierFrosting::Facets(vec![6.0]))
    );
    assert_eq!(reread.tier_frosting.len(), 2);
}

#[test]
fn a_design_without_frosting_writes_a_byte_identical_gcs_file() {
    let design = fixture();
    let schedule = design.to_asc_schedule().expect("exports");
    assert_eq!(design.gcs_frosting_spec().len(), 0);
    let with = to_gcs_string_with_frosting(&schedule, &design.gcs_frosting_spec()).expect("writes");
    assert_eq!(with, to_gcs_string(&schedule).expect("writes"));
}

#[test]
fn asc_has_no_notation_so_the_schedule_is_unchanged_and_the_export_warns_once() {
    let mut design = fixture();
    assert_eq!(design.asc_frosting_note(), None);
    let polished_asc =
        indicatrix_formats::asc::to_asc_string(&design.to_asc_schedule().expect("exports"));
    set(&mut design, 1, Some(TierFrosting::Tier));
    set(&mut design, 3, Some(TierFrosting::Tier));
    let frosted_asc =
        indicatrix_formats::asc::to_asc_string(&design.to_asc_schedule().expect("exports"));
    assert_eq!(
        frosted_asc, polished_asc,
        "no frosting notation is invented"
    );
    let note = design.asc_frosting_note().expect("a frosted design warns");
    assert!(note.contains("2 frosted tiers"), "{note}");
}
