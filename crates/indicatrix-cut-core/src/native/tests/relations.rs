//! Tier relations in the files: the self-contained design file (version 3, written only
//! when a relation exists), the legacy paired sidecar and the autosave, plus the keys of
//! a design file this build does not claim surviving Open then Save.

use crate::{
    design::{
        ConcaveTier, ConcaveTool, ConstraintTier, Design, RelationExpr, TierRelation, ToolMotion,
    },
    edit::Edit,
    native::{
        DesignExtras, DesignLoadError, SaveExtras, design_from_file, design_from_str,
        design_to_file, design_to_string, load_native_only, load_paired, save_native_only_toml,
        to_native_file, to_toml_string,
    },
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_formats::native::design::{self, DESIGN_VERSION_RELATIONS};

fn crown_tier(name: &str, angle_deg: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0, 24.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// C1 (40), C2 (38) and C3 (30) on a 96-tooth gear, where C2 follows C1 - 2.
fn related_design() -> Design {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers = vec![
        crown_tier("C1", 40.0),
        crown_tier("C2", 38.0),
        crown_tier("C3", 30.0),
    ];
    design.ensure_tier_ids();
    let relation = design.parse_relation("C1 - 2").expect("reads");
    let driven = design.tier_ids[1];
    design.tier_relations.insert(driven, relation);
    design
}

fn with_a_concave_tier(mut design: Design) -> Design {
    design.concave_tiers = vec![ConcaveTier {
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
    }];
    design.ensure_concave_tier_ids();
    design
}

fn to_text(design: &Design) -> String {
    design_to_string(design, None, &DesignExtras::default()).expect("serializes")
}

// --- the self-contained design file -----------------------------------------------------

#[test]
fn a_design_with_relations_is_a_version_3_file_and_round_trips() {
    let design = related_design();
    let text = to_text(&design);
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 3\n"));
    let reads = design.tier_id_at(0).expect("C1 has an id").value();
    assert!(
        text.contains(&format!("angle_relation = \"@{reads} - 2\"")),
        "{text}"
    );
    assert_eq!(text.matches("angle_relation").count(), 1);

    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design, design);
    assert!(loaded.design.tier_ids_eq(&design));
    assert_eq!(loaded.design.tier_relation(1), design.tier_relation(1));
    assert_eq!(loaded.design.relation_text(1).as_deref(), Some("C1 - 2"));
    assert_eq!(
        to_text(&loaded.design),
        text,
        "the text is stable under reload"
    );
    assert_eq!(
        design_to_file(&design, None, &DesignExtras::default()).version,
        DESIGN_VERSION_RELATIONS
    );
}

#[test]
fn relations_and_concave_tiers_share_version_3() {
    let design = with_a_concave_tier(related_design());
    let text = to_text(&design);
    assert!(text.starts_with("format = \"indicatrix-design\"\nversion = 3\n"));
    assert!(text.contains("concave_frame"));
    assert!(text.contains("angle_relation"));
    let loaded = design_from_str(&text).expect("opens");
    assert_eq!(loaded.design, design);
    assert_eq!(loaded.design.concave_tiers, design.concave_tiers);
    assert_eq!(loaded.design.tier_relation(1), design.tier_relation(1));
    assert_eq!(to_text(&loaded.design), text);
}

#[test]
fn a_design_without_relations_keeps_its_old_version_and_bytes() {
    let mut design = related_design();
    design.tier_relations.clear();
    let planar = to_text(&design);
    assert!(planar.starts_with("format = \"indicatrix-design\"\nversion = 1\n"));
    assert!(!planar.contains("angle_relation"));

    let concave = to_text(&with_a_concave_tier(design));
    assert!(concave.starts_with("format = \"indicatrix-design\"\nversion = 2\n"));
    assert!(!concave.contains("angle_relation"));
}

#[test]
fn a_stored_angle_that_drifted_from_its_relation_opens_corrected() {
    let design = related_design();
    let mut file = design_to_file(&design, None, &DesignExtras::default());
    file.tiers[1].angle_deg = Some(12.0);
    let loaded = design_from_file(file).expect("opens");
    assert_eq!(loaded.design.tiers[1].angle_deg, 38.0);
}

#[test]
fn a_pavilion_tier_keeps_its_side_when_it_opens() {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers = vec![crown_tier("P1", -40.0), crown_tier("P2", -10.0)];
    design.ensure_tier_ids();
    let relation = design.parse_relation("P1 - 2").expect("reads");
    let driven = design.tier_ids[1];
    design.tier_relations.insert(driven, relation);
    let loaded = design_from_str(&to_text(&design)).expect("opens");
    assert_eq!(loaded.design.tiers[1].angle_deg, -38.0);
}

#[test]
fn a_relation_the_file_cannot_satisfy_is_refused_naming_the_tier() {
    let design = related_design();
    let file = design_to_file(&design, None, &DesignExtras::default());
    let c1 = design.tier_id_at(0).expect("C1 has an id").value();
    let c2 = design.tier_id_at(1).expect("C2 has an id").value();
    let refused = |relation: &str| {
        let mut file = file.clone();
        file.tiers[1].angle_relation = Some(relation.to_owned());
        match design_from_file(file) {
            Err(DesignLoadError::Relation { index, reason }) => {
                assert_eq!(index, 1, "{relation}");
                reason
            }
            other => panic!("{relation}: expected a relation error, got {other:?}"),
        }
    };
    assert!(refused(&format!("@{c1} +")).contains("cannot be read"));
    assert!(refused("C1 - 2").contains("by number"));
    assert!(refused("@9999 - 2").contains("no longer in the design"));
    assert!(refused(&format!("@{c2}")).contains("refers to itself"));
    assert!(refused(&format!("@{c1} + 60")).contains("100.00"));
    let mut looped = file.clone();
    looped.tiers[1].angle_relation = Some(format!("@{c2}"));
    let error = design_from_file(looped).expect_err("a loop");
    assert!(
        error
            .to_string()
            .starts_with("the relation for tier 2 is not usable:"),
        "{error}"
    );
}

/// The load error points at the tier whose relation failed, not at the first tier that has a
/// relation (here C2, which is fine and comes first).
#[test]
fn a_failing_relation_is_blamed_on_its_own_tier() {
    let design = related_design();
    let base = design_to_file(&design, None, &DesignExtras::default());
    let c1 = design.tier_id_at(0).expect("C1 has an id").value();
    let c3 = design.tier_id_at(2).expect("C3 has an id").value();
    let blamed = |relation: String| {
        let mut file = base.clone();
        file.tiers[2].angle_relation = Some(relation);
        match design_from_file(file) {
            Err(DesignLoadError::Relation { index, reason }) => (index, reason),
            other => panic!("expected a relation error, got {other:?}"),
        }
    };

    // C1 + 60 comes out at 100 degrees.
    let (index, reason) = blamed(format!("@{c1} + 60"));
    assert_eq!(index, 2, "C3 is the tier that cannot follow it");
    assert!(
        reason.contains("C3") && reason.contains("100.00"),
        "{reason}"
    );

    // C3 reading itself is a loop of one tier.
    let (index, reason) = blamed(format!("@{c3}"));
    assert_eq!(index, 2);
    assert!(reason.contains("refers to itself"), "{reason}");

    // A relation that reads a tier that is gone names the tier that reads it.
    let (index, reason) = blamed("@9999 - 2".to_owned());
    assert_eq!(index, 2);
    assert!(reason.contains("no longer in the design"), "{reason}");
}

// --- the paired sidecar and the autosave ----------------------------------------------

/// A real schedule whose tier 2 is meant to follow tier 1 minus 2 degrees.
const RELATION_ASC: &str = "GemCad 5.0\n\
     g 4 0.0\n\
     y 1 n\n\
     I 1.62\n\
     a 90.000000 1.00000000 0 1 2 3 G Set stone size.\n\
     a 40.000000 0.60000000 0 1 2 3 G Set stone size.\n\
     a 38.000000 0.55000000 0 1 2 3 G Set stone size.\n";

fn asc_design() -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(RELATION_ASC).expect("fixture must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    let reads = design.tier_id_at(1).expect("an imported tier has an id");
    let driven = design.tier_id_at(2).expect("an imported tier has an id");
    design.tier_relations.insert(
        driven,
        TierRelation::new(RelationExpr::offset_from(reads, -2.0)),
    );
    design
}

#[test]
fn the_paired_sidecar_carries_relations_and_a_plain_one_does_not() {
    let design = asc_design();
    let native = to_native_file(
        &design,
        "design.asc",
        RELATION_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let reads = design.tier_id_at(1).expect("id").value();
    assert_eq!(native.tiers[0].angle_relation, None);
    assert_eq!(native.tiers[1].angle_relation, None);
    assert_eq!(
        native.tiers[2].angle_relation,
        Some(format!("@{reads} - 2"))
    );
    let toml = to_toml_string(&native).expect("serializes");
    assert!(toml.contains("angle_relation"));

    let loaded = load_paired(RELATION_ASC, &toml, false).expect("loads");
    assert_eq!(loaded.design.tier_relation(2), design.tier_relation(2));
    assert_eq!(loaded.design.tiers[2].angle_deg, 38.0);

    // A design without relations writes no such key at all, so its sidecar bytes do
    // not change.
    let mut plain = asc_design();
    plain.tier_relations.clear();
    let plain_native = to_native_file(
        &plain,
        "design.asc",
        RELATION_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    assert!(
        !to_toml_string(&plain_native)
            .expect("ok")
            .contains("angle_relation")
    );
}

#[test]
fn a_relation_waits_for_the_asc_it_was_saved_against() {
    let design = asc_design();
    let native = to_native_file(
        &design,
        "design.asc",
        RELATION_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let toml = to_toml_string(&native).expect("serializes");
    // The same schedule re-touched: the fingerprint no longer matches, so the saved
    // overlay (and the relation, which decides an angle) is not applied.
    let changed = format!(
        "GemCad 5.0\nH Re-touched by GemCAD\n{}",
        &RELATION_ASC[11..]
    );
    let loaded = load_paired(&changed, &toml, false).expect("still loads");
    assert!(loaded.design.tier_relations.is_empty());
}

#[test]
fn the_autosave_restores_relations() {
    let design = related_design();
    let toml =
        save_native_only_toml(&design, "x.asc", None, &SaveExtras::default()).expect("serializes");
    assert!(toml.contains("angle_relation"));
    let restored = load_native_only(&toml).expect("restores").design;
    assert_eq!(restored.tier_relation(1), design.tier_relation(1));
    assert_eq!(restored.tiers[1].angle_deg, 38.0);
    assert!(restored.tier_ids_eq(&design));
}

// --- keys this build does not claim ------------------------------------------------------

#[test]
fn unknown_keys_survive_open_then_save() {
    use toml::Value;
    let design = with_a_concave_tier(related_design());
    let history = vec!["Set C2 = C1 - 2".to_string()];
    let extras = DesignExtras {
        history_entries: &history,
        ..DesignExtras::default()
    };
    let mut file = design_to_file(&design, None, &extras);
    file.unknown
        .insert("future_top".to_string(), Value::String("kept".into()));
    file.preform
        .unknown
        .insert("future_preform".to_string(), Value::Integer(42));
    file.material
        .unknown
        .insert("future_material".to_string(), Value::Boolean(true));
    file.schedule
        .unknown
        .insert("future_schedule".to_string(), Value::Float(0.5));
    file.history
        .as_mut()
        .expect("history is written")
        .unknown
        .insert("future_history".to_string(), Value::Integer(7));
    file.tiers[1]
        .unknown
        .insert("future_tier".to_string(), Value::String("too".into()));
    file.concave_tiers[0]
        .unknown
        .insert("future_concave".to_string(), Value::Integer(9));
    let text = design::to_string(&file).expect("serializes");

    let loaded = design_from_str(&text).expect("opens");
    assert!(loaded.design.file_extras.is_some());
    // `Design` equality leaves the carried keys out: it is authored content only.
    assert_eq!(loaded.design, design);

    let reextras = DesignExtras {
        history_entries: &loaded.history_entries,
        ..DesignExtras::default()
    };
    let again = design_to_string(&loaded.design, None, &reextras).expect("serializes");
    assert_eq!(again, text, "every unknown key is written back");
}

#[test]
fn a_tiers_unknown_keys_follow_the_tier_and_go_when_it_goes() {
    use toml::Value;
    let design = related_design();
    let mut file = design_to_file(&design, None, &DesignExtras::default());
    file.tiers[1]
        .unknown
        .insert("future_tier".to_string(), Value::String("too".into()));
    let text = design::to_string(&file).expect("serializes");
    let mut loaded = design_from_str(&text).expect("opens").design;

    // Moved to the front, the tier takes its keys with it.
    loaded
        .apply_edit(Edit::MoveTier { from: 1, to: 0 })
        .expect("moves");
    let moved = design::parse(&to_text(&loaded)).expect("parses");
    assert!(moved.tiers[0].unknown.contains_key("future_tier"));
    assert!(!moved.tiers[1].unknown.contains_key("future_tier"));
    assert!(!moved.tiers[2].unknown.contains_key("future_tier"));

    // Removed, its keys are not written anywhere.
    let position = loaded
        .tiers
        .iter()
        .position(|tier| tier.name == "C2")
        .expect("C2 is still there");
    loaded
        .apply_edit(Edit::RemoveTier { index: position })
        .expect("removes");
    assert!(!to_text(&loaded).contains("future_tier"));
}

#[test]
fn a_file_with_no_unknown_keys_carries_nothing() {
    let loaded = design_from_str(&to_text(&related_design())).expect("opens");
    assert!(loaded.design.file_extras.is_none());
}

#[test]
fn tier_relations_are_noted_when_a_design_is_exported_as_plain_angles() {
    let design = related_design();
    assert_eq!(
        design.export_notes(),
        vec!["Tier relations are exported as plain angles.".to_string()]
    );
    // A relation is not a loss that needs a question before the export.
    assert_eq!(design.export_warnings().len(), 0);
    let mut plain = design;
    plain.tier_relations.clear();
    assert_eq!(plain.export_notes(), Vec::<String>::new());
}
