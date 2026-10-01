//! [`MirrorOptions`]/[`MirrorCounts`]/[`MirrorProgress`]/[`MirrorOutcome`]/
//! [`MirrorHandle`], and the [`LibraryTransport`] abstraction [`super::sync`]'s
//! algorithm runs generically over. See this group's own `mod.rs` doc comment.

use crate::{
    bridge::library::client::{self as library_client, LibrarySession},
    settings::WorkerSettings,
};
use indicatrix_net::library::{LibraryRequest, LibraryResponse};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// The `source_id` a mirror sync attributes every design it saves/updates to -- distinct
/// per configured worker (its address).
#[must_use]
pub fn mirror_source_id(worker: &WorkerSettings) -> String {
    format!("remote-library:{}", worker.address)
}

/// Tuning knobs for one mirror sync. `Default` is the sensible "just mirror everything
/// reasonable" choice a UI trigger with no advanced options needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorOptions {
    /// An attachment whose advertised [`indicatrix_net::library::AttachedFileMeta::size`]
    /// exceeds this is skipped (the design itself is still saved without it) -- see the
    /// module doc comment's "Attachments" section. `50 MiB` comfortably exceeds any real
    /// competition-results PDF or diagram image in this catalogue while still catching a
    /// genuinely anomalous file.
    pub max_attachment_bytes: u64,
}

impl Default for MirrorOptions {
    fn default() -> Self {
        Self {
            max_attachment_bytes: 50 * 1024 * 1024,
        }
    }
}

/// Running/final tallies for one mirror sync -- both [`MirrorProgress::counts`] (a live
/// snapshot, after each design) and [`MirrorOutcome`]'s completed/cancelled payload (the
/// final snapshot) use this same shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirrorCounts {
    /// Total designs the remote catalogue reported, across every `SearchPage` page.
    pub total_found: usize,
    /// Designs added to the local library by this sync.
    pub new_count: usize,
    /// Existing designs updated by this sync.
    pub updated_count: usize,
    /// Skipped without a `FetchDesign` at all -- neither the summary hash nor the design
    /// revision token changed since the last sync. See the module doc comment's
    /// "Identity and staleness" section.
    pub skipped_unchanged: usize,
    /// Skipped because the user deleted the mirrored design locally (tombstone); never fetched.
    pub skipped_deleted: usize,
    /// A design whose fetch or local save failed for any reason (network error,
    /// database error, or it vanished server-side between `Search` and `FetchDesign`).
    /// Left exactly as it was locally before this sync (if it existed at all) -- never
    /// marked as synced, so the next sync retries it.
    pub failed: usize,
    /// Attachment files downloaded by this sync.
    pub attachments_fetched: usize,
    /// An attachment skipped for exceeding [`MirrorOptions::max_attachment_bytes`] --
    /// its design was still saved, just without this one file's bytes.
    pub attachments_skipped_too_large: usize,
    /// Total bytes of attachments downloaded by this sync.
    pub attachment_bytes_fetched: u64,
    /// A remote design whose `url` already names a LOCAL row this sync has never
    /// mirrored (no `library_mirror_state` entry for it) -- left completely untouched
    /// rather than overwritten, per the "additive/update-only" rule in the mirror
    /// module's own doc comment (a worker serving its own `local://` imports, or
    /// a hand-imported design that happens to collide with a remote page URL, must
    /// never lose its data to a sync). Counted separately from [`Self::failed`] --
    /// this is not an error, and retrying it on the next sync would only skip it
    /// again the same way.
    pub local_conflicts_skipped: usize,
    /// How many `library_mirror_state` rows have no matching local design left
    /// (`Database::count_mirror_states_without_entry`) -- designs this sync (or an
    /// earlier one) mirrored, that the user then deleted locally. "Local delete wins"
    /// is the intended semantics (owner decision, see the mirror module's own doc
    /// comment): a mirror sync never resurrects a design the user deleted, and never
    /// will, so this count is what lets the sync summary say so instead of that
    /// orphaning being entirely invisible. Populated once, at the end of a sync
    /// ([`super::sync::run_mirror_sync`]'s return, whether completed or cancelled) --
    /// not a per-design running tally like the fields above.
    pub orphaned_mirror_states: u64,
}

/// One progress update, delivered after each design this sync examines (whether it was
/// skipped, saved, or failed).
#[derive(Debug, Clone)]
pub struct MirrorProgress {
    /// How many of `counts.total_found` designs have been examined so far, including
    /// this one.
    pub processed: usize,
    /// Running totals of the sync so far.
    pub counts: MirrorCounts,
    /// Title of the design currently being synced.
    pub current_title: String,
}

/// How [`super::sync::run_mirror_sync`] ended.
#[derive(Debug, Clone)]
pub enum MirrorOutcome {
    Completed(MirrorCounts),
    /// Stopped early via [`MirrorHandle::cancel`] -- `MirrorCounts` reflects every
    /// design fully processed before cancellation was observed (always a clean prefix,
    /// never a partial design; see the module doc comment's "Cancellation" section).
    Cancelled(MirrorCounts),
    /// Could not even enumerate the remote catalogue (a `SearchPage` request failed,
    /// on the first page or any later one) -- nothing was written locally at all.
    Failed(String),
}

/// Handle returned by [`super::sync::spawn_mirror_sync`]. Cancelling is cooperative --
/// see the module doc comment's "Cancellation" section -- matching
/// `bridge::export_thread::ExportHandle`'s existing shape.
pub struct MirrorHandle {
    pub(super) cancel: Arc<AtomicBool>,
}

impl MirrorHandle {
    /// Requests cancellation; the running job stops at its next check.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A source of [`LibraryResponse`]s for exactly one [`LibraryRequest`] at a time.
///
/// Exists so [`super::sync::run_mirror_sync`] can be unit-tested against a scripted
/// in-memory fake (`sync::tests::FakeTransport`) with no live socket or TLS handshake.
/// [`LibrarySession`]'s implementation is what production uses: one connect+handshake
/// for the whole sync, reused across every request, reconnecting-and-retrying once per
/// request if the held connection drops -- see that type's own doc comment.
/// [`WorkerSettings`]'s implementation (one real connect+handshake+request+response per
/// call) is not used by [`super::sync::spawn_mirror_sync`], which needs the reused
/// connection instead; it remains available for the interactive remote-browse path
/// (`bridge::library::source`), which calls the same one-shot `request` directly and
/// has no reason to hold a connection open.
pub trait LibraryTransport {
    /// # Errors
    ///
    /// Whatever the underlying transport failed with -- see
    /// `bridge::library::client::LibraryClientError`'s variants.
    fn request(
        &self,
        req: &LibraryRequest,
    ) -> Result<LibraryResponse, library_client::LibraryClientError>;
}

impl LibraryTransport for WorkerSettings {
    fn request(
        &self,
        req: &LibraryRequest,
    ) -> Result<LibraryResponse, library_client::LibraryClientError> {
        library_client::request(self, req)
    }
}

impl LibraryTransport for LibrarySession {
    fn request(
        &self,
        req: &LibraryRequest,
    ) -> Result<LibraryResponse, library_client::LibraryClientError> {
        // Resolves to `LibrarySession`'s own inherent `request` (inherent methods take
        // priority over a trait method of the same name) -- see that type's doc comment.
        self.request(req)
    }
}
