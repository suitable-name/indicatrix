//! Serves `indicatrix_net::library`'s read-only design-library protocol.
//!
//! Backed by a `indicatrix_vault::db::sqlite::Database` -- the shared handler both a
//! library-only build and a `worker` build call for `ClientMessage::Library`.
//!
//! [`handle_request`] is the one entry point: given one `LibraryRequest` and the
//! `Database` opened at startup, it returns exactly one `LibraryResponse`
//! (request/response, never streamed). It never panics on a database error or an
//! unknown id: a query failure becomes `LibraryResponse::Error` (logged in full
//! server-side, reported to the peer only as a generic message), and a
//! `FetchDesign`/`FetchAttachment` for an unmatched id becomes `LibraryResponse::NotFound`.
//!
//! # Versioning: a content hash and a revision token, computed here
//!
//! `DesignSummary::version` is a SHA-256 hash computed at response time (see
//! [`convert::to_summary`], backed by [`version_hash`]) over exactly the fields a search
//! row carries except `entry_id` (the database's row number, not part of the design), in
//! a fixed order, each length-prefixed so adjacent fields can never collide, and each
//! `Option` tagged present/absent before its value.
//!
//! An edit to the angle table, notes, attachments or ratios leaves that hash unchanged,
//! and hashing the full record for every search row would cost up to a thousand record
//! loads per page. So `DesignSummary::design_version` and `DesignRecord::version` are
//! instead one revision token ([`version_hash::revision_token`]): a hash of the design's
//! url and `diagram_entries.updated_at`, the stamp every write that replaces or edits a
//! design bumps. It costs one primary-key lookup per row ([`convert::stamp_design_versions`])
//! and is a version token, not a content hash -- it says "changed since I fetched", not
//! what changed. A `FetchDesign` reads the stamp before loading the record, so a
//! concurrent edit can only make the token older than the content it came with.
//!
//! # Module layout
//!
//! - [`handlers`]: one function per [`LibraryRequest`] variant, plus the shared
//!   [`handlers::db_error`] helper and the two `<- ERROR` codes this module owns.
//! - [`convert`]: wire conversions in both directions between `indicatrix_net::library`
//!   and `indicatrix_vault`'s local storage-layer types.
//! - [`version_hash`]: the summary hash and the revision token backing the version
//!   fields of `DesignSummary`/`DesignRecord`.

use indicatrix_net::library::{LibraryRequest, LibraryResponse};
use indicatrix_vault::db::sqlite::Database;

mod convert;
mod handlers;
#[cfg(test)]
mod tests;
mod version_hash;

use version_hash::Revision;

pub use handlers::{LIBRARY_ERROR_CODE, LIBRARY_VALIDATION_ERROR_CODE};

/// Handles one [`LibraryRequest`] against `db`, producing exactly one [`LibraryResponse`].
/// See the module doc comment.
#[must_use]
pub fn handle_request(request: &LibraryRequest, db: &Database) -> LibraryResponse {
    match request {
        LibraryRequest::Search {
            query,
            shape_filter,
            gear_filter,
            range,
            order,
            tag_filter,
        } => stamp_search_reply(
            handlers::search(
                db,
                query,
                shape_filter,
                gear_filter,
                range,
                *order,
                tag_filter.as_deref(),
            ),
            db,
        ),
        LibraryRequest::FilterOptions => handlers::filter_options(db),
        LibraryRequest::FetchDesign { entry_id } => fetch_design(db, *entry_id),
        LibraryRequest::FetchAttachment { attachment_id } => {
            match db.get_attachment_content(*attachment_id) {
                Ok(Some((name, content))) => LibraryResponse::Attachment { name, content },
                Ok(None) => LibraryResponse::NotFound,
                Err(e) => handlers::db_error("get_attachment_content", &e),
            }
        }
        LibraryRequest::SearchPage {
            query,
            shape_filter,
            gear_filter,
            range,
            cursor,
        } => stamp_search_reply(
            handlers::search_page(db, query, shape_filter, gear_filter, range, *cursor),
            db,
        ),
        LibraryRequest::FetchDesignSource { entry_id } => handlers::design_source(db, *entry_id),
    }
}

/// Handles [`LibraryRequest::FetchDesign`]: the full record of `entry_id`, or
/// [`LibraryResponse::NotFound`].
fn fetch_design(db: &Database, entry_id: i64) -> LibraryResponse {
    // Read before the record so an edit landing in between leaves the record's token
    // older than its content (one extra re-fetch), never newer (a missed edit).
    let revision = Revision::read(db, entry_id);
    match db.get_diagram_full_meta(entry_id) {
        Ok(Some(meta)) => {
            // `get_preview_material` reads only the `preview_material` column,
            // never the two cached preview PNGs `get_preview_images` would also
            // load and decode just to discard here (see that method's own doc
            // comment). A failed lookup degrades to `None` rather than failing the
            // whole fetch; worst case is the re-render offer not appearing.
            let preview_material = db.get_preview_material(entry_id).ok().flatten();
            LibraryResponse::Design(Box::new(convert::to_record(
                &meta,
                preview_material,
                revision,
            )))
        }
        Ok(None) => LibraryResponse::NotFound,
        Err(e) => handlers::db_error("get_diagram_full_meta", &e),
    }
}

/// Fills `DesignSummary::design_version` on the rows of a search reply (the handlers
/// build the rows without the revision stamp); any other reply passes through unchanged.
fn stamp_search_reply(mut response: LibraryResponse, db: &Database) -> LibraryResponse {
    match &mut response {
        LibraryResponse::SearchResults { items, .. } => convert::stamp_design_versions(items, db),
        LibraryResponse::SearchResultsPage { results, .. } => {
            convert::stamp_design_versions(results, db);
        }
        _ => {}
    }
    response
}
