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
//! # Versioning: a content hash computed here, not stored in the database
//!
//! `DesignSummary::version`/`DesignRecord::version` are SHA-256 hashes computed at
//! response time (see [`convert::to_summary`]/[`convert::to_record`], backed by
//! [`version_hash`]) over exactly the fields that response carries, in a fixed order,
//! each length-prefixed so adjacent fields can never collide, and each `Option` tagged
//! present/absent before its value. `indicatrix_vault`'s schema has no `updated_at`/
//! revision column to read instead.
//!
//! # Module layout
//!
//! - [`handlers`]: one function per [`LibraryRequest`] variant, plus the shared
//!   [`handlers::db_error`] helper and the two `<- ERROR` codes this module owns.
//! - [`convert`]: wire conversions in both directions between `indicatrix_net::library`
//!   and `indicatrix_vault`'s local storage-layer types.
//! - [`version_hash`]: the SHA-256 computation backing `DesignSummary`/`DesignRecord`'s
//!   `version` field.

use indicatrix_net::library::{LibraryRequest, LibraryResponse};
use indicatrix_vault::db::sqlite::Database;

mod convert;
mod handlers;
#[cfg(test)]
mod tests;
mod version_hash;

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
        } => handlers::search(
            db,
            query,
            shape_filter,
            gear_filter,
            range,
            *order,
            tag_filter.as_deref(),
        ),
        LibraryRequest::FilterOptions => handlers::filter_options(db),
        LibraryRequest::FetchDesign { entry_id } => match db.get_diagram_full_meta(*entry_id) {
            Ok(Some(meta)) => {
                // A failed preview lookup degrades to `None` rather than failing the
                // whole fetch; worst case is the re-render offer not appearing.
                let preview_material = db
                    .get_preview_images(*entry_id)
                    .ok()
                    .and_then(|p| p.material);
                LibraryResponse::Design(Box::new(convert::to_record(&meta, preview_material)))
            }
            Ok(None) => LibraryResponse::NotFound,
            Err(e) => handlers::db_error("get_diagram_full_meta", &e),
        },
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
        } => handlers::search_page(db, query, shape_filter, gear_filter, range, *cursor),
        LibraryRequest::FetchDesignSource { entry_id } => handlers::design_source(db, *entry_id),
    }
}
