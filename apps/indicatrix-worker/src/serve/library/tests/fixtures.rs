//! Shared test fixtures for `serve::library`'s tests: a populated throwaway database
//! every topic file in this folder seeds its assertions against.

use indicatrix_vault::{
    db::sqlite::Database,
    model::{detail::FacetingDiagramDetail, entry::FacetingDiagramEntry, file::AttachedFile},
};

/// Builds a fresh, populated temp database (read-write) and returns the path;
/// callers reopen it `Database::open_read_only`. Tests never touch
/// `facet_diagrams.sqlite`, only their own throwaway temp files.
pub(super) fn populated_temp_db() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "indicatrix-worker-library-test-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path_str = path.to_str().unwrap();
    let db = Database::new(Some(path_str)).unwrap();

    let entry_id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Round Brilliant".to_string(),
                url: "https://example.test/diagram/1".to_string(),
                design_id: "RB-1".to_string(),
            },
            "facetdiagrams.org",
        )
        .unwrap();

    let mut detail = FacetingDiagramDetail {
        page_url: "https://example.test/diagram/1".to_string(),
        shape: Some("Round".to_string()),
        refractive_index: Some("2.417".to_string()),
        attached_files: vec![AttachedFile {
            name: "schedule.pdf".to_string(),
            url: "https://example.test/schedule.pdf".to_string(),
            content: vec![1, 2, 3, 4, 5],
        }],
        ..Default::default()
    };
    detail.angle_settings_table = vec![indicatrix_vault::model::angle::AngleSetting {
        order_index: 0,
        facet: "P1".to_string(),
        angle: "41.0".to_string(),
        index: "96".to_string(),
        notes: String::new(),
        ..Default::default()
    }];
    db.save_diagram_detail(&detail, entry_id).unwrap();

    path
}
