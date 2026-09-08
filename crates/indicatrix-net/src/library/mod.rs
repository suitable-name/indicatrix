//! The read-only design-library sync protocol: a client (a future viewer, or a mobile
//! client with no renderer compiled in) mirrors designs from a `indicatrix-worker`'s
//! catalogue.
//!
//! [`LibraryRequest`]/[`LibraryResponse`] are their own message family, tagged into the
//! same [`crate::messages::ClientMessage`] envelope the render protocol uses
//! (`ClientMessage::Library`). This module never depends on `indicatrix` and is always
//! compiled in regardless of this crate's `render` feature, so a mobile client can speak
//! it with no renderer.
//!
//! # Pull-mirror, read-only -- for now
//!
//! The server is authoritative; a client mirrors from it. No `Put`/`Delete`/merge
//! request yet, and `indicatrix-worker` never writes to its catalogue in this phase.
//!
//! [`LibraryRequest::Search`] is a single round trip capped at
//! `Database::search_diagrams`'s result cap -- fine for an interactive search box, not
//! for listing a catalogue with more matching rows than that. [`LibraryRequest::SearchPage`]/
//! [`LibraryResponse::SearchResultsPage`] is the keyset-cursor form a full-catalogue walk
//! (a mirror sync) uses instead: same filters plus a cursor, walked page by page until a
//! short page signals the end.
//!
//! Both enums are plain and generically named so a write operation can later be added as
//! a new variant appended at the tail of each. [`DesignSummary::version`]/
//! [`DesignRecord::version`], here for read-side staleness detection, double as what a
//! future `Put` needs for optimistic-concurrency conflict detection.
//!
//! # Staleness: a content hash, not a sequence number
//!
//! [`DesignSummary::version`]/[`DesignRecord::version`] is a SHA-256 hash over the
//! fields that response carries (excluding the hash field itself, and -- for
//! [`DesignRecord`] -- attachment content, metadata only). `indicatrix-worker` computes
//! it at serve time; this crate only carries the resulting 32 bytes.
//!
//! A content hash rather than a sequence number or timestamp, because the underlying
//! schema has neither column and this phase must not write to the database. A value that
//! changes and later changes back looks unchanged, but that's harmless here: worst case
//! is one wasted round trip.
//!
//! [`DesignSummary::version`] covers only what [`DesignSummary`] carries (cheap, up to
//! 1000 rows per search); [`DesignRecord::version`] additionally covers the full detail
//! record and image bytes, so it's authoritative for deciding whether a full re-fetch is
//! needed. Treat the summary hash as a cheap first filter and confirm against the record
//! hash before skipping a `FetchDesign`.
//!
//! # Attachments: fetched separately, one at a time
//!
//! [`DesignRecord`] carries attachment metadata only ([`AttachedFileMeta`]), never
//! content: attachments can hold multi-megabyte PDFs and several designs often share the
//! same one, so inlining content into every `FetchDesign` reply would re-send those
//! bytes per referencing design. A client fetches an attachment's bytes lazily, once per
//! id, via [`LibraryRequest::FetchAttachment`].
//!
//! Per-request memory is bounded to one attachment's bytes at a time, not a whole
//! design's attachment set or the search result set.
//!
//! # Loading a remote design into the local editor
//!
//! [`LibraryRequest::FetchDesignSource`]/[`LibraryResponse::DesignSource`] is a
//! narrower, purpose-built sibling of the general `FetchAttachment` path above: it
//! names the design by `entry_id` rather than an attachment id, and it specifically
//! finds and returns that design's own `.asc` cutting-schedule attachment's TEXT (not
//! an arbitrary attachment's raw bytes), decoded as UTF-8. This is what lets
//! `apps/indicatrix-cut`'s cutting-design editor load a remote design exactly the way
//! it already loads a local one -- see [`LibraryResponse::DesignSourceNotAvailable`]
//! for what a design with no `.asc` attachment gets instead.

use serde::{Deserialize, Serialize};

use crate::messages::ErrorMsg;

