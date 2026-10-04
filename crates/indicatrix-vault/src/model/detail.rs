use super::{angle::AngleSetting, file::AttachedFile};
use serde::{Deserialize, Serialize};

/// A design's full detail row, plus its child angle-setting/attached-file records.
///
/// What [`crate::db::sqlite::Database::save_diagram_detail`] fully replaces on every
/// (re-)sync, and what a fresh scrape/import populates from scratch. See
/// [`crate::model::metadata_update::MetadataUpdate`] for the narrower, hand-correction
/// path that does NOT go through this type (a naive read-modify-write through this
/// struct would silently zero every field a scraped record doesn't carry).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FacetingDiagramDetail {
    /// The source page's URL this detail was scraped from, or empty for a locally
    /// imported design.
    pub page_url: String,
    /// The diagram image's file name, if the source page has one.
    pub diagram_image_name: Option<String>,
    /// The diagram image's raw bytes, if the source page has one.
    pub diagram_image_data: Option<Vec<u8>>,
    /// This design's angle-settings table -- one row per tier.
    pub angle_settings_table: Vec<AngleSetting>,
    /// Every attachment (`.asc`/`.gem`/`.pdf`/native-sidecar, or a scraped page's own
    /// attachments) with full byte content.
    pub attached_files: Vec<AttachedFile>,

    // Specific metadata fields
    /// The competition-entry class/label, for a competition-sourced design; `None` for
    /// a regular (non-competition) design.
    pub competition_diagram: Option<String>,
    /// Length/width ratio, as recorded on the design sheet.
    pub lw_ratio: Option<String>,
    /// Refractive index, as recorded on the design sheet.
    pub refractive_index: Option<String>,
    /// The gear-tooth count, as recorded on the design sheet.
    pub index_gear: Option<String>,
    /// Volume, as recorded on the design sheet.
    pub volume: Option<String>,
    /// The scraped-style `"55+6"` facet-count text -- see
    /// [`crate::model::facets::parse_facets_count`] for how it's split.
    pub facets_count: Option<String>,
    /// The shape name, as recorded/cleaned from the design sheet (free-text; see
    /// `crate::db::sqlite::DEFAULT_SHAPES`'s doc comment for why the catalogue's
    /// vocabulary is only a starting list, not exhaustive).
    pub shape: Option<String>,
    /// The free-text `"Designer; Publication citation"` string as printed on the
    /// design sheet -- see [`Self::designer`]/[`Self::source_citation`] for the
    /// machine-split halves of the same text.
    pub designer_info: Option<String>,

    // Proportion ratios and symmetry, as printed on a design sheet's metadata block
    // alongside `lw_ratio`/`volume` above. `hw_ratio` is the odd one out: most sheets
    // don't print it, so it usually arrives from a design's own metadata instead. All
    // `None` by default; a missing ratio must stay missing, not fabricated.
    // `apps/indicatrix-cut`'s importer derives several from the design's own geometry;
    // see `gui::library::apply_measured_metadata`.
    /// Height/width ratio -- see this field group's own doc comment above for why it's
    /// the odd one out among the five ratios here.
    pub hw_ratio: Option<String>,
    /// Table/width ratio.
    pub tw_ratio: Option<String>,
    /// Upper-girdle/width ratio.
    pub uw_ratio: Option<String>,
    /// Pavilion/width ratio.
    pub pw_ratio: Option<String>,
    /// Culet/width ratio.
    pub cw_ratio: Option<String>,
    /// The rotational fold count (e.g. `4` in "4-fold, mirror-image symmetry").
    pub symmetry_order: Option<String>,
    /// Whether the sheet additionally declares mirror-image symmetry.
    pub mirror_symmetry: Option<bool>,

    /// The designer alone (e.g. `"Capps, Jerry"`) -- the first half of what
    /// [`Self::designer_info`] holds as one free-text `"Designer; Publication
    /// citation"` string. Split out so "every design by X" is a real equality query
    /// against an indexed column rather than a `LIKE '%X%'` scan.
    ///
    /// `designer_info` is deliberately *kept* alongside this and
    /// [`Self::source_citation`]: it's what `search_diagrams`' free-text `LIKE`
    /// matches, and what
    /// `apps/indicatrix-cut` renders -- it stays the display/search convenience, with
    /// these two as the queryable halves.
    pub designer: Option<String>,
    /// The publication citation alone (e.g. `"Lapidary Journal, May 1994, p95"`) --
    /// the second half of [`Self::designer_info`]. See [`Self::designer`].
    pub source_citation: Option<String>,

    // Competition-entry pages only. facetdiagrams.org serves regular designs
    // (`/diagram/...`, inline `<svg>` plus a designer/citation `<li>`) and competition
    // entries (`/diagramus/...`, no inline SVG, attachments labelled `PDF:`/`GEM:` in a
    // `div.attachmentPost` list instead). The three fields below come from the second
    // kind and are `None` on the first -- see `crate::parser::parse_attachment_labels`.
    /// The `PDF:` attachment's file name (e.g. `"2002SSCMasters.pdf"`), the join key
    /// between a competition entry and the PDF corpus in [`Self::attached_files`].
    ///
    /// A plain string, *not* a foreign key: several competition designs routinely name
    /// the same PDF (a multi-design results booklet shared across a competition
    /// class) -- a many-to-one relationship a per-design reference could not express.
    pub pdf_file: Option<String>,
    /// The `GEM:` attachment's file name (e.g. `"USFG-SSC-2020-Novice-1.gem"`), a
    /// `GemCAD` design file. Frequently blank on the page (rendered as a literal
    /// `""`), which reads back here as `None` rather than an empty string.
    pub gem_file: Option<String>,
    /// The numbered shape-category id from the `Shape:` metadata item, e.g. `5` from
    /// `"05. Pear"` -- the stable half of that label, whose text half
    /// ([`Self::shape`]) `crate::util::clean_shape_string` already strips it from.
    ///
    /// Kept as a decimal string, like [`Self::symmetry_order`], bound straight into an
    /// INTEGER column. Not derivable from the article's `shape-diagram-NN-name` CSS
    /// class, which `crate::util::map_shape_class` shows carries *two* conflicting
    /// numberings for several shapes (`04`/`10` both being Emerald) and so isn't a
    /// stable id.
    pub shape_category: Option<String>,

    /// How many of the design's tiers are concave (tool-cut); `0` for a planar design.
    /// What `RangeFilter::has_concave` tests. Separate from [`Self::facets_count`],
    /// whose `"55+6"` text is parsed into `facets`/`girdle_facets` and must stay
    /// two-component.
    pub concave_tiers: u32,
    /// How many concave placements the design has in total (a concave tier can be
    /// repeated around the stone); `0` for a planar design. Counted apart from the
    /// flat `facets` so existing facet-count filters keep their meaning.
    pub concave_facets: u32,
}
