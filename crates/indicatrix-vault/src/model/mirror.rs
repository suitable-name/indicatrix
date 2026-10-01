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

/// One design's last-known remote version values, as of this database's most recent
/// successful sync of it.
///
/// [`Self::summary_version`] is the cheap, search-result-level content hash
/// (`indicatrix_net::library::DesignSummary::version`); [`Self::design_version`] is the
/// full record's O(1) revision token (`DesignRecord::version`, derived from the url and
/// `diagram_entries.updated_at`, not a content hash). A mirror sync compares the former
/// first (skip if unmoved); the latter is the second tier, for a remote edit that leaves
/// the summary hash unchanged -- the wire summary carries it as
/// `DesignSummary::design_version`, so it can be compared without fetching the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorState {
    /// URL of the mirrored design.
    pub url: String,
    /// Which remote server this state was last synced from -- sibling convention to
    /// `crate::db::sqlite::LEGACY_SOURCE_ID`/`crate::local::LOCAL_SOURCE_ID`; a mirror
    /// sync's own `source_id` is the worker's configured address.
    pub source_id: String,
    /// Version digest of the summary row.
    pub summary_version: [u8; 32],
    /// Version digest of the full design.
    pub design_version: [u8; 32],
    /// Tombstone: the user deleted this design locally
    /// ([`crate::db::sqlite::Database::delete_diagram_entry`] sets it). A mirror pass
    /// skips a tombstoned url whatever the remote hashes are, so the deletion holds even
    /// after the remote design changes. Written back to `false` only by a caller that
    /// deliberately re-mirrors the design.
    #[serde(default)]
    pub deleted_locally: bool,
}
