//! The design data types a [`super::LibraryResponse`] carries: a search-result summary
//! ([`DesignSummary`]) and a full design record ([`DesignRecord`]), plus the small
//! per-row types both embed.

use serde::{Deserialize, Serialize};

/// One design as it appears in a [`super::LibraryResponse::SearchResults`] list. Wire
/// counterpart of `indicatrix_vault::model::entry::DiagramListItem`, plus
/// [`Self::version`] (see the module docs on staleness).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignSummary {
    pub entry_id: i64,
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
    /// Wire counterpart of `indicatrix_vault::model::entry::DiagramListItem::ignored`
    /// -- whether the user has marked this design ignored.
    pub ignored: bool,
    /// SHA-256 over every other field above (including [`Self::ignored`], added the
    /// same time this field was) -- see the module docs' "Staleness" section.
    pub version: [u8; 32],
}

/// One angle-schedule row -- wire counterpart of `indicatrix_vault::model::angle::AngleSetting`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AngleSettingWire {
    pub order_index: u32,
    pub facet: String,
    pub angle: String,
    pub index: String,
    pub notes: String,
}

/// One attachment's METADATA -- never its content; see the module docs' "Attachments"
/// section for why content is fetched separately, by [`Self::id`], via
/// [`super::LibraryRequest::FetchAttachment`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedFileMeta {
    pub id: i64,
    pub name: String,
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
    pub entry_id: i64,
    pub title: String,
    pub url: String,
    pub design_id: Option<String>,
    pub page_url: String,
    pub diagram_image_name: Option<String>,
    /// The design's own diagram image (SVG/PNG, central to displaying it) -- kept
    /// inline, unlike attachment content; see the module docs' "Attachments" section.
    pub diagram_image_data: Option<Vec<u8>>,
    pub competition_diagram: Option<String>,
    pub lw_ratio: Option<String>,
    pub refractive_index: Option<String>,
    pub index_gear: Option<String>,
    pub volume: Option<String>,
    pub facets_count: Option<String>,
    pub shape: Option<String>,
    pub designer_info: Option<String>,
    pub angle_settings: Vec<AngleSettingWire>,
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
    pub tw_ratio: Option<String>,
    pub uw_ratio: Option<String>,
    pub pw_ratio: Option<String>,
    pub cw_ratio: Option<String>,
    pub symmetry_order: Option<String>,
    pub mirror_symmetry: Option<bool>,
    pub designer: Option<String>,
    /// SHA-256 over every other field above (including `diagram_image_data` and each
    /// attachment's metadata, but never attachment content) -- see the module docs'
    /// "Staleness" section. The authoritative version for deciding whether a client's
    /// mirror of this one design needs a re-fetch.
    pub version: [u8; 32],
}
