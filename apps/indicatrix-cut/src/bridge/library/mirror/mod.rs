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
//! attachment, or custom material, and it never overwrites a row it did not itself put
//! there.** A local design's identity is its `diagram_entries.url` (a `UNIQUE` column;
//! a locally-imported `.asc` file gets a synthetic `local://<file name>` URL). That
//! scheme does NOT make a `local://...` row unreachable here: a worker can serve its
//! own local imports back over the library protocol under the same synthetic URL
//! scheme, so a remote `DesignSummary`/[`DesignRecord`] can genuinely name a URL a
//! hand-imported local row already owns (the local-row guard). [`sync::design::sync_one_design`]
//! guards against this explicitly -- before ever writing, it checks whether a local row
//! for that `url` already exists and, if so, whether `library_mirror_state` shows it
//! was put there by a PREVIOUS mirror sync; a pre-existing row with no mirror state is
//! left completely untouched (counted as [`options::MirrorCounts::local_conflicts_skipped`],
//! not treated as new/updated or as a failure). There is no "delete anything local sync
//! didn't just see" step either -- unlike a naive rsync-style mirror, "make local match
//! remote" is never attempted, only "pull what remote has, without clobbering what's
//! already here" is.
//!
//! # Local delete wins
//!
//! Deleting a design locally does not remove its `library_mirror_state` row (see that
//! table's own doc comment for why there is deliberately no `ON DELETE CASCADE` back to
//! it): `Database::delete_diagram_entry` marks the row `deleted_locally` -- a tombstone
//! -- in the same transaction as the delete. [`sync::run_mirror_sync`] skips a
//! tombstoned url before any fetch, whatever the remote hashes are, so a design the
//! user deleted after mirroring it once is never resurrected by a later sync, not even
//! after the remote design changes. Comparing hashes alone would not hold: a changed
//! remote summary makes the stored one differ, and the design would be re-created. This
//! is intentional (owner decision), not an oversight: "local delete wins" over "remote
//! mirror always wins". The cost is that the deleted design's `library_mirror_state`
//! row lingers forever with nothing local to point at --
//! [`options::MirrorCounts::skipped_deleted`] counts the tombstoned designs a pass met
//! in the remote catalogue, and [`options::MirrorCounts::orphaned_mirror_states`] (backed by
//! `Database::count_mirror_states_without_entry`) is how the sync summary surfaces that
//! count instead of leaving it entirely invisible. A catalogue that predates the
//! tombstone column has its already-orphaned rows tombstoned by the migration.
//!
//! # Identity and staleness
//!
//! A remote design's `url` is the same field `diagram_entries.url`'s `UNIQUE`
//! constraint uses to decide "is this design already in the database" -- no new
//! identity scheme invented. [`crate::model::mirror::MirrorState`]
//! remembers, per `url`, the last summary hash seen; a sync enumerates the whole remote
//! catalogue's summaries first, then skips a design entirely (no `FetchDesign`, no
//! attachment fetch, no local write) when its hash hasn't moved. A no-op second sync of
//! an unchanged catalogue costs one `SearchPage` request per page plus one
//! `library_mirror_state` read per known design -- zero `FetchDesign`/`FetchAttachment`.
//!
//! The summary hash covers only the search-result fields, so a remote edit to the angle
//! table, notes, attachments or ratios leaves it unchanged. Every summary therefore also
//! carries the design's revision token (`DesignSummary::design_version`), which the
//! worker derives from its per-design revision stamp and which equals the `version` of
//! the fetched record; the state stores it, and [`sync::run_mirror_sync`] compares it as
//! a second tier. It is a version token, not a content hash: any edit on the remote
//! moves it, and a summary that states none (all zero bytes) counts as moved. The
//! worker's summary hash deliberately leaves the remote `entry_id` out, so renumbering
//! the remote database does not force a full re-fetch.
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
//! completion (attachments fetched first, then one transactional `save_design` write
//! of entry and detail together) before the next check.
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
