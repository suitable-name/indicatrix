//! `.gem` and `.gcs` import through the same path as `.asc`.

use super::super::{
    foreign::test_gem::{SYNTHETIC_GEM_TIERS, encode_gem},
    pipeline::import_path,
};
use crate::gui::library::local::helpers::test_support::{
    VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
};
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use indicatrix_formats::gcs::{GcsTierFrosting, to_gcs_string_with_frosting};
use indicatrix_vault::model::{entry::FullDiagramRecord, frosting::StoredTierFrosting};

// --- .gem / .gcs import ---

/// A real `.gcs` excerpt, copied verbatim from `indicatrix_formats::gcs`'s own
/// tests (`GCS_OCTABAR_X_EXCERPT`, private to that crate): one pavilion tier with
/// two facets, `<render>` and a multi-line `<info>`.
const GCS_OCTABAR_X_EXCERPT: &str = r#"<GemCutStudio version="1000">
<index gear="64" base="0" symmetry="4" mirror="0"/>
<tier angle="126.38999938964842" depth="0.6664250328253426" name="P1" instructions="" visible="true" guide="false">
    <facet nx="-0" ny="-0.8049973625502862" nz="-0.59327838852184989" index_angle="0">
        <vertex x="-0.082702055286700479" y="-0.81860733222343418" z="-0.012554459365542562"/>
        <vertex x="-0.24803731741144758" y="-0.87723469910015384" z="0.066994832538740542"/>
        <vertex x="-0.41421356237309492" y="-0.99999999999999944" z="0.23357049979554395"/>
        <vertex x="0.41421356237309509" y="-1" z="0.23357049979554453"/>
        <vertex x="0.24803731773169496" y="-0.87723469921371255" z="0.066994832692824094"/>
        <vertex x="0.082702055286700812" y="-0.81860733222343418" z="-0.012554459365542562"/>
    </facet>
    <facet nx="-0.56921909389659309" ny="-0.56921909389659309" nz="-0.59327838852184989" index_angle="45">
        <vertex x="-0.99999999999999944" y="-0.41421356237309503" z="0.23357049979554412"/>
        <vertex x="-0.41421356237309492" y="-0.99999999999999944" z="0.23357049979554395"/>
        <vertex x="-0.57116930351496131" y="-0.5711693029784638" z="-0.027279076108571262"/>
    </facet>
</tier>
<render material="176 Corundum" refractive_index="1.76" dispersion="0.017999999" clarity="100" density="1.4" lighting_model="Random">
    <color r="0.70980394" g="0.73333335" b="0.94901967"/>
</render>
<info title="FVS-044 Octabar-X PC 21.086F" author="Van Sant, Fred W" date="Star Cuts 1 1998

" header2="This design released into the public domain

" header3="by Keith Wyman in memory of Charles L. Moon

" ri_min="1.7" ri_max="2.1500001" shape="Octagon" footer1="Entered into GCS, filename, shapename, cut sequence and meetpoints revised by Kevin Kane kane2002@telus.net

" footer2="L1000 h0724 f085 i064 b83 ri170-215 Octagon pc21086F FVS-044 Octabar-X by Van Sant, Fred W"/>
</GemCutStudio>
"#;

