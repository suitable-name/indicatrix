//! The self-contained `.indicatrix` design file: round trips, determinism, version
//! gating, kind detection, tier correlation by id and sidecar migration.

use super::fixtures::{simple_design, unsolved_design};
use crate::{
    design::{ConcaveTier, ConcaveTool, Design, TierTarget, ToolMotion},
    native::{
        DESIGN_HISTORY_LIMIT, DesignExtras, DesignFileError, DesignLoadError, FileKind,
        design_from_file, design_from_str, design_to_file, design_to_string, load_paired,
        migrate_sidecar_to_file, save_paired,
    },
    preform::PreformSpec,
};
use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_formats::native::design::{CONCAVE_FRAME_V0, DESIGN_VERSION_RELATIONS, detect_kind};

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
        &format!("version = {}", DESIGN_VERSION_RELATIONS + 1),
        1,
    );
    match design_from_str(&newer) {
        Err(DesignLoadError::File(DesignFileError::UnsupportedVersion { found, .. })) => {
            assert_eq!(found, i64::from(DESIGN_VERSION_RELATIONS + 1));
        }
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
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

#[test]
fn an_old_overlay_sidecar_is_recognised_and_not_opened_as_a_design() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("saves");
    assert_eq!(
        detect_kind(saved.native_toml.as_bytes()),
        FileKind::OverlaySidecar
    );
    assert!(matches!(
        design_from_str(&saved.native_toml),
        Err(DesignLoadError::File(
            DesignFileError::NotADesignFile { .. }
        ))
    ));
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    assert_eq!(detect_kind(text.as_bytes()), FileKind::Design);
}

#[test]
fn a_paired_design_migrates_to_a_self_contained_file() {
    let design = rich_design();
    let history = vec!["Set angle".to_string()];
    let saved =
        save_paired(&design, "design.asc", None, None, Some(&proportions())).expect("saves");
    let saved = {
        // Re-save with a history trail attached to the sidecar.
        let mut native = saved.native.clone();
        native.history = Some(indicatrix_formats::native::HistoryTable::new(
            history.clone(),
        ));
        native
            .unknown
            .insert("future_key".to_string(), toml::Value::Integer(7));
        (saved.asc_text, native)
    };
    let (asc_text, sidecar) = saved;
    let sidecar_text = indicatrix_formats::native::to_toml_string(&sidecar).expect("serializes");
    let paired = load_paired(&asc_text, &sidecar_text, false).expect("paired load");

    let file = migrate_sidecar_to_file(&paired.design, &sidecar);
    assert_eq!(
        file.unknown
            .get("future_key")
            .and_then(toml::Value::as_integer),
        Some(7)
    );
    let text = indicatrix_formats::native::design::to_string(&file).expect("serializes");
    assert!(!text.contains("asc_filename") && !text.contains("asc_sha256"));

    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design, paired.design);
    assert!(loaded.design.tier_ids_eq(&paired.design));
    assert_eq!(
        loaded.printed_proportions.map(figures),
        Some(figures(proportions()))
    );
    assert_eq!(loaded.history_entries, history);
}

