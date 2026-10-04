//! Per-[`LibraryRequest`](indicatrix_net::library::LibraryRequest)-variant handlers:
//! search, filter options, keyset-paginated search, and the `.asc` design-source
//! lookup. [`super::handle_request`] dispatches to these; [`db_error`] is the shared
//! "log in full, report generically" helper every handler here uses on a database
//! failure.

use indicatrix_net::{
    library::{LibraryResponse, RangeFilterWire, SortOrderWire},
    messages::{ErrorMsg, error_codes},
};
use indicatrix_vault::db::sqlite::{Database, DisplayFilters, SEARCH_RESULT_CAP, SortOrder};

use super::convert::{from_range_wire, to_ranges_wire, to_summary};

/// `LibraryResponse::Error` code for a library request refused before any database work.
///
/// Currently only a [`LibraryRequest`](indicatrix_net::library::LibraryRequest) with an
/// out-of-range `tilt_radius_deg` (see [`super::convert::from_range_wire`]).
///
/// An alias of [`error_codes::LIBRARY_REQUEST_INVALID`]: `LibraryResponse::Error` shares
/// the one `ErrorMsg::code` namespace with every other error reply (see that module's
/// doc), so its value is unique across the whole table. Distinct from
/// [`LIBRARY_ERROR_CODE`] so a client can tell "malformed request" apart from "server
/// hit a problem".
pub const LIBRARY_VALIDATION_ERROR_CODE: u32 = error_codes::LIBRARY_REQUEST_INVALID;

/// `LibraryResponse::Error` code for a `LibraryRequest` that failed on this server's side.
///
/// An alias of [`error_codes::LIBRARY_FAILED`], unique across the shared table (see
/// [`LIBRARY_VALIDATION_ERROR_CODE`]).
pub const LIBRARY_ERROR_CODE: u32 = error_codes::LIBRARY_FAILED;

/// Handles [`LibraryRequest::Search`](indicatrix_net::library::LibraryRequest::Search):
/// resolves `order`/`tag_filter` and calls `Database::search_diagrams_display`, so a
/// remote search honours the same sort order and tag-chip restriction a local one does
/// -- rather than the catalogue-order-only `search_diagrams_with_performance_exclusions`,
/// which ignores both.
///
/// `tag_filter` is resolved by NAME via [`Database::tag_id_by_name`] (see
/// `LibraryRequest::Search`'s own doc comment for why a name crosses the wire, never an
/// id): a name this worker's catalogue has never seen resolves to `None` -- no
/// restriction -- the same tolerance `apps/indicatrix-cut`'s own tag-chip lookup has for
/// an unknown tag, rather than an error over an absent chip.
///
/// `local_only`/`id_filter` on [`DisplayFilters`] are always `false`/`None` here: both
/// name concepts local to the client's own database (see `LibraryRequest::Search`'s
/// doc comment) that never cross the wire.
pub(super) fn search(
    db: &Database,
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilterWire,
    order: SortOrderWire,
    tag_filter: Option<&str>,
) -> LibraryResponse {
    let range = match from_range_wire(range) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let tag_filter = match tag_filter.map(|name| db.tag_id_by_name(name)) {
        Some(Ok(id)) => id,
        Some(Err(e)) => return db_error("tag_id_by_name", &e),
        None => None,
    };
    let filters = DisplayFilters {
        order: sort_order_from_wire(order),
        local_only: false,
        tag_filter,
        id_filter: None,
    };
    match db.search_diagrams_display(query, shape_filter, gear_filter, &range, filters) {
        Ok(result) => LibraryResponse::SearchResults {
            items: result.items.iter().map(to_summary).collect(),
            // Capped at SEARCH_RESULT_CAP (1000), so this cast never truncates.
            excluded_for_missing_curves: result.excluded_for_missing_curves as u32,
        },
        Err(e) => db_error("search_diagrams_display", &e),
    }
}

/// Maps a wire [`SortOrderWire`] onto the vault's local [`SortOrder`]. A plain
/// `match`, not a shared derive, since the two types deliberately live in different
/// crates -- see [`SortOrderWire`]'s own doc comment.
pub(super) const fn sort_order_from_wire(order: SortOrderWire) -> SortOrder {
    match order {
        SortOrderWire::CatalogueOrder => SortOrder::CatalogueOrder,
        SortOrderWire::Title => SortOrder::Title,
        SortOrderWire::Newest => SortOrder::Newest,
        SortOrderWire::RecentlyEdited => SortOrder::RecentlyEdited,
    }
}