/// Wire counterpart of `indicatrix_vault::model::filter::RangeFilter`; a `None` bound is
/// unconstrained.
///
/// Kept separate so this crate's wire shapes stay independent of `indicatrix-vault`'s
/// internal representation -- `indicatrix-worker` maps between them (`from_range_wire`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RangeFilterWire {
    pub ri_min: Option<f64>,
    pub ri_max: Option<f64>,
    pub lw_min: Option<f64>,
    pub lw_max: Option<f64>,
    pub volume_min: Option<f64>,
    pub volume_max: Option<f64>,
    pub facets_min: Option<i64>,
    pub facets_max: Option<i64>,
    /// Wire counterpart of `indicatrix_vault::model::filter::RangeFilter::ri_tolerance`
    /// -- `(centre, tolerance)`.
    pub ri_tolerance: Option<(f64, f64)>,
    /// Wire counterpart of
    /// `indicatrix_vault::model::filter::RangeFilter::include_ignored`.
    pub include_ignored: bool,
    /// Wire counterpart of `indicatrix_vault::model::filter::RangeFilter::performance`.
    pub performance: Vec<PerformanceFilterWire>,
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceMetric`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerformanceMetricWire {
    Brilliance,
    Extinction,
    Windowing,
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceBound`. Only
/// `PartialEq`, not `Eq`, since `AtMost`/`AtLeast` carry an [`f32`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PerformanceBoundWire {
    AtMost(f32),
    AtLeast(f32),
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceAggregate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerformanceAggregateWire {
    Worst,
    Mean,
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceFilter`.
/// `tilt_radius_deg` is a plain unvalidated `f32`; `indicatrix-worker`'s
/// `from_range_wire` validates it via `PerformanceFilter::new`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PerformanceFilterWire {
    pub metric: PerformanceMetricWire,
    pub bound: PerformanceBoundWire,
    pub tilt_radius_deg: f32,
    pub aggregate: PerformanceAggregateWire,
}

/// Wire counterpart of `indicatrix_vault::model::filter::AttributeRanges`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AttributeRangesWire {
    pub ri: (f64, f64),
    pub lw_ratio: (f64, f64),
    pub volume: (f64, f64),
    pub facets: (i64, i64),
}

/// One design as it appears in a [`LibraryResponse::SearchResults`] list. Wire
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
/// [`LibraryRequest::FetchAttachment`].
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
/// content -- see [`LibraryRequest::FetchAttachment`]).
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

/// One request in the read-only library-sync protocol. Deliberately request/response,
/// not streamed like `RENDER`; see the module docs for the push-extension room left in
/// the shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LibraryRequest {
    /// List/search designs -- mirrors
    /// `indicatrix_vault::db::sqlite::Database::search_diagrams`'s filters one-for-one.
    /// Capped at that method's result cap (currently 1000); see [`Self::SearchPage`] for
    /// the paginated form a full-catalogue walk needs instead.
    Search {
        query: String,
        shape_filter: String,
        gear_filter: String,
        range: RangeFilterWire,
    },
    /// Scalar catalogue facts a search UI needs for its filter controls -- distinct
    /// shapes, distinct gears, attribute range bounds -- mirroring
    /// `Database::get_unique_shapes`/`get_unique_gears`/`get_attribute_ranges`.
    FilterOptions,
    /// Fetch one design's entry + detail + angle settings + attachment metadata.
    FetchDesign { entry_id: i64 },
    /// Fetch one attachment's raw bytes by id -- see the module docs' "Attachments"
    /// section.
    FetchAttachment { attachment_id: i64 },
    /// Keyset-paginated counterpart of [`Self::Search`], for a caller that needs the
    /// WHOLE matching result set (mirrors `Database::search_diagrams_page`: same
    /// filters, plus `cursor`).
    ///
    /// `cursor` is `None` for the first page, or `Some(entry_id)` of the last
    /// [`DesignSummary`] the previous page returned, to continue strictly after it. See
    /// [`LibraryResponse::SearchResultsPage::next_cursor`] for the stop condition.
    SearchPage {
        query: String,
        shape_filter: String,
        gear_filter: String,
        range: RangeFilterWire,
        cursor: Option<i64>,
    },
    /// Fetch one design's original `.asc` cutting-schedule TEXT (its actual file
    /// bytes, decoded as UTF-8 -- see [`LibraryResponse::DesignSource`]), so a remote
    /// design can be loaded into the local cutting-design editor the same way a
    /// locally-attached `.asc` already is (`gui::editor::loading::design_from_full_record`
    /// on the client, `crate::serve::library::design_source` on this worker).
    ///
    /// Distinct from [`Self::FetchDesign`]: that reply carries attachment METADATA only
    /// (see the module docs' "Attachments" section), never enough to reconstruct the
    /// design's own real mast values -- only a genuine `.asc` attachment's raw content
    /// has those. Appended here, after [`Self::SearchPage`], rather than inserted
    /// earlier in the enum, so postcard's index-based encoding of every existing variant
    /// is undisturbed.
    FetchDesignSource { entry_id: i64 },
}

/// A worker's reply to one [`LibraryRequest`].
///
/// `Design` is boxed to keep this enum's stack footprint close to its other, smaller
/// variants rather than every [`LibraryResponse`] paying for [`DesignRecord`]'s size;
/// serde boxes/unboxes it transparently, so the wire encoding is unaffected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LibraryResponse {
    /// Reply to [`LibraryRequest::Search`]. `excluded_for_missing_curves` mirrors
    /// `indicatrix_vault::model::filter::PerformanceSearchResult::excluded_for_missing_curves`
    /// -- designs an active `range.performance` predicate excluded for having no stored
    /// tilt curve to test (not for failing it). `0` when no performance filter is
    /// active; drives the same "N designs hidden" notice `apps/indicatrix-cut` shows.
    SearchResults {
        items: Vec<DesignSummary>,
        excluded_for_missing_curves: u32,
    },
    FilterOptions {
        shapes: Vec<String>,
        gears: Vec<String>,
        ranges: AttributeRangesWire,
    },
    Design(Box<DesignRecord>),
    Attachment {
        name: String,
        content: Vec<u8>,
    },
    /// [`LibraryRequest::FetchDesign`]/[`FetchAttachment`] named an id this worker's
    /// catalogue has no row for.
    NotFound,
    /// A request-level failure this worker could still form a normal reply for (e.g. a
    /// malformed filter) -- distinct from a transport-level [`crate::messages::NetError`].
    Error(ErrorMsg),
    /// Reply to [`LibraryRequest::SearchPage`]. `next_cursor` is `Some(last_entry_id)`
    /// of `results`' last element when `results` came back a full page (re-request with
    /// `cursor: next_cursor`), or `None` on the final page (`results` may be empty). A
    /// caller loops until `None`.
    ///
    /// `excluded_for_missing_curves` exists for the same reason as on
    /// [`Self::SearchResults`], but is currently always `0` since no caller pages
    /// through a performance-filtered search yet; reserved to avoid a future protocol
    /// bump.
    SearchResultsPage {
        results: Vec<DesignSummary>,
        next_cursor: Option<i64>,
        excluded_for_missing_curves: u32,
    },
    /// Reply to [`LibraryRequest::FetchDesignSource`]: `entry_id`'s real, attached
    /// `.asc` file, decoded as UTF-8 (lossily, same as the client's own local load path
    /// -- see `gui::editor::loading::design_from_full_record`). `file_name` is that
    /// attachment's own bare file name (for `save_paired`-style "Save Native"
    /// round-tripping later), `asc_text` its exact original text. Subject to the same
    /// per-message [`crate::framing::MAX_FRAME_LEN`] wire cap every other reply
    /// (including [`Self::Attachment`]) already carries -- no separate bespoke size
    /// limit is layered on top, matching [`Self::Attachment`]'s own precedent.
    DesignSource {
        entry_id: i64,
        file_name: String,
        asc_text: String,
    },
    /// `entry_id` names a real catalogue entry (unlike [`Self::NotFound`], reused when
    /// `entry_id` itself doesn't exist), but it has no attached `.asc` file to fetch --
    /// see `apps/indicatrix-worker`'s `serve::library::handle_request` for how it's
    /// distinguished from [`Self::NotFound`] server-side (this crate never depends on
    /// that binary, so it can't be linked from here). A remote design with only a
    /// reconstructed (placeholder, every mast `0.0`) schedule has nothing genuine here
    /// to send.
    DesignSourceNotAvailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{read_message, write_message};

    fn sample_summary() -> DesignSummary {
        DesignSummary {
            entry_id: 42,
            title: "Round Brilliant".to_string(),
            url: "https://example.test/diagram/42".to_string(),
            design_id: Some("RB-1".to_string()),
            shape: Some("Round".to_string()),
            index_gear: Some("96".to_string()),
            facets_count: Some("57".to_string()),
            designer_info: Some("Capps, Jerry".to_string()),
            lw_ratio: Some("1.00".to_string()),
            refractive_index: Some("2.417".to_string()),
            volume: Some("0.65".to_string()),
            competition_diagram: None,
            ignored: false,
            version: [7u8; 32],
        }
    }

    #[test]
    fn library_request_variants_round_trip() {
        for req in [
            LibraryRequest::Search {
                query: "round".to_string(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
            },
            LibraryRequest::FilterOptions,
            LibraryRequest::FetchDesign { entry_id: 42 },
            LibraryRequest::FetchAttachment { attachment_id: 9 },
            LibraryRequest::SearchPage {
                query: "round".to_string(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                cursor: None,
            },
            LibraryRequest::SearchPage {
                query: String::new(),
                shape_filter: "Round".to_string(),
                gear_filter: "96".to_string(),
                range: RangeFilterWire::default(),
                cursor: Some(1000),
            },
            LibraryRequest::FetchDesignSource { entry_id: 42 },
        ] {
            let mut buf = Vec::new();
            write_message(&mut buf, &req).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn library_response_variants_round_trip() {
        let design = DesignRecord {
            entry_id: 42,
            title: "Round Brilliant".to_string(),
            url: "https://example.test/diagram/42".to_string(),
            design_id: Some("RB-1".to_string()),
            page_url: "https://example.test/diagram/42".to_string(),
            diagram_image_name: Some("rb.svg".to_string()),
            diagram_image_data: Some(vec![1, 2, 3]),
            competition_diagram: None,
            lw_ratio: Some("1.00".to_string()),
            refractive_index: Some("2.417".to_string()),
            index_gear: Some("96".to_string()),
            volume: Some("0.65".to_string()),
            facets_count: Some("57".to_string()),
            shape: Some("Round".to_string()),
            designer_info: Some("Capps, Jerry".to_string()),
            // Non-default values throughout so a dropped field fails the round-trip.
            preview_material: Some("Diamond".to_string()),
            hw_ratio: Some("1.00".to_string()),
            tw_ratio: Some("0.53".to_string()),
            uw_ratio: Some("0.16".to_string()),
            pw_ratio: Some("0.43".to_string()),
            cw_ratio: Some("0.14".to_string()),
            symmetry_order: Some("8".to_string()),
            mirror_symmetry: Some(true),
            designer: Some("Capps, Jerry".to_string()),
            angle_settings: vec![AngleSettingWire {
                order_index: 0,
                facet: "P1".to_string(),
                angle: "41.0".to_string(),
                index: "96".to_string(),
                notes: String::new(),
            }],
            attachments: vec![AttachedFileMeta {
                id: 1,
                name: "schedule.pdf".to_string(),
                url: "https://example.test/schedule.pdf".to_string(),
                size: 12_345,
            }],
            version: [1u8; 32],
        };

        for resp in [
            LibraryResponse::SearchResults {
                items: vec![sample_summary()],
                excluded_for_missing_curves: 0,
            },
            LibraryResponse::FilterOptions {
                shapes: vec!["Round".to_string()],
                gears: vec!["96".to_string()],
                ranges: AttributeRangesWire {
                    ri: (1.4, 2.5),
                    lw_ratio: (0.8, 1.5),
                    volume: (0.1, 1.0),
                    facets: (20, 200),
                },
            },
            LibraryResponse::Design(Box::new(design)),
            LibraryResponse::Attachment {
                name: "schedule.pdf".to_string(),
                content: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
            LibraryResponse::NotFound,
            LibraryResponse::Error(ErrorMsg {
                code: 1,
                message: "bad filter".to_string(),
            }),
            LibraryResponse::SearchResultsPage {
                results: vec![sample_summary()],
                next_cursor: Some(42),
                excluded_for_missing_curves: 0,
            },
            LibraryResponse::SearchResultsPage {
                results: Vec::new(),
                next_cursor: None,
                excluded_for_missing_curves: 0,
            },
            LibraryResponse::DesignSource {
                entry_id: 42,
                file_name: "round-brilliant.asc".to_string(),
                asc_text: "GemCad 5.0\n...\n".to_string(),
            },
            LibraryResponse::DesignSourceNotAvailable,
        ] {
            let mut buf = Vec::new();
            write_message(&mut buf, &resp).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn client_message_library_round_trips() {
        use crate::messages::ClientMessage;
        let msg = ClientMessage::Library(Box::new(LibraryRequest::FilterOptions));
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: ClientMessage = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    /// A filter with `ri_tolerance`/`include_ignored`/`performance` all set round-trips
    /// through a `LibraryRequest::Search`.
    #[test]
    fn range_filter_wire_with_every_new_field_set_round_trips() {
        let range = RangeFilterWire {
            ri_min: Some(1.4),
            ri_max: Some(2.5),
            lw_min: None,
            lw_max: None,
            volume_min: None,
            volume_max: None,
            facets_min: Some(40),
            facets_max: Some(90),
            ri_tolerance: Some((1.76, 0.02)),
            include_ignored: true,
            performance: vec![
                PerformanceFilterWire {
                    metric: PerformanceMetricWire::Windowing,
                    bound: PerformanceBoundWire::AtMost(20.0),
                    tilt_radius_deg: 37.25,
                    aggregate: PerformanceAggregateWire::Worst,
                },
                PerformanceFilterWire {
                    metric: PerformanceMetricWire::Brilliance,
                    bound: PerformanceBoundWire::AtLeast(55.5),
                    tilt_radius_deg: 90.0,
                    aggregate: PerformanceAggregateWire::Mean,
                },
            ],
        };

        let req = LibraryRequest::Search {
            query: "round".to_string(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &req).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, req);
    }

    /// [`DesignSummary::ignored`] round-trips both settings rather than silently
    /// resetting to `false` on decode.
    #[test]
    fn design_summary_ignored_flag_round_trips_both_settings() {
        for ignored in [false, true] {
            let summary = DesignSummary {
                ignored,
                ..sample_summary()
            };
            let mut buf = Vec::new();
            write_message(&mut buf, &summary).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let decoded: DesignSummary = read_message(&mut cursor).unwrap();
            assert_eq!(decoded, summary);
            assert_eq!(decoded.ignored, ignored);
        }
    }

    /// A nonzero [`LibraryResponse::SearchResults::excluded_for_missing_curves`]
    /// survives the wire (the round-trip test above only exercises `0`, which
    /// wouldn't catch a decode that leaves the field at its zero default).
    #[test]
    fn search_results_exclusion_count_survives_the_wire() {
        let resp = LibraryResponse::SearchResults {
            items: vec![sample_summary()],
            excluded_for_missing_curves: 17,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &resp).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, resp);
        let LibraryResponse::SearchResults {
            excluded_for_missing_curves,
            ..
        } = decoded
        else {
            panic!("expected SearchResults, got {decoded:?}");
        };
        assert_eq!(excluded_for_missing_curves, 17);
    }

    /// [`LibraryRequest::FetchDesignSource`] round-trips its `entry_id` -- a dedicated
    /// test (on top of this field's coverage in
    /// [`library_request_variants_round_trip`]) so this new variant's own contract is
    /// pinned by name.
    #[test]
    fn fetch_design_source_request_round_trips() {
        let req = LibraryRequest::FetchDesignSource { entry_id: 7 };
        let mut buf = Vec::new();
        write_message(&mut buf, &req).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, req);
    }

    /// [`LibraryResponse::DesignSource`] round-trips every field, including `asc_text`
    /// content that itself contains newlines (a real `.asc` file's shape) -- a
    /// dedicated test (on top of this variant's coverage in
    /// [`library_response_variants_round_trip`]) so this new variant's own contract is
    /// pinned by name.
    #[test]
    fn design_source_response_round_trips() {
        let resp = LibraryResponse::DesignSource {
            entry_id: 7,
            file_name: "test.asc".to_string(),
            asc_text: "GemCad 5.0\nR1  c 41.0 96\n".to_string(),
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &resp).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, resp);
    }

    /// [`LibraryResponse::DesignSourceNotAvailable`] round-trips as its own distinct
    /// variant, not collapsing onto [`LibraryResponse::NotFound`] -- the two carry
    /// different meanings (see [`LibraryResponse::DesignSourceNotAvailable`]'s own doc
    /// comment).
    #[test]
    fn design_source_not_available_round_trips_and_differs_from_not_found() {
        let mut buf = Vec::new();
        write_message(&mut buf, &LibraryResponse::DesignSourceNotAvailable).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, LibraryResponse::DesignSourceNotAvailable);
        assert_ne!(decoded, LibraryResponse::NotFound);
    }

    /// Same property as [`search_results_exclusion_count_survives_the_wire`], for
    /// [`LibraryResponse::SearchResultsPage`] (whose field is currently always `0` in
    /// practice).
    #[test]
    fn search_results_page_exclusion_count_survives_the_wire() {
        let resp = LibraryResponse::SearchResultsPage {
            results: vec![sample_summary()],
            next_cursor: Some(42),
            excluded_for_missing_curves: 3,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &resp).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, resp);
        let LibraryResponse::SearchResultsPage {
            excluded_for_missing_curves,
            ..
        } = decoded
        else {
            panic!("expected SearchResultsPage, got {decoded:?}");
        };
        assert_eq!(excluded_for_missing_curves, 3);
    }
}
