//! `save_diagram_detail`/`update_diagram_metadata`/`get_derived_from_title`: a design's
//! full detail row round-tripping its designer-split/competition fields, and the trap
//! `update_diagram_metadata` exists to avoid (a narrow edit must never zero a column
//! outside `MetadataUpdate` or disturb child rows).

use super::{super::*, fixtures::temp_db_path};

/// The five new fields must survive a `save_diagram_detail` round trip into their
/// typed columns -- `shape_category` is `Option<String>` on `FacetDiagramDetail`
/// bound into an INTEGER column, so this pins down it lands as a number, not text.
#[test]
fn save_diagram_detail_persists_the_designer_split_and_competition_fields() {
    let path = temp_db_path("designer_split_roundtrip");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Utopia".to_string(),
        url: "https://facetdiagrams.org/diagramus/utopia/".to_string(),
        design_id: String::new(),
    };
    let entry_id = db
        .save_diagram_entry(&entry, LEGACY_SOURCE_ID)
        .expect("save entry");
    let detail = FacetDiagramDetail {
        designer_info: Some("Capps, Jerry; Lapidary Journal, May 1994, p95".to_string()),
        designer: Some("Capps, Jerry".to_string()),
        source_citation: Some("Lapidary Journal, May 1994, p95".to_string()),
        pdf_file: Some("2002SSCMasters.pdf".to_string()),
        gem_file: None,
        shape_category: Some("5".to_string()),
        ..Default::default()
    };
    db.save_diagram_detail(&detail, entry_id)
        .expect("save detail");

    let (designer, citation, pdf, gem, category) = db
        .conn
        .query_row(
            "SELECT designer, source_citation, pdf_file, gem_file, shape_category
                 FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .expect("read back detail");

    assert_eq!(designer.as_deref(), Some("Capps, Jerry"));
    assert_eq!(citation.as_deref(), Some("Lapidary Journal, May 1994, p95"));
    assert_eq!(pdf.as_deref(), Some("2002SSCMasters.pdf"));
    assert_eq!(gem, None);
    // Read back as an integer, not the "5" string that went in.
    assert_eq!(category, Some(5));

    let _ = std::fs::remove_file(&path);
}

/// `get_derived_from_title` must resolve the recorded source row's own id and
/// title,
/// return `None` when nothing is recorded yet, and return `None` -- not an
/// error -- when the recorded source row has since been deleted (no
/// `FOREIGN KEY` backs this column; see `migrate_diagram_entries_provenance`).
#[test]
fn get_derived_from_title_resolves_the_source_rows_id_and_title() {
    let path = temp_db_path("derived_from_title");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let source = FacetDiagramEntry {
        title: "Original Round Brilliant".to_string(),
        url: "local://original.asc".to_string(),
        design_id: String::new(),
    };
    let source_id = db
        .save_diagram_entry(&source, LEGACY_SOURCE_ID)
        .expect("save source entry");

    let derived = FacetDiagramEntry {
        title: "Original Round Brilliant (edited)".to_string(),
        url: "local://original-edited.asc".to_string(),
        design_id: String::new(),
    };
    let derived_id = db
        .save_diagram_entry(&derived, LEGACY_SOURCE_ID)
        .expect("save derived entry");

    // Nothing recorded yet.
    assert_eq!(db.get_derived_from_title(derived_id).unwrap(), None);

    db.set_derived_from_entry_id(derived_id, Some(source_id))
        .expect("record provenance");
    assert_eq!(
        db.get_derived_from_title(derived_id).unwrap(),
        Some((source_id, "Original Round Brilliant".to_string()))
    );

    // A dangling reference (source since deleted) must read back as `None`.
    db.conn
        .execute(
            "DELETE FROM diagram_entries WHERE id = ?1",
            params![source_id],
        )
        .expect("delete source row");
    assert_eq!(db.get_derived_from_title(derived_id).unwrap(), None);

    let _ = std::fs::remove_file(&path);
}

