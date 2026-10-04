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
use indicatrix_formats::native::design::{CONCAVE_FRAME_V0, DESIGN_VERSION_CONCAVE, detect_kind};

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
        &format!("version = {}", DESIGN_VERSION_CONCAVE + 1),
        1,
    );
    match design_from_str(&newer) {
        Err(DesignLoadError::File(DesignFileError::UnsupportedVersion { found, .. })) => {
            assert_eq!(found, i64::from(DESIGN_VERSION_CONCAVE + 1));
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
