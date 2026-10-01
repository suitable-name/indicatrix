use crate::model::{angle::AngleSetting, file::AttachedFile};
use serde::{Deserialize, Serialize};

/// A catalogue row's identity fields -- what
/// [`crate::db::sqlite::Database::save_diagram_entry`] upserts into `diagram_entries`,
/// keyed on `url`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacetingDiagramEntry {
    /// The design's display title.
    pub title: String,
    /// This design's stable identity key -- `diagram_entries.url` is `UNIQUE`, so
    /// saving the same `url` again upserts the existing row rather than creating a
    /// second one. A real page URL for a scraped source, or a synthetic
    /// `local://<file_name>` for a locally-imported `.asc` (see
    /// `crate::local::LOCAL_SOURCE_ID`).
    pub url: String,
    /// The scraped source's own design-id text, if any.
    pub design_id: String,
}

/// One row of a library search/display result.
///
/// The display-facing projection of a `diagram_entries`/`diagram_details` join (see
/// `crate::db::sqlite::search::build_search_predicate`'s fixed column list, which this
/// struct's field order matches).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramListItem {
    /// `diagram_entries.id`.
    pub id: i64,
    /// The design's display title.
    pub title: String,
    /// This design's stable identity key -- see [`FacetingDiagramEntry::url`].
    pub url: String,
    /// The scraped source's own design-id text, if any.
    pub design_id: Option<String>,
    /// `diagram_details.shape` (free-text, not necessarily drawn from
    /// `crate::db::sqlite::DEFAULT_SHAPES`).
    pub shape: Option<String>,
    /// The gear-tooth count, as display text.
    pub index_gear: Option<String>,
    /// The scraped-style `"55+6"` facet-count display text.
    pub facets_count: Option<String>,
    /// The free-text `"Designer; Publication citation"` display string.
    pub designer_info: Option<String>,
    /// Length/width ratio, as display text.
    pub lw_ratio: Option<String>,
    /// Refractive index, as display text.
    pub refractive_index: Option<String>,
    /// Volume, as display text.
    pub volume: Option<String>,
    /// The competition-entry class/label, for a competition-sourced design.
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
    /// Database identifier.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Source URL.
    pub url: String,
    /// Size in bytes.
    pub size: i64,
}

/// The same record as [`FullDiagramRecord`], but without attachment content.
///
/// [`Self::attached_files`] carries each attachment's METADATA only, never its
/// `content` -- this query never selects `content` at all, so it doesn't pay to load
/// it. See [`crate::db::sqlite::Database::get_diagram_full_meta`].
///
/// Carries `hw_ratio`/`tw_ratio`/`uw_ratio`/`pw_ratio`/`cw_ratio`/`symmetry_order`/
/// `mirror_symmetry`/`designer`/`source_citation`/`pdf_file`/`gem_file`/`shape_category`:
/// `indicatrix_net::library::DesignRecord` is built from this type (never
/// [`FullDiagramRecord`], since it must never carry attachment content), so dropping
/// these fields here would blank the corresponding `apps/indicatrix-cut` detail-pane
/// chips (and, for the last four, a mirrored design's own metadata -- see that
/// protocol's `PROTOCOL_VERSION` v15 history) for every remote-browsed design.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullDiagramMeta {
    /// Identifier of the library entry.
    pub entry_id: i64,
    /// Display title.
    pub title: String,
    /// Source URL.
    pub url: String,
    /// Identifier of the design within its source catalogue.
    pub design_id: Option<String>,
    /// URL of the page the design was taken from.
    pub page_url: String,
    /// File name of the diagram image.
    pub diagram_image_name: Option<String>,
    /// Raw bytes of the diagram image.
    pub diagram_image_data: Option<Vec<u8>>,
    /// Competition the diagram was entered in, if any.
    pub competition_diagram: Option<String>,
    /// Length-to-width ratio.
    pub lw_ratio: Option<String>,
    /// Refractive index.
    pub refractive_index: Option<String>,
    /// Index gear the design is cut on.
    pub index_gear: Option<String>,
    /// Stone volume.
    pub volume: Option<String>,
    /// Number of facets.
    pub facets_count: Option<String>,
    /// Shape category of the stone.
    pub shape: Option<String>,
    /// Free-text designer credit.
    pub designer_info: Option<String>,
    /// Height-to-width ratio.
    pub hw_ratio: Option<String>,
    /// Table-to-width ratio.
    pub tw_ratio: Option<String>,
    /// Upper-girdle-to-width ratio.
    pub uw_ratio: Option<String>,
    /// Pavilion-to-width ratio.
    pub pw_ratio: Option<String>,
    /// Crown-to-width ratio.
    pub cw_ratio: Option<String>,
    /// Rotational symmetry order.
    pub symmetry_order: Option<String>,
    /// Whether the design has mirror symmetry.
    pub mirror_symmetry: Option<bool>,
    /// Designer name.
    pub designer: Option<String>,
    /// See [`crate::model::detail::FacetingDiagramDetail::source_citation`].
    pub source_citation: Option<String>,
    /// See [`crate::model::detail::FacetingDiagramDetail::pdf_file`].
    pub pdf_file: Option<String>,
    /// See [`crate::model::detail::FacetingDiagramDetail::gem_file`].
    pub gem_file: Option<String>,
    /// See [`crate::model::detail::FacetingDiagramDetail::shape_category`].
    pub shape_category: Option<String>,
    /// Per-tier angle settings of the design.
    pub angle_settings: Vec<AngleSetting>,
    /// Files attached to the design.
    pub attached_files: Vec<AttachedFileMeta>,
}

