//! The read-only design-library sync protocol.
//!
//! A client (a future viewer, or a mobile client with no renderer compiled in) mirrors
//! designs from a `indicatrix-worker`'s catalogue.
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
//! # Staleness: a content hash and a revision token
//!
//! [`DesignSummary::version`] is a SHA-256 content hash over the fields the summary
//! carries (excluding the hash field itself and `entry_id`, which is a per-database row
//! id and differs between databases holding the same design). `indicatrix-worker`
//! computes it at serve time; this crate only carries the resulting 32 bytes.
//!
//! [`DesignRecord::version`] and [`DesignSummary::design_version`] are not content
//! hashes: they are an O(1) revision token derived from the design's `url` and its
//! `diagram_entries.updated_at`, so the summary can name the full record's revision
//! without reading the detail rows or image bytes. Equal tokens mean the same stored
//! revision; a token that changes and later changes back looks unchanged, but that's
//! harmless here: worst case is one wasted round trip.
//!
//! [`DesignSummary::version`] is the cheap first filter (up to 1000 rows per search);
//! compare [`DesignSummary::design_version`] against the mirrored token to decide whether
//! a `FetchDesign` is needed, and [`DesignRecord::version`] is the same token on the
//! fetched record.
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
//! finds and returns that design's own `.asc` cutting-instructions attachment's TEXT (not
//! an arbitrary attachment's raw bytes), decoded as UTF-8. This is what lets
//! `apps/indicatrix-cut`'s cutting-design editor load a remote design exactly the way
//! it already loads a local one -- see [`LibraryResponse::DesignSourceNotAvailable`]
//! for what a design with no `.asc` attachment gets instead.
//!
//! Split across submodules by topic:
//!
//! - [`filter`]: wire counterparts of `indicatrix_vault`'s search/filter types.
//! - [`record`]: the design data types a [`LibraryResponse`] carries.
//! - [`protocol`]: the request/response envelope, [`LibraryRequest`]/[`LibraryResponse`].
//!
//! Every item is re-exported here at its original flat `library::` path.

mod filter;
mod protocol;
mod record;
#[cfg(test)]
mod tests;

// So that `tests`' `use super::*;` (see `tests.rs`, moved unaltered) resolves
// `ErrorMsg` without every test needing its own import -- the same convention
// `crate::db::sqlite`'s own `#[cfg(test)]` imports follow in the sibling `indicatrix-vault`
// crate.
#[cfg(test)]
use crate::messages::ErrorMsg;

pub use filter::{
    AttributeRangesWire, PerformanceAggregateWire, PerformanceBoundWire, PerformanceFilterWire,
    PerformanceMetricWire, RangeFilterWire, SortOrderWire,
};
pub use protocol::{LibraryRequest, LibraryResponse};
pub use record::{AngleSettingWire, AttachedFileMeta, DesignRecord, DesignSummary};