/// The sidecar's concave stash and the fingerprint written next to it are both rebuilt from
/// the design; neither may ride along into the design file as an unknown top-level key (it
/// would be rewritten on every save and never read).
#[test]
fn migrating_a_concave_sidecar_drops_its_stash_and_its_fingerprint() {
    use crate::native::convert::{CONCAVE_FLAT_FINGERPRINT_KEY, CONCAVE_TIERS_STASH_KEY};
    let design = concave_design();
    let saved = save_paired(&design, "x.asc", None, None, None).expect("saves");
    assert!(saved.native.unknown.contains_key(CONCAVE_TIERS_STASH_KEY));
    assert!(
        saved
            .native
            .unknown
            .contains_key(CONCAVE_FLAT_FINGERPRINT_KEY),
        "the sidecar carries the fingerprint, so the migration has something to drop"
    );
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("loads");

    let file = migrate_sidecar_to_file(&loaded.design, &saved.native);
    assert!(!file.unknown.contains_key(CONCAVE_TIERS_STASH_KEY));
    assert!(!file.unknown.contains_key(CONCAVE_FLAT_FINGERPRINT_KEY));
    let text = indicatrix_formats::native::design::to_string(&file).expect("serializes");
    assert!(
        !text.contains(CONCAVE_FLAT_FINGERPRINT_KEY) && !text.contains(CONCAVE_TIERS_STASH_KEY),
        "no stash or fingerprint key in the file"
    );
    // The concave tiers themselves still come from the design.
    let reopened = design_from_str(&text).expect("opens");
    assert_eq!(reopened.design.concave_tiers, design.concave_tiers);
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

#[test]
fn the_paired_sidecar_and_the_autosave_keep_concave_tier_ids() {
    use crate::native::{SaveExtras, load_native_only, save_native_only_toml};
    let mut design = concave_design();
    design.concave_tier_ids.reverse();
    design.concave_tiers.reverse();
    let toml =
        save_native_only_toml(&design, "x.asc", None, &SaveExtras::default()).expect("serializes");
    let restored = load_native_only(&toml).expect("restores").design;
    assert_eq!(restored.concave_tier_ids, design.concave_tier_ids);

    let saved = save_paired(&design, "x.asc", None, None, None).expect("saves");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("loads");
    assert_eq!(loaded.design.concave_tier_ids, design.concave_tier_ids);
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

/// A concave record that cannot be written must leave a marker in its slot, so the
/// restore names the position instead of the list silently shrinking.
#[test]
fn a_concave_record_that_cannot_be_stashed_leaves_a_marker_the_restore_refuses() {
    use crate::native::convert::{
        CONCAVE_TIERS_STASH_KEY, concave_tier_tables, stash_entry, unstash_concave_tiers,
    };
    let design = concave_design();
    let good = toml::Value::try_from(concave_tier_tables(&design)[0].clone()).expect("writes");
    let marker = stash_entry(1, Err::<toml::Value, _>("a made-up failure"));
    let toml::Value::String(text) = &marker else {
        panic!("the marker is a text value, not a dropped slot");
    };
    assert!(text.contains("concave tier 2") && text.contains("a made-up failure"));

    let mut unknown = toml::Table::new();
    unknown.insert(
        CONCAVE_TIERS_STASH_KEY.to_string(),
        toml::Value::Array(vec![good, marker]),
    );
    let (position, _) = unstash_concave_tiers(&unknown).expect_err("the marker is refused");
    assert_eq!(position, 1);
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

#[test]
fn the_paired_sidecar_and_the_autosave_carry_concave_tiers_too() {
    use crate::native::{SaveExtras, load_native_only, save_native_only_toml};
    let design = concave_design();
    let toml =
        save_native_only_toml(&design, "x.asc", None, &SaveExtras::default()).expect("serializes");
    let restored = load_native_only(&toml).expect("restores").design;
    assert_eq!(restored.concave_tiers, design.concave_tiers);

    let saved = save_paired(&design, "x.asc", None, None, None).expect("saves");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("loads");
    assert_eq!(loaded.design.concave_tiers, design.concave_tiers);
    // A planar sidecar gains no stash key.
    let planar = save_paired(&simple_design(), "x.asc", None, None, None).expect("saves");
    assert!(!planar.native_toml.contains("concave"));
}

/// The paired `.asc` carries each concave tier as two ASCII footnotes tagged as generated;
/// the paired load takes exactly those out of the design's footnotes again and leaves alone
/// a note the user typed in the same shape.
#[test]
fn the_paired_asc_footnotes_are_ascii_and_a_users_look_alike_note_survives_the_load() {
    let mut design = concave_design();
    design.meta.footnotes = vec![
        "Notch  12.00  5  by hand".to_string(),
        "CYL  +45.00 deg  X = 0.100, Y = 0.200, Z = 0.300  D/W = 0.300, plunge".to_string(),
    ];
    let saved = save_paired(&design, "x.asc", None, None, None).expect("saves");
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

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("loads");
    assert_eq!(loaded.design.meta.footnotes, design.meta.footnotes);
}

/// An older build can edit the flat tiers and re-save the stash verbatim; the stash
/// then describes a different stone. Restoring it is an error, never a silent drop
/// or a silent restore, and a stash from before the fingerprint existed still loads.
#[test]
fn a_stash_saved_against_other_flat_tiers_is_refused_not_restored() {
    use crate::native::{
        LoadNativeOnlyError, LoadPairedError, SaveExtras, load_native_only, save_native_only_toml,
    };
    const KEY: &str = "indicatrix_cut_core_concave_flat_fingerprint";
    let design = concave_design();
    let saved = save_paired(&design, "x.asc", None, None, None).expect("saves");
    assert!(
        saved.native_toml.contains(KEY),
        "the stash carries a fingerprint"
    );

    let forged = |toml: &str| -> String {
        toml.lines()
            .map(|line| {
                if line.trim_start().starts_with(KEY) {
                    format!("{KEY} = \"{}\"", "0".repeat(64))
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(matches!(
        load_paired(&saved.asc_text, &forged(&saved.native_toml), false),
        Err(LoadPairedError::ConcaveStale { .. })
    ));

    // The flat schedule edited under the stash (an older build re-saved the stash).
    // The concave fixture solves, so its sidecar is an ordinary (non-draft) one.
    let fixture = Design::concave_fixture();
    let paired = save_paired(&fixture, "x.asc", None, None, None).expect("saves");
    assert!(paired.draft_reason.is_none());
    load_paired(&paired.asc_text, &paired.native_toml, false).expect("the untouched pair loads");
    let mut edited = fixture;
    edited.tiers[1].angle_deg += 1.0;
    let edited_asc = save_paired(&edited, "x.asc", None, None, None).expect("saves");
    assert!(matches!(
        load_paired(&edited_asc.asc_text, &paired.native_toml, false),
        Err(LoadPairedError::ConcaveStale { .. })
    ));

    let autosave =
        save_native_only_toml(&design, "x.asc", None, &SaveExtras::default()).expect("saves");
    assert!(matches!(
        load_native_only(&forged(&autosave)),
        Err(LoadNativeOnlyError::ConcaveStale { .. })
    ));

    // A stash written before the fingerprint existed has none to check.
    let legacy: String = saved
        .native_toml
        .lines()
        .filter(|line| !line.trim_start().starts_with(KEY))
        .collect::<Vec<_>>()
        .join("\n");
    let loaded = load_paired(&saved.asc_text, &legacy, false).expect("loads");
    assert_eq!(loaded.design.concave_tiers, design.concave_tiers);
}