/// Every `diagram_details` column [`MetadataUpdate`] does NOT cover -- must survive a
/// metadata edit byte-for-byte, including fields `FullDiagramRecord` can't even see.
/// See `update_diagram_metadata`'s doc comment for the trap this guards against.
type UntouchedDetailColumnsRow = (
    String,          // page_url
    Option<String>,  // diagram_image_name
    Option<Vec<u8>>, // diagram_image_data
    Option<String>,  // competition_diagram
    Option<f64>,     // tw_ratio
    Option<f64>,     // uw_ratio
    Option<String>,  // designer
    Option<String>,  // source_citation
    Option<String>,  // pdf_file
    Option<String>,  // gem_file
    Option<i64>,     // shape_category
);

/// `(hw_ratio, cw_ratio, pw_ratio, symmetry_order, mirror_symmetry)` spot-check row.
type RatioAndSymmetrySpotCheckRow = (
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
    Option<bool>,
);

fn read_untouched_detail_columns(db: &Database, entry_id: i64) -> UntouchedDetailColumnsRow {
    db.conn
        .query_row(
            "SELECT page_url, diagram_image_name, diagram_image_data, competition_diagram,
                    tw_ratio, uw_ratio, designer, source_citation, pdf_file, gem_file, shape_category
             FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            },
        )
        .expect("read untouched diagram_details columns")
}

