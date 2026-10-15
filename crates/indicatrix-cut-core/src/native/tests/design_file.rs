//! The self-contained `.isdd` design file: round trips, determinism, version
//! gating, kind detection and tier correlation by id.

use super::fixtures::{simple_design, unsolved_design};
use crate::{
    design::{ConcaveTier, ConcaveTool, Design, StoneRenderEffects, TierTarget, ToolMotion},
    native::{
        DESIGN_HISTORY_LIMIT, DesignExtras, DesignFileError, DesignLoadError, FileKind,
        catalogue_asc, design_from_file, design_from_str, design_to_file, design_to_string,
    },
    preform::PreformSpec,
};
use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_formats::native::design::{CONCAVE_FRAME_V0, DESIGN_VERSION_PLANES, detect_kind};

fn proportions() -> ExternalProportions {
    ExternalProportions {
        vol_w3: Some(0.61),
        lw: Some(1.0),
        cw: Some(0.3),
        pw: Some(0.43),
        hw: Some(0.58),
    }
}

fn figures(p: ExternalProportions) -> [Option<f64>; 5] {
    [p.vol_w3, p.lw, p.cw, p.pw, p.hw]
}

/// A design exercising every authored field the file must carry.
fn rich_design() -> Design {
    let mut design = simple_design();
    design.tier_notes.insert(0, "polish last".to_string());
    design.cheater_offsets_deg.insert(2, 1.25);
    let id = design.tier_id_at(1).expect("tier 1 has an id");
    design
        .tier_targets
        .insert(id, TierTarget::TableWidthMm(4.1));
    design.preform_y_offset = 0.35;
    design.material.body_color_override = Some([0.2, 0.4, 2.8]);
    design.meta.refractive_index = 1.71;
    design
}

fn extras(history: &[String]) -> DesignExtras<'_> {
    DesignExtras {
        history_entries: history,
        ..DesignExtras::default()
    }
}

#[test]
fn a_design_round_trips_bit_identically_through_the_file() {
    let design = rich_design();
    let history = vec!["Set angle".to_string(), "Move tier".to_string()];
    let text =
        design_to_string(&design, Some(&proportions()), &extras(&history)).expect("serializes");
    let loaded = design_from_str(&text).expect("opens");

    assert_eq!(loaded.design, design);
    assert!(loaded.design.tier_ids_eq(&design));
    assert_eq!(
        loaded.design.meta.refractive_index.to_bits(),
        1.71_f64.to_bits()
    );
    assert_eq!(
        loaded.design.material.body_color_override,
        Some([0.2, 0.4, 2.8])
    );
    assert_eq!(
        loaded.printed_proportions.map(figures),
        Some(figures(proportions()))
    );
    assert_eq!(loaded.history_entries, history);
    assert!(!loaded.draft);
    for (a, b) in loaded.design.tiers.iter().zip(&design.tiers) {
        assert_eq!(a.angle_deg.to_bits(), b.angle_deg.to_bits());
    }
}

/// Builds from 2026-09-28 wrote the per-design body colour as `body_colour_override`. Such
/// a design file must still give the design its colour, and saving it again must write the
/// current key once, not both.
/// A design with every per-design stone effect on, and one value that is not a round number.
fn hazy_design() -> Design {
    let mut design = simple_design();
    design.render_effects =
        StoneRenderEffects::from_render_values(0.5, true, 30.0, 120.0, true, 0.0125);
    design
}

/// The stone effects live in the file's `[render_effects]` table and come back exactly.
#[test]
fn the_stone_effects_round_trip_through_the_render_effects_table() {
    let design = hazy_design();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert!(text.contains("[render_effects]"), "{text}");
    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design.render_effects, design.render_effects);
    assert_eq!(loaded.design, design);
    // And the text is stable under a reload, like the rest of the file.
    let again =
        design_to_string(&loaded.design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(again, text);
}

