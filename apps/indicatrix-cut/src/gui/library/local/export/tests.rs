//! Tests for the Library export callbacks: the concave-row notices, the `.gcs` and `.asc`
//! export of a stored record and the file-name sanitiser.

use super::*;
use indicatrix_vault::model::{angle::AngleSetting, entry::FullDiagramRecord, file::AttachedFile};

/// One stored schedule row; a concave row carries a tool name and its tool line.
fn schedule_row(
    order_index: u32,
    facet: &str,
    angle: &str,
    index: &str,
    notes: &str,
    tool: Option<(&str, Option<&str>)>,
) -> AngleSetting {
    AngleSetting {
        order_index,
        facet: facet.to_string(),
        angle: angle.to_string(),
        index: index.to_string(),
        notes: notes.to_string(),
        tool: tool.map(|(name, _)| name.to_string()),
        tool_line: tool.and_then(|(_, line)| line).map(str::to_string),
    }
}

/// The reconstructed `.asc` keeps a concave row as the two footnote lines the editor's
/// export writes (the same lines `Design::append_concave_footnotes` pins in cut-core's own
/// tests), and leaves the flat rows as tiers.
#[test]
fn concave_rows_become_the_footnote_pair_the_editor_export_writes() {
    let mut full = record_with(Vec::new());
    full.angle_settings = vec![
        schedule_row(0, "P1", "41.00", "3", "", None),
        schedule_row(
            1,
            "Groove",
            "-62.00",
            "3 11.50 19",
            "cut to depth",
            Some((
                "Cylinder",
                Some("CYL  +90.00°  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating"),
            )),
        ),
        // No instructions and no stored tool line: one footnote, no trailing spaces.
        schedule_row(2, "Pit", "40.00", "0", "", Some(("Cone", None))),
        // A line break in the notes must not reach the file.
        schedule_row(3, "Notch", "12.00", "5", "by\r\nhand", Some(("Ball", None))),
    ];
    let (flat, footnotes) = split_concave_rows(&full);
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0].facet, "P1");
    assert_eq!(
        footnotes,
        vec![
            "Groove  -62.00  3 11.50 19  cut to depth  [concave tier]".to_string(),
            "CYL  +90.00 deg  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating  \
             [concave tier]"
                .to_string(),
            "Pit  40.00  0  [concave tier]".to_string(),
            "Notch  12.00  5  by  hand  [concave tier]".to_string(),
        ]
    );
    assert!(footnotes.iter().all(|line| line.is_ascii()));
}

#[test]
fn sanitize_filename_replaces_reserved_characters() {
    assert_eq!(
        sanitize_filename("Round: Trichecker/12 \"Special\""),
        "Round_ Trichecker_12 _Special_"
    );
}

#[test]
fn sanitize_filename_falls_back_for_empty_or_blank_titles() {
    assert_eq!(sanitize_filename(""), "design");
    assert_eq!(sanitize_filename("   "), "design");
}

/// A catalogue record with no metadata and the given attachments.
fn record_with(attached_files: Vec<AttachedFile>) -> FullDiagramRecord {
    FullDiagramRecord {
        entry_id: 1,
        title: "Test Design".to_string(),
        url: "local://test.asc".to_string(),
        design_id: None,
        page_url: String::new(),
        diagram_image_name: None,
        diagram_image_data: None,
        competition_diagram: None,
        lw_ratio: None,
        refractive_index: None,
        index_gear: None,
        volume: None,
        facets_count: None,
        shape: None,
        designer_info: None,
        hw_ratio: None,
        tw_ratio: None,
        uw_ratio: None,
        pw_ratio: None,
        cw_ratio: None,
        symmetry_order: None,
        mirror_symmetry: None,
        designer: None,
        source_citation: None,
        pdf_file: None,
        gem_file: None,
        shape_category: None,
        angle_settings: Vec::new(),
        attached_files,
        frosting: Vec::new(),
    }
}

/// The standard round brilliant's cutting instructions as `.asc` text, from
/// the first New Design template.
fn standard_round_brilliant_asc() -> (String, usize) {
    let spec = indicatrix_cut_core::templates::TEMPLATES
        .first()
        .expect("TEMPLATES is non-empty");
    assert_eq!(spec.name, "Standard Round Brilliant");
    let design = indicatrix_cut_core::design::Design::new(
        indicatrix_cut_core::preform::PreformSpec::cylinder(
            spec.gear_teeth.unsigned_abs() as usize,
            1.5,
            1.0,
            1.5,
        ),
        spec.schedule_meta(),
        spec.tiers(),
    );
    let schedule = design
        .to_asc_schedule()
        .expect("the standard round brilliant solves");
    let text = indicatrix_formats::asc::to_asc_string(&schedule).expect("writes as .asc");
    (text, schedule.tiers.len())
}