/// A `.gem` goes through the same `.asc` import path: every tier lands in the
/// angle table, the generated `.asc` and the original `.gem` bytes are both
/// attached, and the row is keyed by the `.gem`'s own file name.
#[test]
fn import_path_imports_a_synthetic_gem_through_the_asc_path() {
    let dir = temp_dir_for_test("gem_import");
    let gem_bytes = encode_gem(SYNTHETIC_GEM_TIERS, 96, "Synthetic Gem");
    std::fs::write(dir.join("synthetic.gem"), &gem_bytes).expect("write synthetic.gem");

    let db_path = temp_db_path_for_test("gem_import");
    let db = open_temp_db(&db_path);
    let outcome = import_path(&db, &dir, false, |_, _| {});
    assert_eq!(
        outcome.imported_ids.len(),
        1,
        "summary: {}",
        outcome.summary
    );
    assert!(!outcome.had_failures, "summary: {}", outcome.summary);
    assert!(
        outcome.summary.contains("Imported 1 file(s)"),
        "summary: {}",
        outcome.summary
    );

    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(outcome.imported_ids[0])
        .expect("query must succeed")
        .expect("row must exist");
    assert_eq!(full.url, "local://synthetic.gem");
    assert_eq!(full.title, "Synthetic Gem");
    assert_eq!(full.angle_settings.len(), SYNTHETIC_GEM_TIERS.len());
    let names: Vec<&str> = full
        .attached_files
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, vec!["synthetic.asc", "synthetic.gem"]);
    assert_eq!(full.attached_files[1].content, gem_bytes);
    let generated_text = String::from_utf8(full.attached_files[0].content.clone())
        .expect("the generated .asc is text");
    let generated =
        indicatrix_formats::asc::parse_asc(&generated_text).expect("the generated .asc must parse");
    assert_eq!(generated.tiers.len(), SYNTHETIC_GEM_TIERS.len());
    assert_eq!(generated.gear_teeth, 96);
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// A `.gcs` imports too, and its converter warnings (here: a hidden tier) are
/// listed in the summary and keep the popup open.
#[test]
fn import_path_imports_a_gcs_and_lists_its_converter_notes() {
    let dir = temp_dir_for_test("gcs_import");
    let hidden = GCS_OCTABAR_X_EXCERPT.replace("visible=\"true\"", "visible=\"false\"");
    std::fs::write(dir.join("octabar.gcs"), hidden).expect("write octabar.gcs");

    let db_path = temp_db_path_for_test("gcs_import");
    let db = open_temp_db(&db_path);
    let outcome = import_path(&db, &dir, false, |_, _| {});
    assert_eq!(
        outcome.imported_ids.len(),
        1,
        "summary: {}",
        outcome.summary
    );
    assert!(outcome.had_notes, "summary: {}", outcome.summary);
    assert!(
        outcome
            .summary
            .contains("Converted with notes: octabar.gcs:")
            && outcome.summary.contains("hidden"),
        "summary: {}",
        outcome.summary
    );

    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(outcome.imported_ids[0])
        .expect("query must succeed")
        .expect("row must exist");
    assert_eq!(full.title, "FVS-044 Octabar-X PC 21.086F");
    assert_eq!(full.angle_settings.len(), 1);
    assert_eq!(full.index_gear.as_deref(), Some("64"));
    assert!(
        full.attached_files.iter().any(|f| f.name == "octabar.gcs"),
        "the original .gcs must be attached"
    );
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// The standard round brilliant as `.gcs` text, with `frosting` stated (tier positions and
/// facet slots, as `Design::gcs_frosting_spec` gives them).
fn round_brilliant_gcs(frosting: &[(usize, GcsTierFrosting)]) -> String {
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let schedule = design.to_asc_schedule().expect("the design exports");
    to_gcs_string_with_frosting(&schedule, frosting).expect("the schedule writes as .gcs")
}

/// The frosting the library stored for the one design `import_path` saved from `dir`.
fn imported_frosting(
    db: &std::sync::Arc<std::sync::Mutex<indicatrix_vault::db::sqlite::Database>>,
    dir: &std::path::Path,
) -> (Vec<StoredTierFrosting>, FullDiagramRecord) {
    let outcome = import_path(db, dir, false, |_, _| {});
    assert_eq!(
        outcome.imported_ids.len(),
        1,
        "summary: {}",
        outcome.summary
    );
    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(outcome.imported_ids[0])
        .expect("query must succeed")
        .expect("row must exist");
    let stored = conn
        .diagram_frosting(outcome.imported_ids[0])
        .expect("the frosting reads");
    assert_eq!(full.frosting, stored, "the record carries the stored rows");
    drop(conn);
    (stored, full)
}

/// A `.gcs` that frosts a whole tier and two facets of another keeps both through the
/// library import (the generated `.asc` cannot): the row stores them, the stone the library
/// draws names the frosted planes, a polished `.gcs` stores and draws none, and re-importing
/// the same name as a polished file clears them again.
#[test]
fn import_path_stores_the_frosting_a_gcs_states_and_a_reimport_replaces_it() {
    let stated = vec![
        (1, GcsTierFrosting::Whole),
        (3, GcsTierFrosting::Facets(vec![1, 3])),
    ];
    let dir = temp_dir_for_test("gcs_frosting");
    std::fs::write(dir.join("rb.gcs"), round_brilliant_gcs(&stated)).expect("write rb.gcs");
    let db_path = temp_db_path_for_test("gcs_frosting");
    let db = open_temp_db(&db_path);

    let (stored, full) = imported_frosting(&db, &dir);
    assert_eq!(
        stored,
        vec![
            StoredTierFrosting::whole(1),
            StoredTierFrosting {
                tier: 3,
                facets: vec![1, 3]
            },
        ]
    );
    let (planes, frosted) = crate::gui::editor::resolve_catalogue_planes_with_frosting(&full);
    // Two facets of tier 3 and every facet of tier 1: more than the two single facets.
    assert!(frosted.len() > 2, "frosted planes: {frosted:?}");
    assert!(
        frosted
            .iter()
            .all(|&index| (index as usize) < planes.planes.len()
                && (index as usize) >= planes.preform_plane_count),
        "frosted planes are facet planes of the stone: {frosted:?}"
    );

    // The same file name again, now polished: the row is replaced and so is its frosting.
    std::fs::write(dir.join("rb.gcs"), round_brilliant_gcs(&[])).expect("rewrite rb.gcs");
    let (stored, full) = imported_frosting(&db, &dir);
    assert!(
        stored.is_empty(),
        "a polished re-import clears the frosting"
    );
    let (_, frosted) = crate::gui::editor::resolve_catalogue_planes_with_frosting(&full);
    assert_eq!(frosted, [] as [u32; 0]);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// A polished `.gcs`, an `.asc` and a `.gem` store no frosting at all: no row in the side
/// table, and the design rows are what they were.
#[test]
fn import_path_stores_no_frosting_for_a_design_without_any() {
    let dir = temp_dir_for_test("no_frosting");
    std::fs::write(dir.join("plain.gcs"), round_brilliant_gcs(&[])).expect("write plain.gcs");
    std::fs::write(dir.join("plain.asc"), VALID_ASC).expect("write plain.asc");
    std::fs::write(
        dir.join("plain.gem"),
        encode_gem(SYNTHETIC_GEM_TIERS, 96, "Plain Gem"),
    )
    .expect("write plain.gem");
    let db_path = temp_db_path_for_test("no_frosting");
    let db = open_temp_db(&db_path);

    let outcome = import_path(&db, &dir, false, |_, _| {});
    assert_eq!(
        outcome.imported_ids.len(),
        3,
        "summary: {}",
        outcome.summary
    );
    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for &id in &outcome.imported_ids {
        assert_eq!(
            conn.diagram_frosting(id).expect("reads"),
            [] as [indicatrix_vault::model::frosting::StoredTierFrosting; 0]
        );
        assert_eq!(
            conn.get_diagram_full(id)
                .expect("reads")
                .expect("a row")
                .frosting,
            [] as [indicatrix_vault::model::frosting::StoredTierFrosting; 0]
        );
    }
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// A corrupt `.gem` is one rejected file with the reader's typed message, never a
/// panic, and the rest of the batch still imports and is summarised.
#[test]
fn import_path_rejects_a_corrupt_gem_per_file_and_still_summarises() {
    let dir = temp_dir_for_test("gem_corrupt");
    std::fs::write(dir.join("good.asc"), VALID_ASC).expect("write good.asc");
    std::fs::write(dir.join("corrupt.gem"), [1_u8, 2, 3]).expect("write corrupt.gem");

    let db_path = temp_db_path_for_test("gem_corrupt");
    let db = open_temp_db(&db_path);
    let outcome = import_path(&db, &dir, false, |_, _| {});
    assert_eq!(
        outcome.imported_ids.len(),
        1,
        "summary: {}",
        outcome.summary
    );
    assert!(outcome.had_failures, "summary: {}", outcome.summary);
    assert!(
        outcome.summary.contains("Imported 1 file(s); 1 skipped")
            && outcome
                .summary
                .contains("corrupt.gem (parse error: not a readable .gem file: byte 0"),
        "summary: {}",
        outcome.summary
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// A directly picked `.gem` is a candidate like a `.asc`, not a rejection, and an
/// untitled design is titled after the file stem.
#[test]
fn import_path_accepts_a_directly_picked_gem_file() {
    let dir = temp_dir_for_test("gem_single");
    let gem_path = dir.join("single.gem");
    std::fs::write(&gem_path, encode_gem(SYNTHETIC_GEM_TIERS, 96, "")).expect("write single.gem");

    let db_path = temp_db_path_for_test("gem_single");
    let db = open_temp_db(&db_path);
    let outcome = import_path(&db, &gem_path, false, |_, _| {});
    assert_eq!(
        outcome.imported_ids.len(),
        1,
        "summary: {}",
        outcome.summary
    );
    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(outcome.imported_ids[0])
        .expect("query must succeed")
        .expect("row must exist");
    assert_eq!(full.title, "single");
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}
