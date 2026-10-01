//! Catalogue records and assertions shared by the preview and tilt batch tests of
//! the design-file-first geometry rule (`super::record_planes_for_batch`).

use crate::gui::library::local::test_gem::{SYNTHETIC_GEM_TIERS, encode_gem};
use indicatrix::geometry::{GpuFacetPlane, cuts::FacetSpec};
use indicatrix_vault::model::{angle::AngleSetting, entry::FullDiagramRecord, file::AttachedFile};

/// The entry id every record here carries.
pub const ENTRY_ID: i64 = 7;

/// A one-tier `.asc` whose pavilion mast (`0.64991234`) the angle table below
/// cannot know.
pub const ASC_TEXT: &str =
    "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";

/// [`ASC_TEXT`]'s one recorded mast.
pub const ASC_MAST: f32 = 0.649_912_34;

/// A record with the synthetic design's angle table and no attachment -- a
/// scraped catalogue detail with no design file.
pub fn angle_table_record() -> FullDiagramRecord {
    let angle_settings = SYNTHETIC_GEM_TIERS
        .iter()
        .zip(0..)
        .map(
            |(&(name, angle, _mast, indices), order_index)| AngleSetting {
                order_index,
                facet: name.to_string(),
                angle: format!("{}", angle.abs()),
                index: indices
                    .iter()
                    .map(|i| format!("{i:02}"))
                    .collect::<Vec<_>>()
                    .join("-"),
                notes: String::new(),
            },
        )
        .collect();
    FullDiagramRecord {
        entry_id: ENTRY_ID,
        title: "Synthetic".to_string(),
        url: "local://synthetic.asc".to_string(),
        design_id: None,
        page_url: String::new(),
        diagram_image_name: None,
        diagram_image_data: None,
        competition_diagram: None,
        lw_ratio: None,
        refractive_index: Some("1.54".to_string()),
        index_gear: Some("96".to_string()),
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
        angle_settings,
        attached_files: Vec::new(),
    }
}

/// [`angle_table_record`] plus one attachment.
pub fn record_with(name: &str, content: Vec<u8>) -> FullDiagramRecord {
    let mut full = angle_table_record();
    full.attached_files.push(AttachedFile {
        name: name.to_string(),
        url: String::new(),
        content,
    });
    full
}

/// [`angle_table_record`] plus the synthetic design as its only design file, a
/// `.gem`.
pub fn gem_record() -> FullDiagramRecord {
    record_with(
        "Synthetic.GEM",
        encode_gem(SYNTHETIC_GEM_TIERS, 96, "Synthetic"),
    )
}

/// The planes the angle table alone builds for `full` -- the batches' geometry
/// before design files were read.
pub fn angle_table_planes(full: &FullDiagramRecord) -> Vec<GpuFacetPlane> {
    let facet_specs: Vec<FacetSpec> = full
        .angle_settings
        .iter()
        .map(|a| FacetSpec {
            facet: a.facet.clone(),
            angle: a.angle.clone(),
            index: a.index.clone(),
            notes: a.notes.clone(),
        })
        .collect();
    crate::gui::library::detail::reconstruct_planes(
        full.shape.as_deref(),
        full.index_gear.as_deref(),
        &facet_specs,
    )
}

/// Whether some plane lies at distance `mast` from the origin.
pub fn has_plane_at(planes: &[GpuFacetPlane], mast: f32) -> bool {
    planes.iter().any(|p| (p.d + mast).abs() < 1e-4)
}

/// Asserts `planes` are the synthetic `.gem`'s real geometry: every plane keeps
/// the origin strictly inside (no zero-mast placeholder) and every tier's
/// recorded mast is present.
pub fn assert_gem_geometry(planes: &[GpuFacetPlane]) {
    assert!(!planes.is_empty(), "the .gem must yield planes");
    assert!(
        planes.iter().all(|p| p.d < -1e-6),
        "every plane must have a non-zero mast"
    );
    for &(name, _angle, mast, _indices) in SYNTHETIC_GEM_TIERS {
        let mast = mast as f32;
        assert!(
            has_plane_at(planes, mast),
            "tier {name}'s mast {mast} must be among the planes"
        );
    }
}
