use crate::model::{angle::AngleSetting, file::AttachedFile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacetDiagramEntry {
    pub title: String,
    pub url: String,
    pub design_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramListItem {
    pub id: i64,
    pub title: String,
    pub url: String,
    pub design_id: Option<String>,
    pub shape: Option<String>,
    pub index_gear: Option<String>,
    pub facets_count: Option<String>,
    pub designer_info: Option<String>,
    pub lw_ratio: Option<String>,
    pub refractive_index: Option<String>,
    pub volume: Option<String>,
    pub competition_diagram: Option<String>,
    /// Whether the user has marked this design ignored (`diagram_entries.ignored`).
    /// Carried on the row itself rather than left for a caller to discover by running
    /// the same search twice (with `RangeFilter::include_ignored` on and off) and
    /// diffing id sets, which would double every search's query cost.
    pub ignored: bool,
}

/// One attachment's metadata -- id, name, url, and byte size -- WITHOUT its content.
///
/// Counterpart of [`crate::model::file::AttachedFile`] for a caller that needs to know
/// what attachments a design has without loading every one's bytes; see
/// [`crate::db::sqlite::Database::get_diagram_full_meta`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedFileMeta {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub size: i64,
}

/// The same record as [`FullDiagramRecord`], but without attachment content.
///
/// [`Self::attached_files`] carries each attachment's METADATA only, never its
/// `content` -- this query never selects `content` at all, so it doesn't pay to load
/// it. See [`crate::db::sqlite::Database::get_diagram_full_meta`].
///
/// Carries `hw_ratio`/`tw_ratio`/`uw_ratio`/`pw_ratio`/`cw_ratio`/`symmetry_order`/
/// `mirror_symmetry`/`designer`: `indicatrix_net::library::DesignRecord` is built from
/// this type (never [`FullDiagramRecord`], since it must never carry attachment
/// content), so dropping these fields here would blank the corresponding
/// `apps/indicatrix-cut` detail-pane chips for every remote-browsed design.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullDiagramMeta {
    pub entry_id: i64,
    pub title: String,
    pub url: String,
    pub design_id: Option<String>,
    pub page_url: String,
    pub diagram_image_name: Option<String>,
    pub diagram_image_data: Option<Vec<u8>>,
    pub competition_diagram: Option<String>,
    pub lw_ratio: Option<String>,
    pub refractive_index: Option<String>,
    pub index_gear: Option<String>,
    pub volume: Option<String>,
    pub facets_count: Option<String>,
    pub shape: Option<String>,
    pub designer_info: Option<String>,
    pub hw_ratio: Option<String>,
    pub tw_ratio: Option<String>,
    pub uw_ratio: Option<String>,
    pub pw_ratio: Option<String>,
    pub cw_ratio: Option<String>,
    pub symmetry_order: Option<String>,
    pub mirror_symmetry: Option<bool>,
    pub designer: Option<String>,
    pub angle_settings: Vec<AngleSetting>,
    pub attached_files: Vec<AttachedFileMeta>,
}

/// Carries every `diagram_details` column [`crate::model::detail::FacetDiagramDetail`] has.
///
/// Must stay in sync with that struct: a caller building a fresh `FacetDiagramDetail`
/// from a `FullDiagramRecord` (to feed `Database::save_diagram_detail`, which fully
/// replaces a design's detail row) would silently zero any field missing here on every
/// such save -- see `Database::update_diagram_metadata`'s doc comment for the
/// narrow-`UPDATE` method that exists so a metadata edit never needs that path at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullDiagramRecord {
    pub entry_id: i64,
    pub title: String,
    pub url: String,
    pub design_id: Option<String>,
    pub page_url: String,
    pub diagram_image_name: Option<String>,
    pub diagram_image_data: Option<Vec<u8>>,
    pub competition_diagram: Option<String>,
    pub lw_ratio: Option<String>,
    pub refractive_index: Option<String>,
    pub index_gear: Option<String>,
    pub volume: Option<String>,
    pub facets_count: Option<String>,
    pub shape: Option<String>,
    pub designer_info: Option<String>,
    pub hw_ratio: Option<String>,
    pub tw_ratio: Option<String>,
    pub uw_ratio: Option<String>,
    pub pw_ratio: Option<String>,
    pub cw_ratio: Option<String>,
    pub symmetry_order: Option<String>,
    pub mirror_symmetry: Option<bool>,
    pub designer: Option<String>,
    pub source_citation: Option<String>,
    pub pdf_file: Option<String>,
    pub gem_file: Option<String>,
    pub shape_category: Option<String>,
    pub angle_settings: Vec<AngleSetting>,
    pub attached_files: Vec<AttachedFile>,
}
