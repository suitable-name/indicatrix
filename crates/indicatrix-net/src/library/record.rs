//! The design data types a [`super::LibraryResponse`] carries: a search-result summary
//! ([`DesignSummary`]) and a full design record ([`DesignRecord`]), plus the small
//! per-row types both embed.

use serde::{Deserialize, Serialize};

/// One design as it appears in a [`super::LibraryResponse::SearchResults`] list. Wire
/// counterpart of `indicatrix_vault::model::entry::DiagramListItem`, plus
/// [`Self::version`] (see the module docs on staleness).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignSummary {
    /// Identifier of the library entry.
    pub entry_id: i64,
    /// Display title.
    pub title: String,
    /// Source URL.
    pub url: String,
    /// Identifier of the design within its source catalogue.
    pub design_id: Option<String>,
    /// Shape category of the stone.
    pub shape: Option<String>,
    /// Index gear the design is cut on.
    pub index_gear: Option<String>,
    /// Number of facets.
    pub facets_count: Option<String>,
    /// Free-text designer credit.
    pub designer_info: Option<String>,
    /// Length-to-width ratio.
    pub lw_ratio: Option<String>,
    /// Refractive index.
    pub refractive_index: Option<String>,
    /// Stone volume.
    pub volume: Option<String>,
    /// Competition the diagram was entered in, if any.
    pub competition_diagram: Option<String>,
    /// Wire counterpart of `indicatrix_vault::model::entry::DiagramListItem::ignored`
    /// -- whether the user has marked this design ignored.
    pub ignored: bool,
    /// SHA-256 over every other field above except [`Self::entry_id`] (the server
    /// database's row number, not part of the design) and [`Self::design_version`],
    /// including [`Self::ignored`] -- see the module docs' "Staleness" section.
    pub version: [u8; 32],
    /// The design's revision token: equal to [`DesignRecord::version`] of the full
    /// record at the same revision, so a client that stored that token when it fetched
    /// the design can tell from the search row alone whether the design changed since,
    /// without a `FetchDesign`. A version token, not a content hash: it moves on every
    /// edit to the design (angle table, notes, attachments, ratios, image) even when
    /// every other field of this summary stays the same, and is all zero bytes when the
    /// server could not read the design's revision (a client must then re-fetch).
    pub design_version: [u8; 32],
}

/// One angle-schedule row -- wire counterpart of `indicatrix_vault::model::angle::AngleSetting`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AngleSettingWire {
    /// Position of the row in cutting order.
    pub order_index: u32,
    /// Facet name.
    pub facet: String,
    /// Cutting angle.
    pub angle: String,
    /// Index wheel position(s).
    pub index: String,
    /// Free-form cutting notes.
    pub notes: String,
}

/// One attachment's METADATA -- never its content; see the module docs' "Attachments"
/// section for why content is fetched separately, by [`Self::id`], via
/// [`super::LibraryRequest::FetchAttachment`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedFileMeta {
    /// Database identifier.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Source URL.
    pub url: String,
    /// Byte length of the attachment's content, without ever loading it -- lets a
    /// client show "PDF, 2.4 MB" or decide whether to fetch it at all before spending a
    /// round trip.
    pub size: u64,
}

/// One design, in full -- entry + detail + angle settings + attachment METADATA (never
/// content -- see [`super::LibraryRequest::FetchAttachment`]).
///
/// Wire counterpart of `indicatrix_vault::model::entry::FullDiagramRecord`, minus
/// attachment content, plus [`Self::version`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignRecord {
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
    /// The design's own diagram image (SVG/PNG, central to displaying it) -- kept
    /// inline, unlike attachment content; see the module docs' "Attachments" section.
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
    /// Per-tier angle settings of the design.
    pub angle_settings: Vec<AngleSettingWire>,
    /// Metadata of the files attached to the design.
    pub attachments: Vec<AttachedFileMeta>,
    /// Wire counterpart of `indicatrix_vault::model::preview::PreviewImages::material`
    /// -- the `indicatrix` material preset the cached previews/tilt-curve were generated
    /// with, or `None` if nothing's generated yet. Lets `apps/indicatrix-cut`'s Tilt
    /// Performance dialog detect a stale cached curve
    /// (`gui::tilt_profile::cached_curve_material_is_stale`) for a remote design too.
    pub preview_material: Option<String>,
    /// Wire counterpart of `indicatrix_vault::model::entry::FullDiagramMeta::hw_ratio`.
    /// This and the seven fields below (`tw_ratio` through `designer`) back
    /// `apps/indicatrix-cut`'s detail-pane ratio/symmetry/designer chips, mirroring what
    /// a local design gets from `FullDiagramRecord`.
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
    /// Wire counterpart of `indicatrix_vault::model::entry::FullDiagramMeta::source_citation`
    /// -- the publication-citation half of `designer_info` (see that field's own doc
    /// comment). Added under `PROTOCOL_VERSION` v15, alongside the three fields below.
    pub source_citation: Option<String>,
    /// Wire counterpart of `FullDiagramMeta::pdf_file` -- a competition entry's `PDF:`
    /// attachment file name.
    pub pdf_file: Option<String>,
    /// Wire counterpart of `FullDiagramMeta::gem_file` -- a competition entry's `GEM:`
    /// attachment file name.
    pub gem_file: Option<String>,
    /// Wire counterpart of `FullDiagramMeta::shape_category` -- the numbered
    /// shape-category id.
    pub shape_category: Option<String>,
    /// The design's revision token (see [`DesignSummary::design_version`], which a
    /// search row of the same design at the same revision carries identically) -- a
    /// version token derived from the server's per-design revision stamp, not a hash of
    /// this record's content. What a client stores to decide whether its mirror of this
    /// one design needs a re-fetch; all zero bytes when the server could not read the
    /// revision. See the module docs' "Staleness" section.
    pub version: [u8; 32],
}