/// Handles
/// [`LibraryRequest::FetchDesignSource`](indicatrix_net::library::LibraryRequest::FetchDesignSource):
/// finds `entry_id`'s design-file attachment with the same rule the client's own
/// local load path uses (`indicatrix_vault::local::design_attachment_position`: the
/// first `.asc`, else the first `.gem`, else the first `.gcs`) and returns it as
/// `.asc` text ([`design_source_reply`]) -- matching that same local path so a
/// design loads identically whether it came from this worker or a local file.
///
/// Three distinct outcomes, matching
/// [`LibraryRequest::FetchDesign`](indicatrix_net::library::LibraryRequest::FetchDesign)'s
/// own convention for `NotFound` plus one new case:
/// - No such `entry_id` at all -> [`LibraryResponse::NotFound`] (same as `FetchDesign`).
/// - `entry_id` exists but has no `.asc`/`.gem`/`.gcs` attachment -> [`LibraryResponse::DesignSourceNotAvailable`]
///   (a design with only a reconstructed, placeholder schedule has no real file bytes to
///   send -- see that variant's own doc comment).
/// - A design-file attachment exists -> [`LibraryResponse::DesignSource`] with its
///   name (for a `.gem`/`.gcs`, the converted `<stem>.asc`) and `.asc` text, or
///   [`LibraryResponse::Error`] when a `.gem`/`.gcs` does not read.
///
/// No bespoke size cap here: a `.asc` schedule is always a small text file, and this
/// reply is bounded the same way [`LibraryResponse::Attachment`] already is, by the
/// transport-level `indicatrix_net::framing::MAX_FRAME_LEN`, not a second check
/// duplicated per response type.
pub(super) fn design_source(db: &Database, entry_id: i64) -> LibraryResponse {
    let meta = match db.get_diagram_full_meta(entry_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            tracing::debug!("FetchDesignSource: entry {entry_id} not found");
            return LibraryResponse::NotFound;
        }
        Err(e) => return db_error("get_diagram_full_meta", &e),
    };
    let Some((position, kind)) = indicatrix_vault::local::design_attachment_position(
        meta.attached_files.iter().map(|f| f.name.as_str()),
    ) else {
        tracing::debug!("FetchDesignSource: entry {entry_id} has no .asc/.gem/.gcs attachment");
        return LibraryResponse::DesignSourceNotAvailable;
    };
    let attachment = &meta.attached_files[position];
    match db.get_attachment_content(attachment.id) {
        Ok(Some((name, content))) => {
            tracing::debug!(
                "FetchDesignSource: entry {entry_id} serving attachment {} ('{name}', {} bytes)",
                attachment.id,
                content.len()
            );
            design_source_reply(entry_id, &name, kind, &content)
        }
        // The metadata query just found this attachment; content going missing here
        // would mean a concurrent delete raced this request -- treat it the same as
        // "no .asc attachment" rather than a hard error.
        Ok(None) => {
            tracing::debug!(
                "FetchDesignSource: entry {entry_id}'s .asc attachment {} vanished concurrently",
                attachment.id
            );
            LibraryResponse::DesignSourceNotAvailable
        }
        Err(e) => db_error("get_attachment_content", &e),
    }
}

/// Handles
/// [`LibraryRequest::FetchDesignNative`](indicatrix_net::library::LibraryRequest::FetchDesignNative):
/// returns `entry_id`'s first self-contained `.indicatrix` attachment (by
/// `indicatrix_vault::local::native_design_attachment_position`, the rule the local
/// loader uses) as raw bytes -- never parsed or re-encoded here, so nothing is lost on
/// the way to a client that must see the concave tiers `.asc` cannot carry.
///
/// Same three outcomes as [`design_source`]: no such entry -> `NotFound`; no native
/// attachment (or one that vanished mid-request) -> `DesignNativeNotAvailable`; else
/// `DesignNative`.
pub(super) fn design_native(db: &Database, entry_id: i64) -> LibraryResponse {
    let meta = match db.get_diagram_full_meta(entry_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            tracing::debug!("FetchDesignNative: entry {entry_id} not found");
            return LibraryResponse::NotFound;
        }
        Err(e) => return db_error("get_diagram_full_meta", &e),
    };
    let Some(position) = indicatrix_vault::local::native_design_attachment_position(
        meta.attached_files.iter().map(|f| f.name.as_str()),
    ) else {
        tracing::debug!("FetchDesignNative: entry {entry_id} has no .indicatrix attachment");
        return LibraryResponse::DesignNativeNotAvailable;
    };
    match db.get_attachment_content(meta.attached_files[position].id) {
        Ok(Some((file_name, content))) => LibraryResponse::DesignNative {
            entry_id,
            file_name,
            content,
        },
        Ok(None) => LibraryResponse::DesignNativeNotAvailable,
        Err(e) => db_error("get_attachment_content", &e),
    }
}

