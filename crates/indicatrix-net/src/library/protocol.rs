//! The request/response envelope of the read-only library-sync protocol:
//! [`LibraryRequest`] and its reply [`LibraryResponse`].

use serde::{Deserialize, Serialize};

use super::{
    filter::{AttributeRangesWire, RangeFilterWire, SortOrderWire},
    record::{DesignRecord, DesignSummary},
};
use crate::messages::ErrorMsg;

/// One request in the read-only library-sync protocol. Deliberately request/response,
/// not streamed like `RENDER`; see the module docs for the push-extension room left in
/// the shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LibraryRequest {
    /// List/search designs -- mirrors
    /// `indicatrix_vault::db::sqlite::Database::search_diagrams_display`'s filters
    /// one-for-one: the sort order and tag chip are carried on this variant, not
    /// silently dropped. Capped at that method's result cap (currently 1000);
    /// see [`Self::SearchPage`] for the paginated form a full-catalogue walk needs
    /// instead.
    ///
    /// # Deliberately NOT on the wire: `local_only`, `id_filter`
    ///
    /// `indicatrix_vault::db::sqlite::DisplayFilters` also carries a `local_only`
    /// ("My designs") flag and an `id_filter` (an explicit entry-id allowlist, used by
    /// a local batch action). Neither is meaningful against a remote catalogue:
    /// `local_only` names designs synced under *this* database's own `local://` import
    /// scheme, and `id_filter` names entry ids from *this* database's own rows -- both
    /// are per-database concepts specific to the client's own local vault, not
    /// something a remote worker's catalogue could answer. Sending them would either
    /// be silently ignored server-side or (worse) accidentally match unrelated rows
    /// that happen to share an id in the remote catalogue.
    Search {
        query: String,
        shape_filter: String,
        gear_filter: String,
        range: RangeFilterWire,
        /// Sort order for the results -- see [`SortOrderWire`].
        order: SortOrderWire,
        /// Restricts results to designs carrying this tag, by NAME (never an id: tag
        /// ids are assigned per-database, so an id minted by one catalogue is
        /// meaningless -- and could accidentally collide with an unrelated tag -- in
        /// another). `None` for no restriction. `indicatrix-worker` resolves the name
        /// to its own local tag id via `tag_id_by_name`; a name that worker has never
        /// seen resolves to no restriction, the same tolerance the app itself already
        /// has for an unknown tag.
        tag_filter: Option<String>,
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
    /// Fetch one design's original `.asc` cutting-instructions TEXT (its actual file
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
    /// Its `code` is from the same shared [`crate::messages::error_codes`] table as every
    /// other `ErrorMsg` (`LIBRARY_FAILED`, `LIBRARY_REQUEST_INVALID`, or
    /// `NO_RENDER_CAPACITY` on a joined worker's connection).
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
    /// attachment's own bare file name (for `save_paired`-style "Save"
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