/// Pins down the trap `update_diagram_metadata` exists to avoid: a fully-populated
/// detail row gets one narrow call that changes exactly two fields (`shape`,
/// `refractive_index`) and resubmits every other `MetadataUpdate` field unchanged,
/// as a pre-filled editor form would. Every column outside `MetadataUpdate` --
/// and `angle_settings`/`attached_files` entirely -- must come back byte-for-byte
/// identical; a regression to delete-and-reinsert would zero those columns or
/// change child rows' ids, either of which this test catches.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one round trip through a fully-populated detail row, a narrow edit, and \
                  every 'must still equal what it started as' assertion; splitting it \
                  would separate the setup from the assertions it's checking"
)]
fn update_diagram_metadata_touches_only_its_own_fields_and_nothing_else() {
    let path = temp_db_path("metadata_update_narrow");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let entry = FacetDiagramEntry {
        title: "Utopia".to_string(),
        url: "https://facetdiagrams.org/diagramus/utopia-narrow/".to_string(),
        design_id: String::new(),
    };
    let entry_id = db
        .save_diagram_entry(&entry, LEGACY_SOURCE_ID)
        .expect("save entry");

    let original = FacetDiagramDetail {
        page_url: "https://facetdiagrams.org/diagramus/utopia-narrow/".to_string(),
        diagram_image_name: Some("utopia.svg".to_string()),
        diagram_image_data: Some(vec![1, 2, 3, 4]),
        angle_settings_table: vec![crate::model::angle::AngleSetting {
            order_index: 0,
            facet: "T".to_string(),
            angle: "0".to_string(),
            index: "-".to_string(),
            notes: String::new(),
        }],
        attached_files: vec![crate::model::file::AttachedFile {
            name: "utopia.asc".to_string(),
            url: String::new(),
            content: b"original bytes, must survive untouched".to_vec(),
        }],
        competition_diagram: Some("2002SSCMasters".to_string()),
        lw_ratio: Some("1.05".to_string()),
        refractive_index: Some("2.417".to_string()),
        index_gear: Some("96".to_string()),
        volume: Some("0.42".to_string()),
        facets_count: Some("57+8".to_string()),
        shape: Some("Round".to_string()),
        designer_info: Some("Capps, Jerry; Lapidary Journal, May 1994, p95".to_string()),
        hw_ratio: Some("0.61".to_string()),
        tw_ratio: Some("0.55".to_string()),
        uw_ratio: Some("0.12".to_string()),
        pw_ratio: Some("0.44".to_string()),
        cw_ratio: Some("0.17".to_string()),
        symmetry_order: Some("8".to_string()),
        mirror_symmetry: Some(true),
        designer: Some("Capps, Jerry".to_string()),
        source_citation: Some("Lapidary Journal, May 1994, p95".to_string()),
        pdf_file: Some("2002SSCMasters.pdf".to_string()),
        gem_file: Some("utopia.gem".to_string()),
        shape_category: Some("5".to_string()),
    };
    db.save_diagram_detail(&original, entry_id)
        .expect("save original detail");

    let before = read_untouched_detail_columns(&db, entry_id);
    let detail_id: i64 = db
        .conn
        .query_row(
            "SELECT id FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| r.get(0),
        )
        .unwrap();
    let angle_count_before: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM angle_settings WHERE detail_id = ?1",
            params![detail_id],
            |r| r.get(0),
        )
        .unwrap();
    let (attachment_id_before, attachment_content_before): (i64, Vec<u8>) = db
        .conn
        .query_row(
            "SELECT id, content FROM attached_files WHERE detail_id = ?1",
            params![detail_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    let update = MetadataUpdate {
        designer_info: original.designer_info.clone(),
        shape: Some("Oval".to_string()),            // the actual edit
        refractive_index: Some("1.76".to_string()), // the actual edit
        index_gear: original.index_gear.clone(),
        facets_count: original.facets_count.clone(),
        symmetry_order: original.symmetry_order.clone(),
        mirror_symmetry: original.mirror_symmetry,
        lw_ratio: original.lw_ratio.clone(),
        hw_ratio: original.hw_ratio.clone(),
        cw_ratio: original.cw_ratio.clone(),
        pw_ratio: original.pw_ratio.clone(),
        volume: original.volume.clone(),
    };
    db.update_diagram_metadata(entry_id, &update)
        .expect("update metadata");

    let full = db.get_diagram_full(entry_id).unwrap().unwrap();
    assert_eq!(full.shape.as_deref(), Some("Oval"));
    assert_eq!(full.refractive_index.as_deref(), Some("1.76"));

    // Resubmitted MetadataUpdate fields must still read back unchanged.
    assert_eq!(full.designer_info, original.designer_info);
    assert_eq!(full.index_gear, original.index_gear);
    assert_eq!(full.facets_count, original.facets_count);
    assert_eq!(full.lw_ratio, original.lw_ratio);
    let (hw, cw, pw, sym, mirror): RatioAndSymmetrySpotCheckRow = db
        .conn
        .query_row(
            "SELECT hw_ratio, cw_ratio, pw_ratio, symmetry_order, mirror_symmetry
             FROM diagram_details WHERE entry_id = ?1",
            params![entry_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert!((hw.unwrap() - 0.61).abs() < 1e-9);
    assert!((cw.unwrap() - 0.17).abs() < 1e-9);
    assert!((pw.unwrap() - 0.44).abs() < 1e-9);
    assert_eq!(sym, Some(8));
    assert_eq!(mirror, Some(true));

    // Every column outside MetadataUpdate must be byte-for-byte identical to before.
    assert_eq!(
        read_untouched_detail_columns(&db, entry_id),
        before,
        "update_diagram_metadata must not touch any diagram_details column outside MetadataUpdate"
    );

    // Never a delete-and-reinsert of children: same row count, same attachment id/bytes.
    let angle_count_after: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM angle_settings WHERE detail_id = ?1",
            params![detail_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(angle_count_after, angle_count_before);
    let (attachment_id_after, attachment_content_after): (i64, Vec<u8>) = db
        .conn
        .query_row(
            "SELECT id, content FROM attached_files WHERE detail_id = ?1",
            params![detail_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        attachment_id_after, attachment_id_before,
        "attached_files row must not be deleted and reinserted (its id would change)"
    );
    assert_eq!(attachment_content_after, attachment_content_before);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn update_diagram_metadata_rejects_unknown_entry_id() {
    let path = temp_db_path("metadata_update_unknown");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let result = db.update_diagram_metadata(999_999, &MetadataUpdate::default());
    assert!(result.is_err());
    let _ = std::fs::remove_file(&path);
}