/// [`design_source`]'s reply for one design-file attachment's bytes: a `.asc` as
/// its own decoded text (Windows-1252 aware -- a legacy degree sign must reach the
/// viewer as `°`, not as U+FFFD), a `.gem`/`.gcs` converted to `.asc` cutting
/// instructions under `<stem>.asc` (`indicatrix_vault::local::design_file_to_asc_text`,
/// the same conversion the desktop editor's local record loader uses). The
/// conversion runs inside `catch_unwind`, so one malformed file is an error reply,
/// never a dead connection thread; a file that does not read is a
/// [`LibraryResponse::Error`] carrying the reader's message.
fn design_source_reply(
    entry_id: i64,
    name: &str,
    kind: indicatrix_vault::local::DesignFileKind,
    content: &[u8],
) -> LibraryResponse {
    let converted = std::panic::catch_unwind(|| {
        indicatrix_vault::local::design_file_to_asc_text(name, kind, content)
    });
    let failure = match converted {
        Ok(Ok(file)) => {
            return LibraryResponse::DesignSource {
                entry_id,
                file_name: file.asc_file_name,
                asc_text: file.asc_text,
            };
        }
        Ok(Err(e)) => format!("'{name}' could not be read: {e}"),
        Err(_) => format!("'{name}' could not be read: internal error"),
    };
    tracing::warn!("FetchDesignSource: entry {entry_id}: {failure}");
    LibraryResponse::Error(ErrorMsg {
        code: LIBRARY_ERROR_CODE,
        message: failure,
        request_id: None,
    })
}

/// Handles
/// [`LibraryRequest::SearchPage`](indicatrix_net::library::LibraryRequest::SearchPage):
/// one keyset-paginated page of `Database::search_diagrams_page`, converted to
/// [`LibraryResponse::SearchResultsPage`].
///
/// Pages at [`SEARCH_RESULT_CAP`] rows. `next_cursor` is `Some` (the last row's
/// `entry_id`) exactly when the page came back full -- cheap-but-not-exact (an
/// occasional harmless extra empty final page) rather than a second COUNT query.
///
/// `excluded_for_missing_curves` is always `0` here: `Database` has no page-scoped
/// counterpart of `search_diagrams_with_performance_exclusions`. Known limitation, not
/// an oversight -- harmless today since this variant's only real caller
/// (`library_mirror`'s exhaustive walk) never sets a performance filter.
pub(super) fn search_page(
    db: &Database,
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilterWire,
    cursor: Option<i64>,
) -> LibraryResponse {
    let range = match from_range_wire(range) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match db.search_diagrams_page(
        query,
        shape_filter,
        gear_filter,
        &range,
        cursor,
        SEARCH_RESULT_CAP,
    ) {
        Ok(items) => {
            let page_full = items.len() == usize::try_from(SEARCH_RESULT_CAP).unwrap_or(usize::MAX);
            let next_cursor = if page_full {
                items.last().map(|i| i.id)
            } else {
                None
            };
            LibraryResponse::SearchResultsPage {
                results: items.iter().map(to_summary).collect(),
                next_cursor,
                // See this function's own doc comment for why this is always `0`.
                excluded_for_missing_curves: 0,
            }
        }
        Err(e) => db_error("search_diagrams_page", &e),
    }
}

/// Handles
/// [`LibraryRequest::FilterOptions`](indicatrix_net::library::LibraryRequest::FilterOptions).
pub(super) fn filter_options(db: &Database) -> LibraryResponse {
    let shapes = match db.get_unique_shapes() {
        Ok(v) => v,
        Err(e) => return db_error("get_unique_shapes", &e),
    };
    let gears = match db.get_unique_gears() {
        Ok(v) => v,
        Err(e) => return db_error("get_unique_gears", &e),
    };
    let ranges = match db.get_attribute_ranges() {
        Ok(v) => v,
        Err(e) => return db_error("get_attribute_ranges", &e),
    };
    LibraryResponse::FilterOptions {
        shapes,
        gears,
        ranges: to_ranges_wire(ranges),
    }
}

/// Logs the real reason server-side and returns a generic [`LibraryResponse::Error`] --
/// the peer never sees the underlying database error text.
pub(super) fn db_error(op: &str, e: &anyhow::Error) -> LibraryResponse {
    tracing::warn!("library request failed ({op}): {e:#}");
    server_error()
}

/// The generic [`LibraryResponse::Error`] for a request this server could not serve:
/// it names no cause, so a caller that already logged the real reason (here
/// [`db_error`], and [`super::LibraryHandle`] for a database that would not open) can
/// reply with it as is.
pub(super) fn server_error() -> LibraryResponse {
    LibraryResponse::Error(ErrorMsg {
        code: LIBRARY_ERROR_CODE,
        message: "internal error serving the design library".to_string(),
        // The library protocol has no request_id/epoch to be stale against.
        request_id: None,
    })
}