/// A design with every effect off writes no table at all, so its file is byte-identical to what
/// builds before the table wrote, and a file without the table reads as "all effects off".
#[test]
fn a_design_without_stone_effects_writes_no_render_effects_table() {
    let mut design = hazy_design();
    design.render_effects = StoneRenderEffects::default();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert!(!text.contains("render_effects"), "{text}");
    // The same design with the effects on differs from it only by that table.
    let with_effects =
        design_to_string(&hazy_design(), None, &DesignExtras::default()).expect("serializes");
    let table_start = with_effects
        .find("[render_effects]")
        .expect("the table is written");
    let without_table = with_effects[..table_start].trim_end();
    assert!(
        text.starts_with(without_table),
        "the table is the only difference before it"
    );
    let loaded = design_from_str(&text).expect("opens");
    assert!(loaded.design.render_effects.is_default());
}

/// A table written by an older or newer revision (a key missing, a key not known) still loads.
#[test]
fn a_partial_render_effects_table_reads_the_missing_keys_as_off() {
    let design = hazy_design();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    let start = text.find("[render_effects]").expect("the table is written");
    let tail = &text[start..];
    let end = tail[1..]
        .find(
            "
[",
        )
        .map_or(tail.len(), |i| i + 1);
    let edited = format!(
        "{}[render_effects]
girdle_frosted = true
a_future_key = 1
{}",
        &text[..start],
        &tail[end..]
    );
    let loaded = design_from_str(&edited).expect("opens");
    let effects = loaded.design.render_effects;
    assert!(effects.girdle_frosted);
    assert!(!effects.c_axis_override_enabled);
    assert!(effects.inclusion_sigma_s.abs() < f64::EPSILON);
}

#[test]
fn a_design_file_with_the_old_body_colour_spelling_still_applies_it() {
    let design = rich_design();
    let current = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(current.matches("body_color_override").count(), 1);
    let old = current.replacen("body_color_override", "body_colour_override", 1);
    assert_ne!(old, current);

    let loaded = design_from_str(&old).expect("an old file opens");
    assert_eq!(
        loaded.design.material.body_color_override,
        Some([0.2, 0.4, 2.8]),
        "the stone must get its colour"
    );
    assert_eq!(loaded.design, design);

    let again =
        design_to_string(&loaded.design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(
        again, current,
        "the re-save is the current format, key once"
    );
}

#[test]
fn the_text_is_deterministic_and_stable_under_reload() {
    let design = rich_design();
    let a = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    let b = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(a, b);
    assert!(a.starts_with("format = \"indicatrix-design\"\nversion = 1\n"));
    assert!(!a.contains("asc_filename") && !a.contains("asc_sha256"));
    assert!(!a.contains("catalogue_entry_id"));

    let reloaded = design_from_str(&a).expect("opens");
    let again =
        design_to_string(&reloaded.design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(a, again);
}

#[test]
fn gear_symmetry_mirror_banner_and_headers_are_real_fields() {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 80, 8, 1.54);
    design.meta.gemcad_version = "GemCad 4.2".to_string();
    design.meta.gear_reference_angle = 2.5;
    design.meta.mirror = false;
    design.meta.headers = vec!["Hex step".to_string(), "second line".to_string()];
    design.meta.footnotes = vec!["note A".to_string()];

    let file = design_to_file(&design, None, &DesignExtras::default());
    assert_eq!(file.schedule.gear_teeth, 80);
    assert_eq!(file.schedule.symmetry_order, 8);
    assert!(!file.schedule.mirror);
    assert_eq!(file.schedule.headers, design.meta.headers);

    let text = indicatrix_formats::native::design::to_string(&file).expect("serializes");
    assert!(text.contains("[schedule]") && text.contains("gear_teeth = 80"));
    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design.meta, design.meta);
}

#[test]
fn a_design_that_does_not_solve_still_round_trips() {
    let design = unsolved_design();
    let file = design_to_file(&design, None, &DesignExtras::default()).with_draft(true);
    let text = indicatrix_formats::native::design::to_string(&file).expect("serializes");
    let loaded = design_from_str(&text).expect("opens");
    assert!(loaded.draft);
    assert_eq!(loaded.design, design);
}

#[test]
fn a_newer_major_version_is_a_typed_error() {
    let design = simple_design();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    let newer = text.replacen(
        "version = 1",
        &format!("version = {}", DESIGN_VERSION_PLANES + 1),
        1,
    );
    match design_from_str(&newer) {
        Err(DesignLoadError::File(DesignFileError::UnsupportedVersion { found, .. })) => {
            assert_eq!(found, i64::from(DESIGN_VERSION_PLANES + 1));
        }
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

/// A design whose preform is a plane set is written as a version 4 file (a build that
/// predates the kind refuses it by version) and opens with the identical set.
#[test]
fn a_plane_set_preform_writes_a_version_4_file_and_round_trips() {
    let mut design = rich_design();
    design.preform = PreformSpec::from_planes(
        PreformSpec::block(2.0, 1.0, 2.0).planes(),
        "plan 'Aqua'".to_string(),
    )
    .expect("a block's planes close");
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert!(
        text.starts_with(&format!(
            "format = \"indicatrix-design\"\nversion = {DESIGN_VERSION_PLANES}\n"
        )),
        "{text}"
    );
    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design, design);
    let again =
        design_to_string(&loaded.design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(again, text);
}

#[test]
fn tiers_are_correlated_by_id_not_by_array_position() {
    let design = rich_design();
    let mut file = design_to_file(&design, None, &DesignExtras::default());
    // Every tier carries its own id and its own authored extras.
    assert!(file.tiers.iter().all(|t| t.tier_id.is_some()));
    file.tiers.reverse();
    let loaded = design_from_file(file).expect("opens").design;

    assert_eq!(loaded.tiers.len(), design.tiers.len());
    for (old_index, tier) in design.tiers.iter().enumerate() {
        let id = design.tier_id_at(old_index).expect("id");
        let new_index = loaded
            .index_of_tier_id(id)
            .expect("id survives the reorder");
        assert_eq!(loaded.tiers[new_index].name, tier.name);
        assert_eq!(loaded.tiers[new_index].angle_deg, tier.angle_deg);
        assert_eq!(loaded.tier_note(new_index), design.tier_note(old_index));
        assert_eq!(
            loaded.cheater_offset_deg(new_index),
            design.cheater_offset_deg(old_index)
        );
        assert_eq!(loaded.tier_target(new_index), design.tier_target(old_index));
    }
}

#[test]
fn history_is_bounded_to_the_newest_entries() {
    let design = simple_design();
    let history: Vec<String> = (0..DESIGN_HISTORY_LIMIT + 5)
        .map(|i| format!("edit {i}"))
        .collect();
    let file = design_to_file(&design, None, &extras(&history));
    let kept = file.history.expect("history written").entries;
    assert_eq!(kept.len(), DESIGN_HISTORY_LIMIT);
    assert_eq!(kept.first().map(String::as_str), Some("edit 5"));
}

/// A file of the removed overlay sidecar format is no design file: it is not recognised
/// as one and is refused with a typed error rather than opened.
#[test]
fn an_old_overlay_sidecar_is_not_a_design_file_and_is_refused() {
    let sidecar = "format_version = 1\n\
                   asc_filename = \"design.asc\"\n\
                   asc_sha256 = \"00\"\n\n\
                   [preform]\n\
                   shape = { kind = \"block\" }\n\
                   half_width = 1.0\n\
                   length_over_width = 1.0\n\
                   depth = 2.0\n";
    assert_eq!(detect_kind(sidecar.as_bytes()), FileKind::Unknown);
    assert!(matches!(
        design_from_str(sidecar),
        Err(DesignLoadError::File(
            DesignFileError::NotADesignFile { .. }
        ))
    ));
    let design = simple_design();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(detect_kind(text.as_bytes()), FileKind::Design);
}

/// Two concave tiers touching every field, one per angle sign, with a cone (which
/// carries a tool angle) and a sphere (which must not).
fn concave_design() -> Design {
    let mut design = rich_design();
    // The base fixture's gear has 4 teeth; the indices below (up to 48) need a real
    // index wheel to be valid (`ConcaveTier::validate` bounds them by gear teeth).
    design.meta.gear_teeth = 96;
    design.concave_tiers = vec![
        ConcaveTier {
            name: "Groove".to_string(),
            angle_deg: -62.0,
            indices: vec![3.0, 11.5, 19.0],
            instructions: "cut to depth".to_string(),
            tool: ConcaveTool::Cone,
            tool_azimuth_deg: -15.0,
            displacement: [-0.25, 0.12, 0.05],
            diameter_ratio: 0.25,
            tool_angle_deg: Some(60.0),
            motion: ToolMotion::Plunge,
        },
        ConcaveTier {
            name: "Dimple".to_string(),
            angle_deg: 30.0,
            indices: vec![0.0, 48.0],
            instructions: String::new(),
            tool: ConcaveTool::Sphere,
            tool_azimuth_deg: 0.0,
            displacement: [0.0, 0.0, 0.1],
            diameter_ratio: 0.4,
            tool_angle_deg: None,
            motion: ToolMotion::Reciprocating,
        },
    ];
    design.ensure_concave_tier_ids();
    design
}

#[test]
fn native_round_trip_with_concave_tiers_preserves_every_field() {
    let design = concave_design();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 2\n"));
    assert!(text.contains(&format!("concave_frame = \"{CONCAVE_FRAME_V0}\"")));
    let loaded = design_from_str(&text).expect("opens");
    // `Design` equality compares every concave field except the regenerated ids.
    assert_eq!(loaded.design, design);
    assert_eq!(loaded.design.concave_tiers, design.concave_tiers);
    assert_eq!(loaded.design.concave_tier_ids.len(), 2);
    let again =
        design_to_string(&loaded.design, None, &DesignExtras::default()).expect("serializes again");
    assert_eq!(again, text);

    // A planar design never becomes version 2.
    let planar = design_to_string(&rich_design(), None, &DesignExtras::default()).expect("ok");
    assert!(planar.starts_with("format = \"indicatrix-design\"\nversion = 1\n"));
    assert!(!planar.contains("concave"));
}

/// The cutting-mode marks of a concave step hang on the tier's id, so the ids must
/// survive a reopen: a flat tier added after the concave ones, then the concave tiers
/// reordered, must come back with exactly the ids they had.
#[test]
fn concave_tier_ids_survive_a_reopen_after_a_flat_tier_and_a_reorder() {
    let mut design = concave_design();
    let mut late = design.tiers[0].clone();
    late.name = "Late crown".to_string();
    design.tiers.push(late);
    let late_id = design.allocate_tier_id();
    design.tier_ids.push(late_id);
    design.concave_tiers.reverse();
    design.concave_tier_ids.reverse();
    assert!(
        design.concave_tier_ids[0] > design.concave_tier_ids[1],
        "the reorder moved the later id to the front"
    );

    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(text.matches("concave_tier_id = ").count(), 2);
    let loaded = design_from_str(&text).expect("opens").design;
    assert_eq!(loaded.concave_tiers, design.concave_tiers);
    assert_eq!(loaded.concave_tier_ids, design.concave_tier_ids);
    assert!(loaded.tier_ids_eq(&design));

    // The counter moved past every restored id, so the next new tier cannot repeat one.
    let mut reopened = loaded;
    let fresh = reopened.allocate_tier_id();
    assert!(!reopened.tier_ids.contains(&fresh));
    assert!(!reopened.concave_tier_ids.contains(&fresh));

    // Saving again writes the same bytes: the ids are stable, not regenerated.
    let again =
        design_to_string(&reopened, None, &DesignExtras::default()).expect("serializes again");
    assert_eq!(again, text);
}

/// A file saved before the key existed still opens; its concave tiers get fresh ids that
/// no flat tier holds. A repeated id, or one a flat tier owns, is kept by one holder only.
#[test]
fn files_without_or_with_clashing_concave_tier_ids_still_open_with_distinct_ids() {
    let design = concave_design();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");

    let mut stripped = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("concave_tier_id"))
        .collect::<Vec<_>>()
        .join("\n");
    stripped.push('\n');
    assert!(!stripped.contains("concave_tier_id"));
    let loaded = design_from_str(&stripped)
        .expect("an old file opens")
        .design;
    assert_eq!(loaded.concave_tiers, design.concave_tiers);
    assert_eq!(loaded.concave_tier_ids.len(), 2);
    assert_ne!(loaded.concave_tier_ids[0], loaded.concave_tier_ids[1]);
    for id in &loaded.concave_tier_ids {
        assert!(!loaded.tier_ids.contains(id), "a fresh id is not a flat id");
    }

    let first = design.concave_tier_ids[0].value();
    let second = design.concave_tier_ids[1].value();
    let repeated = text.replacen(
        &format!("concave_tier_id = {second}"),
        &format!("concave_tier_id = {first}"),
        1,
    );
    let flat_id = design.tier_ids[0].value();
    let on_a_flat_id = text.replacen(
        &format!("concave_tier_id = {first}"),
        &format!("concave_tier_id = {flat_id}"),
        1,
    );
    for clashing in [repeated, on_a_flat_id] {
        let loaded = design_from_str(&clashing).expect("opens").design;
        let mut every: Vec<u64> = loaded
            .tier_ids
            .iter()
            .chain(&loaded.concave_tier_ids)
            .map(|id| id.value())
            .collect();
        let count = every.len();
        every.sort_unstable();
        every.dedup();
        assert_eq!(every.len(), count, "every id in the design is distinct");
        assert!(loaded.tier_ids_eq(&design));
    }
}

#[test]
fn a_concave_tier_with_an_unknown_tool_is_refused_on_open() {
    let text = design_to_string(&concave_design(), None, &DesignExtras::default()).expect("ok");
    let bad = text.replacen("tool = \"CON\"", "tool = \"XYZ\"", 1);
    assert!(matches!(
        design_from_str(&bad),
        Err(DesignLoadError::ConcaveTier { index: 0, .. })
    ));
}

/// The catalogue `.asc` a save stores carries each concave tier as two ASCII footnotes
/// tagged as generated, after the footnotes the user typed (which stay, in order, even
/// when one is shaped exactly like a tool line).
#[test]
fn the_catalogue_asc_footnotes_are_ascii_and_a_users_look_alike_note_stays() {
    let mut design = concave_design();
    design.meta.footnotes = vec![
        "Notch  12.00  5  by hand".to_string(),
        "CYL  +45.00 deg  X = 0.100, Y = 0.200, Z = 0.300  D/W = 0.300, plunge".to_string(),
    ];
    let saved = catalogue_asc(&design, None, None).expect("saves");
    let footnote_lines: Vec<&str> = saved
        .asc_text
        .lines()
        .filter(|line| line.starts_with("F "))
        .collect();
    assert_eq!(footnote_lines.len(), 2 + 2 * design.concave_tiers.len());
    assert!(footnote_lines.iter().all(|line| line.is_ascii()));
    assert!(
        footnote_lines[2..]
            .iter()
            .all(|line| line.ends_with(crate::design::CONCAVE_FOOTNOTE_MARKER)),
        "{footnote_lines:?}"
    );

    // The design file itself keeps the user's footnotes only: the concave tiers are
    // records of their own, never footnotes.
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design.meta.footnotes, design.meta.footnotes);
    assert_eq!(loaded.design.concave_tiers, design.concave_tiers);
}
