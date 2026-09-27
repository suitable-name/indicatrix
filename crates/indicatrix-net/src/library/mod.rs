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
