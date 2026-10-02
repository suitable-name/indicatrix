//! The self-contained `.indicatrix` design file: round trips, determinism, version
//! gating, kind detection, tier correlation by id and sidecar migration.

use super::fixtures::{simple_design, unsolved_design};
use crate::{
    design::{Design, TierTarget},
    native::{
        DESIGN_HISTORY_LIMIT, DesignExtras, DesignFileError, DesignLoadError, FileKind,
        design_from_file, design_from_str, design_to_file, design_to_string, load_paired,
        migrate_sidecar_to_file, save_paired,
    },
    preform::PreformSpec,
};
use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_formats::native::design::{DESIGN_VERSION, detect_kind};

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
    design.material.body_colour_override = Some([0.2, 0.4, 2.8]);
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
        loaded.design.material.body_colour_override,
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
        &format!("version = {}", DESIGN_VERSION + 1),
        1,
    );
    match design_from_str(&newer) {
        Err(DesignLoadError::File(DesignFileError::UnsupportedVersion { found, .. })) => {
            assert_eq!(found, i64::from(DESIGN_VERSION + 1));
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