/// Carries every `diagram_details` column [`crate::model::detail::FacetingDiagramDetail`] has.
///
/// Must stay in sync with that struct: a caller building a fresh `FacetingDiagramDetail`
/// from a `FullDiagramRecord` (to feed `Database::save_diagram_detail`, which fully
/// replaces a design's detail row) would silently zero any field missing here on every
/// such save -- see `Database::update_diagram_metadata`'s doc comment for the
/// narrow-`UPDATE` method that exists so a metadata edit never needs that path at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullDiagramRecord {
    /// Identifier of the library entry.
    pub entry_id: i64,
    /// Display title.
    pub title: String,
    /// Source URL.
    pub url: String,
    /// Identifier of the design within its source catalogue.
    pub design_id: Option<String>,
    /// URL of the page the design was taken from.
    pub page_url: String,
    /// File name of the diagram image.
    pub diagram_image_name: Option<String>,
    /// Raw bytes of the diagram image.
    pub diagram_image_data: Option<Vec<u8>>,
    /// Competition the diagram was entered in, if any.
    pub competition_diagram: Option<String>,
    /// Length-to-width ratio.
    pub lw_ratio: Option<String>,
    /// Refractive index.
    pub refractive_index: Option<String>,
    /// Index gear the design is cut on.
    pub index_gear: Option<String>,
    /// Stone volume.
    pub volume: Option<String>,
    /// Number of facets.
    pub facets_count: Option<String>,
    /// Shape category of the stone.
    pub shape: Option<String>,
    /// Free-text designer credit.
    pub designer_info: Option<String>,
    /// Height-to-width ratio.
    pub hw_ratio: Option<String>,
    /// Table-to-width ratio.
    pub tw_ratio: Option<String>,
    /// Upper-girdle-to-width ratio.
    pub uw_ratio: Option<String>,
    /// Pavilion-to-width ratio.
    pub pw_ratio: Option<String>,
    /// Crown-to-width ratio.
    pub cw_ratio: Option<String>,
    /// Rotational symmetry order.
    pub symmetry_order: Option<String>,
    /// Whether the design has mirror symmetry.
    pub mirror_symmetry: Option<bool>,
    /// Designer name.
    pub designer: Option<String>,
    /// Citation of the original source.
    pub source_citation: Option<String>,
    /// Name of the attached PDF.
    pub pdf_file: Option<String>,
    /// Name of the attached .gem file.
    pub gem_file: Option<String>,
    /// Normalised shape category.
    pub shape_category: Option<String>,
    /// Per-tier angle settings of the design.
    pub angle_settings: Vec<AngleSetting>,
    /// Files attached to the design.
    pub attached_files: Vec<AttachedFile>,
}