/// A `.asc` attachment holding the original Windows-1252 bytes (not valid UTF-8)
/// reads as text through the reader every design-file consumer here shares.
#[test]
fn a_windows_1252_asc_attachment_reads_as_text() {
    let raw =
        b"GemCad 5.0\ng 96 0.0\ny 6 y\nI 1.72\nH Caf\xE9 \xB0\na 41.000000 0.5 92 n T\n".to_vec();
    assert!(std::str::from_utf8(&raw).is_err());
    let full = record_with(vec![AttachedFile {
        name: "cafe.asc".to_string(),
        url: String::new(),
        content: raw,
    }]);
    let file = design_attachment_as_asc(&full)
        .expect("the record has a design file")
        .expect("a .asc always reads as text");
    assert_eq!(file.asc_file_name, "cafe.asc");
    assert!(
        file.asc_text.contains("Caf\u{e9} \u{b0}"),
        "{:?}",
        file.asc_text
    );
}

/// The catalogue `.gcs` export of the standard round brilliant parses back
/// with the `.gcs` reader and keeps every tier.
#[test]
fn gcs_export_of_the_standard_round_brilliant_record_parses_back() {
    let (asc_text, tier_count) = standard_round_brilliant_asc();
    let full = record_with(vec![AttachedFile {
        name: "round.asc".to_string(),
        url: String::new(),
        content: asc_text.into_bytes(),
    }]);
    let export = gcs_export_for_record(&full).expect("a solved design exports");
    assert_eq!(export.file_name, "round.gcs");
    assert!(!export.is_original);
    let parsed =
        indicatrix_formats::gcs::parse_gcs_bytes(&export.bytes).expect("the .gcs parses back");
    assert_eq!(parsed.tiers.len(), tier_count);
}

/// A design imported from a `.gcs` exports that original file unchanged.
#[test]
fn gcs_export_prefers_the_original_gcs_attachment() {
    let full = record_with(vec![
        AttachedFile {
            name: "octabar.asc".to_string(),
            url: String::new(),
            content: b"GemCad 5.0\n".to_vec(),
        },
        AttachedFile {
            name: "octabar.gcs".to_string(),
            url: String::new(),
            content: b"<GemCutStudio version=\"1000\"/>".to_vec(),
        },
    ]);
    let export = gcs_export_for_record(&full).expect("the original is exported");
    assert!(export.is_original);
    assert_eq!(export.file_name, "octabar.gcs");
    assert_eq!(export.bytes, b"<GemCutStudio version=\"1000\"/>");
}

/// Without an attached `.asc` there are no masts, so no `.gcs` is built.
#[test]
fn gcs_export_refuses_a_design_with_no_attached_asc() {
    let err = gcs_export_for_record(&record_with(Vec::new()))
        .expect_err("no attached .asc means no masts");
    assert!(err.contains("no attached .asc"), "{err}");
}

/// A frosted entry's `.asc` export names the loss (the editor's wording, singular and
/// plural); a polished entry says nothing, and the notice titles follow what is lost.
#[test]
fn a_frosted_entrys_asc_export_names_the_frosting_it_drops() {
    use indicatrix_vault::model::frosting::StoredTierFrosting;

    let mut full = record_with(Vec::new());
    assert_eq!(frosting_export_notice(&full), None);
    assert_eq!(loss_notice(None, None), None);

    full.frosting = vec![StoredTierFrosting::whole(1)];
    let one = frosting_export_notice(&full).expect("a frosted entry warns");
    assert!(one.contains("1 frosted tier will"), "{one}");
    assert!(one.contains("no notation for frosting"), "{one}");

    full.frosting.push(StoredTierFrosting {
        tier: 3,
        facets: vec![0, 2],
    });
    let two = frosting_export_notice(&full).expect("a frosted entry warns");
    assert!(two.contains("2 frosted tiers will"), "{two}");

    let (title, text) = loss_notice(None, Some(two.clone())).expect("a notice");
    assert_eq!(title, "Frosting Is Not Part of This Format");
    assert_eq!(text, two);
    let (title, text) = loss_notice(Some("c".to_string()), None).expect("a notice");
    assert_eq!(title, "Concave Tiers Are Not Part of This Format");
    assert_eq!(text, "c");
    let (title, text) =
        loss_notice(Some("c".to_string()), Some("f".to_string())).expect("a notice");
    assert_eq!(title, "Some Content Is Not Part of This Format");
    assert_eq!(text, "c\nf");
}
