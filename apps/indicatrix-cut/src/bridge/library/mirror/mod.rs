//! Pull-mirror sync: copies a remote `indicatrix-worker`'s design library into the
//! local `indicatrix_vault` database so it's available offline.
//!
//! Named `library_mirror`, not `sync`, deliberately: this is machine-to-machine
//! mirroring of the user's own library over the already-authenticated mutual-TLS
//! connection `bridge::remote::enroll` set up -- nothing here fetches a public URL,
//! parses HTML, or runs OCR.
//!
//! # The safety rule, and it is the whole point of this module
//!
//! **This sync is additive and update-only. It never deletes a local design,
//! attachment, or custom material.** A local design's identity is its
//! `diagram_entries.url` (a `UNIQUE` column; a locally-imported `.asc` file gets a
//! synthetic `local://<file name>` URL so it can never collide with a remote page URL).
//! This module only ever touches a row whose `url` a remote [`DesignSummary`]/
//! [`DesignRecord`] actually names, so a `local://...` row is structurally unreachable
//! here. There is no "delete anything local sync didn't just see" step -- unlike a
//! naive rsync-style mirror, "make local match remote" is never attempted, only "pull
//! what remote has" is.
//!
//! # Identity and staleness
//!
//! A remote design's `url` is the same field `diagram_entries.url`'s `UNIQUE`
//! constraint uses to decide "is this design already in the database" -- no new
//! identity scheme invented (cross-source duplicate detection for different sources
//! describing the same physical design is a separate, human-reviewed concern in
//! `crate::model::dedup`, untouched here). [`crate::model::mirror::MirrorState`]
//! remembers, per `url`, the last summary hash seen; a sync enumerates the whole remote
//! catalogue's summaries first, then skips a design entirely (no `FetchDesign`, no
//! attachment fetch, no local write) when its hash hasn't moved. A no-op second sync of
//! an unchanged catalogue costs one `SearchPage` request per page plus one
//! `library_mirror_state` read per known design -- zero `FetchDesign`/`FetchAttachment`.
//!
//! # Pagination
//!
//! [`sync::run_mirror_sync`] walks a keyset cursor (`LibraryRequest::SearchPage`,
//! `id > cursor`) to exhaustion before syncing begins, rather than `OFFSET`-based
//! paging -- a keyset walk can't skip or duplicate a row when another design is
//! inserted server-side mid-walk.
//!
//! # Attachments: fetched eagerly, up to a size cap
//!
//! Unlike interactive browsing (`bridge::library::client`, lazy fetch on open), a
//! mirror's whole purpose is offline availability, so every attachment for a saved
//! design is fetched immediately except one whose advertised
//! [`AttachedFileMeta::size`] exceeds [`options::MirrorOptions::max_attachment_bytes`]
//! -- a safety net against one pathological file, not a general skip switch. A skipped
//! attachment doesn't fail the whole design; only that file is left for a future sync.
//!
//! # Cancellation leaves the database consistent
//!
//! `cancel` is checked once per design, strictly before that design's network calls or
//! local write begin, never mid-design. Once a design starts, it always runs to
//! completion (attachments fetched first, then one transactional
//! `save_diagram_entry`/`save_diagram_detail` write) before the next check.
//! **Invariant:** at any interruption point, the local database reflects some prefix of
//! fully-completed per-design syncs plus whatever was there before -- no design is ever
//! left mid-write. Enumeration itself is not cancellable mid-walk (it does no local
//! writes) and fails outright, nothing written, on any request error.
//!
//! # Module split
//!
//! [`options`] holds the tuning-knob/progress/outcome types plus the
//! [`options::LibraryTransport`] abstraction the algorithm runs generically over;
//! [`sync`] holds the algorithm, its worker-thread wrapper, and its tests.

pub mod options;
pub mod sync;

pub use options::{MirrorHandle, MirrorOptions, MirrorOutcome, MirrorProgress};
pub use sync::spawn_mirror_sync;
