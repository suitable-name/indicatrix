//! Bookkeeping for a local mirror of a remote design-library server -- see
//! `apps/indicatrix-cut`'s pull-mirror sync.
//!
//! `indicatrix_net::library`'s wire protocol carries a content-hash "version" on both
//! `DesignSummary` and `DesignRecord` so a mirroring client can skip re-fetching a
//! design whose hash hasn't moved (see that crate's `library` module doc, "Staleness").
//! But `diagram_entries`/`diagram_details` have no column for the last-seen hash, and
//! deliberately can't grow one (a `indicatrix-worker` serving that data is read-only
//! against it) -- this table is the client-side, purely local equivalent.
//!
//! Keyed by `url`, not `entry_id`, reusing this database's existing identity for "is a
//! synced design the same as a row already here"
//! ([`crate::db::sqlite::Database::save_diagram_entry`]'s `UNIQUE` constraint on
//! `diagram_entries.url`; a local `.asc` import gets a synthetic `local://...` URL so it
//! can never collide with a real remote page URL, see `crate::local::import_asc`) --
//! this table never needs its own "same design" decision.
use serde::{Deserialize, Serialize};

/// One design's last-known remote content hashes, as of this database's most recent
/// successful sync of it.
///
/// [`Self::summary_version`] is the cheap, search-result-level hash
/// (`indicatrix_net::library::DesignSummary::version`); [`Self::design_version`] is the
/// authoritative full-record hash (`DesignRecord::version`) -- a mirror sync checks the
/// former first (skip if unmoved) and the latter before deciding a full re-fetch
/// actually changed anything worth re-saving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorState {
    pub url: String,
    /// Which remote server this state was last synced from -- sibling convention to
    /// `crate::db::sqlite::LEGACY_SOURCE_ID`/`crate::local::LOCAL_SOURCE_ID`; a mirror
    /// sync's own `source_id` is the worker's configured address.
    pub source_id: String,
    pub summary_version: [u8; 32],
    pub design_version: [u8; 32],
}
